#[cfg(windows)]
mod platform {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        path::Path,
        process::{self, Command, Stdio},
        time::SystemTime,
    };

    use anyhow::{Context, Result, anyhow};

    type Handle = isize;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    const SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 258;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const RECOVERY_ARGUMENT: &str = "--ai-dock-recover-ui-after-pid";
    pub fn request_ui_recovery(config_dir: &Path, reason: &str) -> Result<()> {
        fs::create_dir_all(config_dir).context("creating watchdog log directory")?;
        let current_executable = std::env::current_exe().context("locating AI Dock executable")?;
        let current_pid = process::id();
        let mut command = Command::new(&current_executable);
        command
            .arg(RECOVERY_ARGUMENT)
            .arg(current_pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        command
            .spawn()
            .context("launching AI Dock UI recovery helper")?;
        append_log(&config_dir.join("watchdog.log"), reason);

        // This intentionally bypasses Tauri's ExitRequested path. That path needs the
        // wedged event loop and would shut down the detached session host. The helper
        // waits for this process to disappear and then starts a clean UI process.
        process::exit(91);
    }

    pub fn run_recovery_helper_if_requested() -> Result<bool> {
        let mut arguments = std::env::args().skip(1);
        if arguments.next().as_deref() != Some(RECOVERY_ARGUMENT) {
            return Ok(false);
        }
        let old_pid = arguments
            .next()
            .context("UI recovery helper is missing the old process id")?
            .parse::<u32>()
            .context("UI recovery helper received an invalid process id")?;

        wait_for_process_exit(old_pid)?;
        let current_executable = std::env::current_exe().context("locating AI Dock executable")?;
        let mut command = Command::new(current_executable);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        command
            .spawn()
            .context("relaunching AI Dock after UI recovery")?;
        Ok(true)
    }

    fn wait_for_process_exit(process_id: u32) -> Result<()> {
        // SAFETY: OpenProcess returns an owned kernel handle or zero. The handle is
        // closed exactly once after the bounded wait.
        let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, process_id) };
        if handle == 0 {
            // The old process normally exits before the helper reaches OpenProcess.
            return Ok(());
        }
        // Thirty seconds is deliberately bounded so an unexpected PID reuse cannot
        // leave a hidden recovery helper around forever.
        let wait_result = unsafe { WaitForSingleObject(handle, 30_000) };
        let _ = unsafe { CloseHandle(handle) };
        match wait_result {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err(anyhow!(
                "timed out waiting for the old AI Dock UI process {process_id}"
            )),
            result => Err(anyhow!(
                "waiting for the old AI Dock UI process failed with result {result}"
            )),
        }
    }

    fn append_log(path: &Path, message: &str) {
        let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        let _ = writeln!(file, "{:?} {message}", SystemTime::now());
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    use anyhow::{Result, bail};

    pub fn request_ui_recovery(_config_dir: &Path, _reason: &str) -> Result<()> {
        bail!("automatic UI recovery is not implemented on this platform")
    }

    pub fn run_recovery_helper_if_requested() -> Result<bool> {
        Ok(false)
    }
}

pub use platform::{request_ui_recovery, run_recovery_helper_if_requested};
