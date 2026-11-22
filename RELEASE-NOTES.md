# Download Craft 0.11.7

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

## Install

**Windows 10/11.** Run `DownloadCraft-0.11.7-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.11.7-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.11.7-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.11.7_amd64.deb`; Fedora: install
`downloadcraft-0.11.7-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.11.7-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.11.1.xpi` from
`about:debugging#/runtime/this-firefox`. Chrome, Edge and Brave: extract the
Chrome ZIP and load the extracted folder in `chrome://extensions`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.11.7`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


# Earlier: Download Craft 0.11.6

## What's new

- **Problem reports in the app's own style**: after a crash, the next start
  shows a window like the rest of the app, with the error, what a report
  does for everyone, the full details on request, and Report problem. The
  GitHub issue it opens asks what you were doing and is filled in with what
  a fix needs: Windows version, which part of the app, renderer and look,
  the error and where it happened, and the last steps of the latest
  download. Web addresses only show their site and your user folder is
  hidden; nothing is sent until you send it.

## Fixes

- **YouTube downloads at full speed**: for a few seconds YouTube holds a
  stream for the length of its ad and refuses the size check. The video
  then came down as one stream, which YouTube slows to 1-2 MB/s. The size
  now comes from the stream's own link, so every connection works on it: a
  1.15 GB 4K video took 2 minutes instead of about 12.
- **YouTube downloads resume**: the same single stream kept no record of
  its progress, so Resume after a pause began again at 0. It now continues
  from the bytes already downloaded.


# Earlier: Download Craft 0.11.5

## Fixes

- **YouTube videos with Bangla titles download again**: reading a video's
  title cut YouTube's page at a fixed length, and when the cut fell inside
  a Bangla letter (or one of any other non-Latin script) the download
  stopped at once with "connection ended early" and 0 B. The cut now always
  ends on a whole letter.
