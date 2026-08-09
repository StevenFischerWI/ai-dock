# AI Dock

AI Dock is a Windows bottom-edge dock for persistent terminal windows. Each window can contain multiple tabs, so PowerShell, WSL, Claude Code, and Codex CLI continue running when their window is hidden.

## What works

- Reserves the bottom of the current Windows monitor through the native AppBar API.
- Gives every dock item its own terminal window, initially one-third of the monitor width.
- Keeps multiple terminal windows visible at once and places the first three in separate screen columns.
- Places the dock `+` immediately after the final window and provides another `+` inside each window. Both create and open a PowerShell terminal immediately; the in-window version adds it to that window's group.
- Keeps only the dock always on top; terminal and editor windows layer like ordinary desktop windows.
- Lets you drag the terminal and session editor by their slim title strips, and resize them from their lower-right grips. The terminal remembers its size and position across hide/show.
- Click a dock item to restore it when minimized, focus it when visible but behind another window, or minimize it when it is already focused. Other visible windows stay open and keep running, and every terminal tab in the group follows the window.
- Each terminal window has a `−` control that instantly hides it to the dock without stopping its processes. The `×` control asks whether to save the complete window configuration to **Recently closed**, forget it permanently, or cancel. Closing stops all terminal processes and removes the item from the dock.
- Terminal and editor windows disable native transition animations for immediate show/hide behavior.
- Runs interactive terminal applications through ConPTY and renders them with xterm.js.
- Uses the native Windows clipboard for immediate terminal copy/paste; large pastes go through xterm as one bracketed-paste operation. Browser clipboard handling remains as the cross-platform fallback.
- Runs the installed Claude Code and Codex CLIs directly through the same ConPTY terminal path as other interactive programs.
- Creates CLI windows from **AI Dock → New Claude CLI…** and **AI Dock → New Codex CLI…**. Existing `+` buttons retain their fast PowerShell behavior.
- Opens the native Windows Start/Search launcher from the `+` beside active app icons or **AI Dock → Launch Windows app…**, ready for immediate typing and Enter-to-launch.
- Right-click a dock item or terminal tab for a compact native menu that renames it, changes its color, or closes it.
- Includes PowerShell, Codex, Claude, and WSL presets.
- Persists settings in `%APPDATA%\com.aidock.desktop\settings.json` without storing terminal output.
- Re-registers the dock after Explorer/taskbar, display, and DPI changes.
- Prevents multiple dock instances.
- Shows other taskbar-style Windows apps as compact native app icons in a centered strip between the primary terminal groups and the right-side utilities. Window titles remain available as hover tooltips. Click an app to focus or restore it; click it again while focused to minimize it. Right-clicking offers only **Close window** and **Don't show in dock**. **AI Dock → Apps shown in dock…** opens a persistent checklist, so hidden apps can be restored and any number of apps can be checked or unchecked before closing it.
- Tracks newly observed Windows app processes in **AI Dock → Recent apps**. Selecting an app focuses its existing window or relaunches its recorded executable; apps unchecked in **Apps shown in dock…** stay out of this launcher.
- Batches high-volume terminal output into small four-millisecond windows, coalesces resize work to one fit per display frame, and avoids dock re-renders when the Windows app snapshot has not changed.
- Supports multiple named **web app** pins on the right side. AI Dock discovers and caches each site's favicon, with the app-name initial as a fallback. Every pin has its own `http://` or `https://` URL, resizable WebView2 window, and remembered geometry; new web app windows tile right-to-left above the dock. Use **AI Dock → Pin web app…**, and right-click a pin to edit or unpin it.
- Adds a compact down/up chevron in the far-right corner to collapse or expand the Windows taskbar on every monitor. While collapsed, AI Dock moves to the physical bottom edge; if AI Dock hid the taskbar, it restores it before exiting.

## Run the release build

The compiled application is:

```text
src-tauri\target\release\ai-dock.exe
```

Double-click it, or run:

```powershell
& .\src-tauri\target\release\ai-dock.exe
```

The first launch creates a PowerShell window. Use the dock `+` for another PowerShell window or the `+` in a terminal title strip for another PowerShell tab in that window. Click **AI Dock → New Claude CLI…** or **New Codex CLI…**, then choose a project folder. Right-click any tab to rename it, change its color, or close it. Drag a window from the empty area of its title strip. Click the AI Dock logo to open its menu, reopen a saved window from **Recently closed**, hide all visible windows, or exit the app and stop its managed processes.

Click any pinned **web app** at the far right to show or hide it. Each site runs at its real origin in a shared, dedicated WebView2 profile, so windows retain logins, service workers, PWA storage, and cookies. Existing ZenPlan pins and their profile are preserved automatically. Remote pages are intentionally not granted access to AI Dock's Tauri commands. A site's first launch may require a one-time sign-in because this profile is separate from Edge or an already-installed browser PWA.

Existing App Server-backed Codex tabs are migrated automatically to terminal sessions. When a saved thread ID is available, the migrated tab launches `codex resume <session-id>` so the prior conversation remains accessible in the CLI.

## Develop and build

Prerequisites are Node.js, Rust's `x86_64-pc-windows-msvc` toolchain, WebView2, and Visual Studio C++ tools. This machine has a GNU `link.exe` earlier on `PATH`, so the included PowerShell wrapper discovers and selects Microsoft's linker and SDK automatically.

```powershell
npm install
npm run desktop:dev
```

Useful checks:

```powershell
npm run build
npm run desktop:check
npm run desktop:lint
npm run desktop:test
npm run desktop:build
npm audit
```

### Isolated development build

Use the isolated flavor for development and visual testing while the live release continues running:

```powershell
npm run desktop:build:isolated
& .\src-tauri\target-isolated\release\ai-dock.exe
```

For hot-reload development, use `npm run desktop:dev:isolated`. This flavor has a separate application identifier, executable target directory, settings and PowerShell history directory. It does not register as a Windows AppBar or reserve screen space, and its dock and terminal windows carry an orange **Test** treatment. Closing or rebuilding it cannot terminate the live release's terminal processes.

## Project shape

- `src/` — React dock, session editor, and xterm terminal surface.
- `src-tauri/src/appbar.rs` — Windows AppBar registration and recovery.
- `src-tauri/src/session.rs` — portable PTY ownership, input/output, resize, restart, and stop.
- `src-tauri/src/settings.rs` — atomic versioned settings storage.
- `src-tauri/src/windowing.rs` — monitor-aware popup sizing and placement.
- `src-tauri/src/windows_apps.rs` — taskbar-style top-level window discovery and activation.

The PTY manager and UI model are cross-platform. A future macOS build needs a native panel/window-positioning adapter in place of the Windows AppBar implementation.

## Current MVP boundaries

- The dock uses the monitor on which it starts; there is not yet a monitor picker.
- Running shells survive hide/show, but intentionally stop when AI Dock exits or the computer restarts.
- Recently closed windows restore their tab configuration, commands, folders, colors, and geometry; terminal output and stopped processes are not retained.
- AI Dock creates and owns sessions; it does not adopt arbitrary already-open Windows Terminal tabs.
- Launch-at-login, global hotkeys, installer packaging, and automatic updates are follow-up work.
