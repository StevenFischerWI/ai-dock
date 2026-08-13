import { createRoot } from 'react-dom/client';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { DockApp } from './DockApp';
import { ColorApp } from './ColorApp';
import { EditorApp } from './EditorApp';
import { RenameApp } from './RenameApp';
import { TerminalApp } from './TerminalApp';
import { ZenPlanEditorApp } from './ZenPlanEditorApp';
import { WindowAppsApp } from './WindowAppsApp';
import { LauncherApp } from './LauncherApp';
import { AppErrorBoundary } from './AppErrorBoundary';
import { installGlobalErrorHandling } from './errorHandling';
import { api } from './api';
import './styles.css';

installGlobalErrorHandling();

// A dead WebView2 browser can leave its native window responsive even though the
// page and terminal renderer are frozen. Keep a cheap page-level heartbeat so the
// Rust shell can restart the UI while detached terminal sessions keep running.
const sendHeartbeat = () => {
  void api.uiHeartbeat().catch(() => undefined);
};
sendHeartbeat();
window.setInterval(sendHeartbeat, 2_000);

const label = getCurrentWindow().label;
const Component = label.startsWith('terminal-')
    ? TerminalApp
  : label === 'color'
    ? ColorApp
  : label === 'editor'
    ? EditorApp
  : label === 'zenplan-editor'
    ? ZenPlanEditorApp
  : label === 'window-apps'
    ? WindowAppsApp
    : label === 'launcher'
      ? LauncherApp
      : label === 'rename'
        ? RenameApp
        : DockApp;

createRoot(document.getElementById('root')!).render(
  <AppErrorBoundary>
    <Component />
  </AppErrorBoundary>
);
