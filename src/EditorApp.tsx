import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from './api';
import { confirmWithMenu } from './confirmationMenu';
import type { EditorContext, SessionDefinition } from './types';

const palette = ['#2F78C4', '#6C5CE7', '#B14AED', '#D64F70', '#D87524', '#C39B21', '#2A9D68', '#168B91'];

function createSession(defaultWorkingDirectory: string, targetGroupId: string | null): SessionDefinition {
  const id = crypto.randomUUID();
  return {
    id,
    groupId: targetGroupId ?? id,
    name: 'PowerShell',
    color: palette[0],
    commandLine: 'pwsh.exe -NoLogo',
    workingDirectory: defaultWorkingDirectory,
    windowX: null,
    windowY: null,
    windowWidth: null,
    windowHeight: null
  };
}

export function EditorApp() {
  const [session, setSession] = useState<SessionDefinition | null>(null);
  const [isNew, setIsNew] = useState(true);
  const [isNewTab, setIsNewTab] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadContext = useCallback(async () => {
    const context: EditorContext = await api.getEditorContext();
    setSession(
      context.session
        ? { ...context.session }
        : createSession(context.defaultWorkingDirectory, context.targetGroupId)
    );
    setIsNew(!context.session);
    setIsNewTab(!context.session && Boolean(context.targetGroupId));
    setError(null);
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      await loadContext();
      unlisten = await listen('editor-target-changed', () => void loadContext());
    })();
    return () => unlisten?.();
  }, [loadContext]);

  if (!session) {
    return <main className="editor-shell loading">Loading…</main>;
  }

  const applyPreset = (preset: 'powershell' | 'codex' | 'claude' | 'wsl') => {
    const presets = {
      powershell: { name: 'PowerShell', commandLine: 'pwsh.exe -NoLogo' },
      codex: { name: 'Codex', commandLine: 'pwsh.exe -NoLogo -NoExit -Command codex' },
      claude: { name: 'Claude', commandLine: 'pwsh.exe -NoLogo -NoExit -Command claude' },
      wsl: { name: 'WSL', commandLine: 'wsl.exe' }
    };
    setSession((current) => ({ ...current!, ...presets[preset] }));
  };

  const save = async () => {
    if (!session.name.trim()) {
      setError('Give this session a name.');
      return;
    }
    if (!session.commandLine.trim()) {
      setError('Enter a command to run.');
      return;
    }
    if (!session.workingDirectory.trim()) {
      setError('Choose a working directory.');
      return;
    }

    try {
      await api.saveSession({
        ...session,
        name: session.name.trim(),
        commandLine: session.commandLine.trim(),
        workingDirectory: session.workingDirectory.trim()
      });
      await api.hideEditor();
    } catch (caught) {
      setError(String(caught));
    }
  };

  return (
    <main className="editor-shell">
      <header
        className="editor-header window-drag-handle"
        onMouseDown={(event) => {
          if (event.button === 0 && !(event.target as HTMLElement).closest('button, input')) {
            void getCurrentWindow().startDragging();
          }
        }}
      >
        <div>
          <p className="eyebrow">{isNewTab ? 'NEW WINDOW TAB' : isNew ? 'NEW WINDOW' : 'TAB SETTINGS'}</p>
          <h1>{isNewTab ? 'Add a terminal tab' : isNew ? 'Pin a terminal window' : session.name}</h1>
        </div>
        <button className="icon-button" title="Cancel" onClick={() => void api.hideEditor()}>
          ×
        </button>
      </header>

      <section className="preset-row" aria-label="Session presets">
        <button onClick={() => applyPreset('powershell')}>PowerShell</button>
        <button onClick={() => applyPreset('codex')}>Codex</button>
        <button onClick={() => applyPreset('claude')}>Claude</button>
        <button onClick={() => applyPreset('wsl')}>WSL</button>
      </section>

      <label className="field">
        <span>Name</span>
        <input value={session.name} onChange={(event) => setSession({ ...session, name: event.target.value })} />
      </label>

      <fieldset className="color-field">
        <legend>Tab color</legend>
        <div className="color-palette">
          {palette.map((color) => (
            <button
              key={color}
              className={session.color.toUpperCase() === color ? 'selected' : ''}
              style={{ backgroundColor: color }}
              title={color}
              aria-label={`Use ${color}`}
              onClick={() => setSession({ ...session, color })}
            />
          ))}
          <input
            type="color"
            title="Custom color"
            value={session.color}
            onChange={(event) => setSession({ ...session, color: event.target.value.toUpperCase() })}
          />
        </div>
      </fieldset>

      <label className="field">
        <span>Command</span>
        <input
          spellCheck={false}
          value={session.commandLine}
          onChange={(event) => setSession({ ...session, commandLine: event.target.value })}
        />
      </label>

      <label className="field">
        <span>Working directory</span>
        <div className="browse-row">
          <input
            spellCheck={false}
            value={session.workingDirectory}
            onChange={(event) => setSession({ ...session, workingDirectory: event.target.value })}
          />
          <button
            onClick={async () => {
              const selected = await api.pickFolder(session.workingDirectory);
              if (selected) setSession({ ...session, workingDirectory: selected });
            }}
          >
            Browse…
          </button>
        </div>
      </label>

      {error && <p className="form-error">{error}</p>}

      <footer className="editor-footer">
        {!isNew && (
          <div className="secondary-actions">
            <button title="Move left" onClick={() => void api.moveSession(session.id, -1)}>←</button>
            <button title="Move right" onClick={() => void api.moveSession(session.id, 1)}>→</button>
            <button onClick={() => void api.restartSession(session.id)}>Restart</button>
            <button
              className="danger-button"
              onClick={async () => {
                if (await confirmWithMenu(
                  `Remove ${session.name} and stop its process?`,
                  'Remove terminal'
                )) {
                  await api.deleteSession(session.id);
                  await api.hideEditor();
                }
              }}
            >
              Remove
            </button>
          </div>
        )}
        <div className="primary-actions">
          <button onClick={() => void api.hideEditor()}>Cancel</button>
          <button className="primary-button" onClick={() => void save()}>
            {isNewTab ? 'Add tab' : isNew ? 'Pin window' : 'Save'}
          </button>
        </div>
      </footer>
      <div
        className="editor-resize-grip"
        title="Drag to resize"
        onMouseDown={(event) => {
          if (event.button === 0) {
            event.preventDefault();
            void getCurrentWindow().startResizeDragging('SouthEast');
          }
        }}
      />
    </main>
  );
}
