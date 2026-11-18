# Download Craft 0.9.18

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

## Install

**Windows 10/11.** Run `DownloadCraft-0.9.18-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.9.18-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.9.18-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.9.18_amd64.deb`; Fedora: install
`downloadcraft-0.9.18-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.9.18-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.9.18.xpi` from
`about:debugging#/runtime/this-firefox`. Chrome, Edge and Brave: extract the
Chrome ZIP and load the extracted folder in `chrome://extensions`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.9.18`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


# Earlier: Download Craft 0.9.17

## What's new

- **Faster to the finish line.** A connection that goes silent while the
  others are still downloading is replaced within about 6 seconds, instead of
  holding the end of the file for up to a minute. In a test where one
  connection freezes, an 8 MB file finishes in 7 s instead of 31 s.
- **Connections are kept.** A single connection error (a reset, a failed
  handshake) no longer costs a download one of its connections for good: it
  takes three in a row. A connection that was given up is tried again after 8
  seconds instead of 30.
- **No pauses for disk writes.** Each connection keeps receiving while its
  last piece is written to disk, and saving the resume record no longer holds
  up the download.
- **The Download window's early start continues where it is.** The download
  that starts in the background while the Download window is open begins as
  soon as the file's size is known (it waited up to 3 seconds). Pressing
  Download continues it from the same byte, with no "Connecting…" and no
  jump back to 0%.
- **A failed download continues.** Retrying a failed download, or giving it a
  fresh link to the same file, continues from what is already on disk.
- **Petal, reworked.**
  - Downloads are a grid of cards, two to four per row by window width, each
    with a file-type badge (ZIP, EXE, MKV…), its state, progress and buttons.
  - Add URL and the new Add torrent sit at the start of the toolbar. Add URL
    takes the link on the clipboard.
  - The sidebar is one card that fits every item, and the cards line up with
    it and with the summary tiles.
  - List view is a real table again, one download per row (it showed the
    cards).
- **Add torrent in every layout,** with a magnet icon: a magnet link on the
  clipboard, or pick one or more `.torrent` files.
- **Toolbars keep their labels** at the default window size in Craft and
  Ledger (smaller icons), and the view switch says what it does.
- **Ledger's dark side is near-black,** like the other dark looks.
- **Menus close like Windows menus (#21):** a click on empty space, in another
  app, or Esc closes them.
- **TikTok thumbnails:** the extension's download panel shows the video's
  cover on TikTok (and on other sites whose player has no poster image).
- In the list, a paused download's percentage and bar sit centred in the
  Status column, like the other states.

## Fixes

- **Crash with Bangla and other non-English text (#19):** an address or name
  in another script no longer crashes the app.
- **Crash when the graphics device is lost (#20):** after a driver update or
  reset, waking from sleep or a GPU switch, the app saves the list and opens
  its window again instead of crashing. Downloads continue.
