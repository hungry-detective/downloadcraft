# Download Craft 0.11.10

## Fixes

- **Windows 10: no more Windows title bar over the app's** (#29): while
  Add a theme file, Add torrent or Batch had a file dialog open, the main
  window showed Windows' own title bar and frame, and Open folder made it
  flicker. The frame is now kept off before Windows draws it.
- **Windows 10: Settings opens with a dark frame** when the app is dark
  (it sometimes came up with a light frame and a white border), and the
  frame follows a theme change.

## Install

**Windows 10/11.** Run `DownloadCraft-0.11.10-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.11.10-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.11.10-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.11.10_amd64.deb`; Fedora: install
`downloadcraft-0.11.10-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.11.10-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.11.1.xpi` from
`about:debugging#/runtime/this-firefox`. Chrome, Edge and Brave: extract the
Chrome ZIP and load the extracted folder in `chrome://extensions`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.11.10`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


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


# Earlier: Download Craft 0.11.8

## What's new

- **yt-dlp at full speed**: yt-dlp fetched a video file over one connection.
  Now it only finds the file, and the app downloads it over every
  connection, with resume (a 184 MB 1080p video in about 15 seconds). When
  a site refuses the app, yt-dlp downloads it itself as before.
- **Streams made from one file download the file**: a player stream such
  as `movie.mp4/index.m3u8` is fetched as the movie file itself, over every
  connection, with resume, and without a folder of pieces.
- **Pick a quality for a YouTube link in Add URL**: the size box shows the
  quality; click it for Best, 4K to 360p, MP3 or M4A, and the size follows.
  The video's name and size come straight from YouTube.
- **Browser extension 0.11.1**: videos in the extension's list are named by
  their real title, as the button on the player names them.

## Fixes

- **Windows 10: windows no longer get stuck**: the main window and the
  progress windows sometimes stopped following drags and resizes until the
  app restarted (#28). Moving and resizing now go straight through Windows.
- **Video downloads resume after Cancel**: a YouTube or Facebook video kept
  its parts in a work folder that the resume check did not look at, so
  Resume after Cancel or a restart started again from 0. A download that
  yt-dlp began also continues in yt-dlp.
- **Redownload starts again**: it now removes everything the old download
  left (yt-dlp's unfinished files included) before starting, so it no
  longer continues the old one or saves the new file as "name (1)".
- **Remove + delete file** also removes yt-dlp's hidden parts folder.
- **Stopping a video download stops yt-dlp completely**: while yt-dlp was
  still looking the file up, Stop left a program it had started running on
  macOS and Linux.
