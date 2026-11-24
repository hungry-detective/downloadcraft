# Download Craft 0.11.12

## What's new

- **Download history**: Tools → Download history lists every finished
  download, also after it is removed from the list, with search, Open,
  Open folder, Copy link and Download again. Download File Info says when
  the same link (or a file of the same name and size) was downloaded
  before, and a notification says when a new file has the same contents
  as an earlier one.
- **YouTube subscriptions**: Tools → YouTube subscriptions follows a
  channel or playlist; its new videos download by themselves into a
  folder named after it (checked at start and every 6 hours).
- **More file hosts**: Mediafire, Pixeldrain and Mega links download the
  real file (Mega's is decrypted with the key in the link). Google Drive
  folders, Pixeldrain lists and Mediafire folders open in Download all
  links to pick from.
- **Faster downloads**: 16 connections a file by default. A download
  opens 8 and adds more only while they raise the speed (a server that
  shares one speed among all its connections is not crowded).
- **Torrents find peers sooner**: public torrents also ask well-known
  public trackers, and the router opens the port by UPnP (not with a
  network lock or in server mode). Private torrents are left as they are.
- **Send links from your phone**: open `…/?url=<link>` on a paired phone,
  or make a key in Settings → Remote control for a Share-menu shortcut
  (iPhone Shortcuts, Android's HTTP Shortcuts).
- **Series episodes get their names** (browser extension 0.11.3): a video
  played on a page with an episode player is named like
  "Stranger Things S01E02 - Chapter Two - The Weirdo on Maple Street",
  from the player's address (VidSrc, 2Embed, VidFast, MultiEmbed and the
  others) and the page's episode list.

## Fixes

- **Windows 10: no Windows title bar over the app's** (#29): while a file
  dialog was open (Add torrent, Batch, Add a theme file) Windows painted
  its own old-style title bar over the main window, and Open folder made
  it flicker. That painting is now blocked, and Settings and other framed
  windows keep a themed frame.
- **Add download with a YouTube link**: the video's title shows at once
  (it waited for the size, or kept a name left from an earlier link), the
  quality list offers only the sizes the video has, and the box shows
  what is being downloaded (video, music, archive…) with the left
  panel's icons.
- **Stream videos keep their name** instead of "index.m3u8", and a file
  link that only says "Download" shows the page's title and picture in
  the extension.
- **Colons in titles** become " - " in file names ("Spider-Man - Brand
  New Day", not "Spider-Man_ Brand New Day").

## Install

**Windows 10/11.** Run `DownloadCraft-0.11.12-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.11.12-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.11.12-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.11.12_amd64.deb`; Fedora: install
`downloadcraft-0.11.12-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.11.12-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.11.3.xpi` from
`about:debugging#/runtime/this-firefox`. Chrome, Edge and Brave: extract the
Chrome ZIP and load the extracted folder in `chrome://extensions`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.11.12`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


# Earlier: Download Craft 0.11.11

## What's new

- **Everyone hears of a new version**: when a release comes out, a desktop
  notification says so once (clicking it opens About & Updates), besides
  the chip in the title bar. The app looks at start and every six hours.
- **Updates download faster**: the update now comes over 16 connections
  (GitHub limits each one), about twice as fast, with the same checksum
  check; the progress moves smoothly.
- **Firefox extension from Firefox Add-ons**: Settings → Browser
  Integration → Firefox and the Get ready window now have a button to
  install it from addons.mozilla.org, where it stays installed and updates
  itself. The folder for an offline install is still there.

## Fixes

- **Videos from a stream keep their name**: a video the extension found as
  a stream (HLS) was saved as "index.m3u8". It is now named after the page
  (for example "Spider-Man: Brand New Day.mp4"), and the app itself never
  names a stream "index.m3u8" (browser extension 0.11.2).


# Earlier: Download Craft 0.11.10

## Fixes

- **Windows 10: no more Windows title bar over the app's** (#29): while
  Add a theme file, Add torrent or Batch had a file dialog open, the main
  window showed Windows' own title bar and frame, and Open folder made it
  flicker. The frame is now kept off before Windows draws it.
- **Windows 10: Settings opens with a dark frame** when the app is dark
  (it sometimes came up with a light frame and a white border), and the
  frame follows a theme change.
