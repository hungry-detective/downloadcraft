---
name: windows10-windows
description: Rules and checks for Download Craft's windows on Windows 10 (title bars, frames, borderless windows, dragging, resizing, dialogs, dark frames, graphics device loss). Use whenever a change touches a window, a title bar, a frame, a dialog (file pickers, Open folder), window styles, winit/eframe viewport options, platform.rs, or a GitHub issue reports a window or display problem, above all from a Windows 10 user.
---

# Windows 10 windows: what keeps breaking, and how to check

Most Download Craft users who report window problems run **Windows 10**
(build 19041–19045), while development happens on Windows 11. Windows 10
handles borderless windows differently, so a change that looks fine on
Windows 11 can break there. Treat every window change as a Windows 10 change.

## Past Windows 10 issues (learn from them)

| Issue | What the user saw | Cause | Fix (where) |
|---|---|---|---|
| #20 and the other 0.9.16 crashes | Crash "Parent device is lost" | The graphics device was lost (driver reset, sleep) | Save the queue and restart a fresh copy (`app.rs` update(), `platform::guard_gpu`) |
| 0.11.1 (`df0874e`) | Dark band at the top; right and bottom edges cut off | winit leaves `WS_CAPTION` and `WS_THICKFRAME` on borderless windows; Windows 10 reserves that frame inside the window | Strip both styles (`platform::guard_borderless`) |
| #28 | Window stops following drags and resizes until restart | A style change in the middle of the system move/size loop; winit's drag flag left set | Move and resize with `WM_SYSCOMMAND` `SC_MOVE`/`SC_SIZE` (`platform::start_drag`, `begin_resize`); never change styles while `GUI_INMOVESIZE` |
| #29 | Windows' own title bar over the app's while a file dialog is open; flicker on Open folder | winit puts the frame back on every state change (window disabled by a modal dialog, focus lost); the 250 ms watcher removed it too late | Strip the frame in the window procedure on `WM_STYLECHANGING` (`platform::hook_frame_off`) |
| #29 | Settings sometimes opens with a light frame and a white border | The dark frame setting reached the window after its frame was drawn | Set `DWMWA_USE_IMMERSIVE_DARK_MODE` (20, or 19 before build 18985) per window, then redraw with `SWP_FRAMECHANGED` (`platform::keep_frames_dark`) |

## Rules

1. **Never add a frame back by hand.** Borderless windows must never get
   `WS_CAPTION` or `WS_THICKFRAME` again. `hook_frame_off` strips them on
   every style change; don't work around it with `SetWindowLong` calls.
2. **No style changes during a move or resize.** Check `GetGUIThreadInfo`
   `GUI_INMOVESIZE` first. `SWP_FRAMECHANGED` mid-drag ends the system's
   move loop (#28).
3. **Drag and resize through Windows:** `platform::start_drag(ctx)` and
   `platform::begin_resize(ctx, dir)`, never egui's
   `ViewportCommand::StartDrag` / `BeginResize` on a borderless window.
4. **Framed windows** (`--settings`, `--links`, `--basket`, `--mini`,
   `--torrent-files`) keep Windows' frame, and its colour must follow the
   app's theme (`keep_frames_dark`). A new framed window goes in the
   `FRAMED` list in `guard_borderless`.
5. **Anything that blocks or disables the window** (an `rfd` file dialog,
   launching Explorer or a browser, a message box) changes its state, so
   winit restyles it. Test those paths, not just opening the window.
6. **Only visible windows count** in any loop over the process's windows.
   Hidden helper windows (the graphics device's, the input method's) never
   handle messages: they look "not responding" and must be skipped.
7. **The graphics device can be lost** (sleep, driver update, remote
   desktop): never `unwrap` on rendering results. Keep the "save queue and
   restart" path working.

## How to test on a Windows 11 machine

Windows 11 skips the Windows 10 code paths. Force them on:

```
DOWNLOADCRAFT_BORDERLESS_GUARD=1
```

Then run a test copy with its own settings, never the user's own running
app:

```
DOWNLOAD_CRAFT_DATA=<empty temp folder> DOWNLOADCRAFT_BORDERLESS_GUARD=1 download_craft.exe
```

Checks (Python with `ctypes` is enough; stop only the process you started,
never every `download_craft.exe`):

- **Frame stays off:** find the visible `"Window Class"` window of the
  test process, `SetWindowLongW(GWL_STYLE, style | WS_CAPTION |
  WS_THICKFRAME)`, then read the style straight back. `WS_CAPTION` must be
  gone at once, not 250 ms later.
- **Maximize and restore:** post `WM_SYSCOMMAND` `SC_MAXIMIZE` /
  `SC_RESTORE`. The window must change size, stay borderless, and
  `IsHungAppWindow` must stay false.
- **Dark frame:** start `--settings` with `"theme": "dark"` and then
  `"light"` in the test folder's `ui.json`, and read
  `DwmGetWindowAttribute(hwnd, 20)`: 1 for dark, 0 for light.
- **Dialogs:** opening a file dialog (Add torrent, Batch, Theme → Add a
  theme file) and Open folder must not bring Windows' title bar back.

Say plainly in the reply and release notes what could only be checked
on Windows 11 with the guard forced on, and ask the reporter to confirm on
Windows 10.

## Replying to a Windows 10 issue

Explain the cause in plain words, say what changed, link the release, and
ask the reporter to confirm and to share their Windows build (`winver`) if
anything is left. Close the issue when they confirm, or when the fix is
verified.
