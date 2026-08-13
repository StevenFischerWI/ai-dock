//! Out-of-process host for AI Dock's terminal sessions.
//!
//! Terminals outlive the UI by running here rather than inside the Tauri process,
//! so a dock restart or crash leaves the PTYs and their child programs untouched.
//! AI Dock stages a copy of this executable beside its own and launches it
//! detached with `--config-dir`; the two then rendezvous over the file the host
//! writes into that directory.

fn main() {
    if let Err(error) = ai_dock_lib::run_session_host() {
        eprintln!("AI Dock session host failed: {error:#}");
        std::process::exit(1);
    }
}
