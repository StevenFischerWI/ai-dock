use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use anyhow::{Context, Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::models::{SessionDefinition, SessionStatePayload, TerminalOutputPayload};

struct ManagedSession {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    stop_requested: AtomicBool,
}

#[derive(Clone, Default)]
pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<Uuid, Arc<ManagedSession>>>>,
}

impl SessionManager {
    pub fn is_running(&self, session_id: Uuid) -> bool {
        self.sessions.lock().contains_key(&session_id)
    }

    pub fn start(
        &self,
        app: &AppHandle,
        definition: &SessionDefinition,
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        if self.is_running(definition.id) {
            emit_state(app, definition.id, "running", None, None);
            return Ok(());
        }

        emit_state(app, definition.id, "starting", None, None);
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows.clamp(2, 500),
                cols: cols.clamp(2, 500),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening pseudoterminal")?;

        let command_line = command_line_with_session_persistence(app, definition)?;
        let mut command = platform_command(&command_line);
        if !definition.working_directory.trim().is_empty() {
            command.cwd(&definition.working_directory);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("starting {}", definition.command_line))?;
        let killer = child.clone_killer();
        let reader = pair
            .master
            .try_clone_reader()
            .context("opening terminal output")?;
        let writer = pair
            .master
            .take_writer()
            .context("opening terminal input")?;
        drop(pair.slave);

        let managed = Arc::new(ManagedSession {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            stop_requested: AtomicBool::new(false),
        });

        self.sessions.lock().insert(definition.id, managed.clone());
        emit_state(app, definition.id, "running", None, None);

        spawn_output_reader(app.clone(), definition.id, reader);
        spawn_exit_watcher(
            app.clone(),
            definition.id,
            child,
            managed,
            self.sessions.clone(),
        );
        Ok(())
    }

    pub fn write(&self, session_id: Uuid, data: &str) -> Result<()> {
        let session = self
            .sessions
            .lock()
            .get(&session_id)
            .cloned()
            .ok_or_else(|| anyhow!("session is not running"))?;
        let mut writer = session.writer.lock();
        writer
            .write_all(data.as_bytes())
            .context("writing terminal input")?;
        writer.flush().context("flushing terminal input")?;
        Ok(())
    }

    pub fn resize(&self, session_id: Uuid, cols: u16, rows: u16) -> Result<()> {
        let session = self
            .sessions
            .lock()
            .get(&session_id)
            .cloned()
            .ok_or_else(|| anyhow!("session is not running"))?;
        session
            .master
            .lock()
            .resize(PtySize {
                rows: rows.clamp(2, 500),
                cols: cols.clamp(2, 500),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resizing terminal")
    }

    pub fn stop(&self, app: &AppHandle, session_id: Uuid) -> Result<()> {
        let session = self.sessions.lock().remove(&session_id);
        if let Some(session) = session {
            session.stop_requested.store(true, Ordering::Release);
            // portable-pty 0.9 terminates the Windows process but reverses the Win32
            // success check and reports an error on success. Do not surface that false error.
            let _ = session.killer.lock().kill();
        }
        emit_state(app, session_id, "stopped", None, None);
        Ok(())
    }

    pub fn stop_all(&self, app: &AppHandle) {
        let ids: Vec<Uuid> = self.sessions.lock().keys().copied().collect();
        for id in ids {
            let _ = self.stop(app, id);
        }
    }
}

fn platform_command(command_line: &str) -> CommandBuilder {
    #[cfg(windows)]
    {
        let mut command = CommandBuilder::new("cmd.exe");
        command.arg("/d");
        command.arg("/s");
        command.arg("/c");
        command.arg(command_line);
        command
    }

    #[cfg(not(windows))]
    {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-lc");
        command.arg(command_line);
        command
    }
}

#[cfg(windows)]
fn command_line_with_session_persistence(
    app: &AppHandle,
    definition: &SessionDefinition,
) -> Result<String> {
    if !is_interactive_powershell(&definition.command_line) {
        return Ok(definition.command_line.clone());
    }

    let history_directory = app
        .path()
        .app_config_dir()
        .context("resolving AI Dock configuration directory")?
        .join("history");
    fs::create_dir_all(&history_directory).context("creating terminal history directory")?;
    let history_path = history_directory.join(format!("{}.txt", definition.id));
    Ok(powershell_command_with_persistence(
        &definition.command_line,
        &history_path,
    ))
}

#[cfg(not(windows))]
fn command_line_with_session_persistence(
    _app: &AppHandle,
    definition: &SessionDefinition,
) -> Result<String> {
    Ok(definition.command_line.clone())
}

#[cfg(windows)]
fn is_interactive_powershell(command_line: &str) -> bool {
    let trimmed = command_line.trim();
    let (executable, arguments) = if let Some(quoted) = trimmed.strip_prefix('"') {
        let Some(quote_end) = quoted.find('"') else {
            return false;
        };
        (&quoted[..quote_end], quoted[quote_end + 1..].trim())
    } else {
        let argument_start = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        (&trimmed[..argument_start], trimmed[argument_start..].trim())
    };
    let executable_name = Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(executable)
        .to_ascii_lowercase();
    if !matches!(
        executable_name.as_str(),
        "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe"
    ) {
        return false;
    }

    !arguments.split_whitespace().any(|argument| {
        matches!(
            argument
                .trim_start_matches(['-', '/'])
                .to_ascii_lowercase()
                .as_str(),
            "command" | "commandwithargs" | "encodedcommand" | "file" | "c" | "e" | "f"
        )
    })
}

#[cfg(windows)]
fn powershell_command_with_persistence(command_line: &str, history_path: &Path) -> String {
    let escaped_history_path = history_path.to_string_lossy().replace('\'', "''");
    let script = r#"
$aiDockHistoryPath = '__AI_DOCK_HISTORY_PATH__'
try {
  Import-Module PSReadLine -ErrorAction Stop
  Set-PSReadLineOption -HistorySavePath $aiDockHistoryPath -HistorySaveStyle SaveIncrementally -ErrorAction Stop
} catch {}
$global:__AiDockOriginalPrompt = (Get-Command prompt -CommandType Function).ScriptBlock
function global:prompt {
  try {
    $aiDockProviderPath = (Get-Location).ProviderPath
    if ($aiDockProviderPath) {
      $aiDockPayload = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($aiDockProviderPath))
      [Console]::Write(([char]27).ToString() + ']6973;' + $aiDockPayload + [char]7)
    }
  } catch {}
  if ($global:__AiDockOriginalPrompt) { & $global:__AiDockOriginalPrompt } else { 'PS> ' }
}
"#
    .replace("__AI_DOCK_HISTORY_PATH__", &escaped_history_path);
    let encoded_bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let encoded_script = BASE64.encode(encoded_bytes);
    format!(
        "{} -NoExit -EncodedCommand {encoded_script}",
        command_line.trim()
    )
}

fn spawn_output_reader(app: AppHandle, session_id: Uuid, mut reader: Box<dyn Read + Send>) {
    thread::Builder::new()
        .name(format!("ai-dock-output-{session_id}"))
        .spawn(move || {
            let mut buffer = vec![0_u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let payload = TerminalOutputPayload {
                            session_id,
                            data: BASE64.encode(&buffer[..count]),
                        };
                        let _ = app.emit("terminal-output", payload);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
        .expect("failed to spawn terminal output thread");
}

fn spawn_exit_watcher(
    app: AppHandle,
    session_id: Uuid,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    managed: Arc<ManagedSession>,
    sessions: Arc<Mutex<HashMap<Uuid, Arc<ManagedSession>>>>,
) {
    thread::Builder::new()
        .name(format!("ai-dock-process-{session_id}"))
        .spawn(move || {
            let result = child.wait();
            let was_stopped = managed.stop_requested.load(Ordering::Acquire);
            let should_remove = sessions
                .lock()
                .get(&session_id)
                .is_some_and(|current| Arc::ptr_eq(current, &managed));
            if should_remove {
                sessions.lock().remove(&session_id);
            }

            if !was_stopped {
                match result {
                    Ok(status) => {
                        emit_state(&app, session_id, "exited", Some(status.exit_code()), None)
                    }
                    Err(error) => {
                        emit_state(&app, session_id, "failed", None, Some(error.to_string()))
                    }
                }
            }
        })
        .expect("failed to spawn process watcher thread");
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_interactive_powershell_commands() {
        assert!(is_interactive_powershell("pwsh.exe -NoLogo"));
        assert!(is_interactive_powershell(
            r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo"#
        ));
        assert!(!is_interactive_powershell("codex"));
        assert!(!is_interactive_powershell("pwsh.exe -Command Get-Location"));
        assert!(!is_interactive_powershell("powershell.exe -File task.ps1"));
    }

    #[test]
    fn embeds_history_and_directory_reporting_in_powershell_startup() {
        let command = powershell_command_with_persistence(
            "pwsh.exe -NoLogo",
            Path::new(r"C:\AI Dock\history\tab.txt"),
        );
        let encoded = command
            .split_whitespace()
            .next_back()
            .expect("encoded command");
        let bytes = BASE64.decode(encoded).expect("valid base64");
        let words = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        let script = String::from_utf16(&words).expect("valid utf-16");

        assert!(command.starts_with("pwsh.exe -NoLogo -NoExit -EncodedCommand "));
        assert!(script.contains(r"C:\AI Dock\history\tab.txt"));
        assert!(script.contains("HistorySaveStyle SaveIncrementally"));
        assert!(script.contains("]6973;"));
        assert!(!script.contains("function global:codex"));
    }
}

pub fn emit_state(
    app: &AppHandle,
    session_id: Uuid,
    state: &'static str,
    exit_code: Option<u32>,
    message: Option<String>,
) {
    let _ = app.emit(
        "session-state",
        SessionStatePayload {
            session_id,
            state,
            exit_code,
            message,
        },
    );
}
