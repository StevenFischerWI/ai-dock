import { Menu } from '@tauri-apps/api/menu';
import { api } from './api';
import type { ActiveWindowApp, RecentGroup, RecentWindowsApp } from './types';

type DockMenuSelection =
  | 'launch-app'
  | 'new-claude'
  | 'new-codex'
  | 'clear-recent'
  | 'new-web-app'
  | 'manage-apps'
  | 'hide-all'
  | 'exit'
  | { kind: 'restore'; groupId: string }
  | { kind: 'windows-app'; appKey: string };

interface DockMenuOptions {
  hasVisibleTerminals: boolean;
  recentGroups: RecentGroup[];
  activeApps: ActiveWindowApp[];
  recentWindowsApps: RecentWindowsApp[];
  hiddenWindowApps: string[];
}

type RecentAppsMenuOptions = Pick<
  DockMenuOptions,
  'activeApps' | 'recentWindowsApps' | 'hiddenWindowApps'
>;

function launchableRecentApps(
  recentWindowsApps: RecentWindowsApp[],
  hiddenWindowApps: string[]
) {
  return recentWindowsApps
    .filter((app) => !hiddenWindowApps.includes(app.appKey))
    .slice(0, 12);
}

async function launchOrFocusRecentApp(activeApps: ActiveWindowApp[], appKey: string) {
  const running = activeApps.find((app) => app.appKey === appKey);
  if (running) await api.focusExternalWindow(running.handle);
  else await api.launchRecentWindowsApp(appKey);
}

export async function showRecentAppsMenu({
  activeApps,
  recentWindowsApps,
  hiddenWindowApps
}: RecentAppsMenuOptions) {
  const selections: string[] = [];
  const launchableApps = launchableRecentApps(recentWindowsApps, hiddenWindowApps);
  const menu = await Menu.new({
    items: [
      { text: 'Recent apps', enabled: false },
      { item: 'Separator' },
      ...(launchableApps.length > 0
        ? launchableApps.map((app) => ({
            text: app.appName,
            action: () => selections.push(app.appKey)
          }))
        : [{ text: 'No recent apps', enabled: false }])
    ]
  });

  try {
    await menu.popup();
  } finally {
    await menu.close();
  }

  const appKey = selections[0];
  if (appKey) await launchOrFocusRecentApp(activeApps, appKey);
}

export async function showDockMenu({
  hasVisibleTerminals,
  recentGroups,
  activeApps,
  recentWindowsApps,
  hiddenWindowApps
}: DockMenuOptions) {
  const selections: DockMenuSelection[] = [];
  const recentItems = recentGroups.length > 0
    ? [
        ...recentGroups.map((group) => {
          const primary = group.sessions[0];
          const tabCount = group.sessions.length;
          return {
            text: `${primary?.name ?? 'Terminal window'}${tabCount > 1 ? ` (${tabCount} tabs)` : ''}`,
            action: () => {
              selections.push({ kind: 'restore', groupId: group.groupId });
            }
          };
        }),
        { item: 'Separator' as const },
        {
          text: 'Clear recently closed',
          action: () => {
            selections.push('clear-recent');
          }
        }
      ]
    : [{ text: 'No recently closed windows', enabled: false }];
  const launchableApps = launchableRecentApps(recentWindowsApps, hiddenWindowApps);
  const recentAppItems = launchableApps.length > 0
    ? launchableApps.map((app) => ({
        text: app.appName,
        action: () => selections.push({ kind: 'windows-app' as const, appKey: app.appKey })
      }))
    : [{ text: 'No recent apps', enabled: false }];
  const menu = await Menu.new({
    items: [
      { text: 'AI Dock', enabled: false },
      {
        text: 'Launch Windows app…',
        action: () => {
          selections.push('launch-app');
        }
      },
      {
        text: 'Recent apps',
        enabled: launchableApps.length > 0,
        items: recentAppItems
      },
      { item: 'Separator' },
      {
        text: 'New Claude CLI…',
        action: () => {
          selections.push('new-claude');
        }
      },
      {
        text: 'New Codex CLI…',
        action: () => {
          selections.push('new-codex');
        }
      },
      {
        text: 'Pin web app…',
        action: () => {
          selections.push('new-web-app');
        }
      },
      { item: 'Separator' },
      {
        text: 'Apps shown in dock…',
        enabled: activeApps.length > 0,
        action: () => {
          selections.push('manage-apps');
        }
      },
      { item: 'Separator' },
      {
        text: 'Recently closed',
        enabled: recentGroups.length > 0,
        items: recentItems
      },
      { item: 'Separator' },
      {
        text: 'Hide all terminal windows',
        enabled: hasVisibleTerminals,
        action: () => {
          selections.push('hide-all');
        }
      },
      { item: 'Separator' },
      {
        text: 'Exit AI Dock',
        action: () => {
          selections.push('exit');
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
  if (selection === 'launch-app') {
    await api.openWindowsStartSearch();
  } else if (typeof selection === 'object' && selection.kind === 'restore') {
    const groupId = await api.restoreTerminal(selection.groupId);
    await api.activateGroup(groupId);
  } else if (typeof selection === 'object' && selection.kind === 'windows-app') {
    await launchOrFocusRecentApp(activeApps, selection.appKey);
  } else if (selection === 'manage-apps') {
    await api.openWindowAppsEditor();
  } else if (selection === 'new-web-app') {
    await api.openZenPlanEditor();
  } else if (selection === 'new-claude' || selection === 'new-codex') {
    const settings = await api.getSettings();
    const startingDirectory = settings.sessions.find((session) => session.workingDirectory)?.workingDirectory ?? '';
    const workingDirectory = await api.pickFolder(startingDirectory);
    if (workingDirectory) {
      const session = selection === 'new-claude'
        ? await api.addClaudeSession(workingDirectory)
        : await api.addCodexSession(workingDirectory);
      await api.activateGroup(session.groupId);
    }
  } else if (selection === 'clear-recent') {
    await api.clearRecentGroups();
  } else if (selection === 'hide-all') {
    await api.hideTerminal();
  } else if (selection === 'exit') {
    await api.exitApp();
  }
}
