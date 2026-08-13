use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::{
    models::{
        SessionDefinition, SessionStatePayload, TerminalActivityPayload, TerminalOutputPayload,
    },
    session_protocol::{
        HostEvent, HostOutputChunk, HostRequest, HostResponse, HostSessionDefinition, Rendezvous,
        SESSION_HOST_PROTOCOL,
    },
};

const HOST_START_TIMEOUT: Duration = Duration::from_secs(5);
const TERMINAL_WINDOW_PREFIX: &str = "terminal-";
const DOCK_ACTIVITY_INTERVAL: Duration = Duration::from_millis(500);
const BACKLOG_EMIT_BYTES: usize = 256 * 1024;
// xterm retains 12,000 lines, so replaying the host's full 8 MB wire backlog is
// both wasted work and enough to starve every WebView in the shared browser.
const BACKLOG_REPLAY_BYTES: usize = 1024 * 1024;

fn batch_backlog_chunks(backlog: &[HostOutputChunk], last_sequence: &mut u64) -> Vec<String> {
    let mut decoded = Vec::new();
    for chunk in backlog {
        if chunk.sequence <= *last_sequence {
            continue;
        }
        *last_sequence = chunk.sequence;
        if let Ok(bytes) = BASE64.decode(&chunk.data) {
            decoded.extend_from_slice(&bytes);
        }
    }

    let mut replay_start = decoded.len().saturating_sub(BACKLOG_REPLAY_BYTES);
    if replay_start > 0
        && let Some(line_end) = decoded[replay_start..]
            .iter()
            .position(|byte| *byte == b'\n')
    {
        replay_start += line_end + 1;
    }
    decoded[replay_start..]
        .chunks(BACKLOG_EMIT_BYTES)
        .map(|batch| BASE64.encode(batch))
        .collect()
}

struct ControlConnection {
    host_pid: u32,
    port: u16,
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

struct AttachmentContext {
    group_id: Uuid,
    cols: u16,
    rows: u16,
    local_attachment_id: u64,
    last_sequence: u64,
}

impl ControlConnection {
    fn connect(rendezvous: &Rendezvous) -> Result<Self> {
        let writer = connect(rendezvous)?;
        let reader_stream = writer
            .try_clone()
            .context("cloning session host control connection")?;
        Ok(Self {
            host_pid: rendezvous.pid,
            port: rendezvous.port,
            reader: BufReader::new(reader_stream),
            writer,
        })
    }

    fn matches(&self, rendezvous: &Rendezvous) -> bool {
        self.host_pid == rendezvous.pid && self.port == rendezvous.port
    }

    fn request(&mut self, request: &HostRequest) -> Result<HostResponse> {
        write_json_line(&mut self.writer, request)?;
        let mut line = String::new();
        if self
            .reader
            .read_line(&mut line)
            .context("reading session host control response")?
            == 0
        {
            bail!("session host closed the control connection");
        }
        serde_json::from_str(line.trim_end()).context("decoding session host control response")
    }
}

#[derive(Clone)]
pub struct SessionManager {
    config_dir: Arc<PathBuf>,
    rendezvous: Arc<Mutex<Option<Rendezvous>>>,
    control: Arc<Mutex<Option<ControlConnection>>>,
    running: Arc<Mutex<HashSet<Uuid>>>,
    desired: Arc<Mutex<HashMap<Uuid, HostSessionDefinition>>>,
    attachments: Arc<Mutex<HashMap<Uuid, u64>>>,
    last_activity: Arc<Mutex<HashMap<Uuid, Instant>>>,
    next_attachment_id: Arc<AtomicU64>,
    host_start: Arc<Mutex<()>>,
    shutting_down: Arc<AtomicBool>,
}

impl SessionManager {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            config_dir: Arc::new(config_dir),
            rendezvous: Arc::new(Mutex::new(None)),
            control: Arc::new(Mutex::new(None)),
            running: Arc::new(Mutex::new(HashSet::new())),
            desired: Arc::new(Mutex::new(HashMap::new())),
            attachments: Arc::new(Mutex::new(HashMap::new())),
            last_activity: Arc::new(Mutex::new(HashMap::new())),
            next_attachment_id: Arc::new(AtomicU64::new(1)),
            host_start: Arc::new(Mutex::new(())),
            shutting_down: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_running(&self, session_id: Uuid) -> bool {
        self.running.lock().contains(&session_id)
    }

    pub fn start(
        &self,
        app: &AppHandle,
        definition: &SessionDefinition,
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        if self.attachments.lock().contains_key(&definition.id) {
            if self.is_running(definition.id) {
                emit_state(app, definition.id, "running", None, None);
            }
            return Ok(());
        }

        emit_state(app, definition.id, "starting", None, None);
        let command_line = command_line_with_session_persistence(app, definition)?;
        let hosted_definition = HostSessionDefinition::from_session(definition, command_line);
        self.desired
            .lock()
            .insert(definition.id, hosted_definition.clone());

        let rendezvous = self.live_rendezvous(true)?;
        let (reader, response) =
            match attach_connection(&rendezvous, &hosted_definition, cols, rows) {
                Ok(connection) => connection,
                Err(error) => {
                    self.desired.lock().remove(&definition.id);
                    return Err(error);
                }
            };
        let local_attachment_id = self.next_attachment_id.fetch_add(1, Ordering::Relaxed);
        self.attachments
            .lock()
            .insert(definition.id, local_attachment_id);
        let group_id = definition.group_id;
        let manager = self.clone();
        let app = app.clone();
        thread::Builder::new()
            .name(format!("ai-dock-attachment-{}", definition.id))
            .spawn(move || {
                manager.run_attachment(
                    app,
                    hosted_definition,
                    AttachmentContext {
                        group_id,
                        cols,
                        rows,
                        local_attachment_id,
                        last_sequence: 0,
                    },
                    reader,
                    response,
                );
            })
            .context("starting the session attachment reader")?;
        Ok(())
    }

    pub fn write(&self, session_id: Uuid, data: &str) -> Result<()> {
        let rendezvous = self.live_rendezvous(false)?;
        let response = self.control_request(
            &rendezvous,
            &HostRequest::Write {
                token: rendezvous.token.clone(),
                session_id,
                data: data.to_string(),
            },
        );
        if response.is_err() {
            *self.rendezvous.lock() = None;
        }
        expect_ok(response?)
    }

    pub fn resize(&self, session_id: Uuid, cols: u16, rows: u16) -> Result<()> {
        let rendezvous = self.live_rendezvous(false)?;
        let response = self.control_request(
            &rendezvous,
            &HostRequest::Resize {
                token: rendezvous.token.clone(),
                session_id,
                cols,
                rows,
            },
        );
        if response.is_err() {
            *self.rendezvous.lock() = None;
        }
        expect_ok(response?)
    }

    pub fn stop(&self, app: &AppHandle, session_id: Uuid) -> Result<()> {
        self.desired.lock().remove(&session_id);
        self.attachments.lock().remove(&session_id);
        self.last_activity.lock().remove(&session_id);
        self.running.lock().remove(&session_id);
        if let Ok(rendezvous) = self.live_rendezvous(false) {
            let response = self.control_request(
                &rendezvous,
                &HostRequest::Stop {
                    token: rendezvous.token.clone(),
                    session_id,
                },
            );
            if response.is_err() {
                *self.rendezvous.lock() = None;
            }
            if let Err(error) = response.and_then(expect_ok) {
                eprintln!("AI Dock could not notify the session host while closing: {error:#}");
            }
        }
        emit_state(app, session_id, "stopped", None, None);
        Ok(())
    }

    pub fn stop_all(&self, _app: &AppHandle) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.desired.lock().clear();
        self.attachments.lock().clear();
        self.last_activity.lock().clear();
        self.running.lock().clear();
        if let Ok(rendezvous) = self.live_rendezvous(false) {
            let _ = self.control_request(
                &rendezvous,
                &HostRequest::Shutdown {
                    token: rendezvous.token.clone(),
                },
            );
        }
        *self.rendezvous.lock() = None;
        *self.control.lock() = None;
    }

    fn run_attachment(
        &self,
        app: AppHandle,
        definition: HostSessionDefinition,
        mut context: AttachmentContext,
        mut reader: BufReader<TcpStream>,
        initial_response: HostResponse,
    ) {
        let session_id = definition.id;
        let mut reconnect_attempts = 0_u8;
        // Replaying multiple megabytes of retained terminal output from the Tauri
        // command that opened a window blocks the WebView message loop. Process
        // the initial attachment entirely on this worker before reading live data.
        self.accept_attachment_response_for(
            &app,
            session_id,
            context.group_id,
            &initial_response,
            &mut context.last_sequence,
        );
        loop {
            let disconnected = read_attachment_events(&mut reader, |event| match event {
                HostEvent::Output {
                    session_id,
                    sequence,
                    data,
                } => {
                    if sequence > context.last_sequence {
                        context.last_sequence = sequence;
                        self.emit_terminal_output(&app, context.group_id, session_id, data);
                    }
                }
                HostEvent::State {
                    session_id,
                    state,
                    exit_code,
                    message,
                } => self.update_state(&app, session_id, &state, exit_code, message),
            });
            if !self.attachment_is_current(session_id, context.local_attachment_id)
                || !self.desired.lock().contains_key(&session_id)
                || self.shutting_down.load(Ordering::Acquire)
            {
                break;
            }

            self.running.lock().remove(&session_id);
            emit_state(
                &app,
                session_id,
                "disconnected",
                None,
                disconnected.err().map(|error| error.to_string()),
            );

            let mut reconnected = None;
            while reconnect_attempts < 10
                && self.attachment_is_current(session_id, context.local_attachment_id)
                && self.desired.lock().contains_key(&session_id)
                && !self.shutting_down.load(Ordering::Acquire)
            {
                reconnect_attempts += 1;
                thread::sleep(Duration::from_millis(100 * u64::from(reconnect_attempts)));
                let Ok(rendezvous) = self.live_rendezvous(false) else {
                    continue;
                };
                match attach_connection(&rendezvous, &definition, context.cols, context.rows) {
                    Ok((next_reader, response)) => {
                        self.accept_attachment_response_for(
                            &app,
                            session_id,
                            context.group_id,
                            &response,
                            &mut context.last_sequence,
                        );
                        reconnected = Some(next_reader);
                        reconnect_attempts = 0;
                        break;
                    }
                    Err(_) => {
                        *self.rendezvous.lock() = None;
                        continue;
                    }
                }
            }
            let Some(next_reader) = reconnected else {
                break;
            };
            reader = next_reader;
        }

        if self.attachment_is_current(session_id, context.local_attachment_id) {
            self.attachments.lock().remove(&session_id);
            self.running.lock().remove(&session_id);
        }
    }

    fn accept_attachment_response_for(
        &self,
        app: &AppHandle,
        session_id: Uuid,
        group_id: Uuid,
        response: &HostResponse,
        last_sequence: &mut u64,
    ) {
        if let HostResponse::Attached {
            backlog,
            state,
            exit_code,
            message,
            ..
        } = response
        {
            for batch in batch_backlog_chunks(backlog, last_sequence) {
                self.emit_terminal_output(app, group_id, session_id, batch);
            }
            self.update_state(app, session_id, state, *exit_code, message.clone());
        }
    }

    fn update_state(
        &self,
        app: &AppHandle,
        session_id: Uuid,
        state: &str,
        exit_code: Option<u32>,
        message: Option<String>,
    ) {
        if matches!(state, "running" | "starting") {
            self.running.lock().insert(session_id);
        } else {
            self.running.lock().remove(&session_id);
        }
        emit_state(app, session_id, state, exit_code, message);
    }

    fn emit_terminal_output(
        &self,
        app: &AppHandle,
        group_id: Uuid,
        session_id: Uuid,
        data: String,
    ) {
        let _ = app.emit_to(
            format!("{TERMINAL_WINDOW_PREFIX}{group_id}"),
            "terminal-output",
            TerminalOutputPayload { session_id, data },
        );

        let should_notify_dock = {
            let now = Instant::now();
            let mut activity = self.last_activity.lock();
            match activity.get_mut(&session_id) {
                Some(last) if now.duration_since(*last) < DOCK_ACTIVITY_INTERVAL => false,
                Some(last) => {
                    *last = now;
                    true
                }
                None => {
                    activity.insert(session_id, now);
                    true
                }
            }
        };
        if should_notify_dock {
            let _ = app.emit_to(
                "dock",
                "terminal-activity",
                TerminalActivityPayload { session_id },
            );
        }
    }

    fn attachment_is_current(&self, session_id: Uuid, local_attachment_id: u64) -> bool {
        self.attachments
            .lock()
            .get(&session_id)
            .is_some_and(|current| *current == local_attachment_id)
    }

    fn control_request(
        &self,
        rendezvous: &Rendezvous,
        request: &HostRequest,
    ) -> Result<HostResponse> {
        let mut control = self.control.lock();
        if !control
            .as_ref()
            .is_some_and(|connection| connection.matches(rendezvous))
        {
            *control = Some(ControlConnection::connect(rendezvous)?);
        }
        let response = control
            .as_mut()
            .expect("control connection was initialized")
            .request(request);
        if response.is_err() {
            *control = None;
        }
        response
    }

    fn live_rendezvous(&self, start_if_missing: bool) -> Result<Rendezvous> {
        if let Some(rendezvous) = self.rendezvous.lock().clone()
            && (!start_if_missing || ping(&rendezvous).is_ok())
        {
            return Ok(rendezvous);
        }
        *self.rendezvous.lock() = None;
        *self.control.lock() = None;

        if let Ok(rendezvous) = read_rendezvous(&self.config_dir)
            && ping(&rendezvous).is_ok()
        {
            *self.rendezvous.lock() = Some(rendezvous.clone());
            return Ok(rendezvous);
        }
        if !start_if_missing {
            bail!("AI Dock session host is not running");
        }

        let _start_guard = self.host_start.lock();
        if let Ok(rendezvous) = read_rendezvous(&self.config_dir)
            && ping(&rendezvous).is_ok()
        {
            *self.rendezvous.lock() = Some(rendezvous.clone());
            return Ok(rendezvous);
        }
        self.spawn_host()?;
        let started_at = std::time::Instant::now();
        while started_at.elapsed() < HOST_START_TIMEOUT {
            if let Ok(rendezvous) = read_rendezvous(&self.config_dir)
                && ping(&rendezvous).is_ok()
            {
                *self.rendezvous.lock() = Some(rendezvous.clone());
                return Ok(rendezvous);
            }
            thread::sleep(Duration::from_millis(50));
        }
        bail!("AI Dock session host did not become ready within five seconds")
    }

    fn spawn_host(&self) -> Result<()> {
        fs::create_dir_all(self.config_dir.as_ref())
            .context("creating AI Dock session configuration directory")?;
        let source = host_source_path()?;
        if !source.is_file() {
            bail!(
                "AI Dock session host was not found at {}. Rebuild AI Dock so both executables are present",
                source.display()
            );
        }
        let host_bin_dir = self.config_dir.join("session-host-bin");
        fs::create_dir_all(&host_bin_dir).context("creating session host binary directory")?;
        for stale in fs::read_dir(&host_bin_dir).into_iter().flatten().flatten() {
            if stale
                .path()
                .extension()
                .is_some_and(|extension| extension == "exe")
            {
                let _ = fs::remove_file(stale.path());
            }
        }
        let extension = source.extension().and_then(|value| value.to_str());
        let file_name = match extension {
            Some(extension) => format!("ai-dock-session-host-{}.{}", std::process::id(), extension),
            None => format!("ai-dock-session-host-{}", std::process::id()),
        };
        let staged = host_bin_dir.join(file_name);
        fs::copy(&source, &staged).with_context(|| {
            format!(
                "staging session host from {} to {}",
                source.display(),
                staged.display()
            )
        })?;
        let _ = fs::remove_file(self.config_dir.join("session-host.json"));

        let mut command = Command::new(&staged);
        command
            .arg("--config-dir")
            .arg(self.config_dir.as_ref())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        }
        command
            .spawn()
            .context("launching detached AI Dock session host")?;
        Ok(())
    }
}

fn host_source_path() -> Result<PathBuf> {
    let current = std::env::current_exe().context("locating AI Dock executable")?;
    let directory = current
        .parent()
        .context("AI Dock executable has no parent directory")?;
    Ok(directory.join(if cfg!(windows) {
        "ai-dock-session-host.exe"
    } else {
        "ai-dock-session-host"
    }))
}

fn read_rendezvous(config_dir: &Path) -> Result<Rendezvous> {
    let bytes = fs::read(config_dir.join("session-host.json"))
        .context("reading session host rendezvous")?;
    let rendezvous: Rendezvous =
        serde_json::from_slice(&bytes).context("decoding session host rendezvous")?;
    if rendezvous.protocol != SESSION_HOST_PROTOCOL {
        bail!(
            "session host protocol {} is incompatible with UI protocol {}",
            rendezvous.protocol,
            SESSION_HOST_PROTOCOL
        );
    }
    Ok(rendezvous)
}

fn ping(rendezvous: &Rendezvous) -> Result<()> {
    match send_request(
        rendezvous,
        &HostRequest::Ping {
            token: rendezvous.token.clone(),
        },
    )? {
        HostResponse::Pong { protocol, pid }
            if protocol == SESSION_HOST_PROTOCOL && pid == rendezvous.pid =>
        {
            Ok(())
        }
        HostResponse::Error { message } => Err(anyhow!(message)),
        _ => bail!("session host returned an invalid ping response"),
    }
}

fn send_request(rendezvous: &Rendezvous, request: &HostRequest) -> Result<HostResponse> {
    let mut stream = connect(rendezvous)?;
    write_json_line(&mut stream, request)?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader
        .read_line(&mut line)
        .context("reading session host response")?
        == 0
    {
        bail!("session host closed the connection without a response");
    }
    serde_json::from_str(line.trim_end()).context("decoding session host response")
}

fn attach_connection(
    rendezvous: &Rendezvous,
    definition: &HostSessionDefinition,
    cols: u16,
    rows: u16,
) -> Result<(BufReader<TcpStream>, HostResponse)> {
    let mut stream = connect(rendezvous)?;
    write_json_line(
        &mut stream,
        &HostRequest::AttachStart {
            token: rendezvous.token.clone(),
            definition: definition.clone(),
            cols,
            rows,
        },
    )?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader
        .read_line(&mut line)
        .context("reading session attachment response")?
        == 0
    {
        bail!("session host closed the attachment before acknowledging it");
    }
    let response: HostResponse =
        serde_json::from_str(line.trim_end()).context("decoding session attachment response")?;
    match &response {
        HostResponse::Attached { .. } => {
            reader.get_mut().set_read_timeout(None)?;
            Ok((reader, response))
        }
        HostResponse::Error { message } => Err(anyhow!(message.clone())),
        _ => bail!("session host returned an invalid attachment response"),
    }
}

fn connect(rendezvous: &Rendezvous) -> Result<TcpStream> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), rendezvous.port);
    let stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .context("connecting to AI Dock session host")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .context("setting session host read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .context("setting session host write timeout")?;
    stream.set_nodelay(true).context("enabling TCP_NODELAY")?;
    Ok(stream)
}

fn expect_ok(response: HostResponse) -> Result<()> {
    match response {
        HostResponse::Ok => Ok(()),
        HostResponse::Error { message } => Err(anyhow!(message)),
        _ => bail!("session host returned an invalid command response"),
    }
}

fn read_attachment_events(
    reader: &mut BufReader<TcpStream>,
    mut handle: impl FnMut(HostEvent),
) -> Result<()> {
    loop {
        let mut line = String::new();
        if reader
            .read_line(&mut line)
            .context("reading session attachment event")?
            == 0
        {
            bail!("session attachment closed");
        }
        let event: HostEvent =
            serde_json::from_str(line.trim_end()).context("decoding session attachment event")?;
        handle(event);
    }
}

fn write_json_line(writer: &mut impl Write, value: &impl serde::Serialize) -> Result<()> {
    serde_json::to_writer(&mut *writer, value).context("encoding session host request")?;
    writer
        .write_all(b"\n")
        .context("writing session host request")?;
    writer.flush().context("flushing session host request")
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

pub fn emit_state(
    app: &AppHandle,
    session_id: Uuid,
    state: &str,
    exit_code: Option<u32>,
    message: Option<String>,
) {
    let _ = app.emit(
        "session-state",
        SessionStatePayload {
            session_id,
            state: state.to_string(),
            exit_code,
            message,
        },
    );
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

    #[test]
    fn coalesces_attachment_backlog_and_skips_already_seen_output() {
        let backlog = vec![
            HostOutputChunk {
                sequence: 1,
                data: BASE64.encode(b"old"),
            },
            HostOutputChunk {
                sequence: 2,
                data: BASE64.encode(b"hello "),
            },
            HostOutputChunk {
                sequence: 3,
                data: BASE64.encode(b"world"),
            },
        ];
        let mut last_sequence = 1;

        let batches = batch_backlog_chunks(&backlog, &mut last_sequence);

        assert_eq!(last_sequence, 3);
        assert_eq!(batches.len(), 1);
        assert_eq!(BASE64.decode(&batches[0]).unwrap(), b"hello world");
    }

    #[test]
    fn splits_large_attachment_backlogs_into_bounded_event_batches() {
        let backlog = (1..=3)
            .map(|sequence| HostOutputChunk {
                sequence,
                data: BASE64.encode(vec![b'x'; BACKLOG_EMIT_BYTES / 2]),
            })
            .collect::<Vec<_>>();
        let mut last_sequence = 0;

        let batches = batch_backlog_chunks(&backlog, &mut last_sequence);

        assert_eq!(last_sequence, 3);
        assert_eq!(batches.len(), 2);
        assert_eq!(
            BASE64.decode(&batches[0]).unwrap().len(),
            BACKLOG_EMIT_BYTES
        );
        assert_eq!(
            BASE64.decode(&batches[1]).unwrap().len(),
            BACKLOG_EMIT_BYTES / 2
        );
    }

    #[test]
    fn limits_replayed_history_to_the_terminal_scrollback_budget() {
        let backlog = vec![HostOutputChunk {
            sequence: 1,
            data: BASE64.encode(vec![b'x'; BACKLOG_REPLAY_BYTES + 4096]),
        }];
        let mut last_sequence = 0;

        let batches = batch_backlog_chunks(&backlog, &mut last_sequence);
        let replayed_bytes = batches
            .iter()
            .map(|batch| BASE64.decode(batch).unwrap().len())
            .sum::<usize>();

        assert_eq!(last_sequence, 1);
        assert_eq!(replayed_bytes, BACKLOG_REPLAY_BYTES);
        assert!(
            batches
                .iter()
                .all(|batch| { BASE64.decode(batch).unwrap().len() <= BACKLOG_EMIT_BYTES })
        );
    }
}
