import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from './api';
import type { SessionDefinition } from './types';

const palette = [
  ['Blue', '#2F78C4'],
  ['Purple', '#6C5CE7'],
  ['Violet', '#B14AED'],
  ['Pink', '#D64F70'],
  ['Orange', '#D87524'],
  ['Gold', '#C39B21'],
  ['Green', '#2A9D68'],
  ['Teal', '#168B91']
] as const;

export function ColorApp() {
  const [session, setSession] = useState<SessionDefinition | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadContext = useCallback(async () => {
    setSession(await api.getColorContext());
    setError(null);
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      unlisten = await listen('color-target-changed', () => void loadContext());
      await loadContext();
    })();
    return () => unlisten?.();
  }, [loadContext]);

  const chooseColor = async (color: string) => {
    if (!session) return;
    try {
      await api.setSessionColor(session.id, color);
      await api.hideColor();
    } catch (caught) {
      setError(String(caught));
    }
  };

  return (
    <main
      className="color-shell"
      onKeyDown={(event) => {
        if (event.key === 'Escape') void api.hideColor();
      }}
    >
      <header
        className="color-header window-drag-handle"
        onMouseDown={(event) => {
          if (event.button === 0 && !(event.target as HTMLElement).closest('button')) {
            void getCurrentWindow().startDragging();
          }
        }}
      >
        <span className="terminal-color" style={{ backgroundColor: session?.color ?? '#2F78C4' }} />
        <strong>Choose color for {session?.name ?? 'tab'}</strong>
        <button title="Cancel" aria-label="Cancel color change" onClick={() => void api.hideColor()}>×</button>
      </header>
      <div className="tab-color-palette" role="group" aria-label="Tab colors">
        {palette.map(([name, color]) => {
          const selected = session?.color.toUpperCase() === color;
          return (
            <button
              key={color}
              className={selected ? 'selected' : ''}
              style={{ '--swatch-color': color } as React.CSSProperties}
              aria-label={`${name}${selected ? ', selected' : ''}`}
              aria-pressed={selected}
              onClick={() => void chooseColor(color)}
            >
              <span className="tab-color-swatch" />
              <span>{name}</span>
              {selected && <span className="tab-color-check">✓</span>}
            </button>
          );
        })}
      </div>
      {error && <p className="color-error">{error}</p>}
    </main>
  );
}
