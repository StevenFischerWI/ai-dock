use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveWindowApp {
    pub handle: String,
    pub app_key: String,
    pub app_name: String,
    pub title: String,
    pub icon_data_url: Option<String>,
    pub is_focused: bool,
    pub is_minimized: bool,
    #[serde(skip_serializing)]
    pub process_id: u32,
    #[serde(skip_serializing)]
    pub executable_path: String,
}

fn prefers_executable_icon(app_key: &str) -> bool {
    app_key.eq_ignore_ascii_case("chrome.exe")
}

#[cfg(windows)]
#[allow(unsafe_op_in_unsafe_fn)]
mod platform {
    use std::{
        collections::HashMap,
        ffi::{OsStr, c_void},
        os::windows::ffi::OsStrExt,
        path::Path,
        sync::{Mutex, OnceLock},
    };

    use anyhow::{Context, Result};
    use base64::{Engine, engine::general_purpose::STANDARD};

    use super::{ActiveWindowApp, prefers_executable_icon};

    type Hwnd = isize;
    type Handle = isize;

    const GWL_EXSTYLE: i32 = -20;
    const GW_OWNER: u32 = 4;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_APPWINDOW: isize = 0x0004_0000;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const DWMWA_CLOAKED: u32 = 14;
    const SW_SHOW: i32 = 5;
    const SW_RESTORE: i32 = 9;
    const WM_GETICON: u32 = 0x007f;
    const WM_CLOSE: u32 = 0x0010;
    const VK_LWIN: u8 = 0x5b;
    const VK_S: u8 = 0x53;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const ICON_SMALL: usize = 0;
    const ICON_SMALL2: usize = 2;
    const GCLP_HICON: i32 = -14;
    const GCLP_HICONSM: i32 = -34;
    const SMTO_ABORTIFHUNG: u32 = 0x0002;
    const DI_NORMAL: u32 = 0x0003;
    const SHGFI_ICON: u32 = 0x0000_0100;
    const SHGFI_SMALLICON: u32 = 0x0000_0001;
    const DIB_RGB_COLORS: u32 = 0;
    const ICON_SIZE: usize = 32;

    static ICON_CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

    // Mark intentionally hidden third-party windows on the HWND itself. Unlike
    // process memory, this survives an AI Dock UI recovery, so a hidden app's
    // icon remains available after the dock restarts.
    const HIDDEN_WINDOW_PROPERTY: [u16; 25] = [
        65, 73, 95, 68, 79, 67, 75, 95, 73, 78, 83, 84, 65, 78, 84, 76, 89, 95, 72, 73, 68, 68, 69,
        78, 0,
    ];

    #[repr(C)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_pixels_per_meter: i32,
        y_pixels_per_meter: i32,
        colors_used: u32,
        colors_important: u32,
    }

    #[repr(C)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        colors: [u32; 1],
    }

    #[repr(C)]
    struct ShellFileInfo {
        icon: Hwnd,
        icon_index: i32,
        attributes: u32,
        display_name: [u16; 260],
        type_name: [u16; 80],
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn AttachThreadInput(thread: u32, attach_to: u32, attach: i32) -> i32;
        fn BringWindowToTop(hwnd: Hwnd) -> i32;
        fn EnumWindows(callback: unsafe extern "system" fn(Hwnd, isize) -> i32, data: isize)
        -> i32;
        fn GetClassNameW(hwnd: Hwnd, class_name: *mut u16, max_count: i32) -> i32;
        fn GetForegroundWindow() -> Hwnd;
        fn GetWindow(hwnd: Hwnd, command: u32) -> Hwnd;
        fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
        fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, title: *mut u16, max_count: i32) -> i32;
        fn GetWindowThreadProcessId(hwnd: Hwnd, process_id: *mut u32) -> u32;
        fn GetPropW(hwnd: Hwnd, name: *const u16) -> Handle;
        fn IsIconic(hwnd: Hwnd) -> i32;
        fn IsWindow(hwnd: Hwnd) -> i32;
        fn IsWindowVisible(hwnd: Hwnd) -> i32;
        fn PostMessageW(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
        fn RemovePropW(hwnd: Hwnd, name: *const u16) -> Handle;
        fn SetPropW(hwnd: Hwnd, name: *const u16, value: Handle) -> i32;
        fn keybd_event(virtual_key: u8, scan_code: u8, flags: u32, extra_info: usize);
        fn DrawIconEx(
            hdc: Hwnd,
            x: i32,
            y: i32,
            icon: Hwnd,
            width: i32,
            height: i32,
            step: u32,
            brush: Hwnd,
            flags: u32,
        ) -> i32;
        fn DestroyIcon(icon: Hwnd) -> i32;
        fn GetClassLongPtrW(hwnd: Hwnd, index: i32) -> usize;
        fn GetDC(hwnd: Hwnd) -> Hwnd;
        fn ReleaseDC(hwnd: Hwnd, hdc: Hwnd) -> i32;
        fn SendMessageTimeoutW(
            hwnd: Hwnd,
            message: u32,
            wparam: usize,
            lparam: isize,
            flags: u32,
            timeout: u32,
            result: *mut usize,
        ) -> Hwnd;
        fn SetFocus(hwnd: Hwnd) -> Hwnd;
        fn SetForegroundWindow(hwnd: Hwnd) -> i32;
        fn ShowWindow(hwnd: Hwnd, command: i32) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateCompatibleDC(hdc: Hwnd) -> Hwnd;
        fn CreateDIBSection(
            hdc: Hwnd,
            info: *const BitmapInfo,
            usage: u32,
            bits: *mut *mut c_void,
            section: Handle,
            offset: u32,
        ) -> Hwnd;
        fn DeleteDC(hdc: Hwnd) -> i32;
        fn DeleteObject(object: Hwnd) -> i32;
        fn SelectObject(hdc: Hwnd, object: Hwnd) -> Hwnd;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetFileInfoW(
            path: *const u16,
            attributes: u32,
            file_info: *mut ShellFileInfo,
            file_info_size: u32,
            flags: u32,
        ) -> usize;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CloseHandle(handle: Handle) -> i32;
        fn GetCurrentProcessId() -> u32;
        fn GetCurrentThreadId() -> u32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            file_name: *mut u16,
            size: *mut u32,
        ) -> i32;
    }

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmGetWindowAttribute(
            hwnd: Hwnd,
            attribute: u32,
            value: *mut c_void,
            value_size: u32,
        ) -> i32;
    }

    pub fn list() -> Vec<ActiveWindowApp> {
        let mut windows: Vec<ActiveWindowApp> = Vec::new();
        unsafe {
            EnumWindows(
                enum_window,
                (&mut windows as *mut Vec<ActiveWindowApp>) as isize,
            );
        }
        windows.sort_by(|left, right| {
            left.app_key
                .cmp(&right.app_key)
                .then_with(|| left.handle.cmp(&right.handle))
        });
        windows
    }

    unsafe extern "system" fn enum_window(hwnd: Hwnd, data: isize) -> i32 {
        let Some(windows) = (data as *mut Vec<ActiveWindowApp>).as_mut() else {
            return 0;
        };
        let intentionally_hidden = is_intentionally_hidden(hwnd);
        if let Some(window) = window_info(hwnd, intentionally_hidden) {
            windows.push(window);
        }
        1
    }

    unsafe fn is_intentionally_hidden(hwnd: Hwnd) -> bool {
        GetPropW(hwnd, HIDDEN_WINDOW_PROPERTY.as_ptr()) != 0
    }

    unsafe fn window_info(hwnd: Hwnd, intentionally_hidden: bool) -> Option<ActiveWindowApp> {
        if (!intentionally_hidden && IsWindowVisible(hwnd) == 0) || is_cloaked(hwnd) {
            return None;
        }
        let class_name = read_class_name(hwnd);
        if matches!(
            class_name.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return None;
        }
        let extended_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let forced_taskbar = extended_style & WS_EX_APPWINDOW != 0;
        if !forced_taskbar
            && (extended_style & WS_EX_TOOLWINDOW != 0 || GetWindow(hwnd, GW_OWNER) != 0)
        {
            return None;
        }
        let title = read_window_text(hwnd);
        if title.is_empty() {
            return None;
        }
        let mut process_id = 0;
        GetWindowThreadProcessId(hwnd, &mut process_id);
        if process_id == 0 || process_id == GetCurrentProcessId() {
            return None;
        }
        let (executable, executable_path) = process_executable(process_id)?;
        let app_key = executable.to_ascii_lowercase();
        if app_key == "ai-dock.exe" {
            return None;
        }
        let app_name = executable
            .strip_suffix(".exe")
            .unwrap_or(&executable)
            .to_string();
        let icon_data_url = icon_data_url(&app_key, hwnd, &executable_path);
        Some(ActiveWindowApp {
            handle: hwnd.to_string(),
            app_key,
            app_name,
            title,
            icon_data_url,
            is_focused: GetForegroundWindow() == hwnd,
            // Reuse the minimized presentation for windows hidden by AI Dock.
            // Unlike real minimization, SW_HIDE/SW_SHOW has no shell animation.
            is_minimized: intentionally_hidden || IsIconic(hwnd) != 0,
            process_id,
            executable_path,
        })
    }

    unsafe fn is_cloaked(hwnd: Hwnd) -> bool {
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        ) >= 0
            && cloaked != 0
    }

    unsafe fn read_window_text(hwnd: Hwnd) -> String {
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
            .trim()
            .to_string()
    }

    unsafe fn read_class_name(hwnd: Hwnd) -> String {
        let mut buffer = vec![0u16; 256];
        let copied = GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
    }

    unsafe fn process_executable(process_id: u32) -> Option<(String, String)> {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
        if process == 0 {
            return None;
        }
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        let succeeded =
            QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) != 0;
        CloseHandle(process);
        if !succeeded {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        let executable = Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())?;
        Some((executable, path))
    }

    unsafe fn icon_data_url(app_key: &str, hwnd: Hwnd, executable_path: &str) -> Option<String> {
        let cache = ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(cached) = cache.lock().ok()?.get(app_key).cloned() {
            return cached;
        }

        // Chrome windows and installed Chrome PWAs may expose the current site's
        // favicon through WM_GETICON. Keep them visually grouped under Chrome.
        let mut icon = if prefers_executable_icon(app_key) {
            0
        } else {
            window_icon(hwnd)
        };
        let mut owned = false;
        if icon == 0 {
            let path: Vec<u16> = OsStr::new(executable_path)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut file_info: ShellFileInfo = std::mem::zeroed();
            if SHGetFileInfoW(
                path.as_ptr(),
                0,
                &mut file_info,
                std::mem::size_of::<ShellFileInfo>() as u32,
                SHGFI_ICON | SHGFI_SMALLICON,
            ) != 0
            {
                icon = file_info.icon;
                owned = icon != 0;
            }
        }
        let encoded = if icon == 0 { None } else { icon_png(icon) };
        if owned {
            DestroyIcon(icon);
        }
        if let Ok(mut cache) = cache.lock() {
            cache.insert(app_key.to_string(), encoded.clone());
        }
        encoded
    }

    unsafe fn window_icon(hwnd: Hwnd) -> Hwnd {
        for icon_type in [ICON_SMALL2, ICON_SMALL] {
            let mut result = 0usize;
            if SendMessageTimeoutW(
                hwnd,
                WM_GETICON,
                icon_type,
                0,
                SMTO_ABORTIFHUNG,
                50,
                &mut result,
            ) != 0
                && result != 0
            {
                return result as Hwnd;
            }
        }
        let small = GetClassLongPtrW(hwnd, GCLP_HICONSM);
        if small != 0 {
            small as Hwnd
        } else {
            GetClassLongPtrW(hwnd, GCLP_HICON) as Hwnd
        }
    }

    unsafe fn icon_png(icon: Hwnd) -> Option<String> {
        let screen_dc = GetDC(0);
        if screen_dc == 0 {
            return None;
        }
        let memory_dc = CreateCompatibleDC(screen_dc);
        if memory_dc == 0 {
            ReleaseDC(0, screen_dc);
            return None;
        }
        let info = BitmapInfo {
            header: BitmapInfoHeader {
                size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                width: ICON_SIZE as i32,
                height: -(ICON_SIZE as i32),
                planes: 1,
                bit_count: 32,
                compression: 0,
                size_image: (ICON_SIZE * ICON_SIZE * 4) as u32,
                x_pixels_per_meter: 0,
                y_pixels_per_meter: 0,
                colors_used: 0,
                colors_important: 0,
            },
            colors: [0],
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(screen_dc, &info, DIB_RGB_COLORS, &mut bits, 0, 0);
        if bitmap == 0 || bits.is_null() {
            if bitmap != 0 {
                DeleteObject(bitmap);
            }
            DeleteDC(memory_dc);
            ReleaseDC(0, screen_dc);
            return None;
        }
        let previous = SelectObject(memory_dc, bitmap);
        std::ptr::write_bytes(bits, 0, ICON_SIZE * ICON_SIZE * 4);
        let drawn = DrawIconEx(
            memory_dc,
            0,
            0,
            icon,
            ICON_SIZE as i32,
            ICON_SIZE as i32,
            0,
            0,
            DI_NORMAL,
        ) != 0;
        let mut rgba = vec![0u8; ICON_SIZE * ICON_SIZE * 4];
        if drawn {
            let bgra = std::slice::from_raw_parts(bits.cast::<u8>(), rgba.len());
            for (source, target) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
                target[0] = source[2];
                target[1] = source[1];
                target[2] = source[0];
                target[3] = if source[3] == 0 && source[..3].iter().any(|value| *value != 0) {
                    255
                } else {
                    source[3]
                };
            }
        }
        SelectObject(memory_dc, previous);
        DeleteObject(bitmap);
        DeleteDC(memory_dc);
        ReleaseDC(0, screen_dc);
        if !drawn {
            return None;
        }

        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, ICON_SIZE as u32, ICON_SIZE as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().ok()?;
            writer.write_image_data(&rgba).ok()?;
        }
        Some(format!("data:image/png;base64,{}", STANDARD.encode(png)))
    }

    pub fn activate(handle: &str, was_focused: bool) -> Result<bool> {
        let hwnd: Hwnd = handle.parse().context("invalid window handle")?;
        if hwnd == 0 || unsafe { IsWindow(hwnd) } == 0 {
            anyhow::bail!("That window is no longer open");
        }
        let was_hidden = unsafe { RemovePropW(hwnd, HIDDEN_WINDOW_PROPERTY.as_ptr()) != 0 };
        if was_hidden {
            unsafe {
                ShowWindow(
                    hwnd,
                    if IsIconic(hwnd) != 0 {
                        SW_RESTORE
                    } else {
                        SW_SHOW
                    },
                );
            }
            focus_hwnd(hwnd)?;
            return Ok(true);
        }
        let should_hide = was_focused || unsafe { GetForegroundWindow() } == hwnd;
        if should_hide {
            unsafe {
                if SetPropW(hwnd, HIDDEN_WINDOW_PROPERTY.as_ptr(), 1) == 0 {
                    anyhow::bail!("Windows could not mark that app as hidden");
                }
                ShowWindow(hwnd, 0); // SW_HIDE is immediate and avoids shell animations.
            }
            return Ok(false);
        }
        unsafe {
            ShowWindow(
                hwnd,
                if IsIconic(hwnd) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
        }
        focus_hwnd(hwnd)?;
        Ok(true)
    }

    pub fn focus_existing(handle: &str) -> Result<()> {
        let hwnd: Hwnd = handle.parse().context("invalid window handle")?;
        if hwnd == 0 || unsafe { IsWindow(hwnd) } == 0 {
            anyhow::bail!("That window is no longer open");
        }
        unsafe {
            RemovePropW(hwnd, HIDDEN_WINDOW_PROPERTY.as_ptr());
            ShowWindow(
                hwnd,
                if IsIconic(hwnd) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
        }
        focus_hwnd(hwnd)
    }

    pub fn close(handle: &str) -> Result<()> {
        let hwnd: Hwnd = handle.parse().context("invalid window handle")?;
        if hwnd == 0 || unsafe { IsWindow(hwnd) } == 0 {
            anyhow::bail!("That window is no longer open");
        }
        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut process_id);
        }
        if process_id == 0 || process_id == unsafe { GetCurrentProcessId() } {
            anyhow::bail!("AI Dock cannot close that window");
        }
        unsafe {
            RemovePropW(hwnd, HIDDEN_WINDOW_PROPERTY.as_ptr());
        }
        if unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } == 0 {
            anyhow::bail!("Windows could not send the close request");
        }
        Ok(())
    }

    pub fn launch(executable_path: &str) -> Result<()> {
        let path = Path::new(executable_path);
        if !path.is_file() {
            anyhow::bail!("That app is no longer installed at its recorded location");
        }
        std::process::Command::new(path)
            .spawn()
            .with_context(|| format!("launching {}", path.display()))?;
        Ok(())
    }

    pub fn open_start_search() {
        // Let the native AI Dock popup menu finish dismissing before handing
        // Win+S to Windows; otherwise the menu can consume the keystroke.
        std::thread::sleep(std::time::Duration::from_millis(120));
        unsafe {
            keybd_event(VK_LWIN, 0, 0, 0);
            keybd_event(VK_S, 0, 0, 0);
            keybd_event(VK_S, 0, KEYEVENTF_KEYUP, 0);
            keybd_event(VK_LWIN, 0, KEYEVENTF_KEYUP, 0);
        }
    }

    fn focus_hwnd(target: Hwnd) -> Result<()> {
        std::thread::sleep(std::time::Duration::from_millis(50));
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
            unsafe { AttachThreadInput(current_thread, target_thread, 0) };
        }
        if attached_foreground {
            unsafe { AttachThreadInput(current_thread, foreground_thread, 0) };
        }
        if !focused {
            // Windows can legally reject foreground ownership while still raising
            // and showing the requested window. This is a transient OS policy, not
            // an action failure worth surfacing across the dock. Leave the window
            // raised and let a subsequent click retry the activation handoff.
            unsafe {
                BringWindowToTop(target);
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    use anyhow::Result;

    use super::ActiveWindowApp;

    pub fn list() -> Vec<ActiveWindowApp> {
        Vec::new()
    }

    pub fn activate(_handle: &str, _was_focused: bool) -> Result<bool> {
        Ok(false)
    }

    pub fn close(_handle: &str) -> Result<()> {
        Ok(())
    }

    pub fn focus_existing(_handle: &str) -> Result<()> {
        Ok(())
    }

    pub fn launch(executable_path: &str) -> Result<()> {
        std::process::Command::new(executable_path).spawn()?;
        Ok(())
    }

    pub fn open_start_search() {}
}

pub use platform::{activate, close, focus_existing, launch, list, open_start_search};

#[cfg(test)]
mod tests {
    use super::prefers_executable_icon;

    #[test]
    fn chrome_windows_always_use_the_executable_icon() {
        assert!(prefers_executable_icon("chrome.exe"));
        assert!(prefers_executable_icon("CHROME.EXE"));
        assert!(!prefers_executable_icon("msedge.exe"));
        assert!(!prefers_executable_icon("notepad.exe"));
    }
}
