import { Menu } from '@tauri-apps/api/menu';

export async function confirmWithMenu(prompt: string, confirmLabel: string) {
  let confirmed = false;
  const menu = await Menu.new({
    items: [
      { text: prompt, enabled: false },
      { item: 'Separator' },
      {
        text: confirmLabel,
        action: () => {
          confirmed = true;
        }
      },
      { text: 'Cancel' }
    ]
  });
  try {
    await menu.popup();
  } finally {
    await menu.close();
  }
  return confirmed;
}
