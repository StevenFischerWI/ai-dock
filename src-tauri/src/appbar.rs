#[cfg(windows)]
mod platform {
    use std::{
        ffi::c_void,
        mem::size_of,
        sync::{
            Mutex, OnceLock,
            atomic::{AtomicBool, AtomicIsize, Ordering},
        },
    };

    use anyhow::{Context, Result, anyhow};
    use tauri::WebviewWindow;

    type Hwnd = isize;
    type Hmonitor = isize;
    type Wparam = usize;
    type Lparam = isize;
    type Lresult = isize;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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
        lparam: Lparam,
    }

    #[repr(C)]
    struct MonitorInfo {
        cb_size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
    }

    type SubclassProc =
        Option<unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam, usize, usize) -> Lresult>;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHAppBarMessage(message: u32, data: *mut AppBarData) -> usize;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDpiForWindow(hwnd: Hwnd) -> u32;
        fn GetMonitorInfoW(monitor: Hmonitor, info: *mut MonitorInfo) -> i32;
        fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
        fn MonitorFromWindow(hwnd: Hwnd, flags: u32) -> Hmonitor;
        fn RegisterWindowMessageW(value: *const u16) -> u32;
        fn SetWindowPos(
            hwnd: Hwnd,
            insert_after: Hwnd,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
        fn SystemParametersInfoW(
            action: u32,
            parameter: u32,
            value: *mut c_void,
            update: u32,
        ) -> i32;
    }

    #[link(name = "comctl32")]
    unsafe extern "system" {
        fn DefSubclassProc(hwnd: Hwnd, message: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
        fn RemoveWindowSubclass(hwnd: Hwnd, callback: SubclassProc, subclass_id: usize) -> i32;
        fn SetWindowSubclass(
            hwnd: Hwnd,
            callback: SubclassProc,
            subclass_id: usize,
            reference_data: usize,
        ) -> i32;
    }

    const ABM_NEW: u32 = 0;
    const ABM_REMOVE: u32 = 1;
    const ABM_QUERYPOS: u32 = 2;
    const ABM_SETPOS: u32 = 3;
    const ABM_GETTASKBARPOS: u32 = 5;
    const ABM_ACTIVATE: u32 = 6;
    const ABM_WINDOWPOSCHANGED: u32 = 9;
    const ABN_POSCHANGED: usize = 1;
    const ABE_LEFT: u32 = 0;
    const ABE_TOP: u32 = 1;
    const ABE_RIGHT: u32 = 2;
    const ABE_BOTTOM: u32 = 3;
    const APPBAR_CALLBACK: u32 = 0x8000 + 0x51;
    const MONITOR_DEFAULTTOPRIMARY: u32 = 2;
    const HWND_TOPMOST: Hwnd = -1;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const SPI_SETWORKAREA: u32 = 0x002F;
    const SPI_GETWORKAREA: u32 = 0x0030;
    const MONITORINFOF_PRIMARY: u32 = 1;
    const WM_DISPLAYCHANGE: u32 = 0x007E;
    const WM_ACTIVATE: u32 = 0x0006;
    const WM_WINDOWPOSCHANGED: u32 = 0x0047;
    const WM_NCDESTROY: u32 = 0x0082;
    const WM_DPICHANGED: u32 = 0x02E0;
    const SUBCLASS_ID: usize = 0xA1D0_C001;

    static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
    static PREVIOUS_FOREGROUND: AtomicIsize = AtomicIsize::new(0);
    static WORK_AREA_OVERRIDDEN: AtomicBool = AtomicBool::new(false);
    // Positioning the dock reenters itself: SetWindowPos raises WM_WINDOWPOSCHANGED,
    // whose ABM_WINDOWPOSCHANGED makes Explorer answer with ABN_POSCHANGED, which
    // positions again. Run the body once and let that fall through.
    //
    // The guard only catches reentry on one stack. ABN_POSCHANGED arrives posted, so
    // the cycle also runs turn by turn through the message loop, where every pass sees
    // a released guard. Nothing but leaving Explorer's state alone when it is already
    // correct ends that, which is why the calls below are all conditional.
    static POSITIONING: AtomicBool = AtomicBool::new(false);
    // The appbar rectangle last handed to Explorer, so an unchanged position costs
    // neither an ABM_SETPOS nor the ABN_POSCHANGED it would come back as.
    static LAST_APPBAR_RECT: Mutex<Option<Rect>> = Mutex::new(None);
    // Where the dock belongs depends on whether the taskbar is showing, and the two
    // answers sit a dock height apart. Collapsing goes through ShowWindowAsync, so
    // asking the window during the notification storm that same call provokes returns
    // whichever answer it has reached, and the dock alternates between both
    // placements. Track what was asked for and let that decide instead.
    static TASKBAR_VISIBLE: AtomicBool = AtomicBool::new(true);

    pub struct AppBarManager {
        hwnd: Hwnd,
        height_dip: f64,
    }

    // The HWND belongs to Tauri's UI thread and is used only by Win32 calls.
    unsafe impl Send for AppBarManager {}
    unsafe impl Sync for AppBarManager {}

    impl AppBarManager {
        pub fn register(window: &WebviewWindow, height_dip: f64) -> Result<Self> {
            let hwnd = window.hwnd().context("getting dock HWND")?.0 as Hwnd;
            let height_reference = height_dip.round().clamp(32.0, 64.0) as usize;
            let subclassed = unsafe {
                SetWindowSubclass(
                    hwnd,
                    Some(appbar_subclass_proc),
                    SUBCLASS_ID,
                    height_reference,
                )
            };
            if subclassed == 0 {
                return Err(anyhow!("SetWindowSubclass failed"));
            }

            register_raw(hwnd, height_reference as f64)?;
            Ok(Self {
                hwnd,
                height_dip: height_reference as f64,
            })
        }

        pub fn reposition(&self, taskbar_visible: bool) -> Result<()> {
            // The caller just asked Windows for this state, so it outranks anything
            // the taskbar window reports while that request is still landing.
            TASKBAR_VISIBLE.store(taskbar_visible, Ordering::Release);
            position_raw(self.hwnd, self.height_dip, taskbar_visible)
        }

        pub fn take_was_previous_foreground(&self, window: &WebviewWindow) -> Result<bool> {
            let target = window.hwnd().context("getting terminal HWND")?.0 as Hwnd;
            Ok(PREVIOUS_FOREGROUND.swap(0, Ordering::AcqRel) == target)
        }

        pub fn take_was_previous_handle(&self, handle: &str) -> Result<bool> {
            let target: Hwnd = handle.parse().context("invalid window handle")?;
            Ok(PREVIOUS_FOREGROUND.swap(0, Ordering::AcqRel) == target)
        }
    }

    impl Drop for AppBarManager {
        fn drop(&mut self) {
            let mut data = appbar_data(self.hwnd);
            unsafe {
                SHAppBarMessage(ABM_REMOVE, &mut data);
                RemoveWindowSubclass(self.hwnd, Some(appbar_subclass_proc), SUBCLASS_ID);
            }
            let _ = restore_primary_work_area(self.hwnd);
        }
    }

    unsafe extern "system" fn appbar_subclass_proc(
        hwnd: Hwnd,
        message: u32,
        wparam: Wparam,
        lparam: Lparam,
        _subclass_id: usize,
        reference_data: usize,
    ) -> Lresult {
        let taskbar_created = taskbar_created_message();
        if message == taskbar_created {
            let _ = register_raw(hwnd, reference_data as f64);
        } else if (message == APPBAR_CALLBACK && wparam == ABN_POSCHANGED)
            || message == WM_DISPLAYCHANGE
            || message == WM_DPICHANGED
        {
            let _ = position_raw(
                hwnd,
                reference_data as f64,
                TASKBAR_VISIBLE.load(Ordering::Acquire),
            );
        } else if message == WM_ACTIVATE {
            if (wparam & 0xffff) != 0 && lparam != 0 {
                PREVIOUS_FOREGROUND.store(lparam, Ordering::Release);
            }
            let mut data = appbar_data(hwnd);
            data.lparam = isize::from((wparam & 0xffff) != 0);
            unsafe {
                SHAppBarMessage(ABM_ACTIVATE, &mut data);
            }
        } else if message == WM_WINDOWPOSCHANGED {
            let mut data = appbar_data(hwnd);
            unsafe {
                SHAppBarMessage(ABM_WINDOWPOSCHANGED, &mut data);
            }
        } else if message == WM_NCDESTROY {
            let mut data = appbar_data(hwnd);
            unsafe {
                SHAppBarMessage(ABM_REMOVE, &mut data);
                RemoveWindowSubclass(hwnd, Some(appbar_subclass_proc), SUBCLASS_ID);
            }
            let _ = restore_primary_work_area(hwnd);
        }

        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }

    fn register_raw(hwnd: Hwnd, height_dip: f64) -> Result<()> {
        // Clean up a stale registration if Windows has reused an HWND after an
        // ungraceful exit, then register this exact window from a known state.
        let mut stale = appbar_data(hwnd);
        unsafe {
            SHAppBarMessage(ABM_REMOVE, &mut stale);
        }
        // Explorer no longer holds a position for this window, so the next pass has to
        // publish one even if it matches what the previous registration used.
        *LAST_APPBAR_RECT.lock().unwrap_or_else(|e| e.into_inner()) = None;

        let mut data = appbar_data(hwnd);
        data.callback_message = APPBAR_CALLBACK;
        let result = unsafe { SHAppBarMessage(ABM_NEW, &mut data) };
        if result == 0 {
            return Err(anyhow!("SHAppBarMessage(ABM_NEW) failed"));
        }
        // Registration and an Explorer restart are the moments the real taskbar state
        // is settled and authoritative, so resynchronise the tracked intent here.
        let visible = crate::windows_taskbar::is_visible();
        TASKBAR_VISIBLE.store(visible, Ordering::Release);
        position_raw(hwnd, height_dip, visible)
    }

    fn position_raw(hwnd: Hwnd, height_dip: f64, taskbar_visible: bool) -> Result<()> {
        if POSITIONING.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let result = position_locked(hwnd, height_dip, taskbar_visible);
        POSITIONING.store(false, Ordering::Release);
        result
    }

    fn position_locked(hwnd: Hwnd, height_dip: f64, taskbar_visible: bool) -> Result<()> {
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) };
        if monitor == 0 {
            return Err(anyhow!("MonitorFromWindow failed"));
        }
        let mut monitor_info = MonitorInfo {
            cb_size: size_of::<MonitorInfo>() as u32,
            monitor: Rect::default(),
            work: Rect::default(),
            flags: 0,
        };
        if unsafe { GetMonitorInfoW(monitor, &mut monitor_info) } == 0 {
            return Err(anyhow!("GetMonitorInfoW failed"));
        }

        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let height = (height_dip * f64::from(dpi) / 96.0).round() as i32;
        let mut data = appbar_data(hwnd);
        data.edge = ABE_BOTTOM;
        data.rect = monitor_info.monitor;
        data.rect.top = data.rect.bottom - height;
        unsafe {
            SHAppBarMessage(ABM_QUERYPOS, &mut data);
        }
        data.rect.top = data.rect.bottom - height;

        let window_rect = if taskbar_visible {
            data.rect
        } else {
            Rect {
                left: monitor_info.monitor.left,
                top: monitor_info.monitor.bottom - height,
                right: monitor_info.monitor.right,
                bottom: monitor_info.monitor.bottom,
            }
        };

        // Register the rectangle the dock actually occupies rather than the one
        // ABM_QUERYPOS approved. With the taskbar collapsed those differ, because
        // Explorer keeps the hidden taskbar's reservation and answers as though the
        // dock still had to sit above it. Publishing the real position lets Explorer
        // derive a work area that describes the layout on screen, so there is nothing
        // left for us to contradict -- and contradicting it never held anyway, since
        // Explorer recomputes from that table and overwrites whatever we set.
        data.rect = window_rect;
        // Explorer answers every ABM_SETPOS with an ABN_POSCHANGED, so republishing a
        // rectangle it already holds keeps the cycle alive between message-loop turns.
        // Only speak up when the position actually moved.
        {
            let mut last = LAST_APPBAR_RECT.lock().unwrap_or_else(|e| e.into_inner());
            if *last != Some(data.rect) {
                unsafe {
                    SHAppBarMessage(ABM_SETPOS, &mut data);
                }
                *last = Some(data.rect);
            }
        }
        // Moving the window raises WM_WINDOWPOSCHANGED, which reports to Explorer and
        // earns another ABN_POSCHANGED. Doing that unconditionally meant any nudge --
        // minimizing an unrelated window is enough, by way of WM_ACTIVATE -- started a
        // reposition that kept feeding itself. Touch the window only to change it.
        if window_rect != current_window_rect(hwnd) {
            let positioned = unsafe {
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    window_rect.left,
                    window_rect.top,
                    window_rect.right - window_rect.left,
                    window_rect.bottom - window_rect.top,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
            };
            if positioned == 0 {
                return Err(anyhow!("SetWindowPos failed"));
            }
        }
        Ok(())
    }

    /// The window's present screen rectangle, or a sentinel that compares unequal to
    /// any real position so a failed read still lets the move through.
    fn current_window_rect(hwnd: Hwnd) -> Rect {
        let mut rect = Rect::default();
        if unsafe { GetWindowRect(hwnd, &raw mut rect) } == 0 {
            return Rect {
                left: i32::MIN,
                top: i32::MIN,
                right: i32::MIN,
                bottom: i32::MIN,
            };
        }
        rect
    }

    /// Puts back a work area published by an older build of this app.
    ///
    /// Nothing sets that override any more -- the AppBar registration alone tells
    /// Explorer where the dock is, and a value it disagrees with never survived its
    /// next recalculation anyway. This remains so an upgrade over a version that did
    /// leave one behind still hands the desktop back intact.
    fn restore_primary_work_area(hwnd: Hwnd) -> Result<()> {
        if !WORK_AREA_OVERRIDDEN.swap(false, Ordering::AcqRel) {
            return Ok(());
        }

        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) };
        if monitor == 0 {
            return Err(anyhow!(
                "MonitorFromWindow failed while restoring work area"
            ));
        }
        let monitor_info = read_monitor_info(monitor)?;
        if monitor_info.flags & MONITORINFOF_PRIMARY == 0 {
            return Ok(());
        }

        let mut work = monitor_info.monitor;
        if crate::windows_taskbar::is_visible() {
            let mut taskbar = appbar_data(0);
            let found = unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut taskbar) } != 0;
            if found {
                work = work_area_with_appbar(work, taskbar.edge, taskbar.rect);
            }
        }
        set_primary_work_area(work)
    }

    fn read_monitor_info(monitor: Hmonitor) -> Result<MonitorInfo> {
        let mut monitor_info = MonitorInfo {
            cb_size: size_of::<MonitorInfo>() as u32,
            monitor: Rect::default(),
            work: Rect::default(),
            flags: 0,
        };
        if unsafe { GetMonitorInfoW(monitor, &mut monitor_info) } == 0 {
            return Err(anyhow!("GetMonitorInfoW failed"));
        }
        Ok(monitor_info)
    }

    fn set_primary_work_area(mut work: Rect) -> Result<()> {
        let mut current = Rect::default();
        let read_current = unsafe {
            SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut current).cast::<c_void>(), 0)
        };
        if read_current != 0 && current == work {
            return Ok(());
        }

        // Do not use SPIF_SENDCHANGE here. That flag synchronously broadcasts
        // WM_SETTINGCHANGE from Tauri's single UI thread. A display/DPI change
        // can re-enter one of our own window procedures (or encounter another
        // hung desktop window) and freeze every AI Dock window indefinitely.
        // SPI_SETWORKAREA updates Windows' work-area state before returning;
        // Aero Snap and FancyZones read that state directly.
        let updated = unsafe {
            SystemParametersInfoW(SPI_SETWORKAREA, 0, (&raw mut work).cast::<c_void>(), 0)
        };
        if updated == 0 {
            return Err(anyhow!("SystemParametersInfoW(SPI_SETWORKAREA) failed"));
        }
        Ok(())
    }

    fn work_area_with_appbar(mut work: Rect, edge: u32, appbar: Rect) -> Rect {
        match edge {
            ABE_LEFT => work.left = work.left.max(appbar.right),
            ABE_TOP => work.top = work.top.max(appbar.bottom),
            ABE_RIGHT => work.right = work.right.min(appbar.left),
            ABE_BOTTOM => work.bottom = work.bottom.min(appbar.top),
            _ => {}
        }
        work
    }

    fn appbar_data(hwnd: Hwnd) -> AppBarData {
        AppBarData {
            cb_size: size_of::<AppBarData>() as u32,
            hwnd,
            ..AppBarData::default()
        }
    }

    fn taskbar_created_message() -> u32 {
        *TASKBAR_CREATED.get_or_init(|| {
            let wide: Vec<u16> = "TaskbarCreated".encode_utf16().chain(Some(0)).collect();
            unsafe { RegisterWindowMessageW(wide.as_ptr()) }
        })
    }

    #[allow(dead_code)]
    fn _assert_pointer_width() {
        let _ = size_of::<*mut c_void>();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn restoring_work_area_accounts_for_a_bottom_taskbar() {
            let monitor = Rect {
                left: 0,
                top: 0,
                right: 3440,
                bottom: 1440,
            };
            let taskbar = Rect {
                top: 1392,
                ..monitor
            };

            assert_eq!(
                work_area_with_appbar(monitor, ABE_BOTTOM, taskbar),
                Rect {
                    bottom: 1392,
                    ..monitor
                }
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use anyhow::Result;
    use tauri::WebviewWindow;

    pub struct AppBarManager;

    impl AppBarManager {
        pub fn register(_window: &WebviewWindow, _height_dip: f64) -> Result<Self> {
            Ok(Self)
        }

        pub fn reposition(&self, _taskbar_visible: bool) -> Result<()> {
            Ok(())
        }

        pub fn take_was_previous_foreground(&self, _window: &WebviewWindow) -> Result<bool> {
            Ok(false)
        }

        pub fn take_was_previous_handle(&self, _handle: &str) -> Result<bool> {
            Ok(false)
        }
    }
}

pub use platform::AppBarManager;
