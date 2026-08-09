#[cfg(windows)]
mod platform {
    use std::ptr::null;

    use anyhow::{Result, anyhow};

    type Hwnd = isize;

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

    pub fn set_visible(visible: bool) -> Result<()> {
        let windows = taskbar_windows();
        if windows.is_empty() {
            return Err(anyhow!("Windows taskbar was not found"));
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
