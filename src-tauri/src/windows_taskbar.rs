#[cfg(windows)]
mod platform {
    use std::{
        mem::size_of,
        ptr::null,
        sync::atomic::{AtomicU32, Ordering},
    };

    use anyhow::{Result, anyhow};

    type Hwnd = isize;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct AppBarData {
        cb_size: u32,
        hwnd: Hwnd,
        callback_message: u32,
        edge: u32,
        rect: Rect,
        lparam: isize,
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHAppBarMessage(message: u32, data: *mut AppBarData) -> usize;
    }

    const ABM_GETSTATE: u32 = 4;
    const ABM_SETSTATE: u32 = 10;
    const ABS_AUTOHIDE: u32 = 0x0000_0001;

    /// Sentinel meaning nothing has been saved yet.
    const NO_SAVED_STATE: u32 = u32::MAX;
    static SAVED_STATE: AtomicU32 = AtomicU32::new(NO_SAVED_STATE);

    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
        fn FindWindowExW(
            parent: Hwnd,
            child_after: Hwnd,
            class_name: *const u16,
            window_name: *const u16,
        ) -> Hwnd;
        fn IsWindowVisible(window: Hwnd) -> i32;
        fn ShowWindowAsync(window: Hwnd, command: i32) -> i32;
    }

    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;

    pub fn is_visible() -> bool {
        taskbar_windows()
            .iter()
            .any(|window| unsafe { IsWindowVisible(*window) } != 0)
    }

    /// Reads the taskbar's autohide / always-on-top flags.
    fn state() -> u32 {
        let mut data = AppBarData {
            cb_size: size_of::<AppBarData>() as u32,
            ..AppBarData::default()
        };
        unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) as u32 }
    }

    fn set_state(state: u32) {
        let mut data = AppBarData {
            cb_size: size_of::<AppBarData>() as u32,
            lparam: state as isize,
            ..AppBarData::default()
        };
        unsafe {
            SHAppBarMessage(ABM_SETSTATE, &mut data);
        }
    }

    /// Releases or reinstates the screen space Explorer reserves for the taskbar.
    ///
    /// Hiding the taskbar window leaves its AppBar reservation registered, so Explorer
    /// keeps clamping the work area to the bar it is no longer drawing and every
    /// maximized window stops short of the screen edge. Auto-hide is the state that
    /// actually surrenders the reservation, so collapse switches the taskbar into it
    /// and expanding puts back whatever was in force beforehand.
    fn set_space_reserved(reserved: bool) {
        if reserved {
            let saved = SAVED_STATE.swap(NO_SAVED_STATE, Ordering::AcqRel);
            if saved != NO_SAVED_STATE {
                set_state(saved);
            }
            return;
        }

        // Only the first collapse records a baseline; later ones would record the
        // auto-hide state we just installed and lose the user's real preference.
        let _ = SAVED_STATE.compare_exchange(
            NO_SAVED_STATE,
            state(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        set_state(ABS_AUTOHIDE);
    }

    pub fn set_visible(visible: bool) -> Result<()> {
        let windows = taskbar_windows();
        if windows.is_empty() {
            return Err(anyhow!("Windows taskbar was not found"));
        }

        // Surrender the reservation before hiding, and reinstate it after showing, so
        // the work area is never left describing a bar that is not on screen.
        if !visible {
            set_space_reserved(false);
        }

        let command = if visible { SW_SHOW } else { SW_HIDE };
        let mut failed = 0;
        for window in windows {
            if unsafe { ShowWindowAsync(window, command) } == 0 {
                failed += 1;
            }
        }
        if failed > 0 {
            return Err(anyhow!(
                "Windows could not update {failed} taskbar window(s)"
            ));
        }

        if visible {
            set_space_reserved(true);
        }
        Ok(())
    }

    fn taskbar_windows() -> Vec<Hwnd> {
        let primary_class = wide("Shell_TrayWnd");
        let secondary_class = wide("Shell_SecondaryTrayWnd");
        let mut windows = Vec::new();
        let primary = unsafe { FindWindowW(primary_class.as_ptr(), null()) };
        if primary != 0 {
            windows.push(primary);
        }

        let mut previous = 0;
        loop {
            let window = unsafe { FindWindowExW(0, previous, secondary_class.as_ptr(), null()) };
            if window == 0 {
                break;
            }
            windows.push(window);
            previous = window;
        }
        windows
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::wide;

        #[test]
        fn window_classes_are_null_terminated() {
            let value = wide("Shell_TrayWnd");
            assert_eq!(value.last(), Some(&0));
            assert_eq!(value.len(), "Shell_TrayWnd".encode_utf16().count() + 1);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use anyhow::{Result, bail};

    pub fn is_visible() -> bool {
        true
    }

    pub fn set_visible(_visible: bool) -> Result<()> {
        bail!("Windows taskbar controls are only available on Windows")
    }
}

pub use platform::{is_visible, set_visible};
