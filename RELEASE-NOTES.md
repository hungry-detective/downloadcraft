# Download Craft 0.9.14

## What's new

**YouTube, read by the app itself**
- **Every quality up to 4K and 8K on the app's own engine.** The app reads YouTube the way a smart
  TV does and gets the full-length stream links, so 1440p and 4K now download on the app's own
  connections, with pause, resume, the speed limit and Part Info. yt-dlp takes over only for what
  this cannot read (a sign-in, an age check, a live stream), and **yt-dlp first** in Settings →
  Plugins → YouTube still always asks yt-dlp.
- **Videos with a short ad** (many music videos) download too: the app waits the few seconds YouTube
  asks for, as a TV would play the ad, and the progress window counts it down.
- **YouTube audio: ♪ MP3 and ♪ M4A** in the video menu on every video. M4A is YouTube's own sound
  track as served; MP3 is made from it with FFmpeg (best VBR), on Windows, macOS and Linux.
- **The quality menu lists every height the video has**, 1440p, 4K and 8K included, even when
  YouTube hides the links in the page (it used to stop at 1080p).
- **Pause and resume continue** instead of starting over: a fresh YouTube link from another server
  no longer throws the downloaded part away, and the picture and sound keep their parts across
  attempts. Clicking the same quality again in the browser resumes a paused or failed row.
- The progress window's **Status** names each stage before the first byte (Reading YouTube,
  Waiting 5s for YouTube, Getting info, yt-dlp fallback), then which engine downloads: **app
  engine** in green or **yt-dlp** in blue.
- **Part Info** lists every connection of both the picture and the sound, as IDM does.
- Downloads the app merges itself no longer fail at 99% with "connection ended early".
- The Download File Info window shows a YouTube video's size and name in seconds.

**Downloads**
- **Refresh download address** is a real window now: it says why the link stopped working, waits
  for you to click the download link again in the browser (the new link continues the download),
  takes a pasted link, and opens the page. It also **opens by itself** when a download fails on a
  dead link (a server name that no longer exists, 404, an expired 403).
- The progress window has **Pause** and **Cancel** only (Cancel and Close did the same).

**Updates and the browser extension**
- **The title bar shows every stage of an update:** Update, then Updating 45% while it installs
  in the background, then **Restart to update** when it is installed while downloads are running.
- **The extension lives in a folder that never moves:** the app keeps its own version in
  `%APPDATA%\download_craft\browser-extension` (Settings → Browser Integration → Extension folder).
  Loaded from there once, it survives the app being updated or unpacked somewhere else, and **it
  reloads itself** when the app ships a newer version.
- The extension's notices are a modern card at the bottom center of the page, with a **Reload tab**
  button when the extension was updated.

**Fixes**
- The Sounds page's rows line up, and nothing runs past the card's edge.
- A download's log lines no longer get mixed together, and a long log with a Bengali name can no
  longer crash the app while it is trimmed.

## Updating from 0.9.13

Use **Update now** in the app: it downloads this release, checks it and restarts. Then load the
extension once from the app's extension folder (Settings → Browser Integration → Extension folder →
Open folder; `chrome://extensions` → Load unpacked). From then on it updates with the app.

## Install

**Windows 10/11.** Extract `DownloadCraft-0.9.14-windows-x64-portable.zip` and run `download_craft.exe` —
nothing to install. Your queue and settings in `%APPDATA%\download_craft` carry over.

**macOS 12+.** Unzip `DownloadCraft-0.9.14-macos-Universal.zip` and move `Download Craft.app` into
Applications. This development build is not notarized: right-click → Open the first time.

**Linux (x86-64, glibc 2.28+).**
- Ubuntu, Debian, Mint, Pop!_OS: `sudo apt install ./downloadcraft_0.9.14_amd64.deb`
- Fedora: `sudo dnf install ./downloadcraft-0.9.14-1.x86_64.rpm`
- Other distributions: `DownloadCraft-0.9.14-x86_64.AppImage` (mark it executable; without FUSE run it
  with `--appimage-extract-and-run`).

**Browser extension (0.9.14).**
- Chrome / Edge / Brave: the app keeps the extension in `%APPDATA%\download_craft\browser-extension`
  (Settings → Browser Integration). Open `chrome://extensions`, enable Developer mode, **Load
  unpacked** → that folder. `DownloadCraft-Chrome-0.9.14.zip` holds the same files.
- Firefox: `about:debugging#/runtime/this-firefox` → **Load Temporary Add-on…** →
  `DownloadCraft-Firefox-0.9.14.xpi` (unsigned).
- Start Download Craft once: it links itself to your browsers.

## Media helpers

Settings → Plugins downloads and updates **yt-dlp**, **FFmpeg + ffprobe** and a portable
**JavaScript runtime** (Deno) into the app's own folder — no installers, no PATH changes, no sudo.
System copies on PATH or a custom file can be chosen instead. The app's own YouTube reader uses the
same Deno to solve YouTube's player challenge (with yt-dlp's public-domain solver scripts).

RAR archives are read with the UnRAR source code (freeware licence, extraction only); see `NOTICE.md`.
