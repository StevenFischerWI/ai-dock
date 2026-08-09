import { Menu } from '@tauri-apps/api/menu';
import { api } from './api';
import type { ActiveWindowApp, RecentWindowsApp } from './types';

type WindowAppAction = 'toggle-pin' | 'close' | 'hide';

export async function showWindowAppMenu(activeWindow: ActiveWindowApp, isPinned: boolean) {
  const selections: WindowAppAction[] = [];
  const menu = await Menu.new({
    items: [
      {
        text: isPinned ? 'Unpin from dock' : 'Pin to dock',
        action: () => selections.push('toggle-pin')
      },
      {
        text: 'Close window',
        action: () => selections.push('close')
      },
      {
        text: "Don't show in dock",
        action: () => selections.push('hide')
      }
    ]
  });
  try {
    await menu.popup();
  } finally {
    await menu.close();
  }

  const selection = selections[0];
  if (selection === 'toggle-pin') {
    await api.setWindowAppPinned(activeWindow.appKey, !isPinned);
  } else if (selection === 'close') {
    await api.closeExternalWindow(activeWindow.handle);
  } else if (selection === 'hide') {
    await api.setWindowAppVisible(activeWindow.appKey, false);
  }
}

export async function showRecentWindowAppMenu(app: RecentWindowsApp, isPinned: boolean) {
  const selections: WindowAppAction[] = [];
  const menu = await Menu.new({
    items: [{
      text: isPinned ? 'Unpin from dock' : 'Pin to dock',
      action: () => selections.push('toggle-pin')
    }]
  });
  try {
    await menu.popup();
  } finally {
    await menu.close();
  }
  if (selections[0] === 'toggle-pin') {
    await api.setWindowAppPinned(app.appKey, !isPinned);
  }
}
