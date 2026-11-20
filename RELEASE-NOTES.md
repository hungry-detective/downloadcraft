# Download Craft 0.11.5

## Fixes

- **YouTube videos with Bangla titles download again**: reading a video's
  title cut YouTube's page at a fixed length, and when the cut fell inside
  a Bangla letter (or one of any other non-Latin script) the download
  stopped at once with "connection ended early" and 0 B. The cut now always
  ends on a whole letter.

## Install

**Windows 10/11.** Run `DownloadCraft-0.11.5-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.11.5-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.11.5-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.11.5_amd64.deb`; Fedora: install
`downloadcraft-0.11.5-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.11.5-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.11.0.xpi` from
`about:debugging#/runtime/this-firefox`. Chrome, Edge and Brave: extract the
Chrome ZIP and load the extracted folder in `chrome://extensions`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.11.5`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


# Earlier: Download Craft 0.11.4

## What's new

- **YouTube videos start about twice as fast**: reading a video used to
  download YouTube's player script (2-3 MB) every time. It is now kept on
  this computer and reused, and fetched alongside the other first steps
  instead of after them: about 4 s instead of 7-8 s before the download
  begins (YouTube still holds each stream for the length of its pre-roll ad).

## Fixes

- **A pasted YouTube link gets the video's name**: a link added by hand
  (not caught from the browser) was named `download_<number>.bin`; it now
  shows and saves the video's title, as a browser capture does.


# Earlier: Download Craft 0.11.3

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
