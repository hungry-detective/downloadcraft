# Download Craft 0.9.15

## What's new

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
