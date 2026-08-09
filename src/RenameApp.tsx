import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from './api';
import type { SessionDefinition } from './types';

export function RenameApp() {
  const inputRef = useRef<HTMLInputElement>(null);
  const [session, setSession] = useState<SessionDefinition | null>(null);
  const [name, setName] = useState('');
  const [error, setError] = useState<string | null>(null);

  const loadContext = useCallback(async () => {
    const next = await api.getRenameContext();
    if (!next) return;
    setSession(next);
    setName(next.name);
    setError(null);
    window.setTimeout(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    }, 0);
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      unlisten = await listen('rename-target-changed', () => void loadContext());
      await loadContext();
    })();
    return () => unlisten?.();
  }, [loadContext]);

  const submit = async () => {
    const trimmed = name.trim();
    if (!session || !trimmed) {
      setError('Enter a tab name.');
      return;
    }
    try {
      await api.renameSession(session.id, trimmed);
      await api.hideRename();
    } catch (caught) {
      setError(String(caught));
    }
  };

  return (
    <main
      className="rename-shell"
      onKeyDown={(event) => {
        if (event.key === 'Escape') void api.hideRename();
      }}
    >
      <header
        className="rename-header window-drag-handle"
        onMouseDown={(event) => {
          if (event.button === 0 && !(event.target as HTMLElement).closest('button, input')) {
            void getCurrentWindow().startDragging();
          }
        }}
      >
        <span className="terminal-color" style={{ backgroundColor: session?.color ?? '#2F78C4' }} />
        <strong>Rename terminal tab</strong>
        <button title="Cancel" aria-label="Cancel rename" onClick={() => void api.hideRename()}>×</button>
      </header>
      <div className="rename-row">
        <input
          ref={inputRef}
          value={name}
          maxLength={80}
          aria-label="Terminal tab name"
          onChange={(event) => setName(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') void submit();
          }}
        />
        <button onClick={() => void api.hideRename()}>Cancel</button>
        <button className="primary-button" onClick={() => void submit()}>Rename</button>
      </div>
      {error && <p className="rename-error">{error}</p>}
    </main>
  );
}
