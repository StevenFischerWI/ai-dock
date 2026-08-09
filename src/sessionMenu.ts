import { Menu } from '@tauri-apps/api/menu';
import { api } from './api';
import { confirmWithMenu } from './confirmationMenu';
import type { SessionDefinition } from './types';

interface SessionMenuOptions {
  closeLabel: string;
  closePrompt: string;
  onClose: () => Promise<void>;
  archiveLabel?: string;
  onArchive?: () => Promise<void>;
}

type MenuSelection =
  | { kind: 'color' }
  | { kind: 'rename' }
  | { kind: 'archive' }
  | { kind: 'close' };

export async function showSessionMenu(session: SessionDefinition, options: SessionMenuOptions) {
  const selections: MenuSelection[] = [];
  const menu = await Menu.new({
    items: [
      { text: session.name, enabled: false },
      {
        text: 'Change color…',
        action: () => {
          selections.push({ kind: 'color' });
        }
      },
      {
        text: 'Rename…',
        action: () => {
          selections.push({ kind: 'rename' });
        }
      },
      { item: 'Separator' },
      ...(options.archiveLabel && options.onArchive ? [{
        text: options.archiveLabel,
        action: () => {
          selections.push({ kind: 'archive' });
        }
      }] : []),
      {
        text: options.closeLabel,
        action: () => {
          selections.push({ kind: 'close' });
        }
      }
    ]
  });
  try {
    await menu.popup();
  } finally {
    await menu.close();
  }

  const selection = selections[0];
  if (selection?.kind === 'color') {
    await api.openColor(session.id);
  } else if (selection?.kind === 'rename') {
    await api.openRename(session.id);
  } else if (selection?.kind === 'archive' && options.onArchive) {
    await options.onArchive();
  } else if (selection?.kind === 'close') {
    const confirmed = await confirmWithMenu(options.closePrompt, options.closeLabel);
    if (confirmed) await options.onClose();
  }
}
