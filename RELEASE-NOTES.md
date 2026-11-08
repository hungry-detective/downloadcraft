# Download Craft 0.9.15

## What's new

- **Starts on old graphics cards (#14):** when the graphics driver cannot open
  the window with OpenGL (an Intel HD Graphics 2000 on Windows 11, for one),
  Download Craft starts again with Direct3D 12, or Windows' built-in software
  renderer (WARP), and keeps using it. If no renderer works, it says why in a
  message and in `startup-error.log` instead of closing without a word.
- **Images get their file type (#13):** a picture saved from Civitai (or any
  site whose image address has no extension) is named `….jpg`, `….png`,
  `….webp` or `….avif` from the file type the browser reports.
- **Drag finished downloads onto a page (#15):** in the extension's Downloads
  list, drag a finished file onto an upload box, as with Chrome's own download
  bubble (files up to 100 MB).
- **Brave and Chrome from Flatpak (#16):** the browser connection is set up
  inside a Flatpak browser's own folder too, and starts the app on the host.
- **Interface size slider (#10):** any size from 75% to 200%, and the Settings
  window follows it.
- **Any font (#10):** every font installed on the computer, free fonts
  downloaded on one click (Inter, Roboto, Open Sans, Noto Sans, Poppins, Lato),
  a font file, or a link to a .ttf/.otf (or a .zip of a family).
- **Updates in Settings → Updates (#10):** check, update and restart there; the
  app now also looks for new releases every six hours while it runs, not only
  at launch.
- **Part-by-part progress bar (#10):** the progress window's bar fills each
  connection's part where it lies in the file, as IDM's does.
- **Double-click to open, remember folders (#12):** two options in Settings →
  Download Engine → Storage. One makes double-clicking a finished download open
  the file. The other makes the Download window start at the last folder used
  and list the last 8 folders in its menu. Both are off by default.
- The Queues window's name box matches its row, and toolbar tooltips no longer
  break one word per line in a narrow window (#10).
- **Faster startup on Linux:** the main window appears without waiting for
  browser native-host registration, extension synchronization, or update cleanup.
- **Progress windows return by default:** starting a download opens its own
  progress window. Turn this off in **Settings → Progress windows** to keep
  downloads in the main list.
- **Reliable captures and downloads:** browser captures keep separate quality
  choices, closing a progress window no longer pauses the transfer, and the app
  stays live while minimized.
- **Better Linux window grouping:** the main app and its dialogs appear under
  one desktop application icon.
- **Improved video filenames:** resolved titles replace generic CDN names.

## Install

**Windows 10/11.** Extract `DownloadCraft-0.9.15-windows-x64-portable.zip` and run
`download_craft.exe` — nothing to install.

**macOS 12+.** Unzip `DownloadCraft-0.9.15-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.9.15_amd64.deb`; Fedora: install
`downloadcraft-0.9.15-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.9.15-x86_64.AppImage`.

**Browser extensions.** Chrome, Edge and Brave: extract the Chrome ZIP and load
the extracted folder in `chrome://extensions`. Firefox: load
`DownloadCraft-Firefox-0.9.15.xpi` from
`about:debugging#/runtime/this-firefox`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.

Your queue and settings carry over between versions.
