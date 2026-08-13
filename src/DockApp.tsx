import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { api } from './api';
import { isTestBuild } from './buildFlavor';
import { showRecentAppsMenu } from './dockMenu';
import { showSessionMenu } from './sessionMenu';
import { showZenPlanMenu } from './zenPlanMenu';
import { showRecentWindowAppMenu, showWindowAppMenu } from './windowAppMenu';
import { runUiAction } from './errorHandling';
import type {
  ActiveWindowApp,
  AppSettings,
  SessionState,
  SessionStatePayload,
  TerminalActivityPayload,
  TerminalVisibilityPayload,
  ZenPlanVisibilityPayload
} from './types';

const dockDateFormatter = new Intl.DateTimeFormat(undefined, {
  weekday: 'short',
  month: 'numeric',
  day: 'numeric'
});
const dockTimeFormatter = new Intl.DateTimeFormat(undefined, {
  hour: 'numeric',
  minute: '2-digit'
});

function activeAppListsMatch(left: ActiveWindowApp[], right: ActiveWindowApp[]) {
  return left.length === right.length && left.every((app, index) => {
    const candidate = right[index];
    return app.handle === candidate.handle
      && app.appKey === candidate.appKey
      && app.appName === candidate.appName
      && app.title === candidate.title
      && app.iconDataUrl === candidate.iconDataUrl
      && app.isFocused === candidate.isFocused
      && app.isMinimized === candidate.isMinimized;
  });
}

interface HorizontalDropTarget {
  id: string;
  placeAfter: boolean;
}

interface PointerDrag {
  id: string;
  pointerId: number;
  startX: number;
  startY: number;
  startScreenX: number;
  lastX: number;
  dragging: boolean;
}

function reconcileActiveAppOrder(apps: ActiveWindowApp[], preferredHandles: string[]) {
  const appsByHandle = new Map(apps.map((app) => [app.handle, app]));
  const ordered = preferredHandles
    .map((handle) => appsByHandle.get(handle))
    .filter((app): app is ActiveWindowApp => Boolean(app));
  const retainedHandles = new Set(ordered.map((app) => app.handle));
  ordered.push(...apps.filter((app) => !retainedHandles.has(app.handle)));
  return ordered;
}

function reorderById<T>(
  items: T[],
  sourceId: string,
  targetId: string,
  placeAfter: boolean,
  getId: (item: T) => string
) {
  const sourceIndex = items.findIndex((item) => getId(item) === sourceId);
  if (sourceIndex < 0 || !items.some((item) => getId(item) === targetId)) return items;
  const next = [...items];
  const [source] = next.splice(sourceIndex, 1);
  const targetIndex = next.findIndex((item) => getId(item) === targetId);
  next.splice(targetIndex + (placeAfter ? 1 : 0), 0, source);
  return next;
}

function horizontalDropTarget(
  selector: string,
  idAttribute: string,
  drag: PointerDrag | null,
  clientX: number,
  screenX: number
): HorizontalDropTarget | null {
  const buttons = Array.from(document.querySelectorAll<HTMLButtonElement>(selector));
  const sourceButton = buttons.find((button) => button.getAttribute(idAttribute) === drag?.id);
  const sourceBounds = sourceButton?.getBoundingClientRect();
  const projectedX = sourceBounds && drag
    ? sourceBounds.left + sourceBounds.width / 2 + screenX - drag.startScreenX
    : Number.NaN;
  const pointerXs = [clientX, screenX - window.screenX, projectedX];
  const matches = pointerXs.map((pointerX) => {
    const button = buttons.find((candidate) => {
      const bounds = candidate.getBoundingClientRect();
      return pointerX >= bounds.left && pointerX <= bounds.right;
    });
    if (!button) return null;
    const bounds = button.getBoundingClientRect();
    return { button, bounds, pointerX };
  });
  const match = matches.find((candidate) => (
    candidate !== null && candidate.button.getAttribute(idAttribute) !== drag?.id
  )) ?? matches.find((candidate) => candidate !== null);
  const targetId = match?.button.getAttribute(idAttribute);
  if (!match || !targetId || targetId === drag?.id) return null;
  return {
    id: targetId,
    placeAfter: match.pointerX >= match.bounds.left + match.bounds.width / 2
  };
}

function windowAppDropTargetFromDelta(drag: PointerDrag): HorizontalDropTarget | null {
  const buttons = Array.from(
    document.querySelectorAll<HTMLButtonElement>('[data-window-handle]')
  );
  const sourceIndex = buttons.findIndex(
    (button) => button.getAttribute('data-window-handle') === drag.id
  );
  if (sourceIndex < 0 || buttons.length < 2) return null;

  const centers = buttons.map((button) => {
    const bounds = button.getBoundingClientRect();
    return bounds.left + bounds.width / 2;
  });
  const slotWidth = Math.max(1, Math.abs(centers[1] - centers[0]));
  const slotOffset = Math.round((drag.lastX - drag.startX) / slotWidth);
  const targetIndex = Math.max(0, Math.min(buttons.length - 1, sourceIndex + slotOffset));
  if (targetIndex === sourceIndex) return null;
  const targetId = buttons[targetIndex].getAttribute('data-window-handle');
  return targetId ? { id: targetId, placeAfter: targetIndex > sourceIndex } : null;
}

export function DockApp() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [visibleGroups, setVisibleGroups] = useState<Set<string>>(() => new Set());
  const [visibleZenPlanNotebooks, setVisibleZenPlanNotebooks] = useState<Set<string>>(() => new Set());
  const [activeApps, setActiveApps] = useState<ActiveWindowApp[]>([]);
  const [windowsTaskbarVisible, setWindowsTaskbarVisible] = useState(true);
  const [states, setStates] = useState<Record<string, SessionState>>({});
  const [now, setNow] = useState(() => new Date());
  const [draggedWindowHandle, setDraggedWindowHandle] = useState<string | null>(null);
  const [windowAppDropTarget, setWindowAppDropTarget] = useState<HorizontalDropTarget | null>(null);
  const [draggedPinnedAppKey, setDraggedPinnedAppKey] = useState<string | null>(null);
  const [pinnedAppDropTarget, setPinnedAppDropTarget] = useState<HorizontalDropTarget | null>(null);
  const [draggedNotebookId, setDraggedNotebookId] = useState<string | null>(null);
  const [notebookDropTarget, setNotebookDropTarget] = useState<HorizontalDropTarget | null>(null);
  const visibleRef = useRef(visibleGroups);
  const settingsRef = useRef<AppSettings | null>(settings);
  const activeAppOrderRef = useRef<string[]>([]);
  const windowAppPointerDragRef = useRef<PointerDrag | null>(null);
  const windowAppDropTargetRef = useRef<HorizontalDropTarget | null>(null);
  const suppressWindowAppClickRef = useRef(false);
  const pinnedAppPointerDragRef = useRef<PointerDrag | null>(null);
  const pinnedAppDropTargetRef = useRef<HorizontalDropTarget | null>(null);
  const suppressPinnedAppClickRef = useRef(false);
  const notebookPointerDragRef = useRef<PointerDrag | null>(null);
  const notebookDropTargetRef = useRef<HorizontalDropTarget | null>(null);
  const suppressNotebookClickRef = useRef(false);

  useEffect(() => {
    visibleRef.current = visibleGroups;
  }, [visibleGroups]);

  useEffect(() => {
    let timer: number | undefined;
    const updateClock = () => {
      setNow(new Date());
      const untilNextMinute = 60_050 - (Date.now() % 60_000);
      timer = window.setTimeout(updateClock, untilNextMinute);
    };
    const untilNextMinute = 60_050 - (Date.now() % 60_000);
    timer = window.setTimeout(updateClock, untilNextMinute);
    return () => window.clearTimeout(timer);
  }, []);

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    void (async () => {
      const [loadedVisible, loadedZenPlanVisible, loadedTaskbarVisible, loadedApps] = await Promise.all([
        api.getVisibleGroups(),
        api.getVisibleZenPlanNotebooks(),
        api.isWindowsTaskbarVisible(),
        api.listActiveWindows()
      ]);
      // Listing apps may add newly observed processes to the recent-app launcher.
      // Read settings afterward so the first menu opening sees that update.
      const loadedSettings = await api.getSettings();
      if (!disposed) {
        const orderedApps = reconcileActiveAppOrder(loadedApps, activeAppOrderRef.current);
        activeAppOrderRef.current = orderedApps.map((app) => app.handle);
        settingsRef.current = loadedSettings;
        setSettings(loadedSettings);
        setVisibleGroups(new Set(loadedVisible));
        setVisibleZenPlanNotebooks(new Set(loadedZenPlanVisible));
        setWindowsTaskbarVisible(loadedTaskbarVisible);
        setActiveApps(orderedApps);
      }

      unlisteners.push(
        await listen<AppSettings>('settings-changed', (event) => {
          settingsRef.current = event.payload;
          setSettings(event.payload);
        }),
        await listen<TerminalVisibilityPayload>('terminal-visibility', (event) => {
          setVisibleGroups((current) => {
            const next = new Set(current);
            if (event.payload.visible) next.add(event.payload.groupId);
            else next.delete(event.payload.groupId);
            return next;
          });
        }),
        await listen<ZenPlanVisibilityPayload>('zenplan-visibility', (event) => {
          setVisibleZenPlanNotebooks((current) => {
            const next = new Set(current);
            if (event.payload.visible) next.add(event.payload.notebookId);
            else next.delete(event.payload.notebookId);
            return next;
          });
        }),
        await listen<SessionStatePayload>('session-state', (event) => {
          setStates((current) => ({ ...current, [event.payload.sessionId]: event.payload.state }));
        }),
        await listen<TerminalActivityPayload>('terminal-activity', (event) => {
          setStates((current) => {
            const session = settingsRef.current?.sessions.find(
              (candidate) => candidate.id === event.payload.sessionId
            );
            const shouldFlag = Boolean(session && !visibleRef.current.has(session.groupId));
            return shouldFlag ? { ...current, [event.payload.sessionId]: 'attention' } : current;
          });
        })
      );
    })();

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    const refresh = async () => {
      try {
        const [taskbarVisible, apps] = await Promise.all([
          api.isWindowsTaskbarVisible(),
          api.listActiveWindows()
        ]);
        if (!disposed) {
          const orderedApps = reconcileActiveAppOrder(apps, activeAppOrderRef.current);
          activeAppOrderRef.current = orderedApps.map((app) => app.handle);
          setWindowsTaskbarVisible((current) => current === taskbarVisible ? current : taskbarVisible);
          setActiveApps((current) => activeAppListsMatch(current, orderedApps) ? current : orderedApps);
        }
      } catch {
        // A transient Windows enumeration failure should not disturb the dock.
      } finally {
        if (!disposed) timer = window.setTimeout(refresh, 1200);
      }
    };
    timer = window.setTimeout(refresh, 1200);
    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, []);

  const sessions = settings?.sessions ?? [];
  const hiddenWindowApps = settings?.hiddenWindowApps ?? [];
  const pinnedWindowApps = (settings?.pinnedWindowsApps ?? [])
    .filter((app) => !hiddenWindowApps.includes(app.appKey));
  const pinnedWindowAppKeys = new Set(pinnedWindowApps.map((app) => app.appKey));
  const shownApps = activeApps.filter((app) => (
    !hiddenWindowApps.includes(app.appKey) && !pinnedWindowAppKeys.has(app.appKey)
  ));
  const groups = sessions.reduce<Array<{ id: string; sessions: typeof sessions }>>((result, session) => {
    const existing = result.find((group) => group.id === session.groupId);
    if (existing) existing.sessions.push(session);
    else result.push({ id: session.groupId, sessions: [session] });
    return result;
  }, []);

  const groupState = (groupSessions: typeof sessions): SessionState => {
    const priority: SessionState[] = ['attention', 'failed', 'starting', 'running', 'exited', 'stopped'];
    return priority.find((candidate) =>
      groupSessions.some((session) => (states[session.id] ?? 'stopped') === candidate)
    ) ?? 'stopped';
  };

  const openDockMenu = () => runUiAction(
    'Could not open the AI Dock launcher',
    api.toggleLauncher
  );

  const openRecentAppsMenu = () => runUiAction(
    'Could not open recent apps',
    () => showRecentAppsMenu({
      activeApps,
      recentWindowsApps: settings?.recentWindowsApps ?? [],
      hiddenWindowApps
    })
  );

  const clearWindowAppDrag = () => {
    windowAppPointerDragRef.current = null;
    windowAppDropTargetRef.current = null;
    setDraggedWindowHandle(null);
    setWindowAppDropTarget(null);
  };

  const updateWindowAppDropTarget = () => {
    const drag = windowAppPointerDragRef.current;
    const nextTarget = drag ? windowAppDropTargetFromDelta(drag) : null;
    windowAppDropTargetRef.current = nextTarget;
    setWindowAppDropTarget(nextTarget);
  };

  const reorderWindowApps = (sourceHandle: string, target: HorizontalDropTarget) => {
    setActiveApps((current) => {
      const reordered = reorderById(
        current,
        sourceHandle,
        target.id,
        target.placeAfter,
        (app) => app.handle
      );
      activeAppOrderRef.current = reordered.map((app) => app.handle);
      return reordered;
    });
  };

  const clearPinnedAppDrag = () => {
    pinnedAppPointerDragRef.current = null;
    pinnedAppDropTargetRef.current = null;
    setDraggedPinnedAppKey(null);
    setPinnedAppDropTarget(null);
  };

  const updatePinnedAppDropTarget = (clientX: number, screenX: number) => {
    const nextTarget = horizontalDropTarget(
      '[data-pinned-app-key]',
      'data-pinned-app-key',
      pinnedAppPointerDragRef.current,
      clientX,
      screenX
    );
    pinnedAppDropTargetRef.current = nextTarget;
    setPinnedAppDropTarget(nextTarget);
  };

  const refreshActiveAppsSoon = () => {
    window.setTimeout(() => {
      void api.listActiveWindows()
        .then((apps) => {
          const orderedApps = reconcileActiveAppOrder(apps, activeAppOrderRef.current);
          activeAppOrderRef.current = orderedApps.map((app) => app.handle);
          setActiveApps(orderedApps);
        })
        .catch(() => undefined);
    }, 120);
  };

  const activateWindowApp = (activeWindow: ActiveWindowApp) => runUiAction(
    `Could not activate ${activeWindow.appName}`,
    async () => {
      await api.activateExternalWindow(activeWindow.handle, activeWindow.isFocused);
      refreshActiveAppsSoon();
    }
  );

  const clearNotebookDrag = () => {
    notebookPointerDragRef.current = null;
    notebookDropTargetRef.current = null;
    setDraggedNotebookId(null);
    setNotebookDropTarget(null);
  };

  const updateNotebookDropTarget = (clientX: number, screenX: number) => {
    const nextTarget = horizontalDropTarget(
      '[data-notebook-id]',
      'data-notebook-id',
      notebookPointerDragRef.current,
      clientX,
      screenX
    );
    notebookDropTargetRef.current = nextTarget;
    setNotebookDropTarget(nextTarget);
  };

  return (
    <main
      className={`dock-shell ${isTestBuild ? 'is-test-build' : ''}`}
      onContextMenu={(event) => {
        if ((event.target as Element).closest('button')) return;
        event.preventDefault();
        void openRecentAppsMenu();
      }}
    >
      <section className="dock-primary">
        <button
          className="dock-brand"
          title="AI Dock Start"
          aria-haspopup="menu"
          onClick={() => void openDockMenu()}
          onContextMenu={(event) => {
            event.preventDefault();
            void openDockMenu();
          }}
        >
          <span className="brand-mark">AI</span>
          <span className="brand-name">{isTestBuild ? 'Dock Test' : 'Dock'}</span>
        </button>

        <button
          className={`terminal-visibility-button ${visibleGroups.size > 0 ? 'is-active' : ''}`}
          aria-label={visibleGroups.size > 0
            ? 'Hide all terminal windows'
            : 'Show all terminal windows'}
          aria-pressed={visibleGroups.size > 0}
          onClick={() => runUiAction(
            visibleGroups.size > 0
              ? 'Could not hide all terminal windows'
              : 'Could not show all terminal windows',
            api.toggleAllTerminals
          )}
        >
          <span className="terminal-stack-glyph" aria-hidden="true" />
        </button>

        <div className="dock-tabs" role="tablist" aria-label="Terminal windows">
        {groups.map((group, index) => {
          const primary = group.sessions[0];
          const state = groupState(group.sessions);
          const selected = visibleGroups.has(group.id);
          return (
            <button
              key={group.id}
              role="tab"
              aria-selected={selected}
              aria-label={`${primary.name}, ${group.sessions.length} terminal tab${group.sessions.length === 1 ? '' : 's'}`}
              className={`dock-tab ${selected ? 'is-active' : ''}`}
              style={{ '--session-color': primary.color } as React.CSSProperties}
              onClick={() => void api.activateGroup(group.id)}
              onContextMenu={(event) => {
                event.preventDefault();
                runUiAction('Terminal menu action failed', () => showSessionMenu(primary, {
                  closeLabel: 'Forget window',
                  closePrompt: `Forget ${primary.name} and permanently remove all ${group.sessions.length} terminal tab${group.sessions.length === 1 ? '' : 's'} in it?`,
                  archiveLabel: 'Close and save to history',
                  onArchive: () => api.closeTerminal(group.id, true),
                  onClose: async () => {
                    await api.closeTerminal(group.id, false);
                  }
                }));
              }}
            >
              <span className={`state-dot state-${state}`} />
              <span className="tab-index">{index + 1}</span>
              <span className="tab-label">{primary.name}</span>
              {group.sessions.length > 1 && <span className="tab-count">{group.sessions.length}</span>}
            </button>
          );
        })}
        <button
          className="icon-button add-button"
          title="New PowerShell window"
          onClick={() => void (async () => {
            const session = await api.addPowerShellTerminal();
            await api.activateGroup(session.groupId);
          })()}
        >
          +
        </button>
        </div>
      </section>

      <div className="dock-window-apps" role="list" aria-label="Pinned and active Windows apps">
        {pinnedWindowApps.map((pinnedApp) => {
          const activeWindow = activeApps.find((candidate) => (
            candidate.appKey === pinnedApp.appKey && candidate.isFocused
          )) ?? activeApps.find((candidate) => candidate.appKey === pinnedApp.appKey);
          const iconDataUrl = activeWindow?.iconDataUrl ?? pinnedApp.iconDataUrl;
          return (
            <button
              key={`pinned-${pinnedApp.appKey}`}
              role="listitem"
              className={`window-app-button is-pinned ${activeWindow?.isFocused ? 'is-active' : ''} ${activeWindow?.isMinimized ? 'is-minimized' : ''} ${draggedPinnedAppKey === pinnedApp.appKey ? 'is-dragging' : ''} ${pinnedAppDropTarget?.id === pinnedApp.appKey ? (pinnedAppDropTarget.placeAfter ? 'drop-after' : 'drop-before') : ''}`}
              data-pinned-app-key={pinnedApp.appKey}
              aria-label={`${pinnedApp.appName}, pinned${activeWindow ? ', running' : ''}`}
              onClick={() => {
                if (suppressPinnedAppClickRef.current) {
                  suppressPinnedAppClickRef.current = false;
                  return;
                }
                if (activeWindow) {
                  activateWindowApp(activeWindow);
                } else {
                  runUiAction(
                    `Could not launch ${pinnedApp.appName}`,
                    () => api.launchRecentWindowsApp(pinnedApp.appKey)
                  );
                }
              }}
              onPointerDown={(event) => {
                if (event.pointerType === 'mouse' && event.button !== 0) return;
                pinnedAppPointerDragRef.current = {
                  id: pinnedApp.appKey,
                  pointerId: event.pointerId,
                  startX: event.clientX,
                  startY: event.clientY,
                  startScreenX: event.screenX,
                  lastX: event.clientX,
                  dragging: false
                };
                event.currentTarget.setPointerCapture(event.pointerId);
              }}
              onPointerMove={(event) => {
                const drag = pinnedAppPointerDragRef.current;
                if (!drag || drag.pointerId !== event.pointerId) return;
                if (!drag.dragging) {
                  const distance = Math.hypot(
                    event.clientX - drag.startX,
                    event.clientY - drag.startY
                  );
                  if (distance < 5) return;
                  drag.dragging = true;
                  setDraggedPinnedAppKey(drag.id);
                }
                event.preventDefault();
                updatePinnedAppDropTarget(event.clientX, event.screenX);
              }}
              onPointerUp={(event) => {
                const drag = pinnedAppPointerDragRef.current;
                if (!drag || drag.pointerId !== event.pointerId) return;
                const target = pinnedAppDropTargetRef.current;
                if (drag.dragging) {
                  suppressPinnedAppClickRef.current = true;
                  window.setTimeout(() => {
                    suppressPinnedAppClickRef.current = false;
                  }, 0);
                  if (target && target.id !== drag.id) {
                    void api.reorderPinnedWindowApp(drag.id, target.id, target.placeAfter);
                  }
                }
                if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                  event.currentTarget.releasePointerCapture(event.pointerId);
                }
                clearPinnedAppDrag();
              }}
              onPointerCancel={(event) => {
                if (pinnedAppPointerDragRef.current?.pointerId === event.pointerId) {
                  clearPinnedAppDrag();
                }
              }}
              onContextMenu={(event) => {
                event.preventDefault();
                runUiAction(
                  'Pinned app menu action failed',
                  () => activeWindow
                    ? showWindowAppMenu(activeWindow, true)
                    : showRecentWindowAppMenu(pinnedApp, true)
                );
              }}
            >
              {iconDataUrl
                ? <img className="window-app-icon" src={iconDataUrl} alt="" draggable={false} />
                : <span className="window-app-mark">{pinnedApp.appName.slice(0, 1).toUpperCase()}</span>}
            </button>
          );
        })}
        {shownApps.map((activeWindow) => (
          <button
            key={activeWindow.handle}
            role="listitem"
            className={`window-app-button ${activeWindow.isFocused ? 'is-active' : ''} ${activeWindow.isMinimized ? 'is-minimized' : ''} ${draggedWindowHandle === activeWindow.handle ? 'is-dragging' : ''} ${windowAppDropTarget?.id === activeWindow.handle ? (windowAppDropTarget.placeAfter ? 'drop-after' : 'drop-before') : ''}`}
            data-window-handle={activeWindow.handle}
            aria-label={`${activeWindow.appName}: ${activeWindow.title}`}
            onClick={() => {
              if (suppressWindowAppClickRef.current) {
                suppressWindowAppClickRef.current = false;
                return;
              }
              activateWindowApp(activeWindow);
            }}
            onPointerDown={(event) => {
              if (event.pointerType === 'mouse' && event.button !== 0) return;
              windowAppPointerDragRef.current = {
                id: activeWindow.handle,
                pointerId: event.pointerId,
                startX: event.clientX,
                startY: event.clientY,
                startScreenX: event.screenX,
                lastX: event.clientX,
                dragging: false
              };
              event.currentTarget.setPointerCapture(event.pointerId);
            }}
            onPointerMove={(event) => {
              const drag = windowAppPointerDragRef.current;
              if (!drag || drag.pointerId !== event.pointerId) return;
              if (!drag.dragging) {
                const distance = Math.hypot(
                  event.clientX - drag.startX,
                  event.clientY - drag.startY
                );
                if (distance < 5) return;
                drag.dragging = true;
                setDraggedWindowHandle(drag.id);
              }
              drag.lastX = event.clientX;
              event.preventDefault();
              updateWindowAppDropTarget();
            }}
            onPointerUp={(event) => {
              const drag = windowAppPointerDragRef.current;
              if (!drag || drag.pointerId !== event.pointerId) return;
              const target = windowAppDropTargetRef.current;
              if (drag.dragging) {
                suppressWindowAppClickRef.current = true;
                window.setTimeout(() => {
                  suppressWindowAppClickRef.current = false;
                }, 0);
                if (target) reorderWindowApps(drag.id, target);
              }
              if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                event.currentTarget.releasePointerCapture(event.pointerId);
              }
              clearWindowAppDrag();
            }}
            onPointerCancel={(event) => {
              if (windowAppPointerDragRef.current?.pointerId === event.pointerId) {
                clearWindowAppDrag();
              }
            }}
            onContextMenu={(event) => {
              event.preventDefault();
              runUiAction('App menu action failed', () => showWindowAppMenu(activeWindow, false));
            }}
          >
            {activeWindow.iconDataUrl
              ? <img className="window-app-icon" src={activeWindow.iconDataUrl} alt="" draggable={false} />
              : <span className="window-app-mark">{activeWindow.appName.slice(0, 1).toUpperCase()}</span>}
          </button>
        ))}
        <button
          className="app-launcher-button"
          title="Launch a Windows app"
          aria-label="Launch a Windows app"
          onClick={() => void api.openWindowsStartSearch()}
        >
          +
        </button>
      </div>

      <div className="dock-utility-actions">
        <div className="zenplan-pins" aria-label="Pinned web apps">
          {(settings?.zenplanNotebooks ?? []).map((notebook) => {
            const visible = visibleZenPlanNotebooks.has(notebook.id);
            return (
              <button
                key={notebook.id}
                className={`zenplan-dock-button ${visible ? 'is-active' : ''} ${draggedNotebookId === notebook.id ? 'is-dragging' : ''} ${notebookDropTarget?.id === notebook.id ? (notebookDropTarget.placeAfter ? 'drop-after' : 'drop-before') : ''}`}
                aria-pressed={visible}
                aria-label={notebook.name}
                data-notebook-id={notebook.id}
                onClick={() => {
                  if (suppressNotebookClickRef.current) {
                    suppressNotebookClickRef.current = false;
                    return;
                  }
                  void api.toggleZenPlan(notebook.id);
                }}
                onPointerDown={(event) => {
                  if (event.pointerType === 'mouse' && event.button !== 0) return;
                  notebookPointerDragRef.current = {
                    id: notebook.id,
                    pointerId: event.pointerId,
                    startX: event.clientX,
                    startY: event.clientY,
                    startScreenX: event.screenX,
                    lastX: event.clientX,
                    dragging: false
                  };
                  event.currentTarget.setPointerCapture(event.pointerId);
                }}
                onPointerMove={(event) => {
                  const drag = notebookPointerDragRef.current;
                  if (!drag || drag.pointerId !== event.pointerId) return;
                  if (!drag.dragging) {
                    const distance = Math.hypot(
                      event.clientX - drag.startX,
                      event.clientY - drag.startY
                    );
                    if (distance < 5) return;
                    drag.dragging = true;
                    setDraggedNotebookId(drag.id);
                  }
                  event.preventDefault();
                  updateNotebookDropTarget(event.clientX, event.screenX);
                }}
                onPointerUp={(event) => {
                  const drag = notebookPointerDragRef.current;
                  if (!drag || drag.pointerId !== event.pointerId) return;
                  const target = notebookDropTargetRef.current;
                  if (drag.dragging) {
                    suppressNotebookClickRef.current = true;
                    window.setTimeout(() => {
                      suppressNotebookClickRef.current = false;
                    }, 0);
                    if (target && target.id !== drag.id) {
                      void api.reorderZenPlanNotebook(
                        drag.id,
                        target.id,
                        target.placeAfter
                      );
                    }
                  }
                  if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                    event.currentTarget.releasePointerCapture(event.pointerId);
                  }
                  clearNotebookDrag();
                }}
                onPointerCancel={(event) => {
                  if (notebookPointerDragRef.current?.pointerId === event.pointerId) {
                    clearNotebookDrag();
                  }
                }}
                onContextMenu={(event) => {
                  event.preventDefault();
                  runUiAction('Web app menu action failed', () => showZenPlanMenu(notebook));
                }}
              >
                <span className="zenplan-mark">
                  <span>{notebook.name.trim().slice(0, 1).toUpperCase() || 'W'}</span>
                  {notebook.iconDataUrl && (
                    <img
                      className="web-app-favicon"
                      src={notebook.iconDataUrl}
                      alt=""
                      draggable={false}
                      onError={(event) => {
                        event.currentTarget.style.display = 'none';
                      }}
                    />
                  )}
                </span>
                <span className="zenplan-label">{notebook.name}</span>
              </button>
            );
          })}
        </div>
        <button
          className={`taskbar-toggle-button ${windowsTaskbarVisible ? '' : 'is-hidden'}`}
          aria-pressed={!windowsTaskbarVisible}
          title={`${windowsTaskbarVisible ? 'Collapse' : 'Expand'} Windows taskbar`}
          aria-label={`${windowsTaskbarVisible ? 'Collapse' : 'Expand'} Windows taskbar`}
          onClick={() => void api.toggleWindowsTaskbar()
            .then(setWindowsTaskbarVisible)
            .catch(() => undefined)}
        >
          <span
            className={`taskbar-chevron ${windowsTaskbarVisible ? 'is-down' : 'is-up'}`}
            aria-hidden="true"
          />
        </button>
        <div className="dock-clock" title={now.toLocaleString()} aria-label={now.toLocaleString()}>
          <span className="dock-date">{dockDateFormatter.format(now)}</span>
          <span className="dock-time">{dockTimeFormatter.format(now)}</span>
        </div>
      </div>
    </main>
  );
}
