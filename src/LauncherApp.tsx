import { useCallback, useEffect, useMemo, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from './api';
import { isTestBuild } from './buildFlavor';
import { runUiAction } from './errorHandling';
import { showRecentWindowAppMenu } from './windowAppMenu';
import type { ActiveWindowApp, AppSettings, RecentWindowsApp } from './types';

function AppIcon({ app, activeApps }: { app: RecentWindowsApp; activeApps: ActiveWindowApp[] }) {
  const active = activeApps.find((candidate) => candidate.appKey === app.appKey);
  const iconDataUrl = active?.iconDataUrl ?? app.iconDataUrl;
  return iconDataUrl
    ? <img className="launcher-app-icon" src={iconDataUrl} alt="" draggable={false} />
    : <span className="launcher-app-fallback">{app.appName.slice(0, 1).toUpperCase()}</span>;
}

export function LauncherApp() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [activeApps, setActiveApps] = useState<ActiveWindowApp[]>([]);

  const refresh = useCallback(async () => {
    const [nextSettings, nextApps] = await Promise.all([
      api.getSettings(),
      api.listActiveWindows()
    ]);
    setSettings(nextSettings);
    setActiveApps(nextApps);
  }, []);

  useEffect(() => {
    const launcher = getCurrentWindow();
    const unlisteners: Array<() => void> = [];
    runUiAction('Could not load the launcher', async () => {
      await refresh();
      unlisteners.push(
        await listen<AppSettings>('settings-changed', (event) => setSettings(event.payload)),
        await launcher.onFocusChanged(({ payload: focused }) => {
          if (focused) runUiAction('Could not refresh the launcher', refresh);
        })
      );
    });
    return () => unlisteners.forEach((unlisten) => unlisten());
  }, [refresh]);

  const hiddenApps = settings?.hiddenWindowApps ?? [];
  const recentApps = useMemo(
    () => (settings?.recentWindowsApps ?? [])
      .filter((app) => !hiddenApps.includes(app.appKey))
      .slice(0, 8),
    [hiddenApps, settings?.recentWindowsApps]
  );
  const recentGroups = settings?.recentGroups.slice(0, 5) ?? [];
  const webApps = settings?.zenplanNotebooks ?? [];
  const pinnedAppKeys = new Set((settings?.pinnedWindowsApps ?? []).map((app) => app.appKey));

  const perform = (context: string, action: () => Promise<unknown>) => {
    runUiAction(context, async () => {
      await api.hideLauncher();
      await action();
    });
  };

  const openRecentApp = (app: RecentWindowsApp) => {
    perform(`Could not launch ${app.appName}`, async () => {
      const running = activeApps.find((candidate) => candidate.appKey === app.appKey);
      if (running) await api.focusExternalWindow(running.handle);
      else await api.launchRecentWindowsApp(app.appKey);
    });
  };

  const addCli = (kind: 'claude' | 'codex') => {
    perform(`Could not start ${kind === 'claude' ? 'Claude' : 'Codex'}`, async () => {
      const startingDirectory = settings?.sessions
        .find((session) => session.workingDirectory)?.workingDirectory ?? '';
      const workingDirectory = await api.pickFolder(startingDirectory);
      if (!workingDirectory) return;
      const session = kind === 'claude'
        ? await api.addClaudeSession(workingDirectory)
        : await api.addCodexSession(workingDirectory);
      await api.activateGroup(session.groupId);
    });
  };

  return (
    <main
      className={`launcher-shell ${isTestBuild ? 'is-test-build' : ''}`}
      onKeyDown={(event) => {
        if (event.key === 'Escape') void api.hideLauncher();
      }}
    >
      <header className="launcher-header">
        <span className="launcher-brand-mark">AI</span>
        <strong>AI Dock</strong>
        {isTestBuild && <span className="launcher-test-badge">TEST</span>}
      </header>

      <button
        className="launcher-search"
        onClick={() => perform('Could not open Windows Search', api.openWindowsStartSearch)}
      >
        <span aria-hidden="true">⌕</span>
        Launch app…
      </button>

      <div className="launcher-columns">
        <section className="launcher-main-column">
          <h2>Recent apps</h2>
          <div className="launcher-app-list">
            {recentApps.length === 0 && <p className="launcher-empty">Recently launched apps appear here.</p>}
            {recentApps.map((app) => (
              <button
                key={app.appKey}
                className="launcher-app-row"
                onClick={() => openRecentApp(app)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  runUiAction(
                    'Could not change the app pin',
                    () => showRecentWindowAppMenu(app, pinnedAppKeys.has(app.appKey))
                  );
                }}
              >
                <AppIcon app={app} activeApps={activeApps} />
                <span>{app.appName}</span>
                {activeApps.some((candidate) => candidate.appKey === app.appKey) && (
                  <span className="launcher-running-dot" title="Running" />
                )}
              </button>
            ))}
          </div>

          {recentGroups.length > 0 && (
            <>
              <div className="launcher-section-heading">
                <h2>Recently closed</h2>
              <button onClick={() => perform('Could not clear recent terminals', api.clearRecentGroups)}>
                Clear
              </button>
              </div>
              <div className="launcher-recent-groups">
                {recentGroups.map((group) => (
                  <button
                    key={group.groupId}
                    onClick={() => perform('Could not restore the terminal window', async () => {
                      const groupId = await api.restoreTerminal(group.groupId);
                      await api.activateGroup(groupId);
                    })}
                  >
                    <span>{group.sessions[0]?.name ?? 'Terminal window'}</span>
                    <small>{group.sessions.length} tab{group.sessions.length === 1 ? '' : 's'}</small>
                  </button>
                ))}
              </div>
            </>
          )}
        </section>

        <aside className="launcher-side-column">
          <h2>Terminal</h2>
          <button onClick={() => perform('Could not start PowerShell', async () => {
            const session = await api.addPowerShellTerminal();
            await api.activateGroup(session.groupId);
          })}>PowerShell</button>
          <button onClick={() => addCli('claude')}>Claude…</button>
          <button onClick={() => addCli('codex')}>Codex…</button>

          <h2>Web apps</h2>
          {webApps.map((webApp) => (
            <button
              key={webApp.id}
              className="launcher-web-app"
              onClick={() => perform(`Could not open ${webApp.name}`, () => api.toggleZenPlan(webApp.id))}
            >
              <span className="launcher-web-mark">
                {webApp.iconDataUrl
                  ? <img src={webApp.iconDataUrl} alt="" draggable={false} />
                  : webApp.name.slice(0, 1).toUpperCase()}
              </span>
              <span>{webApp.name}</span>
            </button>
          ))}
          <button onClick={() => perform('Could not open the web app editor', () => api.openZenPlanEditor())}>
            + Pin web app…
          </button>

          <h2>Settings</h2>
          <button onClick={() => perform('Could not open dock app settings', api.openWindowAppsEditor)}>
            Dock apps…
          </button>
        </aside>
      </div>

      <footer className="launcher-footer">
        <button onClick={() => perform('Could not hide terminal windows', () => api.hideTerminal())}>
          Hide terminals
        </button>
        <span />
        <button className="launcher-exit" onClick={() => runUiAction('Could not exit AI Dock', api.exitApp)}>
          Exit
        </button>
      </footer>
    </main>
  );
}
