# Download Craft 0.11.2

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

## Install

**Windows 10/11.** Run `DownloadCraft-0.11.2-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.11.2-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.11.2-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.11.2_amd64.deb`; Fedora: install
`downloadcraft-0.11.2-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.11.2-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.11.0.xpi` from
`about:debugging#/runtime/this-firefox`. Chrome, Edge and Brave: extract the
Chrome ZIP and load the extracted folder in `chrome://extensions`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.11.2`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


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


# Earlier: Download Craft 0.11.0

## What's new

- **Faster, measured against IDM**: on the same PC and local test server,
  Download Craft finished a 1 GB file 2.2× faster than IDM 6.43, a download
  whose connections kept being cut 3.2× faster, and a 1 MB file 15× faster.
  - Small downloads finish at once: the app no longer waits for its next
    check before opening more connections or after the last byte.
  - A connection that is cut after receiving data reconnects straight away
    (it waited half a second).
  - The results and the test scripts are in the README and `tools/bench`.
- **Classic layout refresh**:
  - The sidebar shows All, then Categories and Status sections that fold
    from their headers, with today's figures at the foot.
  - Toolbar and sidebar icons are in the text colour: white in dark mode,
    black in light mode.
  - The Options window is compact: one row of tabs, section titles instead
    of frames, buttons of one width, the same height on every tab, and it
    opens in the middle of the screen. The file type lists show their usual
    contents.
- **Petal: fold the sidebar**: the button beside "Library" folds the sidebar
  to its icons (names on hover), so the downloads fit three cards to a row.
- **A new tray menu**: drawn by the app, only as wide as its rows (no empty
  strip on the right), with shorter rows, in dark or light with the app.
- **Windows remember where you left them**: the Download File Info and
  progress windows open where you last moved them, and Part Info stays open
  or folded the way you left it.
- **One look for every window**: the progress, Download File Info and
  Download complete windows use the main window's own colours in dark,
  AMOLED and light.
- **Ledger is now part of Classic**: the layouts are Craft, Classic and Petal.
  If you used Ledger, the app opens in Classic, which is built on it.
- **Streams keep their parts in one folder**: an HLS download keeps its
  segments in a "name.parts" folder next to the file (not hundreds of loose
  files), removed when the file is joined. Its size is estimated within a
  few seconds instead of after checking every segment.
- **Browser extension**: now named "Download Craft". It is on
  [addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
  and the app accepts the Microsoft Edge Add-ons version (in review).

## Fixes

- With "Download complete" switched off, the progress window closes when the
  download finishes (it stayed open and looked like that dialog).
- The progress window no longer jumps back to the middle of the screen when
  Part Info is folded or more parts appear.
- Remove & Delete also removes a video's work folder (its picture and sound
  parts) and a stream's parts folder.
- The size icon in the Download File Info window follows the text colour.


# Earlier: Download Craft 0.10.0

## What's new

- **Bangla interface**: Settings → Appearance → Language. Every window, menu
  and message is in natural Bangla and switches straight away, no restart
  needed. Bangla text is drawn properly, and the interface is as quick as in
  English.
- **YouTube live streams, HDR and 8K**: a live stream records until you press
  Stop, and Stop keeps a playable file. HDR videos get their own rows in the
  quality menu and stay HDR. Saved videos keep their cover picture, chapters
  and title inside the file (Settings → Plugins → YouTube).
- **Options window for the Classic layout**: Classic's Settings now look and
  work like a classic download manager's Options dialog. It has General, File
  types, Save to, Downloads, Connection, Proxy / Socks, Sites Logins,
  Dial Up / VPN and Sounds tabs, with OK and Help.
  - **Per-server connections:** a set number of connections for each server.
  - **Duplicate links:** what to do when the same link is added again (ask,
    add a numbered copy, overwrite, or show/resume the existing one).
  - **Start without the dialog:** downloads can start, or just join the
    queue, without showing the download window.
  - **Pick a queue:** choose a queue when you press "Download Later" or add a
    batch.
  - **Server file dates:** saved files can keep the server's date.
  - **Download limit warning:** a notice before the limit pauses downloads.
  - **FTP:** downloads through your proxy, and in active mode when PASV is off.
  - **Dial Up / VPN:** connects a Windows VPN, with redial attempts.
  - **Skip addresses:** a list of addresses that are never captured.
- **Find a setting**: type in the box at the top of Settings and pick a
  result. It opens the right page and points at the setting.
- **Streams show their size and time left**: HLS and DASH streams now show an
  estimated size, a moving progress bar and the time remaining.
- **Drag to reorder the queue**: in the Queues panel, drag a row to where it
  should start.
- **Failed downloads say what to do next**: the progress window explains each
  failure in plain words (link expired, sign-in needed, disk full, the server
  is limiting…). The failure notification has **Retry** and **Open page**
  buttons.
- **AMOLED black theme**, available for every layout.
- **New tray menu** with icons, following dark, light or system mode.
- **Classic layout refresh**:
  - The Categories panel is white with rounded corners, and its rows fill the
    panel.
  - Click a branch to open or close it.
  - Outline icons for Stop, Delete and similar tools.
  - Toolbar labels stay on one line.
- **Browser extension**:
  - A "Small download button" option shows only a round download icon on
    video players.
  - The button keeps a gap from the player's edge.
  - After you drag it, the next video puts it back in its corner.

## Fixes

- Deleting a download's unfinished file and then resuming it showed the old
  amount as already downloaded. It now says "Resume file not found" and starts
  again from the beginning.
- The tray menu stayed in the old language after switching language.
- The Craft toolbar no longer flickers in Bangla, and it spaces its buttons
  evenly.
- Settings: interface size applies straight away and has a Reset button.
  Descriptions are never hidden behind buttons or drop-down lists.
- Removing a finished torrent also removes its folder.
- Sites like `*.example.com` in the skipped-sites list are now matched.
- The app is quieter when idle and handles long download lists faster.
- **Security fixes:**
  - Opening a downloaded file can no longer run a command hidden in its name.
  - File names from websites can no longer leave the download folder.
  - The browser extension can no longer read files outside your download
    folders.
  - Web pages can no longer use the extension to list or control your
    downloads.
