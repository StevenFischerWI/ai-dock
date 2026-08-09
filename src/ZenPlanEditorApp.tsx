import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from './api';

export function ZenPlanEditorApp() {
  const nameRef = useRef<HTMLInputElement>(null);
  const [notebookId, setNotebookId] = useState<string>();
  const [name, setName] = useState('');
  const [url, setUrl] = useState('');
  const [error, setError] = useState<string | null>(null);

  const loadContext = useCallback(async () => {
    const context = await api.getZenPlanEditorContext();
    setNotebookId(context.notebook?.id);
    setName(context.notebook?.name ?? 'Web App');
    setUrl(context.notebook?.url ?? context.defaultUrl);
    setError(null);
    window.setTimeout(() => {
      nameRef.current?.focus();
      nameRef.current?.select();
    }, 0);
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      unlisten = await listen('zenplan-editor-target-changed', () => void loadContext());
      await loadContext();
    })();
    return () => unlisten?.();
  }, [loadContext]);

  const submit = async () => {
    try {
      await api.saveZenPlanNotebook(notebookId, name, url);
      await api.hideZenPlanEditor();
    } catch (caught) {
      setError(String(caught));
    }
  };

  return (
    <main
      className="zenplan-editor-shell"
      onKeyDown={(event) => {
        if (event.key === 'Escape') void api.hideZenPlanEditor();
      }}
    >
      <header
        className="zenplan-editor-header window-drag-handle"
        onMouseDown={(event) => {
          if (event.button === 0 && !(event.target as HTMLElement).closest('button, input')) {
            void getCurrentWindow().startDragging();
          }
        }}
      >
        <span className="zenplan-mark">W</span>
        <strong>{notebookId ? 'Edit web app pin' : 'Pin web app'}</strong>
        <button title="Cancel" aria-label="Cancel" onClick={() => void api.hideZenPlanEditor()}>×</button>
      </header>
      <div className="zenplan-editor-fields">
        <label>
          <span>Name</span>
          <input
            ref={nameRef}
            value={name}
            maxLength={80}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          <span>Web app URL</span>
          <input
            type="url"
            value={url}
            placeholder="https://example.com/app"
            spellCheck={false}
            onChange={(event) => setUrl(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') void submit();
            }}
          />
        </label>
      </div>
      <footer className="zenplan-editor-footer">
        {error && <p>{error}</p>}
        <button onClick={() => void api.hideZenPlanEditor()}>Cancel</button>
        <button className="primary-button" onClick={() => void submit()}>{notebookId ? 'Save' : 'Pin'}</button>
      </footer>
    </main>
  );
}
