import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { api } from './api';
import { isTestBuild } from './buildFlavor';
import { showSessionMenu } from './sessionMenu';
import { runUiAction } from './errorHandling';
import type {
  AppSettings,
  SessionDefinition,
  SessionState,
  SessionStatePayload,
  TerminalOutputPayload
} from './types';

interface TerminalEntry {
  terminal: Terminal;
  fit: FitAddon;
  inputQueue: Promise<unknown>;
  lastPtyCols: number;
  lastPtyRows: number;
}

function resizePtyIfNeeded(entry: TerminalEntry, sessionId: string) {
  if (entry.lastPtyCols === entry.terminal.cols && entry.lastPtyRows === entry.terminal.rows) return;
  entry.lastPtyCols = entry.terminal.cols;
  entry.lastPtyRows = entry.terminal.rows;
  void api.resizeSession(sessionId, entry.terminal.cols, entry.terminal.rows);
}

interface TerminalPaneProps {
  session: SessionDefinition;
  active: boolean;
  onStatus: (sessionId: string, status: SessionState) => void;
}

function decodeBase64(value: string): Uint8Array {
  const decoded = atob(value);
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
}

function decodeBase64Text(value: string): string {
  return new TextDecoder().decode(decodeBase64(value));
}

async function copyTextToClipboard(text: string) {
  try {
    await api.writeClipboardText(text);
    return;
  } catch {
    // Native clipboard access is Windows-only for now. Keep the browser path as
    // a fallback so a future macOS build still has functional copy support.
  }
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const textarea = document.createElement('textarea');
    textarea.value = text;
    textarea.style.position = 'fixed';
    textarea.style.opacity = '0';
    document.body.appendChild(textarea);
    textarea.select();
    document.execCommand('copy');
    textarea.remove();
  }
}

async function readTextFromClipboard() {
  try {
    return await api.readClipboardText();
  } catch {
    try {
      return await navigator.clipboard.readText();
    } catch {
      return null;
    }
  }
}

function terminalTheme(color: string) {
  return {
    background: '#090D15',
    foreground: '#E8EDF5',
    cursor: color,
    selectionBackground: '#36577F99',
    black: '#151B27', red: '#FF6B7A', green: '#70D68F', yellow: '#F1C76F',
    blue: '#70A9FF', magenta: '#CC8CF0', cyan: '#67D8D2', white: '#DCE3EE',
    brightBlack: '#66738A', brightRed: '#FF8491', brightGreen: '#8AE5A4',
    brightYellow: '#FFE18C', brightBlue: '#8DBDFF', brightMagenta: '#DEA8F7',
    brightCyan: '#84ECE6', brightWhite: '#FFFFFF'
  };
}

function fitPreservingViewport(entry: TerminalEntry) {
  const buffer = entry.terminal.buffer.active;
  const linesFromBottom = Math.max(0, buffer.baseY - buffer.viewportY);
  entry.fit.fit();

  const restoreViewport = () => {
    const resizedBuffer = entry.terminal.buffer.active;
    if (linesFromBottom === 0) {
      entry.terminal.scrollToBottom();
    } else {
      entry.terminal.scrollToLine(Math.max(0, resizedBuffer.baseY - linesFromBottom));
    }
  };

  // xterm reflows wrapped lines during fit. Restore once synchronously and once after
  // layout settles so a window resize cannot leave the viewport far back in scrollback.
  restoreViewport();
  return restoreViewport;
}

function TerminalPane({ session, active, onStatus }: TerminalPaneProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const entryRef = useRef<TerminalEntry | null>(null);
  const onStatusRef = useRef(onStatus);

  useEffect(() => {
    onStatusRef.current = onStatus;
  }, [onStatus]);

  useEffect(() => {
    const terminal = entryRef.current?.terminal;
    if (terminal) terminal.options.theme = terminalTheme(session.color);
  }, [session.color]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    let disposed = false;
    let frame = 0;
    let restoreFrame = 0;
    let resizeFrame = 0;
    let outputTimer: number | undefined;
    const unlisteners: Array<() => void> = [];
    const terminal = new Terminal({
      cursorBlink: true,
      cursorStyle: 'bar',
      fontFamily: 'Cascadia Mono, Cascadia Code, Consolas, monospace',
      fontSize: 14,
      lineHeight: 1.12,
      scrollback: 12000,
      theme: terminalTheme(session.color)
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host);

    const entry: TerminalEntry = {
      terminal,
      fit,
      inputQueue: Promise.resolve(),
      lastPtyCols: 0,
      lastPtyRows: 0
    };
    const pendingOutput: Uint8Array[] = [];
    let pendingOutputBytes = 0;
    let outputWriteInProgress = false;
    // xterm parses writes synchronously on the WebView thread. Small slices keep
    // tab clicks, Ctrl+H, painting, and dock heartbeats responsive while several
    // tabs replay their persisted scrollback at the same time.
    const outputBatchBytes = 16 * 1024;
    const flushOutput = () => {
      outputTimer = undefined;
      if (disposed || outputWriteInProgress || pendingOutputBytes === 0) return;

      const batchBytes = Math.min(pendingOutputBytes, outputBatchBytes);
      const output = new Uint8Array(batchBytes);
      let offset = 0;
      while (offset < batchBytes) {
        const chunk = pendingOutput[0];
        const take = Math.min(chunk.length, batchBytes - offset);
        output.set(chunk.subarray(0, take), offset);
        offset += take;
        if (take === chunk.length) {
          pendingOutput.shift();
        } else {
          pendingOutput[0] = chunk.subarray(take);
        }
      }
      pendingOutputBytes -= batchBytes;
      outputWriteInProgress = true;
      entry.terminal.write(output, () => {
        outputWriteInProgress = false;
        if (!disposed && pendingOutputBytes > 0 && outputTimer === undefined) {
          // Let the WebView paint and service input between large scrollback batches.
          outputTimer = window.setTimeout(flushOutput, 0);
        }
      });
    };
    const queueOutput = (encoded: string) => {
      const chunk = decodeBase64(encoded);
      pendingOutput.push(chunk);
      pendingOutputBytes += chunk.length;
      if (pendingOutputBytes >= outputBatchBytes && !outputWriteInProgress) {
        window.clearTimeout(outputTimer);
        outputTimer = window.setTimeout(flushOutput, 0);
      } else if (outputTimer === undefined) {
        outputTimer = window.setTimeout(flushOutput, 4);
      }
    };
    let wheelRemainder = 0;
    const queueInput = (data: string) => {
      entry.inputQueue = entry.inputQueue
        .then(() => api.writeSession(session.id, data))
        .catch(() => undefined);
    };
    const handleScrollbackWheel = (event: WheelEvent) => {
      const hasTerminalScrollback = entry.terminal.buffer.active.baseY > 0;
      if (!hasTerminalScrollback) return;

      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();

      let lines: number;
      if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) {
        lines = Math.sign(event.deltaY) * Math.max(1, entry.terminal.rows - 1);
      } else if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) {
        lines = Math.trunc(event.deltaY);
      } else {
        wheelRemainder += event.deltaY;
        lines = Math.trunc(wheelRemainder / 24);
        wheelRemainder -= lines * 24;
      }

      if (lines !== 0) entry.terminal.scrollLines(lines);
    };
    host.addEventListener('wheel', handleScrollbackWheel, { capture: true, passive: false });
    terminal.attachCustomKeyEventHandler((event) => {
      if (event.type !== 'keydown') return true;
      const copyShortcut = (event.ctrlKey || event.metaKey)
        && !event.altKey
        && event.key.toLowerCase() === 'c';
      if (copyShortcut && terminal.hasSelection()) {
        const selection = terminal.getSelection();
        if (selection) void copyTextToClipboard(selection);
        return false;
      }
      const pasteShortcut = ((event.ctrlKey || event.metaKey)
        && !event.altKey
        && event.key.toLowerCase() === 'v')
        || (event.shiftKey && !event.ctrlKey && !event.metaKey && event.key === 'Insert');
      if (pasteShortcut) {
        // Returning false stops xterm's key processing, but it does not cancel the
        // browser's default clipboard action. Without preventDefault, the native
        // clipboard read below and xterm's subsequent paste event both reach the
        // PTY, causing every paste to be entered twice.
        event.preventDefault();
        event.stopPropagation();
        void readTextFromClipboard().then((text) => {
          if (text) terminal.paste(text);
        });
        return false;
      }
      if (!event.shiftKey) return true;
      if (event.key === 'PageUp') {
        if (entry.terminal.buffer.active.baseY > 0) terminal.scrollPages(-1);
        else return true;
        return false;
      }
      if (event.key === 'PageDown') {
        if (entry.terminal.buffer.active.baseY > 0) terminal.scrollPages(1);
        else return true;
        return false;
      }
      return true;
    });
    const directoryDisposable = terminal.parser.registerOscHandler(6973, (data) => {
      try {
        const workingDirectory = decodeBase64Text(data).trim();
        if (workingDirectory) {
          void api.updateSessionWorkingDirectory(session.id, workingDirectory);
        }
      } catch {
        // Ignore malformed shell-integration messages and leave the saved folder unchanged.
      }
      return true;
    });
    const inputDisposable = terminal.onData(queueInput);
    entryRef.current = entry;

    const fitAndResize = () => {
      if (disposed) return;
      const restoreViewport = fitPreservingViewport(entry);
      cancelAnimationFrame(restoreFrame);
      restoreFrame = requestAnimationFrame(restoreViewport);
      resizePtyIfNeeded(entry, session.id);
    };
    const scheduleFitAndResize = () => {
      cancelAnimationFrame(resizeFrame);
      resizeFrame = requestAnimationFrame(fitAndResize);
    };
    const observer = new ResizeObserver(scheduleFitAndResize);
    observer.observe(host);

    void (async () => {
      const listeners = await Promise.all([
        listen<TerminalOutputPayload>('terminal-output', (event) => {
          if (event.payload.sessionId === session.id) {
            queueOutput(event.payload.data);
          }
        }),
        listen<SessionStatePayload>('session-state', (event) => {
          if (event.payload.sessionId === session.id) {
            onStatusRef.current(session.id, event.payload.state);
          }
        })
      ]);
      if (disposed) {
        listeners.forEach((unlisten) => unlisten());
        return;
      }
      unlisteners.push(...listeners);
      onStatusRef.current(session.id, 'starting');
      frame = requestAnimationFrame(() => {
        fitAndResize();
        if (active) entry.terminal.focus();
        void api.startSession(session.id, entry.terminal.cols, entry.terminal.rows);
      });
    })();

    return () => {
      disposed = true;
      cancelAnimationFrame(frame);
      cancelAnimationFrame(restoreFrame);
      cancelAnimationFrame(resizeFrame);
      window.clearTimeout(outputTimer);
      observer.disconnect();
      host.removeEventListener('wheel', handleScrollbackWheel, true);
      unlisteners.forEach((unlisten) => unlisten());
      inputDisposable.dispose();
      directoryDisposable.dispose();
      terminal.dispose();
      entryRef.current = null;
    };
  }, [session.id]);

  useEffect(() => {
    if (!active) return;
    const frame = requestAnimationFrame(() => {
      const entry = entryRef.current;
      if (!entry) return;
      fitPreservingViewport(entry);
      entry.terminal.focus();
      resizePtyIfNeeded(entry, session.id);
    });
    return () => cancelAnimationFrame(frame);
  }, [active, session.id]);

  return (
    <div className={`terminal-pane ${active ? 'is-active' : ''}`} aria-hidden={!active}>
      <div className="terminal-surface" ref={hostRef} />
    </div>
  );
}

export function TerminalApp() {
  const terminalWindowRef = useRef(getCurrentWindow());
  const terminalWindow = terminalWindowRef.current;
  const groupId = terminalWindow.label.slice('terminal-'.length);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [statuses, setStatuses] = useState<Record<string, SessionState>>({});
  const [closeChoiceOpen, setCloseChoiceOpen] = useState(false);
  const [closingWindow, setClosingWindow] = useState(false);
  const [closeError, setCloseError] = useState<string | null>(null);

  const applySettings = useCallback((next: AppSettings) => {
    const groupSessions = next.sessions.filter((session) => session.groupId === groupId);
    setSettings(next);
    setActiveSessionId((current) =>
      current && groupSessions.some((session) => session.id === current)
        ? current
        : groupSessions[0]?.id ?? null
    );
  }, [groupId]);

  useEffect(() => {
    let disposed = false;
    let boundsTimer: number | undefined;
    const unlisteners: Array<() => void> = [];

    const persistBounds = () => {
      window.clearTimeout(boundsTimer);
      boundsTimer = window.setTimeout(() => {
        void (async () => {
          const [position, size] = await Promise.all([
            terminalWindow.outerPosition(),
            terminalWindow.outerSize()
          ]);
          await api.saveTerminalBounds(groupId, position.x, position.y, size.width, size.height);
        })();
      }, 180);
    };

    void (async () => {
      unlisteners.push(
        await terminalWindow.onMoved(persistBounds),
        await terminalWindow.onResized(persistBounds),
        await listen<AppSettings>('settings-changed', (event) => applySettings(event.payload))
      );
      const loaded = await api.getSettings();
      if (!disposed) applySettings(loaded);
    })();

    return () => {
      disposed = true;
      window.clearTimeout(boundsTimer);
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [applySettings, groupId, terminalWindow]);

  const groupSessions = useMemo(
    () => settings?.sessions.filter((session) => session.groupId === groupId) ?? [],
    [groupId, settings]
  );

  useEffect(() => {
    const switchTerminalTab = (event: KeyboardEvent) => {
      if (!event.ctrlKey || event.altKey || event.metaKey || event.shiftKey) return;
      if (!/^[1-9]$/.test(event.key)) return;
      const session = groupSessions[Number(event.key) - 1];
      if (!session) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      setActiveSessionId(session.id);
    };
    window.addEventListener('keydown', switchTerminalTab, true);
    return () => window.removeEventListener('keydown', switchTerminalTab, true);
  }, [groupSessions]);

  const activeSession = groupSessions.find((session) => session.id === activeSessionId) ?? groupSessions[0];
  const updateStatus = useCallback((sessionId: string, status: SessionState) => {
    setStatuses((current) => ({ ...current, [sessionId]: status }));
  }, []);
  const closeWindow = async (remember: boolean) => {
    setClosingWindow(true);
    setCloseError(null);
    try {
      await api.closeTerminal(groupId, remember);
    } catch (error) {
      setCloseError(String(error));
      setClosingWindow(false);
    }
  };

  return (
    <main className={`terminal-shell ${isTestBuild ? 'is-test-build' : ''}`}>
      <div
        className="terminal-resize-edge"
        title="Drag to resize vertically"
        onMouseDown={(event) => {
          if (event.button === 0) {
            event.preventDefault();
            void terminalWindow.startResizeDragging('North');
          }
        }}
      />
      <header
        className="terminal-header window-drag-handle"
        onMouseDown={(event) => {
          if (event.button === 0 && !(event.target as HTMLElement).closest('button')) {
            void terminalWindow.startDragging();
          }
        }}
      >
        <div className="terminal-tabs" role="tablist" aria-label="Terminals in this window">
          {groupSessions.map((session, index) => {
            const status = statuses[session.id] ?? 'starting';
            return (
              <button
                key={session.id}
                role="tab"
                aria-selected={session.id === activeSession?.id}
                className={`terminal-tab ${session.id === activeSession?.id ? 'is-active' : ''}`}
                style={{ '--session-color': session.color } as React.CSSProperties}
                title={`${session.name}\n${session.workingDirectory}\n${status}${index < 9 ? `\nCtrl+${index + 1} to switch` : ''}\nRight-click to change color or close`}
                onClick={() => setActiveSessionId(session.id)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  runUiAction('Terminal menu action failed', () => showSessionMenu(session, {
                    closeLabel: 'Close tab',
                    closePrompt: `Close ${session.name} and stop its terminal process?`,
                    onClose: () => api.deleteSession(session.id)
                  }));
                }}
              >
                <span className={`state-dot state-${status}`} />
                <span>{session.name}</span>
              </button>
            );
          })}
          <button
            className="terminal-add-tab"
            title="Add a terminal tab to this window"
            aria-label="Add a terminal tab to this window"
            onClick={() => void (async () => {
              const session = await api.addPowerShellTerminal(groupId);
              setActiveSessionId(session.id);
            })()}
          >
            +
          </button>
        </div>
        <div className="terminal-actions">
          {isTestBuild && <span className="test-build-badge">TEST</span>}
          <button
            className="terminal-minimize-button"
            title="Minimize to AI Dock"
            aria-label="Minimize terminal window to AI Dock"
            onClick={() => void api.hideTerminal(groupId)}
          >
            −
          </button>
          <button
            className="terminal-close-button"
            title="Close window"
            aria-label="Close window"
            onClick={() => {
              setCloseError(null);
              setCloseChoiceOpen(true);
            }}
          >
            ×
          </button>
        </div>
      </header>
      <div className="terminal-host">
        {groupSessions.map((session) => (
          <TerminalPane
            key={session.id}
            session={session}
            active={session.id === activeSession?.id}
            onStatus={updateStatus}
          />
        ))}
      </div>
      {closeChoiceOpen && (
        <div className="close-window-backdrop" role="presentation">
          <section
            className="close-window-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="close-window-title"
          >
            <h2 id="close-window-title">Close {activeSession?.name ?? 'this window'}?</h2>
            <p>
              This stops {groupSessions.length} terminal process{groupSessions.length === 1 ? '' : 'es'}
              {' '}and removes the window from the dock.
            </p>
            <p className="close-window-hint">
              Saving to history remembers its tabs, colors, folders, commands, size, and position.
            </p>
            {closeError && <p className="close-window-error">{closeError}</p>}
            <div className="close-window-actions">
              <button disabled={closingWindow} onClick={() => setCloseChoiceOpen(false)}>Cancel</button>
              <button
                className="close-window-forget"
                disabled={closingWindow}
                onClick={() => void closeWindow(false)}
              >
                Forget
              </button>
              <button
                className="close-window-save"
                disabled={closingWindow}
                onClick={() => void closeWindow(true)}
              >
                {closingWindow ? 'Closing…' : 'Save to history'}
              </button>
            </div>
          </section>
        </div>
      )}
      <div
        className="terminal-resize-grip"
        title="Drag to resize"
        onMouseDown={(event) => {
          if (event.button === 0) {
            event.preventDefault();
            void terminalWindow.startResizeDragging('SouthEast');
          }
        }}
      />
    </main>
  );
}
