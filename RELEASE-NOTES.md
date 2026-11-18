# Download Craft 0.10.0

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

## Install

**Windows 10/11.** Run `DownloadCraft-0.10.0-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.10.0-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.10.0-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.10.0_amd64.deb`; Fedora: install
`downloadcraft-0.10.0-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.10.0-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.10.0.xpi` from
`about:debugging#/runtime/this-firefox`. Chrome, Edge and Brave: extract the
Chrome ZIP and load the extracted folder in `chrome://extensions`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.10.0`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


# Earlier: Download Craft 0.9.18

## What's new

- **The Download window's head start is the download itself.** While the
  Download window is open, the app is already downloading the file, YouTube
  videos included. Press Download and the progress window opens where it has
  got to, with its connections already receiving, instead of starting again
  from "Connecting…". Cancel deletes what was fetched.
- **Classic layout** (Theme → Layout → Classic): laid out the way Internet
  Download Manager users know it. A toolbar of coloured icons over their
  labels in that order (Add URL, Add torrent, Resume, Stop, Stop All, Delete,
  Delete Completed, Options, Scheduler, Start/Stop Queue, Grabber), a
  Categories tree with a picture for every folder, and the list as File Name,
  Size, Status, Time left, Transfer rate, Last Try Date. Light and dark.
- **Torrents show their swarm.** Part Info lists the seeds, peers and
  connections, a bar of the files, and each connected peer with its client,
  what it has sent and how fast.
- **One notice per finished download.** The Download complete window, or one
  system notification when that window is off. The browser extension no
  longer adds a second one (#24).
- **Clipboard watching can be turned off:** Settings → Appearance →
  Clipboard (#24). Off, the clipboard is read only when you click Add URL.
- The title bar's window buttons and search icon are larger, the search box is
  narrower and has no blue glow, and a maximized window shows the usual
  "restore" icon.

## Fixes

- **Menus, right-click and moving the window stopped working after the window
  was minimized and brought back** (#24). Clicks now come from the mouse's own
  press and release, and a release lost while the window was away no longer
  leaves a button "held".
- **Torrents:** Cancel really stops a torrent (it went on downloading in the
  background while the row said Paused), and Resume continues it from what is
  on disk. Pause and Resume follow the torrent's real state. A stopped row no
  longer keeps showing its last speed.
- **macOS:** the app has its icon in Finder, the Dock and Launchpad (#25).
- Settings → Appearance fits all four layouts on one row at any width.
