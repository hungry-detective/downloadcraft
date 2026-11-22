# Download Craft 0.11.11

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

## Install

**Windows 10/11.** Run `DownloadCraft-0.11.11-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.11.11-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.11.11-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.11.11_amd64.deb`; Fedora: install
`downloadcraft-0.11.11-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.11.11-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.11.2.xpi` from
`about:debugging#/runtime/this-firefox`. Chrome, Edge and Brave: extract the
Chrome ZIP and load the extracted folder in `chrome://extensions`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.11.11`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


# Earlier: Download Craft 0.11.10

## Fixes

- **Windows 10: no more Windows title bar over the app's** (#29): while
  Add a theme file, Add torrent or Batch had a file dialog open, the main
  window showed Windows' own title bar and frame, and Open folder made it
  flicker. The frame is now kept off before Windows draws it.
- **Windows 10: Settings opens with a dark frame** when the app is dark
  (it sometimes came up with a light frame and a white border), and the
  frame follows a theme change.


# Earlier: Download Craft 0.11.9

## What's new

- **Click'n'Load**: link sites with a Click'n'Load button hand their whole
  package to Download Craft (port 9666, this computer only). The links open
  in Download all links in a folder named after the package, and its
  archive passwords are saved for extracting. Settings → Browser
  Integration → Capture turns it off.
- **Archive passwords**: save the passwords you use in Settings →
  Archives; extracting tries them in turn (ZIP with AES, 7z, RAR) and the
  one that worked moves to the top. **.tar, .tar.gz and .tar.xz** extract
  too.
- **The scheduler can wake the computer** from sleep to start the queue
  (Windows), remembers its time across restarts, and the computer stays
  awake while downloads run.
- **Portable mode**: Settings → Portable keeps settings, the download list
  and the helpers in a `data` folder next to the program, for a copy on a
  USB stick.
- **Freeze reports**: if a window stops responding for 15 seconds, the app
  writes a report like a crash report and offers it when the window comes
  back (or at the next start).

## Fixes

- **Update progress in About matches the title bar**: About was up to 2
  seconds behind and moved in 5 % jumps; it now follows every percent, the
  bar glides, and the spinning ring no longer stutters.
- **Raising or lifting the speed limit takes effect at once**: at a low
  limit a running download kept its old pace for up to a minute.
- **Long video titles in deep folders save**: names are shortened to fit
  Windows' 260-character path limit, keeping the extension and quality.
- **Download File Info says when the file is already there**: a mark on
  the name shows the name it will be saved as, instead of quietly adding
  "(1)".
- **Files go to the right folder**: .opus, .aac, .wma, .m4v, .flv, .xz,
  .apk and others were sorted into "Other".
