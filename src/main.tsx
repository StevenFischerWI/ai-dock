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
import './styles.css';

installGlobalErrorHandling();

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
