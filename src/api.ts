import { invoke } from '@tauri-apps/api/core';
import type {
  ActiveWindowApp,
  AppSettings,
  EditorContext,
  SessionDefinition,
  ZenPlanEditorContext,
  ZenPlanNotebook
} from './types';

export const api = {
  uiHeartbeat: () => invoke<void>('ui_heartbeat'),
  getSettings: () => invoke<AppSettings>('get_settings'),
  getVisibleGroups: () => invoke<string[]>('get_visible_groups'),
  getVisibleZenPlanNotebooks: () => invoke<string[]>('get_visible_zenplan_notebooks'),
  toggleZenPlan: (notebookId: string) => invoke<void>('toggle_zenplan', { notebookId }),
  listActiveWindows: () => invoke<ActiveWindowApp[]>('list_active_windows'),
  activateExternalWindow: (handle: string, wasFocused: boolean) =>
    invoke<boolean>('activate_external_window', { handle, wasFocused }),
  focusExternalWindow: (handle: string) => invoke<void>('focus_external_window', { handle }),
  closeExternalWindow: (handle: string) => invoke<void>('close_external_window', { handle }),
  launchRecentWindowsApp: (appKey: string) =>
    invoke<void>('launch_recent_windows_app', { appKey }),
  openWindowsStartSearch: () => invoke<void>('open_windows_start_search'),
  readClipboardText: () => invoke<string | null>('read_clipboard_text'),
  writeClipboardText: (text: string) => invoke<void>('write_clipboard_text', { text }),
  setWindowAppPinned: (appKey: string, pinned: boolean) =>
    invoke<void>('set_window_app_pinned', { appKey, pinned }),
  reorderPinnedWindowApp: (appKey: string, targetKey: string, placeAfter: boolean) =>
    invoke<void>('reorder_pinned_windows_app', { appKey, targetKey, placeAfter }),
  setWindowAppVisible: (appKey: string, visible: boolean) =>
    invoke<void>('set_window_app_visible', { appKey, visible }),
  openWindowAppsEditor: () => invoke<void>('open_window_apps_editor'),
  hideWindowAppsEditor: () => invoke<void>('hide_window_apps_editor'),
  toggleLauncher: () => invoke<void>('toggle_launcher'),
  hideLauncher: () => invoke<void>('hide_launcher'),
  isWindowsTaskbarVisible: () => invoke<boolean>('is_windows_taskbar_visible'),
  toggleWindowsTaskbar: () => invoke<boolean>('toggle_windows_taskbar'),
  activateGroup: (groupId: string) => invoke<void>('activate_group', { groupId }),
  toggleAllTerminals: () => invoke<boolean>('toggle_all_terminals'),
  startSession: (sessionId: string, cols: number, rows: number) =>
    invoke<void>('start_session', { sessionId, cols, rows }),
  hideTerminal: (groupId?: string) => invoke<void>('hide_terminal', { groupId: groupId ?? null }),
  closeTerminal: (groupId: string, remember: boolean) =>
    invoke<void>('close_terminal', { groupId, remember }),
  restoreTerminal: (groupId: string) => invoke<string>('restore_terminal', { groupId }),
  clearRecentGroups: () => invoke<void>('clear_recent_groups'),
  stopSession: (sessionId: string) => invoke<void>('stop_session', { sessionId }),
  restartSession: (sessionId: string) => invoke<void>('restart_session', { sessionId }),
  writeSession: (sessionId: string, data: string) => invoke<void>('write_session', { sessionId, data }),
  resizeSession: (sessionId: string, cols: number, rows: number) =>
    invoke<void>('resize_session', { sessionId, cols, rows }),
  updateSessionWorkingDirectory: (sessionId: string, workingDirectory: string) =>
    invoke<void>('update_session_working_directory', { sessionId, workingDirectory }),
  openEditor: (sessionId?: string, groupId?: string) =>
    invoke<void>('open_editor', { sessionId: sessionId ?? null, groupId: groupId ?? null }),
  getEditorContext: () => invoke<EditorContext>('get_editor_context'),
  hideEditor: () => invoke<void>('hide_editor'),
  saveSession: (session: SessionDefinition) => invoke<void>('save_session', { session }),
  addPowerShellTerminal: (groupId?: string) =>
    invoke<SessionDefinition>('add_powershell_terminal', { groupId: groupId ?? null }),
  addClaudeSession: (workingDirectory: string, groupId?: string) =>
    invoke<SessionDefinition>('add_claude_session', {
      workingDirectory,
      groupId: groupId ?? null
    }),
  addCodexSession: (workingDirectory: string, groupId?: string) =>
    invoke<SessionDefinition>('add_codex_session', {
      workingDirectory,
      groupId: groupId ?? null
    }),
  setSessionColor: (sessionId: string, color: string) =>
    invoke<void>('set_session_color', { sessionId, color }),
  renameSession: (sessionId: string, name: string) =>
    invoke<void>('rename_session', { sessionId, name }),
  openRename: (sessionId: string) => invoke<void>('open_rename', { sessionId }),
  getRenameContext: () => invoke<SessionDefinition | null>('get_rename_context'),
  hideRename: () => invoke<void>('hide_rename'),
  openColor: (sessionId: string) => invoke<void>('open_color', { sessionId }),
  getColorContext: () => invoke<SessionDefinition | null>('get_color_context'),
  hideColor: () => invoke<void>('hide_color'),
  openZenPlanEditor: (notebookId?: string) =>
    invoke<void>('open_zenplan_editor', { notebookId: notebookId ?? null }),
  getZenPlanEditorContext: () => invoke<ZenPlanEditorContext>('get_zenplan_editor_context'),
  hideZenPlanEditor: () => invoke<void>('hide_zenplan_editor'),
  saveZenPlanNotebook: (notebookId: string | undefined, name: string, url: string) =>
    invoke<ZenPlanNotebook>('save_zenplan_notebook', {
      notebookId: notebookId ?? null,
      name,
      url
    }),
  deleteZenPlanNotebook: (notebookId: string) =>
    invoke<void>('delete_zenplan_notebook', { notebookId }),
  reorderZenPlanNotebook: (notebookId: string, targetId: string, placeAfter: boolean) =>
    invoke<void>('reorder_zenplan_notebook', { notebookId, targetId, placeAfter }),
  deleteSession: (sessionId: string) => invoke<void>('delete_session', { sessionId }),
  moveSession: (sessionId: string, direction: number) =>
    invoke<void>('move_session', { sessionId, direction }),
  pickFolder: (startingDirectory: string) => invoke<string | null>('pick_folder', { startingDirectory }),
  saveTerminalBounds: (groupId: string, x: number, y: number, width: number, height: number) =>
    invoke<void>('save_terminal_bounds', { groupId, x, y, width, height }),
  exitApp: () => invoke<void>('exit_app')
};
