#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    match ai_dock_lib::run_ui_recovery_helper_if_requested() {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("AI Dock UI recovery helper failed: {error:#}");
            return;
        }
    }
    ai_dock_lib::run();
}
