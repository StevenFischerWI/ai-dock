use anyhow::{Context, Result};
use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

#[cfg(windows)]
pub fn disable_window_animations(window: &WebviewWindow) -> Result<()> {
    use std::{ffi::c_void, mem::size_of};

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attribute: u32,
            value: *const c_void,
            value_size: u32,
        ) -> i32;
    }

    const DWMWA_TRANSITIONS_FORCEDISABLED: u32 = 3;
    let hwnd = window.hwnd().context("getting window HWND")?.0 as isize;
    let disabled: i32 = 1;
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            (&disabled as *const i32).cast(),
            size_of::<i32>() as u32,
        )
    };
    if result < 0 {
        anyhow::bail!("DwmSetWindowAttribute failed with HRESULT {result:#x}");
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn disable_window_animations(_window: &WebviewWindow) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
pub fn install_ctrl_h_handler<FHide, FRestore>(
    window: &WebviewWindow,
    on_hide: FHide,
    on_restore: FRestore,
) -> Result<()>
where
    FHide: Fn() + Send + Sync + 'static,
    FRestore: Fn() + Send + Sync + 'static,
{
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };
    use webview2_com::{
        AcceleratorKeyPressedEventHandler,
        Microsoft::Web::WebView2::Win32::COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
    };
    use windows_sys::Win32::{
        Foundation::HWND,
        UI::{
            Input::KeyboardAndMouse::{
                MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
            },
            WindowsAndMessaging::{GetForegroundWindow, MSG, PM_REMOVE, PeekMessageW, WM_HOTKEY},
        },
    };

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetKeyState(virtual_key: i32) -> i16;
        fn ShowWindow(hwnd: isize, command: i32) -> i32;
    }

    const VK_CONTROL: i32 = 0x11;
    const VK_H: u32 = 0x48;
    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;
    const CTRL_H_HOTKEY_ID: i32 = 0x41D0;
    const UNDO_DURATION: Duration = Duration::from_secs(10);
    const HOTKEY_REGISTRATION_DELAY: Duration = Duration::from_millis(60);
    const FOCUS_POLL_INTERVAL: Duration = Duration::from_millis(20);

    static CTRL_H_UNDO_GENERATION: AtomicU64 = AtomicU64::new(0);

    fn arm_restore(window: WebviewWindow, hwnd: isize, on_restore: Arc<dyn Fn() + Send + Sync>) {
        let generation = CTRL_H_UNDO_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = thread::Builder::new()
            .name("ai-dock-ctrl-h-undo".to_string())
            .spawn(move || {
                // Let the first keypress finish before registering the same chord
                // globally; otherwise the still-held H key can look like an undo.
                thread::sleep(HOTKEY_REGISTRATION_DELAY);
                if CTRL_H_UNDO_GENERATION.load(Ordering::SeqCst) != generation {
                    return;
                }

                let registered = unsafe {
                    RegisterHotKey(
                        std::ptr::null_mut(),
                        CTRL_H_HOTKEY_ID,
                        MOD_CONTROL | MOD_NOREPEAT,
                        VK_H,
                    ) != 0
                };
                if !registered {
                    eprintln!("AI Dock could not arm the Ctrl+H undo shortcut");
                    return;
                }

                let started = Instant::now();
                let mut fallback_foreground: HWND = std::ptr::null_mut();
                let mut message = MSG::default();
                loop {
                    if CTRL_H_UNDO_GENERATION.load(Ordering::SeqCst) != generation
                        || started.elapsed() >= UNDO_DURATION
                    {
                        break;
                    }

                    let has_hotkey = unsafe {
                        PeekMessageW(
                            &mut message,
                            std::ptr::null_mut(),
                            WM_HOTKEY,
                            WM_HOTKEY,
                            PM_REMOVE,
                        ) != 0
                    };
                    if has_hotkey && message.wParam == CTRL_H_HOTKEY_ID as usize {
                        // Show natively first for an immediate undo, then synchronize
                        // Tauri and the dock's visibility indicator.
                        unsafe {
                            ShowWindow(hwnd, SW_SHOW);
                        }
                        if let Err(error) = window.show() {
                            eprintln!("AI Dock could not synchronize Ctrl+H restore: {error}");
                        }
                        if let Err(error) = focus_window(&window) {
                            eprintln!(
                                "AI Dock could not focus the Ctrl+H restored window: {error:#}"
                            );
                        }
                        on_restore();
                        break;
                    }

                    let foreground = unsafe { GetForegroundWindow() };
                    if foreground != hwnd as HWND && !foreground.is_null() {
                        if fallback_foreground.is_null() {
                            // Windows automatically focuses the window underneath the
                            // one just hidden. That initial transfer is not a user action.
                            fallback_foreground = foreground;
                        } else if foreground != fallback_foreground {
                            // A subsequent focus change means the user moved on, so Ctrl+H
                            // belongs to the newly focused application again.
                            break;
                        }
                    }
                    thread::sleep(FOCUS_POLL_INTERVAL);
                }

                unsafe {
                    UnregisterHotKey(std::ptr::null_mut(), CTRL_H_HOTKEY_ID);
                }
            });
    }

    let hwnd = window.hwnd().context("getting Ctrl+H window HWND")?.0 as isize;
    let tauri_window = window.clone();
    let on_hide: Arc<dyn Fn() + Send + Sync> = Arc::new(on_hide);
    let on_restore: Arc<dyn Fn() + Send + Sync> = Arc::new(on_restore);
    window
        .with_webview(move |webview| {
            let controller = webview.controller();
            let mut token = 0_i64;
            let result = unsafe {
                controller.add_AcceleratorKeyPressed(
                    &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        let mut event_kind = Default::default();
                        let mut virtual_key = 0_u32;
                        args.KeyEventKind(&mut event_kind)?;
                        args.VirtualKey(&mut virtual_key)?;
                        if event_kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                            && virtual_key == VK_H
                            && GetKeyState(VK_CONTROL) < 0
                        {
                            args.SetHandled(true)?;
                            // Hide natively inside the accelerator callback so the shortcut
                            // remains instant even while the Tauri event loop is busy. Follow
                            // it with Tauri's hide operation so its visibility state stays in
                            // sync and the next dock click restores the window.
                            ShowWindow(hwnd, SW_HIDE);
                            if let Err(error) = tauri_window.hide() {
                                eprintln!(
                                    "AI Dock could not synchronize Ctrl+H visibility: {error}"
                                );
                            }
                            on_hide();
                            arm_restore(tauri_window.clone(), hwnd, Arc::clone(&on_restore));
                        }
                        Ok(())
                    })),
                    &mut token,
                )
            };
            if let Err(error) = result {
                eprintln!("AI Dock could not install Ctrl+H: {error}");
            }
        })
        .context("installing Ctrl+H handler")?;
    Ok(())
}

#[cfg(not(windows))]
pub fn install_ctrl_h_handler<FHide, FRestore>(
    _window: &WebviewWindow,
    _on_hide: FHide,
    _on_restore: FRestore,
) -> Result<()>
where
    FHide: Fn() + Send + Sync + 'static,
    FRestore: Fn() + Send + Sync + 'static,
{
    Ok(())
}

#[cfg(windows)]
pub fn focus_window(window: &WebviewWindow) -> Result<()> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn AttachThreadInput(thread: u32, attach_to: u32, attach: i32) -> i32;
        fn BringWindowToTop(hwnd: isize) -> i32;
        fn GetForegroundWindow() -> isize;
        fn GetWindowThreadProcessId(hwnd: isize, process_id: *mut u32) -> u32;
        fn SetFocus(hwnd: isize) -> isize;
        fn SetForegroundWindow(hwnd: isize) -> i32;
    }

    // Tauri's ordinary focus request is enough in many cases, but Windows may
    // reject it when the click came through a taskbar-style AppBar. Continue
    // with the attached-input foreground handoff even if that request fails.
    let _ = window.set_focus();
    // Let the AppBar mouse-activation sequence finish before overriding the
    // foreground window; otherwise Windows can immediately restore the app
    // that was active behind the dock.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let target = window.hwnd().context("getting terminal HWND")?.0 as isize;
    let foreground = unsafe { GetForegroundWindow() };
    if foreground == target {
        return Ok(());
    }

    let current_thread = unsafe { GetCurrentThreadId() };
    let foreground_thread = if foreground == 0 {
        0
    } else {
        unsafe { GetWindowThreadProcessId(foreground, std::ptr::null_mut()) }
    };
    let target_thread = unsafe { GetWindowThreadProcessId(target, std::ptr::null_mut()) };
    let attached_foreground = foreground_thread != 0
        && foreground_thread != current_thread
        && unsafe { AttachThreadInput(current_thread, foreground_thread, 1) } != 0;
    let attached_target = target_thread != 0
        && target_thread != current_thread
        && target_thread != foreground_thread
        && unsafe { AttachThreadInput(current_thread, target_thread, 1) } != 0;

    let mut focused = false;
    for _ in 0..3 {
        unsafe {
            BringWindowToTop(target);
            SetForegroundWindow(target);
            SetFocus(target);
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
        if unsafe { GetForegroundWindow() } == target {
            focused = true;
            break;
        }
    }

    if attached_target {
        unsafe {
            AttachThreadInput(current_thread, target_thread, 0);
        }
    }
    if attached_foreground {
        unsafe {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
    }

    if !focused {
        // Windows may refuse foreground ownership while still raising the window.
        // Keep activation best-effort so a transient OS focus lock does not become
        // a dock-wide error toast; the next click can retry the handoff.
        unsafe {
            BringWindowToTop(target);
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn focus_window(window: &WebviewWindow) -> Result<()> {
    window.set_focus().context("requesting terminal focus")
}

pub fn position_terminal(
    app: &tauri::AppHandle,
    terminal: &WebviewWindow,
    popup_width: Option<f64>,
    popup_height: f64,
    saved_position: Option<(i32, i32)>,
    slot: usize,
) -> Result<()> {
    let dock = app
        .get_webview_window("dock")
        .context("dock window is unavailable")?;
    let monitor = dock
        .current_monitor()
        .context("reading dock monitor")?
        .context("dock is not on a monitor")?;
    let dock_position = dock.outer_position().context("reading dock position")?;
    let scale = monitor.scale_factor();
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let desired_outer_width = popup_width
        .map(|width| width.round() as u32)
        .unwrap_or_else(|| default_outer_width(monitor_size.width, scale));
    let desired_outer_height = popup_height.round() as u32;
    let current_inner = terminal
        .inner_size()
        .context("reading terminal client size")?;
    let current_outer = terminal
        .outer_size()
        .context("reading terminal frame size")?;
    let frame_width = current_outer.width.saturating_sub(current_inner.width);
    let frame_height = current_outer.height.saturating_sub(current_inner.height);
    let client_width = desired_outer_width.saturating_sub(frame_width).max(320);
    let client_height = desired_outer_height.saturating_sub(frame_height).max(220);

    terminal
        .set_size(PhysicalSize::new(client_width, client_height))
        .context("sizing terminal window")?;
    let positioned_outer = terminal
        .outer_size()
        .context("reading resized terminal frame")?;
    let (left, top) = saved_position.unwrap_or_else(|| {
        default_position(
            monitor_position.x,
            monitor_position.y,
            monitor_size.width,
            dock_position.y,
            positioned_outer.width,
            positioned_outer.height,
            slot,
        )
    });
    terminal
        .set_position(PhysicalPosition::new(left, top))
        .context("positioning terminal window")?;
    Ok(())
}

pub fn position_right_panel(
    app: &tauri::AppHandle,
    panel: &WebviewWindow,
    saved_width: Option<f64>,
    saved_height: Option<f64>,
    saved_position: Option<(i32, i32)>,
    slot: usize,
) -> Result<()> {
    let dock = app
        .get_webview_window("dock")
        .context("dock window is unavailable")?;
    let monitor = dock
        .current_monitor()
        .context("reading dock monitor")?
        .context("dock is not on a monitor")?;
    let dock_position = dock.outer_position().context("reading dock position")?;
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let scale = monitor.scale_factor();
    let desired_outer_width = saved_width
        .map(|width| width.round() as u32)
        .unwrap_or_else(|| default_outer_width(monitor_size.width, scale));
    let available_height = dock_position
        .y
        .saturating_sub(monitor_position.y)
        .saturating_sub(8)
        .max((420.0 * scale).round() as i32) as u32;
    let desired_outer_height = saved_height
        .map(|height| height.round() as u32)
        .unwrap_or(available_height);
    let current_inner = panel.inner_size().context("reading panel client size")?;
    let current_outer = panel.outer_size().context("reading panel frame size")?;
    let frame_width = current_outer.width.saturating_sub(current_inner.width);
    let frame_height = current_outer.height.saturating_sub(current_inner.height);
    panel
        .set_size(PhysicalSize::new(
            desired_outer_width.saturating_sub(frame_width).max(420),
            desired_outer_height.saturating_sub(frame_height).max(360),
        ))
        .context("sizing right panel")?;
    let positioned_outer = panel.outer_size().context("reading resized panel frame")?;
    let default_position = right_panel_position(
        monitor_position.x,
        monitor_position.y,
        monitor_size.width,
        dock_position.y,
        positioned_outer.width,
        positioned_outer.height,
        slot,
    );
    let (left, top) = saved_position.unwrap_or(default_position);
    panel
        .set_position(PhysicalPosition::new(left, top))
        .context("positioning right panel")?;
    Ok(())
}

pub fn position_launcher(app: &tauri::AppHandle, launcher: &WebviewWindow) -> Result<()> {
    let dock = app
        .get_webview_window("dock")
        .context("dock window is unavailable")?;
    let monitor = dock
        .current_monitor()
        .context("reading dock monitor")?
        .context("dock is not on a monitor")?;
    let dock_position = dock.outer_position().context("reading dock position")?;
    let launcher_size = launcher.outer_size().context("reading launcher size")?;
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let (left, top) = launcher_position(
        monitor_position.x,
        monitor_position.y,
        monitor_size.width,
        dock_position.x,
        dock_position.y,
        launcher_size.width,
        launcher_size.height,
    );
    launcher
        .set_position(PhysicalPosition::new(left, top))
        .context("positioning launcher")?;
    Ok(())
}

fn launcher_position(
    monitor_left: i32,
    monitor_top: i32,
    monitor_width: u32,
    dock_left: i32,
    dock_top: i32,
    launcher_width: u32,
    launcher_height: u32,
) -> (i32, i32) {
    let maximum_left = monitor_left + monitor_width.saturating_sub(launcher_width) as i32;
    (
        (dock_left + 5).clamp(monitor_left, maximum_left),
        (dock_top - launcher_height as i32 - 5).max(monitor_top),
    )
}

fn default_outer_width(monitor_width: u32, scale: f64) -> u32 {
    (monitor_width / 3).max((420.0 * scale).round() as u32)
}

fn default_position(
    monitor_left: i32,
    monitor_top: i32,
    monitor_width: u32,
    dock_top: i32,
    window_width: u32,
    window_height: u32,
    slot: usize,
) -> (i32, i32) {
    let columns = (monitor_width / window_width.max(1)).max(1) as usize;
    let column = slot % columns;
    let row = slot / columns;
    let group_width = (columns as u32 * window_width).min(monitor_width);
    let group_left = monitor_left + (monitor_width - group_width) as i32 / 2;
    let left = group_left + column as i32 * window_width as i32;
    let base_top = dock_top - window_height as i32;
    let top = (base_top - row as i32 * 32).max(monitor_top);
    (left, top)
}

fn right_panel_position(
    monitor_left: i32,
    monitor_top: i32,
    monitor_width: u32,
    dock_top: i32,
    window_width: u32,
    window_height: u32,
    slot: usize,
) -> (i32, i32) {
    let columns = (monitor_width / window_width.max(1)).max(1) as usize;
    let column = slot % columns;
    (
        monitor_left + monitor_width.saturating_sub(window_width) as i32
            - column as i32 * window_width as i32,
        (dock_top - window_height as i32).max(monitor_top),
    )
}

#[cfg(test)]
mod tests {
    use super::{default_outer_width, default_position, launcher_position, right_panel_position};

    #[test]
    fn defaults_to_one_third_of_a_wide_monitor() {
        assert_eq!(default_outer_width(3440, 1.0), 1146);
    }

    #[test]
    fn keeps_the_minimum_width_on_small_or_scaled_monitors() {
        assert_eq!(default_outer_width(900, 1.0), 420);
        assert_eq!(default_outer_width(1500, 1.5), 630);
    }

    #[test]
    fn anchors_the_launcher_above_the_left_edge_of_the_dock() {
        assert_eq!(launcher_position(0, 0, 3440, 0, 1400, 420, 380), (5, 1015));
        assert_eq!(
            launcher_position(-1920, 0, 1920, -1920, 1040, 420, 380),
            (-1915, 655)
        );
    }

    #[test]
    fn assigns_the_first_three_windows_to_separate_columns() {
        assert_eq!(default_position(0, 0, 3440, 1352, 1146, 700, 0), (1, 652));
        assert_eq!(
            default_position(0, 0, 3440, 1352, 1146, 700, 1),
            (1147, 652)
        );
        assert_eq!(
            default_position(0, 0, 3440, 1352, 1146, 700, 2),
            (2293, 652)
        );
    }

    #[test]
    fn anchors_the_web_panel_to_the_right_above_the_dock() {
        assert_eq!(
            right_panel_position(0, 0, 3440, 1352, 1146, 900, 0),
            (2294, 452)
        );
        assert_eq!(
            right_panel_position(0, 0, 3440, 1352, 1146, 900, 1),
            (1148, 452)
        );
    }
}
