# Download Craft 0.11.3

## Fixes

- **Windows 10: the window shows in full**: Windows 10 kept part of the
  window for a frame the app does not use, so the picture sat a few pixels
  low and to the right: a dark band ran along the top and the status bar was
  cut off at the bottom. The app's own windows now draw edge to edge.
  Resizing works as before, from the window's edges.
- **Windows 10: Settings keeps its title bar**: the earlier fix took the
  title bar off windows that use Windows' own (Settings, Download all
  links, the drop basket, the mini window, torrent files). They are left
  whole now.
- **Store extensions connect to the app**: a PC that updated from an older
  version kept the browser connection settings that only knew the extension
  from GitHub, so the Firefox and Edge store versions could not reach the
  app. They are brought up to date when the app starts.
- **Downloads start sooner**: after you press Download in the Download File
  Info window, the app starts at once instead of at its next 2-second check.
  The progress window opens in about a third of a second instead of two
  thirds.

## Install

**Windows 10/11.** Run `DownloadCraft-0.11.3-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.11.3-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.11.3-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.11.3_amd64.deb`; Fedora: install
`downloadcraft-0.11.3-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.11.3-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.11.0.xpi` from
`about:debugging#/runtime/this-firefox`. Chrome, Edge and Brave: extract the
Chrome ZIP and load the extracted folder in `chrome://extensions`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.11.3`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


# Earlier: Download Craft 0.11.2

## Fixes

- **Update checks work when GitHub's limit is reached**: GitHub answers only
  60 update questions an hour per internet connection (shared by everyone
  behind it). Past that, "Check for updates" said nothing was found; it now
  reads the release from GitHub's own pages instead, with the same checksum
  check before anything is installed.
- **Updates install while the browser helper runs**: the small helper the
  browser keeps open runs the app's program. An update moves the old one
  aside; a second update before the browser closed could stop at that old
  copy and install nothing. It now goes ahead.


# Earlier: Download Craft 0.11.1

## What's new

- **Browser downloads start sooner when the app is closed**: when a download
  is caught from the browser and Download Craft is not open, the app now
  starts quietly in the tray as the Download File Info window appears. The
  download begins inside it straight away, so pressing Download simply shows
  it, the same as when the app is already open (it used to start the app
  only after the click, then connect again).
- **Tray menu**: no light border, rounded corners, and the app's own colours
  in dark, AMOLED and light.

## Fixes

- **Windows 10**: the app's own title bar no longer shows a dark strip above
  it, and the Download File Info window is no longer cut off at the bottom.
- The browser extension keeps its version (0.11.0) when it has not changed;
  there is nothing new to install in the browser.
