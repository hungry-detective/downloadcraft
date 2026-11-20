# Download Craft 0.11.6

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

## Install

**Windows 10/11.** Run `DownloadCraft-0.11.6-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.11.6-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.11.6-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.11.6_amd64.deb`; Fedora: install
`downloadcraft-0.11.6-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.11.6-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.11.0.xpi` from
`about:debugging#/runtime/this-firefox`. Chrome, Edge and Brave: extract the
Chrome ZIP and load the extracted folder in `chrome://extensions`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.11.6`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


# Earlier: Download Craft 0.11.5

## Fixes

- **YouTube videos with Bangla titles download again**: reading a video's
  title cut YouTube's page at a fixed length, and when the cut fell inside
  a Bangla letter (or one of any other non-Latin script) the download
  stopped at once with "connection ended early" and 0 B. The cut now always
  ends on a whole letter.


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
