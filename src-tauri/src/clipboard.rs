#[cfg(windows)]
mod platform {
    use std::{ptr, slice, thread, time::Duration};

    use windows_sys::Win32::{
        Foundation::GlobalFree,
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        },
        UI::WindowsAndMessaging::GetForegroundWindow,
    };

    const CF_UNICODETEXT: u32 = 13;
    const MAX_CLIPBOARD_TEXT_BYTES: usize = 64 * 1024 * 1024;

    struct ClipboardGuard;

    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    fn open_clipboard() -> Result<ClipboardGuard, String> {
        for _ in 0..10 {
            if unsafe { OpenClipboard(GetForegroundWindow()) } != 0 {
                return Ok(ClipboardGuard);
            }
            thread::sleep(Duration::from_millis(3));
        }
        Err(format!(
            "Windows clipboard is busy: {}",
            std::io::Error::last_os_error()
        ))
    }

    pub fn read_text() -> Result<Option<String>, String> {
        let _clipboard = open_clipboard()?;
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if handle.is_null() {
            return Ok(None);
        }
        let byte_count = unsafe { GlobalSize(handle) };
        if !(2..=MAX_CLIPBOARD_TEXT_BYTES).contains(&byte_count) {
            return Ok(None);
        }
        let text_pointer = unsafe { GlobalLock(handle) }.cast::<u16>();
        if text_pointer.is_null() {
            return Err(format!(
                "Could not read the Windows clipboard: {}",
                std::io::Error::last_os_error()
            ));
        }
        let units = unsafe { slice::from_raw_parts(text_pointer, byte_count / 2) };
        let text_length = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..text_length]);
        unsafe {
            GlobalUnlock(handle);
        }
        Ok(Some(text))
    }

    pub fn write_text(text: &str) -> Result<(), String> {
        let wide = text
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let byte_count = wide
            .len()
            .checked_mul(size_of::<u16>())
            .ok_or_else(|| "Clipboard text is too large".to_string())?;
        if byte_count > MAX_CLIPBOARD_TEXT_BYTES {
            return Err("Clipboard text is too large".to_string());
        }

        let allocation = unsafe { GlobalAlloc(GMEM_MOVEABLE, byte_count) };
        if allocation.is_null() {
            return Err(format!(
                "Could not allocate Windows clipboard memory: {}",
                std::io::Error::last_os_error()
            ));
        }
        let destination = unsafe { GlobalLock(allocation) }.cast::<u16>();
        if destination.is_null() {
            unsafe {
                GlobalFree(allocation);
            }
            return Err(format!(
                "Could not prepare Windows clipboard text: {}",
                std::io::Error::last_os_error()
            ));
        }
        unsafe {
            ptr::copy_nonoverlapping(wide.as_ptr(), destination, wide.len());
            GlobalUnlock(allocation);
        }

        let clipboard = match open_clipboard() {
            Ok(clipboard) => clipboard,
            Err(error) => {
                unsafe {
                    GlobalFree(allocation);
                }
                return Err(error);
            }
        };
        if unsafe { EmptyClipboard() } == 0 {
            unsafe {
                GlobalFree(allocation);
            }
            return Err(format!(
                "Could not clear the Windows clipboard: {}",
                std::io::Error::last_os_error()
            ));
        }
        if unsafe { SetClipboardData(CF_UNICODETEXT, allocation) }.is_null() {
            unsafe {
                GlobalFree(allocation);
            }
            return Err(format!(
                "Could not write the Windows clipboard: {}",
                std::io::Error::last_os_error()
            ));
        }
        drop(clipboard);
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn read_text() -> Result<Option<String>, String> {
        Err("Native clipboard access is not available on this platform".to_string())
    }

    pub fn write_text(_text: &str) -> Result<(), String> {
        Err("Native clipboard access is not available on this platform".to_string())
    }
}

pub use platform::{read_text, write_text};
