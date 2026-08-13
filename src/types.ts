export interface SessionDefinition {
  id: string;
  groupId: string;
  name: string;
  color: string;
  commandLine: string;
  workingDirectory: string;
  windowX: number | null;
  windowY: number | null;
  windowWidth: number | null;
  windowHeight: number | null;
}

export interface RecentGroup {
  groupId: string;
  sessions: SessionDefinition[];
}

export interface ZenPlanNotebook {
  id: string;
  name: string;
  url: string;
  iconDataUrl: string | null;
  windowX: number | null;
  windowY: number | null;
  windowWidth: number | null;
  windowHeight: number | null;
}

export interface ActiveWindowApp {
  handle: string;
  appKey: string;
  appName: string;
  title: string;
  iconDataUrl: string | null;
  isFocused: boolean;
  isMinimized: boolean;
}

export interface RecentWindowsApp {
  appKey: string;
  appName: string;
  executablePath: string;
  iconDataUrl: string | null;
}

export interface AppSettings {
  schemaVersion: number;
  dockHeight: number;
  popupWidth: number | null;
  popupHeight: number;
  terminalX: number | null;
  terminalY: number | null;
  sessions: SessionDefinition[];
  recentGroups: RecentGroup[];
  hiddenWindowApps: string[];
  recentWindowsApps: RecentWindowsApp[];
  pinnedWindowsApps: RecentWindowsApp[];
  zenplanNotebooks: ZenPlanNotebook[];
  zenplanX: number | null;
  zenplanY: number | null;
  zenplanWidth: number | null;
  zenplanHeight: number | null;
}

export interface ZenPlanVisibilityPayload {
  notebookId: string;
  visible: boolean;
}

export interface ZenPlanEditorContext {
  notebook: ZenPlanNotebook | null;
  defaultUrl: string;
}

export type SessionState = 'stopped' | 'starting' | 'running' | 'attention' | 'exited' | 'failed';

export interface SessionStatePayload {
  sessionId: string;
  state: SessionState;
  exitCode?: number | null;
  message?: string | null;
}

export interface TerminalOutputPayload {
    sessionId: string;
    data: string;
}

export interface TerminalActivityPayload {
  sessionId: string;
}

export interface TerminalVisibilityPayload {
  groupId: string;
  visible: boolean;
}

export interface EditorContext {
  session: SessionDefinition | null;
  targetGroupId: string | null;
  defaultWorkingDirectory: string;
}
