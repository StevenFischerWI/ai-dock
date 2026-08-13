use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CURRENT_SCHEMA_VERSION: u32 = 15;
pub const MAX_RECENT_GROUPS: usize = 50;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LegacySessionKind {
    #[default]
    Terminal,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDefinition {
    pub id: Uuid,
    #[serde(default)]
    pub group_id: Uuid,
    pub name: String,
    pub color: String,
    #[serde(default, rename = "kind", skip_serializing)]
    legacy_kind: LegacySessionKind,
    pub command_line: String,
    pub working_directory: String,
    #[serde(default, rename = "codexThreadId", skip_serializing)]
    legacy_codex_thread_id: Option<String>,
    #[serde(default)]
    pub window_x: Option<i32>,
    #[serde(default)]
    pub window_y: Option<i32>,
    #[serde(default)]
    pub window_width: Option<f64>,
    #[serde(default)]
    pub window_height: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentGroup {
    pub group_id: Uuid,
    pub sessions: Vec<SessionDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZenPlanNotebook {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub icon_data_url: Option<String>,
    #[serde(default)]
    pub window_x: Option<i32>,
    #[serde(default)]
    pub window_y: Option<i32>,
    #[serde(default)]
    pub window_width: Option<f64>,
    #[serde(default)]
    pub window_height: Option<f64>,
}

impl Default for ZenPlanNotebook {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "ZenPlan".to_string(),
            url: "https://www.getzenplan.com/".to_string(),
            icon_data_url: None,
            window_x: None,
            window_y: None,
            window_width: None,
            window_height: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentWindowsApp {
    pub app_key: String,
    pub app_name: String,
    pub executable_path: String,
    #[serde(default)]
    pub icon_data_url: Option<String>,
}

impl SessionDefinition {
    pub fn default_powershell() -> Self {
        let id = Uuid::new_v4();
        Self {
            id,
            group_id: id,
            name: "PowerShell".to_string(),
            color: "#2F78C4".to_string(),
            legacy_kind: LegacySessionKind::Terminal,
            command_line: "pwsh.exe -NoLogo".to_string(),
            working_directory: default_working_directory(),
            legacy_codex_thread_id: None,
            window_x: None,
            window_y: None,
            window_width: None,
            window_height: None,
        }
    }

    pub fn claude_cli(working_directory: String) -> Self {
        let id = Uuid::new_v4();
        Self {
            id,
            group_id: id,
            name: "Claude".to_string(),
            color: "#D87524".to_string(),
            legacy_kind: LegacySessionKind::Terminal,
            command_line: "pwsh.exe -NoLogo -NoExit -Command claude".to_string(),
            working_directory,
            legacy_codex_thread_id: None,
            window_x: None,
            window_y: None,
            window_width: None,
            window_height: None,
        }
    }

    pub fn codex_cli(working_directory: String) -> Self {
        let id = Uuid::new_v4();
        Self {
            id,
            group_id: id,
            name: "Codex".to_string(),
            color: "#D64F70".to_string(),
            legacy_kind: LegacySessionKind::Terminal,
            command_line: "pwsh.exe -NoLogo -NoExit -Command codex".to_string(),
            working_directory,
            legacy_codex_thread_id: None,
            window_x: None,
            window_y: None,
            window_width: None,
            window_height: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub schema_version: u32,
    pub dock_height: f64,
    #[serde(default)]
    pub popup_width: Option<f64>,
    pub popup_height: f64,
    #[serde(default)]
    pub terminal_x: Option<i32>,
    #[serde(default)]
    pub terminal_y: Option<i32>,
    pub sessions: Vec<SessionDefinition>,
    #[serde(default)]
    pub recent_groups: Vec<RecentGroup>,
    #[serde(default)]
    pub hidden_window_apps: Vec<String>,
    #[serde(default)]
    pub recent_windows_apps: Vec<RecentWindowsApp>,
    #[serde(default)]
    pub pinned_windows_apps: Vec<RecentWindowsApp>,
    #[serde(default)]
    pub zenplan_notebooks: Vec<ZenPlanNotebook>,
    // Kept for one migration cycle so schema-10 installations retain their existing
    // ZenPlan window location when it becomes the first notebook pin.
    #[serde(default)]
    pub zenplan_x: Option<i32>,
    #[serde(default)]
    pub zenplan_y: Option<i32>,
    #[serde(default)]
    pub zenplan_width: Option<f64>,
    #[serde(default)]
    pub zenplan_height: Option<f64>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            dock_height: 40.0,
            popup_width: None,
            popup_height: 620.0,
            terminal_x: None,
            terminal_y: None,
            sessions: vec![SessionDefinition::default_powershell()],
            recent_groups: Vec::new(),
            hidden_window_apps: Vec::new(),
            recent_windows_apps: Vec::new(),
            pinned_windows_apps: Vec::new(),
            zenplan_notebooks: Vec::new(),
            zenplan_x: None,
            zenplan_y: None,
            zenplan_width: None,
            zenplan_height: None,
        }
    }
}

impl AppSettings {
    pub fn normalize(&mut self) {
        if self.schema_version < 2 {
            self.dock_height = 40.0;
        }
        if self.schema_version < 4 {
            // Geometry was previously saved with mixed logical/physical units. Reset it once so
            // scaled displays get a stable one-third-width default instead of compounding size.
            self.popup_width = None;
            self.popup_height = 700.0;
            self.terminal_x = None;
            self.terminal_y = None;
        }
        // Schema 10 had one dedicated ZenPlan panel instead of a list of pins.
        // Migrate that panel once, while allowing schema-12 users to unpin every
        // web app without ZenPlan being silently recreated on the next launch.
        if self.schema_version < 11 && self.zenplan_notebooks.is_empty() {
            let notebook = ZenPlanNotebook {
                window_x: self.zenplan_x,
                window_y: self.zenplan_y,
                window_width: self.zenplan_width,
                window_height: self.zenplan_height,
                ..ZenPlanNotebook::default()
            };
            self.zenplan_notebooks.push(notebook);
        }
        self.schema_version = CURRENT_SCHEMA_VERSION;
        self.dock_height = self.dock_height.clamp(32.0, 64.0);
        self.popup_width = self.popup_width.map(|width| width.clamp(420.0, 5120.0));
        self.popup_height = self.popup_height.clamp(240.0, 1400.0);
        self.zenplan_width = self.zenplan_width.map(|width| width.clamp(480.0, 5120.0));
        self.zenplan_height = self
            .zenplan_height
            .map(|height| height.clamp(420.0, 2160.0));
        let mut notebook_ids = std::collections::HashSet::new();
        self.zenplan_notebooks.retain_mut(|notebook| {
            notebook.name = notebook.name.trim().to_string();
            notebook.url = notebook.url.trim().to_string();
            notebook.icon_data_url = notebook
                .icon_data_url
                .take()
                .filter(|icon| is_valid_favicon_data_url(icon));
            if notebook.id.is_nil() || !notebook_ids.insert(notebook.id) {
                notebook.id = Uuid::new_v4();
                notebook_ids.insert(notebook.id);
            }
            notebook.window_width = notebook
                .window_width
                .map(|width| width.clamp(480.0, 5120.0));
            notebook.window_height = notebook
                .window_height
                .map(|height| height.clamp(420.0, 2160.0));
            !notebook.name.is_empty() && is_valid_web_app_url(&notebook.url)
        });
        self.hidden_window_apps = self
            .hidden_window_apps
            .drain(..)
            .map(|app| app.trim().to_ascii_lowercase())
            .filter(|app| !app.is_empty() && app.len() <= 260)
            .collect();
        self.hidden_window_apps.sort();
        self.hidden_window_apps.dedup();
        normalize_windows_apps(&mut self.recent_windows_apps, 20);
        normalize_windows_apps(&mut self.pinned_windows_apps, 24);
        self.sessions.retain(|session| {
            !session.name.trim().is_empty() && !session.command_line.trim().is_empty()
        });
        for session in &mut self.sessions {
            normalize_session(session);
        }
        for recent in &mut self.recent_groups {
            if recent.group_id.is_nil() {
                recent.group_id = recent
                    .sessions
                    .first()
                    .map_or_else(Uuid::new_v4, |session| {
                        if session.group_id.is_nil() {
                            session.id
                        } else {
                            session.group_id
                        }
                    });
            }
            recent.sessions.retain(|session| {
                !session.name.trim().is_empty() && !session.command_line.trim().is_empty()
            });
            for session in &mut recent.sessions {
                session.group_id = recent.group_id;
                normalize_session(session);
            }
        }
        self.recent_groups
            .retain(|recent| !recent.sessions.is_empty());
        self.recent_groups.truncate(MAX_RECENT_GROUPS);
    }

    pub fn reorder_zenplan_notebook(
        &mut self,
        notebook_id: Uuid,
        target_id: Uuid,
        place_after: bool,
    ) -> Option<bool> {
        let source_index = self
            .zenplan_notebooks
            .iter()
            .position(|notebook| notebook.id == notebook_id)?;
        self.zenplan_notebooks
            .iter()
            .position(|notebook| notebook.id == target_id)?;
        if notebook_id == target_id {
            return Some(false);
        }

        let notebook = self.zenplan_notebooks.remove(source_index);
        let target_index = self
            .zenplan_notebooks
            .iter()
            .position(|candidate| candidate.id == target_id)
            .expect("target pin was checked before removing a different pin");
        let insertion_index = target_index + usize::from(place_after);
        self.zenplan_notebooks.insert(insertion_index, notebook);
        Some(true)
    }

    pub fn reorder_pinned_windows_app(
        &mut self,
        app_key: &str,
        target_key: &str,
        place_after: bool,
    ) -> Option<bool> {
        let source_index = self
            .pinned_windows_apps
            .iter()
            .position(|app| app.app_key == app_key)?;
        self.pinned_windows_apps
            .iter()
            .position(|app| app.app_key == target_key)?;
        if app_key == target_key {
            return Some(false);
        }

        let app = self.pinned_windows_apps.remove(source_index);
        let target_index = self
            .pinned_windows_apps
            .iter()
            .position(|candidate| candidate.app_key == target_key)
            .expect("target pin was checked before removing a different pin");
        let insertion_index = target_index + usize::from(place_after);
        self.pinned_windows_apps.insert(insertion_index, app);
        Some(true)
    }

    pub fn take_group(&mut self, group_id: Uuid) -> Vec<SessionDefinition> {
        let mut removed = Vec::new();
        self.sessions.retain(|session| {
            if session.group_id == group_id {
                removed.push(session.clone());
                false
            } else {
                true
            }
        });
        removed
    }

    pub fn remember_group(&mut self, group_id: Uuid, sessions: Vec<SessionDefinition>) {
        if sessions.is_empty() {
            return;
        }
        self.recent_groups
            .retain(|recent| recent.group_id != group_id);
        self.recent_groups
            .insert(0, RecentGroup { group_id, sessions });
        self.recent_groups.truncate(MAX_RECENT_GROUPS);
    }

    pub fn restore_group(&mut self, group_id: Uuid) -> Option<Uuid> {
        let index = self
            .recent_groups
            .iter()
            .position(|recent| recent.group_id == group_id)?;
        let mut recent = self.recent_groups.remove(index);
        let restored_group_id = if self
            .sessions
            .iter()
            .any(|session| session.group_id == recent.group_id)
        {
            Uuid::new_v4()
        } else {
            recent.group_id
        };
        for session in &mut recent.sessions {
            if self.sessions.iter().any(|active| active.id == session.id) {
                session.id = Uuid::new_v4();
            }
            session.group_id = restored_group_id;
        }
        self.sessions.extend(recent.sessions);
        Some(restored_group_id)
    }
}

fn normalize_windows_apps(apps: &mut Vec<RecentWindowsApp>, maximum: usize) {
    let mut app_keys = std::collections::HashSet::new();
    apps.retain_mut(|app| {
        app.app_key = app.app_key.trim().to_ascii_lowercase();
        app.app_name = app.app_name.trim().to_string();
        app.executable_path = app.executable_path.trim().to_string();
        app.icon_data_url = app
            .icon_data_url
            .take()
            .filter(|icon| is_valid_favicon_data_url(icon));
        !app.app_key.is_empty()
            && app.app_key.len() <= 260
            && !app.app_name.is_empty()
            && app.app_name.len() <= 260
            && !app.executable_path.is_empty()
            && app.executable_path.len() <= 32_768
            && app_keys.insert(app.app_key.clone())
    });
    apps.truncate(maximum);
}

fn normalize_session(session: &mut SessionDefinition) {
    if session.group_id.is_nil() {
        session.group_id = session.id;
    }
    session.name = session.name.trim().to_string();
    session.command_line = session.command_line.trim().to_string();
    session.working_directory = session.working_directory.trim().to_string();
    session.legacy_codex_thread_id = session
        .legacy_codex_thread_id
        .take()
        .map(|thread_id| thread_id.trim().to_string())
        .filter(|thread_id| !thread_id.is_empty());
    let was_app_server_session = session.legacy_kind == LegacySessionKind::Codex
        || session
            .command_line
            .eq_ignore_ascii_case("codex app-server");
    if was_app_server_session {
        session.command_line = session
            .legacy_codex_thread_id
            .as_deref()
            .filter(|thread_id| Uuid::parse_str(thread_id).is_ok())
            .map_or_else(
                || "pwsh.exe -NoLogo -NoExit -Command codex".to_string(),
                |thread_id| format!("pwsh.exe -NoLogo -NoExit -Command codex resume {thread_id}"),
            );
    }
    session.legacy_kind = LegacySessionKind::Terminal;
    session.legacy_codex_thread_id = None;
    if !is_valid_color(&session.color) {
        session.color = "#2F78C4".to_string();
    }
    session.window_width = session.window_width.map(|width| width.clamp(420.0, 5120.0));
    session.window_height = session
        .window_height
        .map(|height| height.clamp(240.0, 1400.0));
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalVisibilityPayload {
    pub group_id: Uuid,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZenPlanVisibilityPayload {
    pub notebook_id: Uuid,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatePayload {
    pub session_id: Uuid,
    pub state: String,
    pub exit_code: Option<u32>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputPayload {
    pub session_id: Uuid,
    pub data: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalActivityPayload {
    pub session_id: Uuid,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorContext {
    pub session: Option<SessionDefinition>,
    pub target_group_id: Option<Uuid>,
    pub default_working_directory: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZenPlanEditorContext {
    pub notebook: Option<ZenPlanNotebook>,
    pub default_url: String,
}

pub fn default_working_directory() -> String {
    dirs::home_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
        .to_string_lossy()
        .to_string()
}

pub fn is_valid_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

pub fn is_valid_web_app_url(url: &str) -> bool {
    if url.len() > 2_048 {
        return false;
    }
    let Ok(parsed) = tauri::Url::parse(url) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
}

pub fn is_valid_favicon_data_url(icon: &str) -> bool {
    const MAX_ENCODED_ICON_LENGTH: usize = 400_000;
    icon.len() <= MAX_ENCODED_ICON_LENGTH
        && [
            "data:image/png;base64,",
            "data:image/jpeg;base64,",
            "data:image/gif;base64,",
            "data:image/webp;base64,",
            "data:image/svg+xml;base64,",
            "data:image/x-icon;base64,",
            "data:image/vnd.microsoft.icon;base64,",
            "data:image/avif;base64,",
        ]
        .iter()
        .any(|prefix| icon.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_six_digit_hex_colors() {
        assert!(is_valid_color("#12AbEF"));
        assert!(!is_valid_color("12ABEF"));
        assert!(!is_valid_color("#XYZXYZ"));
        assert!(!is_valid_color("#1234"));
    }

    #[test]
    fn accepts_http_and_https_web_app_urls() {
        assert!(is_valid_web_app_url("https://www.getzenplan.com/"));
        assert!(is_valid_web_app_url("https://notion.so/workspace"));
        assert!(is_valid_web_app_url("http://localhost:7034/planner"));
        assert!(!is_valid_web_app_url("javascript:alert(1)"));
        assert!(!is_valid_web_app_url("file:///C:/private.txt"));
        assert!(!is_valid_web_app_url("https://user:secret@example.com/"));
        assert!(!is_valid_web_app_url("not a URL"));
    }

    #[test]
    fn allows_every_web_app_to_be_unpinned() {
        let mut settings = AppSettings::default();
        assert!(settings.zenplan_notebooks.is_empty());

        settings.normalize();

        assert!(settings.zenplan_notebooks.is_empty());
        assert_eq!(settings.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn reorders_web_app_pins_before_or_after_a_target() {
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let third_id = Uuid::new_v4();
        let pin = |id, name: &str| ZenPlanNotebook {
            id,
            name: name.to_string(),
            ..ZenPlanNotebook::default()
        };
        let mut settings = AppSettings {
            zenplan_notebooks: vec![
                pin(first_id, "First"),
                pin(second_id, "Second"),
                pin(third_id, "Third"),
            ],
            ..AppSettings::default()
        };

        assert_eq!(
            settings.reorder_zenplan_notebook(first_id, third_id, true),
            Some(true)
        );
        assert_eq!(
            settings
                .zenplan_notebooks
                .iter()
                .map(|pin| pin.name.as_str())
                .collect::<Vec<_>>(),
            ["Second", "Third", "First"]
        );

        assert_eq!(
            settings.reorder_zenplan_notebook(first_id, second_id, false),
            Some(true)
        );
        assert_eq!(
            settings
                .zenplan_notebooks
                .iter()
                .map(|pin| pin.name.as_str())
                .collect::<Vec<_>>(),
            ["First", "Second", "Third"]
        );
    }

    #[test]
    fn accepts_only_bounded_image_data_for_favicons() {
        assert!(is_valid_favicon_data_url("data:image/png;base64,iVBORw0="));
        assert!(is_valid_favicon_data_url(
            "data:image/svg+xml;base64,PHN2Zz48L3N2Zz4="
        ));
        assert!(!is_valid_favicon_data_url(
            "data:text/html;base64,PHNjcmlwdD4="
        ));
        assert!(!is_valid_favicon_data_url(&format!(
            "data:image/png;base64,{}",
            "A".repeat(400_000)
        )));
    }

    #[test]
    fn normalizes_and_deduplicates_recent_windows_apps() {
        let mut settings = AppSettings {
            recent_windows_apps: vec![
                RecentWindowsApp {
                    app_key: " Code.EXE ".to_string(),
                    app_name: " Visual Studio Code ".to_string(),
                    executable_path: r" C:\Apps\Code.exe ".to_string(),
                    icon_data_url: None,
                },
                RecentWindowsApp {
                    app_key: "code.exe".to_string(),
                    app_name: "Duplicate".to_string(),
                    executable_path: r"C:\Other\Code.exe".to_string(),
                    icon_data_url: None,
                },
            ],
            ..AppSettings::default()
        };

        settings.normalize();

        assert_eq!(settings.recent_windows_apps.len(), 1);
        assert_eq!(settings.recent_windows_apps[0].app_key, "code.exe");
        assert_eq!(
            settings.recent_windows_apps[0].app_name,
            "Visual Studio Code"
        );
        assert_eq!(
            settings.recent_windows_apps[0].executable_path,
            r"C:\Apps\Code.exe"
        );
    }

    #[test]
    fn normalizes_and_reorders_pinned_windows_apps() {
        let app = |key: &str, name: &str| RecentWindowsApp {
            app_key: key.to_string(),
            app_name: name.to_string(),
            executable_path: format!(r"C:\Apps\{name}.exe"),
            icon_data_url: None,
        };
        let mut settings = AppSettings {
            pinned_windows_apps: vec![
                app("first.exe", "First"),
                app("second.exe", "Second"),
                app("third.exe", "Third"),
            ],
            ..AppSettings::default()
        };

        settings.normalize();
        assert_eq!(
            settings.reorder_pinned_windows_app("first.exe", "third.exe", true),
            Some(true)
        );
        assert_eq!(
            settings
                .pinned_windows_apps
                .iter()
                .map(|app| app.app_name.as_str())
                .collect::<Vec<_>>(),
            ["Second", "Third", "First"]
        );
    }

    #[test]
    fn normalizes_bounds_and_bad_session_color() {
        let mut settings = AppSettings {
            dock_height: 500.0,
            popup_width: Some(9000.0),
            popup_height: 100.0,
            ..AppSettings::default()
        };
        settings.sessions[0].color = "not-a-color".to_string();

        settings.normalize();

        assert_eq!(settings.dock_height, 64.0);
        assert_eq!(settings.popup_width, Some(5120.0));
        assert_eq!(settings.popup_height, 240.0);
        assert_eq!(settings.sessions[0].color, "#2F78C4");
    }

    #[test]
    fn migrates_the_original_dock_to_the_compact_height() {
        let mut settings = AppSettings {
            schema_version: 1,
            dock_height: 48.0,
            ..AppSettings::default()
        };

        settings.normalize();

        assert_eq!(settings.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(settings.dock_height, 40.0);
    }

    #[test]
    fn resets_mixed_unit_geometry_from_schema_three() {
        let mut settings = AppSettings {
            schema_version: 3,
            popup_width: Some(1146.0),
            popup_height: 778.0,
            terminal_x: Some(588),
            terminal_y: Some(566),
            ..AppSettings::default()
        };

        settings.normalize();

        assert_eq!(settings.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(settings.popup_width, None);
        assert_eq!(settings.popup_height, 700.0);
        assert_eq!(settings.terminal_x, None);
        assert_eq!(settings.terminal_y, None);
    }

    #[test]
    fn normalizes_each_terminal_windows_size() {
        let mut settings = AppSettings::default();
        settings.sessions[0].window_width = Some(100.0);
        settings.sessions[0].window_height = Some(9000.0);

        settings.normalize();

        assert_eq!(settings.sessions[0].window_width, Some(420.0));
        assert_eq!(settings.sessions[0].window_height, Some(1400.0));
    }

    #[test]
    fn normalizes_zenplan_window_size() {
        let mut settings = AppSettings {
            zenplan_width: Some(100.0),
            zenplan_height: Some(9000.0),
            ..AppSettings::default()
        };

        settings.normalize();

        assert_eq!(settings.zenplan_width, Some(480.0));
        assert_eq!(settings.zenplan_height, Some(2160.0));
    }

    #[test]
    fn migrates_the_single_zenplan_panel_to_a_notebook_pin() {
        let mut settings = AppSettings {
            schema_version: 10,
            zenplan_notebooks: Vec::new(),
            zenplan_x: Some(2000),
            zenplan_y: Some(100),
            zenplan_width: Some(900.0),
            zenplan_height: Some(1000.0),
            ..AppSettings::default()
        };

        settings.normalize();

        assert_eq!(settings.zenplan_notebooks.len(), 1);
        assert_eq!(settings.zenplan_notebooks[0].window_x, Some(2000));
        assert_eq!(settings.zenplan_notebooks[0].window_width, Some(900.0));
    }

    #[test]
    fn migrates_existing_sessions_into_independent_window_groups() {
        let mut settings = AppSettings {
            schema_version: 5,
            ..AppSettings::default()
        };
        settings.sessions[0].group_id = Uuid::nil();
        let session_id = settings.sessions[0].id;

        settings.normalize();

        assert_eq!(settings.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(settings.sessions[0].group_id, session_id);
    }

    #[test]
    fn migrates_app_server_sessions_to_resumable_cli_commands() {
        let mut legacy = serde_json::to_value(SessionDefinition::default_powershell())
            .expect("serialize session");
        legacy["kind"] = serde_json::json!("codex");
        legacy["commandLine"] = serde_json::json!("codex app-server");
        legacy["codexThreadId"] = serde_json::json!("019fd83c-cf5b-73a0-a2f2-080a71e6fd90");
        let session: SessionDefinition = serde_json::from_value(legacy).expect("legacy session");
        let mut settings = AppSettings {
            schema_version: 7,
            sessions: vec![session],
            ..AppSettings::default()
        };

        settings.normalize();

        assert_eq!(settings.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            settings.sessions[0].command_line,
            "pwsh.exe -NoLogo -NoExit -Command codex resume 019fd83c-cf5b-73a0-a2f2-080a71e6fd90"
        );
        let serialized = serde_json::to_value(&settings.sessions[0]).expect("serialize migration");
        assert!(serialized.get("kind").is_none());
        assert!(serialized.get("codexThreadId").is_none());
    }

    #[test]
    fn remembers_and_restores_a_complete_window_group() {
        let mut settings = AppSettings::default();
        let group_id = settings.sessions[0].group_id;
        let mut second = SessionDefinition::default_powershell();
        second.group_id = group_id;
        second.name = "Second tab".to_string();
        settings.sessions.push(second);

        let removed = settings.take_group(group_id);
        settings.remember_group(group_id, removed);

        assert!(settings.sessions.is_empty());
        assert_eq!(settings.recent_groups.len(), 1);
        assert_eq!(settings.recent_groups[0].sessions.len(), 2);

        let restored_group_id = settings.restore_group(group_id).expect("restore group");

        assert_eq!(restored_group_id, group_id);
        assert!(settings.recent_groups.is_empty());
        assert_eq!(settings.sessions.len(), 2);
        assert!(
            settings
                .sessions
                .iter()
                .all(|session| session.group_id == group_id)
        );
    }
}
