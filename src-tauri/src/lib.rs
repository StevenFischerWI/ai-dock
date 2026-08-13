mod appbar;
mod clipboard;
mod favicons;
mod models;
mod session;
mod session_host;
mod session_protocol;
mod settings;
mod watchdog;
mod windowing;
mod windows_apps;
mod windows_taskbar;

use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use anyhow::Context;
use models::{
    AppSettings, EditorContext, RecentWindowsApp, SessionDefinition, TerminalVisibilityPayload,
    ZenPlanEditorContext, ZenPlanNotebook, ZenPlanVisibilityPayload, default_working_directory,
    is_valid_color, is_valid_web_app_url,
};
use parking_lot::Mutex;
use session::{SessionManager, emit_state};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use uuid::Uuid;

pub struct AppState {
    settings_path: PathBuf,
    settings: Mutex<AppSettings>,
    sessions: SessionManager,
    editor_target: Mutex<(Option<Uuid>, Option<Uuid>)>,
    rename_target: Mutex<Option<Uuid>>,
    color_target: Mutex<Option<Uuid>>,
    zenplan_editor_target: Mutex<Option<Uuid>>,
    taskbar_hidden_by_ai_dock: Mutex<bool>,
    terminal_focus_loss: Mutex<Option<(Uuid, Instant)>>,
    zenplan_focus_loss: Mutex<Option<(Uuid, Instant)>>,
    launcher_focus_loss: Mutex<Option<Instant>>,
    launcher_opened_at: Mutex<Option<Instant>>,
    observed_window_app_instances: Mutex<HashSet<(String, u32)>>,
    ui_heartbeats: Arc<Mutex<HashMap<String, Instant>>>,
    monitored_webviews: Arc<Mutex<HashSet<String>>>,
    ui_restart_requested: Arc<AtomicBool>,
}

fn is_test_build() -> bool {
    option_env!("AI_DOCK_BUILD_FLAVOR") == Some("test")
}

impl AppState {
    fn new(settings_path: PathBuf) -> Self {
        let first_run = !settings_path.exists();
        let loaded_settings = settings::load(&settings_path);
        if first_run && let Err(error) = settings::save(&settings_path, &loaded_settings) {
            eprintln!("AI Dock could not save first-run settings: {error:#}");
        }
        let session_config_dir = settings_path
            .parent()
            .map_or_else(|| std::env::temp_dir().join("AiDock"), PathBuf::from);
        Self {
            settings: Mutex::new(loaded_settings),
            settings_path,
            sessions: SessionManager::new(session_config_dir),
            editor_target: Mutex::new((None, None)),
            rename_target: Mutex::new(None),
            color_target: Mutex::new(None),
            zenplan_editor_target: Mutex::new(None),
            taskbar_hidden_by_ai_dock: Mutex::new(false),
            terminal_focus_loss: Mutex::new(None),
            zenplan_focus_loss: Mutex::new(None),
            launcher_focus_loss: Mutex::new(None),
            launcher_opened_at: Mutex::new(None),
            observed_window_app_instances: Mutex::new(HashSet::new()),
            ui_heartbeats: Arc::new(Mutex::new(HashMap::new())),
            monitored_webviews: Arc::new(Mutex::new(HashSet::new())),
            ui_restart_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    fn save_and_emit(&self, app: &AppHandle) -> Result<(), String> {
        let settings = self.settings.lock().clone();
        settings::save(&self.settings_path, &settings).map_err(|error| format!("{error:#}"))?;
        app.emit("settings-changed", settings)
            .map_err(|error| error.to_string())
    }

    fn save(&self) -> Result<(), String> {
        let settings = self.settings.lock().clone();
        settings::save(&self.settings_path, &settings).map_err(|error| format!("{error:#}"))
    }

    fn definition(&self, session_id: Uuid) -> Result<SessionDefinition, String> {
        self.settings
            .lock()
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .cloned()
            .ok_or_else(|| "Session was not found".to_string())
    }

    fn restore_taskbar_if_needed(&self) {
        let mut hidden = self.taskbar_hidden_by_ai_dock.lock();
        if *hidden {
            let _ = windows_taskbar::set_visible(true);
            *hidden = false;
        }
    }

    fn record_terminal_focus_loss(&self, group_id: Uuid) {
        *self.terminal_focus_loss.lock() = Some((group_id, Instant::now()));
    }

    fn take_recent_terminal_focus_loss(&self, group_id: Uuid) -> bool {
        self.terminal_focus_loss
            .lock()
            .take()
            .is_some_and(|(lost_group_id, lost_at)| {
                lost_group_id == group_id && lost_at.elapsed() <= Duration::from_millis(500)
            })
    }

    fn record_zenplan_focus_loss(&self, notebook_id: Uuid) {
        *self.zenplan_focus_loss.lock() = Some((notebook_id, Instant::now()));
    }

    fn take_recent_zenplan_focus_loss(&self, notebook_id: Uuid) -> bool {
        self.zenplan_focus_loss
            .lock()
            .take()
            .is_some_and(|(lost_notebook_id, lost_at)| {
                lost_notebook_id == notebook_id && lost_at.elapsed() <= Duration::from_millis(500)
            })
    }

    fn record_launcher_focus_loss(&self) {
        *self.launcher_focus_loss.lock() = Some(Instant::now());
    }

    fn record_launcher_opened(&self) {
        *self.launcher_opened_at.lock() = Some(Instant::now());
    }

    fn launcher_is_settling(&self) -> bool {
        self.launcher_opened_at
            .lock()
            .is_some_and(|opened_at| opened_at.elapsed() <= Duration::from_millis(250))
    }

    fn take_recent_launcher_focus_loss(&self) -> bool {
        self.launcher_focus_loss
            .lock()
            .take()
            .is_some_and(|lost_at| lost_at.elapsed() <= Duration::from_millis(350))
    }
}

pub fn run_session_host() -> anyhow::Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let mut config_dir = None;
    while let Some(argument) = arguments.next() {
        if argument == "--config-dir" {
            config_dir = arguments.next().map(PathBuf::from);
        }
    }
    let config_dir = config_dir.context("the session host requires --config-dir")?;
    session_host::run(&config_dir)
}

pub fn run_ui_recovery_helper_if_requested() -> anyhow::Result<bool> {
    watchdog::run_recovery_helper_if_requested()
}

const TERMINAL_WINDOW_PREFIX: &str = "terminal-";
const ZENPLAN_WINDOW_PREFIX: &str = "zenplan-";
const WEBVIEW_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
const WEBVIEW_WATCHDOG_GRACE: Duration = Duration::from_secs(15);
const WEBVIEW_WATCHDOG_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DockWindowToggleAction {
    Restore,
    Hide,
    Focus,
    Show,
}

fn dock_window_toggle_action(
    is_minimized: bool,
    is_visible: bool,
    was_focused_before_dock_click: bool,
) -> DockWindowToggleAction {
    if is_minimized {
        DockWindowToggleAction::Restore
    } else if is_visible && was_focused_before_dock_click {
        DockWindowToggleAction::Hide
    } else if is_visible {
        DockWindowToggleAction::Focus
    } else {
        DockWindowToggleAction::Show
    }
}

fn terminal_window_label(group_id: Uuid) -> String {
    format!("{TERMINAL_WINDOW_PREFIX}{group_id}")
}

fn group_id_from_window_label(label: &str) -> Option<Uuid> {
    label
        .strip_prefix(TERMINAL_WINDOW_PREFIX)
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn zenplan_window_label(notebook_id: Uuid) -> String {
    format!("{ZENPLAN_WINDOW_PREFIX}{notebook_id}")
}

fn notebook_id_from_window_label(label: &str) -> Option<Uuid> {
    label
        .strip_prefix(ZENPLAN_WINDOW_PREFIX)
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn queue_web_app_favicon_refresh(app: AppHandle, notebook_id: Uuid, url: String) {
    tauri::async_runtime::spawn(async move {
        let Some(icon_data_url) = favicons::fetch_web_app_favicon(&url).await else {
            return;
        };
        let state = app.state::<AppState>();
        let changed = {
            let mut settings = state.settings.lock();
            let Some(notebook) = settings
                .zenplan_notebooks
                .iter_mut()
                .find(|notebook| notebook.id == notebook_id && notebook.url == url)
            else {
                return;
            };
            if notebook.icon_data_url.as_deref() == Some(&icon_data_url) {
                false
            } else {
                notebook.icon_data_url = Some(icon_data_url);
                true
            }
        };
        if changed && let Err(error) = state.save_and_emit(&app) {
            eprintln!("AI Dock could not cache a web app favicon: {error}");
        }
    });
}

fn emit_terminal_visibility(app: &AppHandle, group_id: Uuid, visible: bool) -> Result<(), String> {
    app.emit(
        "terminal-visibility",
        TerminalVisibilityPayload { group_id, visible },
    )
    .map_err(|error| error.to_string())
}

fn emit_zenplan_visibility(
    app: &AppHandle,
    notebook_id: Uuid,
    visible: bool,
) -> Result<(), String> {
    app.emit(
        "zenplan-visibility",
        ZenPlanVisibilityPayload {
            notebook_id,
            visible,
        },
    )
    .map_err(|error| error.to_string())
}

fn create_terminal_window(
    app: &AppHandle,
    group_id: Uuid,
    definition: &SessionDefinition,
) -> Result<WebviewWindow, String> {
    let window = WebviewWindowBuilder::new(
        app,
        terminal_window_label(group_id),
        WebviewUrl::App("index.html".into()),
    )
    .title(format!("AI Dock — {}", definition.name))
    // Keep local app windows in one WebView2 environment. Creating another
    // environment while terminal events are flowing can re-enter Tauri's resource
    // callback and deadlock its window-manager mutex.
    .inner_size(900.0, 620.0)
    .min_inner_size(420.0, 240.0)
    .decorations(false)
    .resizable(true)
    .always_on_top(false)
    .skip_taskbar(true)
    .shadow(true)
    .visible(false)
    .build()
    .map_err(|error| error.to_string())?;
    windowing::disable_window_animations(&window).map_err(|error| format!("{error:#}"))?;
    let shortcut_app = app.clone();
    let restore_app = app.clone();
    windowing::install_ctrl_h_handler(
        &window,
        move || {
            let _ = emit_terminal_visibility(&shortcut_app, group_id, false);
        },
        move || {
            let _ = emit_terminal_visibility(&restore_app, group_id, true);
        },
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok(window)
}

fn create_zenplan_window(
    app: &AppHandle,
    notebook: &ZenPlanNotebook,
) -> Result<WebviewWindow, String> {
    let url = notebook
        .url
        .parse()
        .map_err(|error| format!("invalid web app URL: {error}"))?;
    let data_directory = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?
        .join("zenplan-webview");
    let window = WebviewWindowBuilder::new(
        app,
        zenplan_window_label(notebook.id),
        WebviewUrl::External(url),
    )
    .title(&notebook.name)
    // Keep remote apps in their own shared WebView2 environment. Retain the
    // original folder name so existing ZenPlan sessions remain signed in. A
    // remote page must not share the dock and terminal windows' IPC controller.
    .data_directory(data_directory)
    .inner_size(720.0, 820.0)
    .min_inner_size(480.0, 420.0)
    .decorations(true)
    .resizable(true)
    .always_on_top(false)
    .skip_taskbar(true)
    .shadow(true)
    .visible(false)
    .build()
    .map_err(|error| error.to_string())?;
    windowing::disable_window_animations(&window).map_err(|error| format!("{error:#}"))?;
    let shortcut_app = app.clone();
    let shortcut_window = window.clone();
    let notebook_id = notebook.id;
    let restore_app = app.clone();
    windowing::install_ctrl_h_handler(
        &window,
        move || {
            let state = shortcut_app.state::<AppState>();
            let _ = persist_zenplan_bounds(notebook_id, &shortcut_window, state.inner());
            let _ = emit_zenplan_visibility(&shortcut_app, notebook_id, false);
        },
        move || {
            let _ = emit_zenplan_visibility(&restore_app, notebook_id, true);
        },
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok(window)
}

fn persist_zenplan_bounds(
    notebook_id: Uuid,
    window: &WebviewWindow,
    state: &AppState,
) -> Result<(), String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let size = window.outer_size().map_err(|error| error.to_string())?;
    {
        let mut settings = state.settings.lock();
        let notebook = settings
            .zenplan_notebooks
            .iter_mut()
            .find(|notebook| notebook.id == notebook_id)
            .ok_or_else(|| "Pinned web app was not found".to_string())?;
        notebook.window_x = Some(position.x);
        notebook.window_y = Some(position.y);
        notebook.window_width = Some(f64::from(size.width).clamp(480.0, 5120.0));
        notebook.window_height = Some(f64::from(size.height).clamp(420.0, 2160.0));
    }
    state.save()
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.settings.lock().clone()
}

#[tauri::command]
fn get_visible_groups(app: AppHandle, state: State<'_, AppState>) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    state
        .settings
        .lock()
        .sessions
        .iter()
        .map(|session| session.group_id)
        .filter(|group_id| seen.insert(*group_id))
        .filter(|group_id| {
            app.get_webview_window(&terminal_window_label(*group_id))
                .is_some_and(|window| {
                    window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false)
                })
        })
        .collect()
}

#[tauri::command]
fn get_visible_zenplan_notebooks(app: AppHandle, state: State<'_, AppState>) -> Vec<Uuid> {
    state
        .settings
        .lock()
        .zenplan_notebooks
        .iter()
        .filter(|notebook| {
            app.get_webview_window(&zenplan_window_label(notebook.id))
                .is_some_and(|window| {
                    window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false)
                })
        })
        .map(|notebook| notebook.id)
        .collect()
}

#[tauri::command]
async fn toggle_zenplan(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: Uuid,
) -> Result<(), String> {
    toggle_zenplan_inner(&app, state.inner(), notebook_id)
}

fn toggle_zenplan_inner(
    app: &AppHandle,
    state: &AppState,
    notebook_id: Uuid,
) -> Result<(), String> {
    let settings = state.settings.lock().clone();
    let notebook = settings
        .zenplan_notebooks
        .iter()
        .find(|notebook| notebook.id == notebook_id)
        .cloned()
        .ok_or_else(|| "Pinned web app was not found".to_string())?;
    let label = zenplan_window_label(notebook_id);
    let window = match app.get_webview_window(&label) {
        Some(window) => window,
        None => create_zenplan_window(app, &notebook)?,
    };
    let is_minimized = window.is_minimized().map_err(|error| error.to_string())?;
    let is_visible = window.is_visible().map_err(|error| error.to_string())?;
    let was_previous_foreground = app
        .try_state::<appbar::AppBarManager>()
        .map(|appbar| appbar.take_was_previous_foreground(&window))
        .transpose()
        .map_err(|error| format!("{error:#}"))?
        .unwrap_or(false);
    let recently_lost_focus = state.take_recent_zenplan_focus_loss(notebook_id);
    let is_focused = is_visible && window.is_focused().map_err(|error| error.to_string())?;
    let action = dock_window_toggle_action(
        is_minimized,
        is_visible,
        was_previous_foreground || recently_lost_focus || is_focused,
    );

    match action {
        DockWindowToggleAction::Restore => {
            window.unminimize().map_err(|error| error.to_string())?;
            window.show().map_err(|error| error.to_string())?;
            windowing::focus_window(&window).map_err(|error| format!("{error:#}"))?;
            emit_zenplan_visibility(app, notebook_id, true)
        }
        DockWindowToggleAction::Hide => {
            persist_zenplan_bounds(notebook_id, &window, state)?;
            window.hide().map_err(|error| error.to_string())?;
            emit_zenplan_visibility(app, notebook_id, false)
        }
        DockWindowToggleAction::Focus => {
            windowing::focus_window(&window).map_err(|error| format!("{error:#}"))?;
            emit_zenplan_visibility(app, notebook_id, true)
        }
        DockWindowToggleAction::Show => {
            let slot = settings
                .zenplan_notebooks
                .iter()
                .position(|candidate| candidate.id == notebook_id)
                .unwrap_or(0);
            windowing::position_right_panel(
                app,
                &window,
                notebook.window_width,
                notebook.window_height,
                notebook.window_x.zip(notebook.window_y),
                slot,
            )
            .map_err(|error| format!("{error:#}"))?;
            window.show().map_err(|error| error.to_string())?;
            windowing::focus_window(&window).map_err(|error| format!("{error:#}"))?;
            emit_zenplan_visibility(app, notebook_id, true)
        }
    }
}

#[tauri::command]
fn list_active_windows(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Vec<windows_apps::ActiveWindowApp> {
    let windows = windows_apps::list();
    let current_instances = windows
        .iter()
        .map(|window| (window.app_key.clone(), window.process_id))
        .collect::<HashSet<_>>();
    let newly_observed = {
        let mut observed = state.observed_window_app_instances.lock();
        let newly_observed = current_instances
            .difference(&observed)
            .cloned()
            .collect::<HashSet<_>>();
        *observed = current_instances;
        newly_observed
    };

    if !newly_observed.is_empty() {
        let mut seen_apps = HashSet::new();
        let mut recent = windows
            .iter()
            .filter(|window| {
                newly_observed.contains(&(window.app_key.clone(), window.process_id))
                    && seen_apps.insert(window.app_key.clone())
            })
            .collect::<Vec<_>>();
        // Insert the focused app last so it becomes the first launcher entry.
        recent.sort_by_key(|window| window.is_focused);
        if !recent.is_empty() {
            {
                let mut settings = state.settings.lock();
                for window in recent {
                    let remembered = RecentWindowsApp {
                        app_key: window.app_key.clone(),
                        app_name: window.app_name.clone(),
                        executable_path: window.executable_path.clone(),
                        icon_data_url: window.icon_data_url.clone(),
                    };
                    if let Some(pinned) = settings
                        .pinned_windows_apps
                        .iter_mut()
                        .find(|candidate| candidate.app_key == window.app_key)
                    {
                        *pinned = remembered.clone();
                    }
                    settings
                        .recent_windows_apps
                        .retain(|candidate| candidate.app_key != window.app_key);
                    settings.recent_windows_apps.insert(0, remembered);
                }
                settings.recent_windows_apps.truncate(20);
            }
            if let Err(error) = state.save_and_emit(&app) {
                eprintln!("AI Dock could not remember recent Windows apps: {error}");
            }
        }
    }
    windows
}

#[tauri::command]
fn launch_recent_windows_app(state: State<'_, AppState>, app_key: String) -> Result<(), String> {
    let key = app_key.trim().to_ascii_lowercase();
    let settings = state.settings.lock();
    let executable_path = settings
        .recent_windows_apps
        .iter()
        .chain(settings.pinned_windows_apps.iter())
        .find(|app| app.app_key == key)
        .map(|app| app.executable_path.clone())
        .ok_or_else(|| "That recent app is no longer in AI Dock's launcher".to_string())?;
    drop(settings);
    windows_apps::launch(&executable_path).map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn activate_external_window(
    app: AppHandle,
    handle: String,
    was_focused: bool,
) -> Result<bool, String> {
    let was_previous_foreground = app
        .try_state::<appbar::AppBarManager>()
        .and_then(|appbar| appbar.take_was_previous_handle(&handle).ok())
        .unwrap_or(false);
    windows_apps::activate(&handle, was_focused || was_previous_foreground)
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn focus_external_window(handle: String) -> Result<(), String> {
    windows_apps::focus_existing(&handle).map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn close_external_window(handle: String) -> Result<(), String> {
    windows_apps::close(&handle).map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn open_windows_start_search() {
    windows_apps::open_start_search();
}

#[tauri::command]
fn read_clipboard_text() -> Result<Option<String>, String> {
    clipboard::read_text()
}

#[tauri::command]
fn write_clipboard_text(text: String) -> Result<(), String> {
    clipboard::write_text(&text)
}

#[tauri::command]
fn set_window_app_pinned(
    app: AppHandle,
    state: State<'_, AppState>,
    app_key: String,
    pinned: bool,
) -> Result<(), String> {
    let key = app_key.trim().to_ascii_lowercase();
    if key.is_empty() || key.len() > 260 {
        return Err("Invalid app name".to_string());
    }
    {
        let mut settings = state.settings.lock();
        settings
            .pinned_windows_apps
            .retain(|candidate| candidate.app_key != key);
        if pinned {
            let remembered = settings
                .recent_windows_apps
                .iter()
                .find(|candidate| candidate.app_key == key)
                .cloned()
                .ok_or_else(|| "That app is no longer in AI Dock's launcher".to_string())?;
            settings.pinned_windows_apps.push(remembered);
            settings
                .hidden_window_apps
                .retain(|candidate| candidate != &key);
        }
    }
    state.save_and_emit(&app)
}

#[tauri::command]
fn reorder_pinned_windows_app(
    app: AppHandle,
    state: State<'_, AppState>,
    app_key: String,
    target_key: String,
    place_after: bool,
) -> Result<(), String> {
    let changed = {
        let mut settings = state.settings.lock();
        settings
            .reorder_pinned_windows_app(
                &app_key.trim().to_ascii_lowercase(),
                &target_key.trim().to_ascii_lowercase(),
                place_after,
            )
            .ok_or_else(|| "Pinned app was not found".to_string())?
    };
    if changed {
        state.save_and_emit(&app)?;
    }
    Ok(())
}

#[tauri::command]
fn set_window_app_visible(
    app: AppHandle,
    state: State<'_, AppState>,
    app_key: String,
    visible: bool,
) -> Result<(), String> {
    let key = app_key.trim().to_ascii_lowercase();
    if key.is_empty() || key.len() > 260 {
        return Err("Invalid app name".to_string());
    }
    {
        let mut settings = state.settings.lock();
        settings
            .hidden_window_apps
            .retain(|candidate| candidate != &key);
        if !visible {
            settings
                .pinned_windows_apps
                .retain(|candidate| candidate.app_key != key);
            settings.hidden_window_apps.push(key);
            settings.hidden_window_apps.sort();
            settings.hidden_window_apps.dedup();
        }
    }
    state.save_and_emit(&app)
}

#[tauri::command]
fn open_window_apps_editor(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("window-apps")
        .ok_or_else(|| "App checklist is unavailable".to_string())?;
    window
        .show()
        .and_then(|_| window.set_focus())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn hide_window_apps_editor(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("window-apps") {
        window.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn toggle_launcher(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let launcher = app
        .get_webview_window("launcher")
        .ok_or_else(|| "AI Dock launcher is unavailable".to_string())?;
    if launcher.is_visible().map_err(|error| error.to_string())? {
        launcher.hide().map_err(|error| error.to_string())?;
        launcher
            .set_always_on_top(false)
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    if state.take_recent_launcher_focus_loss() {
        launcher
            .set_always_on_top(false)
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    windowing::position_launcher(&app, &launcher).map_err(|error| format!("{error:#}"))?;
    state.record_launcher_opened();
    launcher
        .set_always_on_top(true)
        .map_err(|error| error.to_string())?;
    launcher.show().map_err(|error| error.to_string())?;
    windowing::focus_window(&launcher).map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn hide_launcher(app: AppHandle) -> Result<(), String> {
    if let Some(launcher) = app.get_webview_window("launcher") {
        launcher.hide().map_err(|error| error.to_string())?;
        launcher
            .set_always_on_top(false)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn is_windows_taskbar_visible() -> bool {
    windows_taskbar::is_visible()
}

#[tauri::command]
fn toggle_windows_taskbar(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    toggle_windows_taskbar_inner(&app, state.inner())
}

fn toggle_windows_taskbar_inner(app: &AppHandle, state: &AppState) -> Result<bool, String> {
    let visible = windows_taskbar::is_visible();
    let next_visible = !visible;
    windows_taskbar::set_visible(next_visible).map_err(|error| format!("{error:#}"))?;
    if let Some(appbar) = app.try_state::<appbar::AppBarManager>() {
        appbar
            .reposition(next_visible)
            .map_err(|error| format!("{error:#}"))?;
    }
    *state.taskbar_hidden_by_ai_dock.lock() = !next_visible;
    Ok(next_visible)
}

#[tauri::command]
async fn activate_group(
    app: AppHandle,
    state: State<'_, AppState>,
    group_id: Uuid,
) -> Result<(), String> {
    let settings = state.settings.lock();
    let definition = settings
        .sessions
        .iter()
        .find(|session| session.group_id == group_id)
        .cloned()
        .ok_or_else(|| "Terminal window was not found".to_string())?;
    let mut seen = HashSet::new();
    let slot = settings
        .sessions
        .iter()
        .map(|session| session.group_id)
        .filter(|candidate| seen.insert(*candidate))
        .position(|candidate| candidate == group_id)
        .unwrap_or(0);
    let popup_height = settings.popup_height;
    drop(settings);

    let label = terminal_window_label(group_id);
    let terminal = match app.get_webview_window(&label) {
        Some(window) => window,
        None => create_terminal_window(&app, group_id, &definition)?,
    };
    let is_minimized = terminal.is_minimized().map_err(|error| error.to_string())?;
    let is_visible = terminal.is_visible().map_err(|error| error.to_string())?;
    let was_previous_foreground = app
        .try_state::<appbar::AppBarManager>()
        .map(|appbar| appbar.take_was_previous_foreground(&terminal))
        .transpose()
        .map_err(|error| format!("{error:#}"))?
        .unwrap_or(false);
    let recently_lost_focus = state.take_recent_terminal_focus_loss(group_id);
    let is_focused = is_visible && terminal.is_focused().map_err(|error| error.to_string())?;
    let action = dock_window_toggle_action(
        is_minimized,
        is_visible,
        was_previous_foreground || recently_lost_focus || is_focused,
    );

    match action {
        DockWindowToggleAction::Restore => {
            terminal.unminimize().map_err(|error| error.to_string())?;
            terminal.show().map_err(|error| error.to_string())?;
            windowing::focus_window(&terminal).map_err(|error| format!("{error:#}"))?;
            emit_terminal_visibility(&app, group_id, true)
        }
        DockWindowToggleAction::Hide => {
            terminal.hide().map_err(|error| error.to_string())?;
            emit_terminal_visibility(&app, group_id, false)
        }
        DockWindowToggleAction::Focus => {
            windowing::focus_window(&terminal).map_err(|error| format!("{error:#}"))?;
            emit_terminal_visibility(&app, group_id, true)
        }
        DockWindowToggleAction::Show => {
            terminal.show().map_err(|error| error.to_string())?;
            windowing::position_terminal(
                &app,
                &terminal,
                definition.window_width,
                definition.window_height.unwrap_or(popup_height),
                definition.window_x.zip(definition.window_y),
                slot,
            )
            .map_err(|error| format!("{error:#}"))?;
            windowing::focus_window(&terminal).map_err(|error| format!("{error:#}"))?;
            emit_terminal_visibility(&app, group_id, true)
        }
    }
}

#[tauri::command]
async fn toggle_all_terminals(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    let (groups, popup_height) = {
        let settings = state.settings.lock();
        let mut seen = HashSet::new();
        let groups = settings
            .sessions
            .iter()
            .filter(|session| seen.insert(session.group_id))
            .cloned()
            .enumerate()
            .map(|(slot, definition)| (definition.group_id, definition, slot))
            .collect::<Vec<_>>();
        (groups, settings.popup_height)
    };

    let any_visible = groups.iter().any(|(group_id, _, _)| {
        app.get_webview_window(&terminal_window_label(*group_id))
            .is_some_and(|window| {
                window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false)
            })
    });
    if any_visible {
        hide_terminal(app, state, None)?;
        return Ok(false);
    }

    let mut first_window = None;
    for (group_id, definition, slot) in groups {
        let (terminal, created) = match app.get_webview_window(&terminal_window_label(group_id)) {
            Some(window) => (window, false),
            None => (create_terminal_window(&app, group_id, &definition)?, true),
        };
        if terminal.is_minimized().map_err(|error| error.to_string())? {
            terminal.unminimize().map_err(|error| error.to_string())?;
        }
        if created {
            windowing::position_terminal(
                &app,
                &terminal,
                definition.window_width,
                definition.window_height.unwrap_or(popup_height),
                definition.window_x.zip(definition.window_y),
                slot,
            )
            .map_err(|error| format!("{error:#}"))?;
        }
        terminal.show().map_err(|error| error.to_string())?;
        emit_terminal_visibility(&app, group_id, true)?;
        if first_window.is_none() {
            first_window = Some(terminal);
        }
    }
    if let Some(terminal) = first_window {
        windowing::focus_window(&terminal).map_err(|error| format!("{error:#}"))?;
    }
    Ok(true)
}

#[tauri::command]
fn start_session(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let definition = state.definition(session_id)?;
    state
        .sessions
        .start(&app, &definition, cols, rows)
        .map_err(|error| {
            let message = format!("{error:#}");
            emit_state(&app, session_id, "failed", None, Some(message.clone()));
            message
        })
}

#[tauri::command]
fn hide_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    group_id: Option<Uuid>,
) -> Result<(), String> {
    let group_ids = group_id.map_or_else(
        || {
            let mut seen = HashSet::new();
            state
                .settings
                .lock()
                .sessions
                .iter()
                .map(|session| session.group_id)
                .filter(|candidate| seen.insert(*candidate))
                .collect()
        },
        |id| vec![id],
    );
    for id in group_ids {
        if let Some(terminal) = app.get_webview_window(&terminal_window_label(id)) {
            terminal.hide().map_err(|error| error.to_string())?;
            emit_terminal_visibility(&app, id, false)?;
        }
    }
    Ok(())
}

#[tauri::command]
fn close_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    group_id: Uuid,
    remember: bool,
) -> Result<(), String> {
    let removed = {
        let mut settings = state.settings.lock();
        let removed = settings.take_group(group_id);
        if remember {
            settings.remember_group(group_id, removed.clone());
        }
        removed
    };
    if removed.is_empty() {
        return Err("Terminal window was not found".to_string());
    }
    for session in &removed {
        state
            .sessions
            .stop(&app, session.id)
            .map_err(|error| format!("{error:#}"))?;
    }
    state.save_and_emit(&app)?;
    if let Some(terminal) = app.get_webview_window(&terminal_window_label(group_id)) {
        terminal.destroy().map_err(|error| error.to_string())?;
    }
    emit_terminal_visibility(&app, group_id, false)
}

#[tauri::command]
fn restore_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    group_id: Uuid,
) -> Result<Uuid, String> {
    let restored_group_id = state
        .settings
        .lock()
        .restore_group(group_id)
        .ok_or_else(|| "Recently closed window was not found".to_string())?;
    state.save_and_emit(&app)?;
    Ok(restored_group_id)
}

#[tauri::command]
fn clear_recent_groups(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.settings.lock().recent_groups.clear();
    state.save_and_emit(&app)
}

#[tauri::command]
fn write_session(state: State<'_, AppState>, session_id: Uuid, data: String) -> Result<(), String> {
    state
        .sessions
        .write(session_id, &data)
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn resize_session(
    state: State<'_, AppState>,
    session_id: Uuid,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    if !state.sessions.is_running(session_id) {
        return Ok(());
    }
    state
        .sessions
        .resize(session_id, cols, rows)
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn update_session_working_directory(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
    working_directory: String,
) -> Result<(), String> {
    let working_directory = working_directory.trim();
    if working_directory.is_empty() || !std::path::Path::new(working_directory).is_dir() {
        return Ok(());
    }

    let mut settings = state.settings.lock();
    let session = settings
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| "Session was not found".to_string())?;
    if session.working_directory == working_directory {
        return Ok(());
    }
    session.working_directory = working_directory.to_string();
    drop(settings);
    state.save_and_emit(&app)
}

#[tauri::command]
fn stop_session(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
) -> Result<(), String> {
    state
        .sessions
        .stop(&app, session_id)
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn restart_session(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
) -> Result<(), String> {
    let definition = state.definition(session_id)?;
    state
        .sessions
        .stop(&app, session_id)
        .map_err(|error| format!("{error:#}"))?;
    state
        .sessions
        .start(&app, &definition, 120, 36)
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn open_editor(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Option<Uuid>,
    group_id: Option<Uuid>,
) -> Result<(), String> {
    if let Some(id) = session_id {
        state.definition(id)?;
    }
    if let Some(id) = group_id
        && !state
            .settings
            .lock()
            .sessions
            .iter()
            .any(|session| session.group_id == id)
    {
        return Err("Terminal window was not found".to_string());
    }
    *state.editor_target.lock() = (session_id, group_id);
    app.emit("editor-target-changed", (session_id, group_id))
        .map_err(|error| error.to_string())?;
    let editor = app
        .get_webview_window("editor")
        .ok_or_else(|| "Editor window is unavailable".to_string())?;
    editor.show().map_err(|error| error.to_string())?;
    editor.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
fn get_editor_context(state: State<'_, AppState>) -> EditorContext {
    let (target, target_group_id) = *state.editor_target.lock();
    EditorContext {
        session: target.and_then(|id| {
            state
                .settings
                .lock()
                .sessions
                .iter()
                .find(|session| session.id == id)
                .cloned()
        }),
        target_group_id,
        default_working_directory: default_working_directory(),
    }
}

#[tauri::command]
fn hide_editor(app: AppHandle) -> Result<(), String> {
    if let Some(editor) = app.get_webview_window("editor") {
        editor.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn open_rename(app: AppHandle, state: State<'_, AppState>, session_id: Uuid) -> Result<(), String> {
    state.definition(session_id)?;
    *state.rename_target.lock() = Some(session_id);
    app.emit("rename-target-changed", session_id)
        .map_err(|error| error.to_string())?;
    let rename = app
        .get_webview_window("rename")
        .ok_or_else(|| "Rename window is unavailable".to_string())?;
    rename
        .set_always_on_top(true)
        .map_err(|error| error.to_string())?;
    let result = rename
        .show()
        .and_then(|_| rename.set_focus())
        .map_err(|error| error.to_string());
    if result.is_err() {
        let _ = rename.set_always_on_top(false);
    }
    result
}

#[tauri::command]
fn get_rename_context(state: State<'_, AppState>) -> Option<SessionDefinition> {
    let target = *state.rename_target.lock();
    target.and_then(|id| {
        state
            .settings
            .lock()
            .sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
    })
}

#[tauri::command]
fn hide_rename(app: AppHandle) -> Result<(), String> {
    if let Some(rename) = app.get_webview_window("rename") {
        rename.hide().map_err(|error| error.to_string())?;
        rename
            .set_always_on_top(false)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn open_color(app: AppHandle, state: State<'_, AppState>, session_id: Uuid) -> Result<(), String> {
    state.definition(session_id)?;
    *state.color_target.lock() = Some(session_id);
    app.emit("color-target-changed", session_id)
        .map_err(|error| error.to_string())?;
    let color = app
        .get_webview_window("color")
        .ok_or_else(|| "Color window is unavailable".to_string())?;
    color
        .set_always_on_top(true)
        .map_err(|error| error.to_string())?;
    let result = color
        .show()
        .and_then(|_| color.set_focus())
        .map_err(|error| error.to_string());
    if result.is_err() {
        let _ = color.set_always_on_top(false);
    }
    result
}

#[tauri::command]
fn get_color_context(state: State<'_, AppState>) -> Option<SessionDefinition> {
    let target = *state.color_target.lock();
    target.and_then(|id| {
        state
            .settings
            .lock()
            .sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
    })
}

#[tauri::command]
fn hide_color(app: AppHandle) -> Result<(), String> {
    if let Some(color) = app.get_webview_window("color") {
        color.hide().map_err(|error| error.to_string())?;
        color
            .set_always_on_top(false)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn open_zenplan_editor(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: Option<Uuid>,
) -> Result<(), String> {
    if let Some(notebook_id) = notebook_id
        && !state
            .settings
            .lock()
            .zenplan_notebooks
            .iter()
            .any(|notebook| notebook.id == notebook_id)
    {
        return Err("Pinned web app was not found".to_string());
    }
    *state.zenplan_editor_target.lock() = notebook_id;
    app.emit("zenplan-editor-target-changed", notebook_id)
        .map_err(|error| error.to_string())?;
    let editor = app
        .get_webview_window("zenplan-editor")
        .ok_or_else(|| "Web app pin editor is unavailable".to_string())?;
    editor
        .show()
        .and_then(|_| editor.set_focus())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_zenplan_editor_context(state: State<'_, AppState>) -> ZenPlanEditorContext {
    let target = *state.zenplan_editor_target.lock();
    ZenPlanEditorContext {
        notebook: target.and_then(|id| {
            state
                .settings
                .lock()
                .zenplan_notebooks
                .iter()
                .find(|notebook| notebook.id == id)
                .cloned()
        }),
        default_url: "https://".to_string(),
    }
}

#[tauri::command]
fn hide_zenplan_editor(app: AppHandle) -> Result<(), String> {
    if let Some(editor) = app.get_webview_window("zenplan-editor") {
        editor.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn save_zenplan_notebook(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: Option<Uuid>,
    name: String,
    url: String,
) -> Result<ZenPlanNotebook, String> {
    let name = name.trim().to_string();
    let url = url.trim().to_string();
    if name.is_empty() {
        return Err("Enter a web app name".to_string());
    }
    if name.len() > 80 {
        return Err("Web app names can be at most 80 characters".to_string());
    }
    if !is_valid_web_app_url(&url) {
        return Err(
            "Use a valid http:// or https:// web app URL without embedded credentials".to_string(),
        );
    }

    let mut existing_window_to_recreate = None;
    let notebook = {
        let mut settings = state.settings.lock();
        if let Some(notebook_id) = notebook_id {
            let existing = settings
                .zenplan_notebooks
                .iter_mut()
                .find(|notebook| notebook.id == notebook_id)
                .ok_or_else(|| "Pinned web app was not found".to_string())?;
            if existing.url != url {
                existing_window_to_recreate = Some(zenplan_window_label(notebook_id));
                existing.icon_data_url = None;
            }
            existing.name = name;
            existing.url = url;
            existing.clone()
        } else {
            let notebook = ZenPlanNotebook {
                id: Uuid::new_v4(),
                name,
                url,
                icon_data_url: None,
                window_x: None,
                window_y: None,
                window_width: None,
                window_height: None,
            };
            settings.zenplan_notebooks.push(notebook.clone());
            notebook
        }
    };
    if let Some(label) = existing_window_to_recreate {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.destroy();
        }
        let _ = app.emit(
            "zenplan-visibility",
            models::ZenPlanVisibilityPayload {
                notebook_id: notebook.id,
                visible: false,
            },
        );
    } else if let Some(window) = app.get_webview_window(&zenplan_window_label(notebook.id)) {
        let _ = window.set_title(&notebook.name);
    }
    state.save_and_emit(&app)?;
    if notebook.icon_data_url.is_none() {
        queue_web_app_favicon_refresh(app, notebook.id, notebook.url.clone());
    }
    Ok(notebook)
}

#[tauri::command]
fn delete_zenplan_notebook(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: Uuid,
) -> Result<(), String> {
    {
        let mut settings = state.settings.lock();
        let previous_len = settings.zenplan_notebooks.len();
        settings
            .zenplan_notebooks
            .retain(|notebook| notebook.id != notebook_id);
        if settings.zenplan_notebooks.len() == previous_len {
            return Err("Pinned web app was not found".to_string());
        }
    }
    if let Some(window) = app.get_webview_window(&zenplan_window_label(notebook_id)) {
        let _ = window.destroy();
    }
    let _ = app.emit(
        "zenplan-visibility",
        models::ZenPlanVisibilityPayload {
            notebook_id,
            visible: false,
        },
    );
    state.save_and_emit(&app)
}

#[tauri::command]
fn reorder_zenplan_notebook(
    app: AppHandle,
    state: State<'_, AppState>,
    notebook_id: Uuid,
    target_id: Uuid,
    place_after: bool,
) -> Result<(), String> {
    let changed = state
        .settings
        .lock()
        .reorder_zenplan_notebook(notebook_id, target_id, place_after)
        .ok_or_else(|| "Pinned web app was not found".to_string())?;
    if changed {
        state.save_and_emit(&app)?;
    }
    Ok(())
}

#[tauri::command]
fn save_session(
    app: AppHandle,
    state: State<'_, AppState>,
    mut session: SessionDefinition,
) -> Result<(), String> {
    session.name = session.name.trim().to_string();
    session.command_line = session.command_line.trim().to_string();
    session.working_directory = session.working_directory.trim().to_string();
    if session.name.is_empty() || session.command_line.is_empty() {
        return Err("Name and command are required".to_string());
    }
    if !is_valid_color(&session.color) {
        return Err("Color must be a six-digit hex value".to_string());
    }
    if !std::path::Path::new(&session.working_directory).is_dir() {
        return Err("Working directory does not exist".to_string());
    }
    if session.group_id.is_nil() {
        session.group_id = session.id;
    }

    let mut settings = state.settings.lock();
    if let Some(existing) = settings
        .sessions
        .iter_mut()
        .find(|existing| existing.id == session.id)
    {
        *existing = session;
    } else {
        if session.group_id != session.id {
            let group_leader = settings
                .sessions
                .iter()
                .find(|candidate| candidate.group_id == session.group_id)
                .ok_or_else(|| "Terminal window was not found".to_string())?;
            session.window_x = group_leader.window_x;
            session.window_y = group_leader.window_y;
            session.window_width = group_leader.window_width;
            session.window_height = group_leader.window_height;
        }
        settings.sessions.push(session);
    }
    drop(settings);
    state.save_and_emit(&app)
}

#[tauri::command]
fn add_powershell_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    group_id: Option<Uuid>,
) -> Result<SessionDefinition, String> {
    let mut session = SessionDefinition::default_powershell();
    let mut settings = state.settings.lock();
    if let Some(group_id) = group_id {
        let group_leader = settings
            .sessions
            .iter()
            .find(|candidate| candidate.group_id == group_id)
            .ok_or_else(|| "Terminal window was not found".to_string())?;
        session.group_id = group_id;
        session.window_x = group_leader.window_x;
        session.window_y = group_leader.window_y;
        session.window_width = group_leader.window_width;
        session.window_height = group_leader.window_height;
    }
    settings.sessions.push(session.clone());
    drop(settings);
    state.save_and_emit(&app)?;
    Ok(session)
}

#[tauri::command]
fn add_claude_session(
    app: AppHandle,
    state: State<'_, AppState>,
    working_directory: String,
    group_id: Option<Uuid>,
) -> Result<SessionDefinition, String> {
    add_cli_session(
        &app,
        &state,
        working_directory,
        group_id,
        SessionDefinition::claude_cli,
    )
}

#[tauri::command]
fn add_codex_session(
    app: AppHandle,
    state: State<'_, AppState>,
    working_directory: String,
    group_id: Option<Uuid>,
) -> Result<SessionDefinition, String> {
    add_cli_session(
        &app,
        &state,
        working_directory,
        group_id,
        SessionDefinition::codex_cli,
    )
}

fn add_cli_session(
    app: &AppHandle,
    state: &State<'_, AppState>,
    working_directory: String,
    group_id: Option<Uuid>,
    create: impl FnOnce(String) -> SessionDefinition,
) -> Result<SessionDefinition, String> {
    let working_directory = working_directory.trim();
    if !std::path::Path::new(working_directory).is_dir() {
        return Err("Working directory does not exist".to_string());
    }

    let mut session = create(working_directory.to_string());
    let mut settings = state.settings.lock();
    if let Some(group_id) = group_id {
        let group_leader = settings
            .sessions
            .iter()
            .find(|candidate| candidate.group_id == group_id)
            .ok_or_else(|| "Terminal window was not found".to_string())?;
        session.group_id = group_id;
        session.window_x = group_leader.window_x;
        session.window_y = group_leader.window_y;
        session.window_width = group_leader.window_width;
        session.window_height = group_leader.window_height;
    }
    settings.sessions.push(session.clone());
    drop(settings);
    state.save_and_emit(app)?;
    Ok(session)
}

#[tauri::command]
fn set_session_color(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
    color: String,
) -> Result<(), String> {
    if !is_valid_color(&color) {
        return Err("Color must be a six-digit hex value".to_string());
    }
    let mut settings = state.settings.lock();
    let session = settings
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| "Session was not found".to_string())?;
    session.color = color.to_uppercase();
    drop(settings);
    state.save_and_emit(&app)
}

#[tauri::command]
fn rename_session(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
    name: String,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Tab name is required".to_string());
    }
    let mut settings = state.settings.lock();
    let session = settings
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| "Session was not found".to_string())?;
    session.name = name.to_string();
    drop(settings);
    state.save_and_emit(&app)
}

#[tauri::command]
fn delete_session(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
) -> Result<(), String> {
    let group_id = state.definition(session_id)?.group_id;
    let _ = state.sessions.stop(&app, session_id);
    let group_is_empty = {
        let mut settings = state.settings.lock();
        settings.sessions.retain(|session| session.id != session_id);
        !settings
            .sessions
            .iter()
            .any(|session| session.group_id == group_id)
    };
    if group_is_empty
        && let Some(terminal) = app.get_webview_window(&terminal_window_label(group_id))
    {
        terminal.destroy().map_err(|error| error.to_string())?;
        emit_terminal_visibility(&app, group_id, false)?;
    }
    state.save_and_emit(&app)
}

#[tauri::command]
fn move_session(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
    direction: i32,
) -> Result<(), String> {
    let mut settings = state.settings.lock();
    let Some(index) = settings
        .sessions
        .iter()
        .position(|session| session.id == session_id)
    else {
        return Err("Session was not found".to_string());
    };
    let group_id = settings.sessions[index].group_id;
    let group_indices: Vec<usize> = settings
        .sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| session.group_id == group_id)
        .map(|(index, _)| index)
        .collect();
    let position = group_indices
        .iter()
        .position(|candidate| *candidate == index)
        .unwrap_or(0);
    let target_position =
        (position as i32 + direction.signum()).clamp(0, group_indices.len() as i32 - 1) as usize;
    let target = group_indices[target_position];
    if index != target {
        settings.sessions.swap(index, target);
    }
    drop(settings);
    state.save_and_emit(&app)
}

#[tauri::command]
async fn pick_folder(starting_directory: String) -> Option<String> {
    let mut dialog = rfd::AsyncFileDialog::new();
    if std::path::Path::new(&starting_directory).is_dir() {
        dialog = dialog.set_directory(starting_directory);
    }
    dialog
        .pick_folder()
        .await
        .map(|folder| folder.path().to_string_lossy().to_string())
}

#[tauri::command]
fn save_terminal_bounds(
    state: State<'_, AppState>,
    group_id: Uuid,
    x: i32,
    y: i32,
    width: f64,
    height: f64,
) -> Result<(), String> {
    {
        let mut settings = state.settings.lock();
        let mut found = false;
        for session in settings
            .sessions
            .iter_mut()
            .filter(|session| session.group_id == group_id)
        {
            found = true;
            session.window_x = Some(x);
            session.window_y = Some(y);
            session.window_width = Some(width.clamp(420.0, 5120.0));
            session.window_height = Some(height.clamp(240.0, 1400.0));
        }
        if !found {
            return Err("Terminal window was not found".to_string());
        }
    }
    state.save()
}

#[tauri::command]
fn ui_heartbeat(window: WebviewWindow, state: State<'_, AppState>) {
    state
        .ui_heartbeats
        .lock()
        .insert(window.label().to_string(), Instant::now());
}

fn append_watchdog_log(path: &std::path::Path, message: &str) {
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{:?} {message}", SystemTime::now());
}

fn start_webview_heartbeat_watchdog(
    heartbeats: Arc<Mutex<HashMap<String, Instant>>>,
    monitored_webviews: Arc<Mutex<HashSet<String>>>,
    restart_requested: Arc<AtomicBool>,
    config_dir: PathBuf,
) -> anyhow::Result<()> {
    fs::create_dir_all(&config_dir).context("creating WebView watchdog log directory")?;
    let log_path = config_dir.join("watchdog.log");
    thread::Builder::new()
        .name("ai-dock-webview-watchdog".to_string())
        .spawn(move || {
            thread::sleep(WEBVIEW_WATCHDOG_GRACE);
            loop {
                thread::sleep(WEBVIEW_WATCHDOG_INTERVAL);

                // Never call AppHandle::webview_windows from this thread. That API
                // takes Tauri's window-manager lock, which is exactly the lock this
                // watchdog must remain independent from when the UI is deadlocked.
                let stale_label = {
                    let monitored = monitored_webviews.lock();
                    let recent = heartbeats.lock();
                    monitored.iter().find_map(|label| {
                        recent
                            .get(label)
                            .is_none_or(|seen| seen.elapsed() > WEBVIEW_HEARTBEAT_TIMEOUT)
                            .then(|| label.clone())
                    })
                };

                let Some(stale_label) = stale_label else {
                    continue;
                };
                if restart_requested.swap(true, Ordering::SeqCst) {
                    break;
                }
                let reason = format!(
                    "WebView heartbeat stopped for {stale_label}; recovering UI without stopping detached sessions; pid={}",
                    std::process::id()
                );
                if let Err(error) = watchdog::request_ui_recovery(&config_dir, &reason) {
                    append_watchdog_log(
                        &log_path,
                        &format!("Could not start UI recovery helper: {error:#}"),
                    );
                    restart_requested.store(false, Ordering::SeqCst);
                }
            }
        })
        .context("starting WebView heartbeat watchdog")?;
    Ok(())
}

#[tauri::command]
fn exit_app(app: AppHandle, state: State<'_, AppState>) {
    state.restore_taskbar_if_needed();
    state.sessions.stop_all(&app);
    app.exit(0);
}

pub fn run() {
    // Register application state before Tauri creates configured WebViews. Registering it
    // inside setup races the pages' first invoke calls on fast machines.
    let settings_path = dirs::config_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("AiDock"))
        .join(if is_test_build() {
            "com.aidock.desktop.test"
        } else {
            "com.aidock.desktop"
        })
        .join("settings.json");
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _working_directory| {
                if let Some(dock) = app.get_webview_window("dock") {
                    let _ = dock.show();
                    let _ = dock.set_focus();
                }
            },
        ))
        .manage(AppState::new(settings_path))
        .setup(|app| {
            let state = app.state::<AppState>();
            let (dock_height, favicon_refreshes) = {
                let settings = state.settings.lock();
                (
                    settings.dock_height,
                    settings
                        .zenplan_notebooks
                        .iter()
                        .filter(|notebook| notebook.icon_data_url.is_none())
                        .map(|notebook| (notebook.id, notebook.url.clone()))
                        .collect::<Vec<_>>(),
                )
            };
            for (notebook_id, url) in favicon_refreshes {
                queue_web_app_favicon_refresh(app.handle().clone(), notebook_id, url);
            }
            let dock = app
                .get_webview_window("dock")
                .context("dock window was not created")?;
            let watchdog_config_dir = state
                .settings_path
                .parent()
                .map_or_else(|| std::env::temp_dir().join("AiDock"), PathBuf::from);
            state
                .ui_heartbeats
                .lock()
                .insert(dock.label().to_string(), Instant::now());
            state
                .monitored_webviews
                .lock()
                .insert(dock.label().to_string());
            // A WebView2 browser can briefly stop answering WM_NULL while xterm
            // rebuilds a large retained scrollback buffer even though JavaScript
            // and terminal input remain healthy. The page heartbeat is the reliable
            // liveness signal; the native message-loop watchdog produced restart
            // loops during normal session reattachment.
            start_webview_heartbeat_watchdog(
                state.ui_heartbeats.clone(),
                state.monitored_webviews.clone(),
                state.ui_restart_requested.clone(),
                watchdog_config_dir,
            )?;
            if is_test_build() && option_env!("AI_DOCK_TEST_APPBAR") != Some("1") {
                dock.set_title("AI Dock Test")?;
                dock.set_always_on_top(false)?;
                dock.center()?;
            } else {
                let appbar = appbar::AppBarManager::register(&dock, dock_height)
                    .context("registering Windows AppBar")?;
                app.manage(appbar);
            }
            if let Some(editor) = app.get_webview_window("editor") {
                let _ = windowing::disable_window_animations(&editor);
            }
            if let Some(rename) = app.get_webview_window("rename") {
                let _ = windowing::disable_window_animations(&rename);
            }
            if let Some(color) = app.get_webview_window("color") {
                let _ = windowing::disable_window_animations(&color);
            }
            if let Some(editor) = app.get_webview_window("zenplan-editor") {
                let _ = windowing::disable_window_animations(&editor);
            }
            if let Some(editor) = app.get_webview_window("window-apps") {
                let _ = windowing::disable_window_animations(&editor);
            }
            if let Some(launcher) = app.get_webview_window("launcher") {
                let _ = windowing::disable_window_animations(&launcher);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(false) = event
                && window.label() == "launcher"
            {
                let state = window.app_handle().state::<AppState>();
                if !state.launcher_is_settling() {
                    state.record_launcher_focus_loss();
                    let _ = window.hide();
                    let _ = window.set_always_on_top(false);
                }
            }
            if let tauri::WindowEvent::Focused(false) = event
                && let Some(group_id) = group_id_from_window_label(window.label())
            {
                window
                    .app_handle()
                    .state::<AppState>()
                    .record_terminal_focus_loss(group_id);
            }
            if let tauri::WindowEvent::Focused(false) = event
                && let Some(notebook_id) = notebook_id_from_window_label(window.label())
            {
                window
                    .app_handle()
                    .state::<AppState>()
                    .record_zenplan_focus_loss(notebook_id);
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Some(notebook_id) = notebook_id_from_window_label(window.label()) {
                    let state = window.app_handle().state::<AppState>();
                    if let Some(zenplan) = window
                        .app_handle()
                        .get_webview_window(&zenplan_window_label(notebook_id))
                    {
                        let _ = persist_zenplan_bounds(notebook_id, &zenplan, state.inner());
                    }
                    let _ = window.hide();
                    let _ = window.app_handle().emit(
                        "zenplan-visibility",
                        models::ZenPlanVisibilityPayload {
                            notebook_id,
                            visible: false,
                        },
                    );
                    return;
                }
                if let Some(group_id) = group_id_from_window_label(window.label()) {
                    let _ = window.hide();
                    let _ = emit_terminal_visibility(window.app_handle(), group_id, false);
                } else {
                    match window.label() {
                        "editor" => {
                            let _ = window.hide();
                        }
                        "rename" => {
                            let _ = window.hide();
                            let _ = window.set_always_on_top(false);
                        }
                        "color" => {
                            let _ = window.hide();
                            let _ = window.set_always_on_top(false);
                        }
                        "zenplan-editor" => {
                            let _ = window.hide();
                        }
                        "window-apps" => {
                            let _ = window.hide();
                        }
                        "launcher" => {
                            let _ = window.hide();
                            let _ = window.set_always_on_top(false);
                        }
                        "dock" => window.app_handle().exit(0),
                        _ => {}
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            ui_heartbeat,
            get_settings,
            get_visible_groups,
            get_visible_zenplan_notebooks,
            toggle_zenplan,
            list_active_windows,
            launch_recent_windows_app,
            activate_external_window,
            focus_external_window,
            close_external_window,
            open_windows_start_search,
            read_clipboard_text,
            write_clipboard_text,
            set_window_app_pinned,
            reorder_pinned_windows_app,
            set_window_app_visible,
            open_window_apps_editor,
            hide_window_apps_editor,
            toggle_launcher,
            hide_launcher,
            is_windows_taskbar_visible,
            toggle_windows_taskbar,
            activate_group,
            toggle_all_terminals,
            start_session,
            hide_terminal,
            close_terminal,
            restore_terminal,
            clear_recent_groups,
            write_session,
            resize_session,
            update_session_working_directory,
            stop_session,
            restart_session,
            open_editor,
            get_editor_context,
            hide_editor,
            open_rename,
            get_rename_context,
            hide_rename,
            open_color,
            get_color_context,
            hide_color,
            open_zenplan_editor,
            get_zenplan_editor_context,
            hide_zenplan_editor,
            save_zenplan_notebook,
            delete_zenplan_notebook,
            reorder_zenplan_notebook,
            save_session,
            add_powershell_terminal,
            add_claude_session,
            add_codex_session,
            set_session_color,
            rename_session,
            delete_session,
            move_session,
            pick_folder,
            save_terminal_bounds,
            exit_app
        ]);

    let application = builder
        .build(tauri::generate_context!())
        .expect("failed to build AI Dock");
    application.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { code, .. } = event {
            let state = app.state::<AppState>();
            let notebooks = state.settings.lock().zenplan_notebooks.clone();
            for notebook in notebooks {
                if let Some(window) = app.get_webview_window(&zenplan_window_label(notebook.id)) {
                    let _ = persist_zenplan_bounds(notebook.id, &window, state.inner());
                }
            }
            state.restore_taskbar_if_needed();
            if code != Some(tauri::RESTART_EXIT_CODE) {
                state.sessions.stop_all(app);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{DockWindowToggleAction, dock_window_toggle_action};

    #[test]
    fn dock_windows_share_taskbar_style_toggle_behavior() {
        assert_eq!(
            dock_window_toggle_action(true, true, false),
            DockWindowToggleAction::Restore
        );
        assert_eq!(
            dock_window_toggle_action(false, true, true),
            DockWindowToggleAction::Hide
        );
        assert_eq!(
            dock_window_toggle_action(false, true, false),
            DockWindowToggleAction::Focus
        );
        assert_eq!(
            dock_window_toggle_action(false, false, false),
            DockWindowToggleAction::Show
        );
    }
}
