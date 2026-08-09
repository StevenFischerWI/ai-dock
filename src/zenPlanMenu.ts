import { Menu } from '@tauri-apps/api/menu';
import { api } from './api';
import type { ZenPlanNotebook } from './types';

export async function showZenPlanMenu(notebook: ZenPlanNotebook) {
  const selections: Array<'edit' | 'unpin'> = [];
  const menu = await Menu.new({
    items: [
      { text: notebook.name, enabled: false },
      {
        text: 'Edit pin…',
        action: () => selections.push('edit')
      },
      { item: 'Separator' },
      {
        text: 'Unpin from AI Dock',
        action: () => selections.push('unpin')
      }
    ]
  });
  try {
    await menu.popup();
  } finally {
    await menu.close();
  }
  if (selections[0] === 'edit') {
    await api.openZenPlanEditor(notebook.id);
  } else if (selections[0] === 'unpin') {
    await api.deleteZenPlanNotebook(notebook.id);
  }
}
