import { useCallback, useEffect, useMemo, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from './api';
import type { ActiveWindowApp, AppSettings } from './types';

export function WindowAppsApp() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [activeWindows, setActiveWindows] = useState<ActiveWindowApp[]>([]);

  const refreshWindows = useCallback(async () => {
    try {
      const next = await api.listActiveWindows();
      setActiveWindows((current) => {
        const unchanged = current.length === next.length && current.every((app, index) => {
          const candidate = next[index];
          return app.handle === candidate.handle
            && app.appKey === candidate.appKey
            && app.appName === candidate.appName
            && app.title === candidate.title
            && app.iconDataUrl === candidate.iconDataUrl
            && app.isFocused === candidate.isFocused
            && app.isMinimized === candidate.isMinimized;
        });
        return unchanged ? current : next;
      });
    } catch {
      // Keep the last good snapshot if Windows enumeration briefly fails.
    }
  }, []);

  useEffect(() => {
    const currentWindow = getCurrentWindow();
    const unlisteners: Array<() => void> = [];
    void (async () => {
      const loadedSettings = await api.getSettings();
      setSettings(loadedSettings);
      unlisteners.push(
        await listen<AppSettings>('settings-changed', (event) => {
          setSettings(event.payload);
        }),
        await currentWindow.onFocusChanged(({ payload: focused }) => {
          if (focused) void refreshWindows();
        })
      );
      if (await currentWindow.isVisible()) await refreshWindows();
    })();
    const interval = window.setInterval(() => {
      void currentWindow.isVisible().then((visible) => {
        if (visible) void refreshWindows();
      });
    }, 2000);
    return () => {
      unlisteners.forEach((unlisten) => unlisten());
      window.clearInterval(interval);
    };
  }, [refreshWindows]);

  const apps = useMemo(() => {
    const grouped = new Map<string, { appKey: string; appName: string; iconDataUrl: string | null; count: number }>();
    for (const activeWindow of activeWindows) {
      const existing = grouped.get(activeWindow.appKey);
      if (existing) existing.count += 1;
      else grouped.set(activeWindow.appKey, {
        appKey: activeWindow.appKey,
        appName: activeWindow.appName,
        iconDataUrl: activeWindow.iconDataUrl,
        count: 1
      });
    }
    return [...grouped.values()].sort((left, right) => left.appName.localeCompare(right.appName));
  }, [activeWindows]);

  const setVisible = async (appKey: string, visible: boolean) => {
    setSettings((current) => {
      if (!current) return current;
      const hidden = current.hiddenWindowApps.filter((candidate) => candidate !== appKey);
      if (!visible) hidden.push(appKey);
      return { ...current, hiddenWindowApps: hidden };
    });
    try {
      await api.setWindowAppVisible(appKey, visible);
    } catch {
      setSettings(await api.getSettings());
    }
  };

  return (
    <main className="window-apps-shell">
      <header
        className="window-apps-header window-drag-handle"
        onMouseDown={(event) => {
          if (event.button === 0 && !(event.target as HTMLElement).closest('button, input, label')) {
            void getCurrentWindow().startDragging();
          }
        }}
      >
        <strong>Apps shown in dock</strong>
        <button title="Done" aria-label="Close app list" onClick={() => void api.hideWindowAppsEditor()}>×</button>
      </header>
      <div className="window-apps-list">
        {apps.length === 0 && <p className="window-apps-empty">No other active Windows apps.</p>}
        {apps.map((app) => {
          const visible = !(settings?.hiddenWindowApps ?? []).includes(app.appKey);
          return (
            <label key={app.appKey} className="window-app-choice">
              <input
                type="checkbox"
                checked={visible}
                onChange={(event) => void setVisible(app.appKey, event.target.checked)}
              />
              {app.iconDataUrl
                ? <img src={app.iconDataUrl} alt="" />
                : <span className="window-app-choice-fallback">{app.appName.slice(0, 1).toUpperCase()}</span>}
              <span className="window-app-choice-name">{app.appName}</span>
              {app.count > 1 && <span className="window-app-choice-count">{app.count} windows</span>}
            </label>
          );
        })}
      </div>
      <footer className="window-apps-footer">
        <span>Changes apply immediately.</span>
        <button className="primary-button" onClick={() => void api.hideWindowAppsEditor()}>Done</button>
      </footer>
    </main>
  );
}
