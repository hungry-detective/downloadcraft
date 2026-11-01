# Download Craft 0.9.13

## What's new

**Downloads**
- **Sites behind Cloudflare**: when a site's bot shield refuses the app but not the browser (the
  TLS fingerprint differs), Download Craft retries as a browser would, through yt-dlp's built-in
  browser impersonation, keeping the cookies, Referer and headers. Works for HLS streams and plain files.
- **Add download head start**: the main window's Add download dialog now starts a plain file in the
  background as soon as its size is known, like the browser's Download File Info window. Press
  Download and the progress window opens partway through; Cancel deletes what was fetched.
- **The file name follows the link**: paste a different link into Add download and the name changes
  with it (then to the server's own name). A name you typed yourself stays.
- **yt-dlp install fixed**: a freshly downloaded yt-dlp is given up to two minutes to answer its
  first start (antivirus scans and unpacking took longer than the old 5 seconds on slower PCs and
  failed the install). The same for FFmpeg and Deno. An older copy that is still running is moved
  aside instead of failing the update.
- Linux: downloads no longer get the whole save path as their name (`_home_you_Downloads_file.ext`) (#6).

**Settings**
- **Proxy / Socks page**: no proxy, system settings, an automatic configuration script (PAC), or a
  manual HTTP or SOCKS5 proxy with login and http/https choice; **Get system proxy** fills it in. A
  switch decides whether downloads caught from the browser use the browser's proxy.
- **Sounds page**: download complete, download failed, queue started and queue finished, each on or
  off, with your own .wav file or the system sound, and a Play button.
- **Appearance**: **Interface size** (90–175%), a **Font** choice (Segoe UI, Arial, Calibri,
  Tahoma, Verdana, Trebuchet MS or any .ttf/.otf file), clearer contrast in both themes, and a wider
  search box (#8).
- The Settings pages, plugins and browser cards use the same outline icons as the main window's
  sidebar, so they read well in dark and light mode.
- **Use the Windows title bar** (Settings → Window): for custom or stripped Windows builds where
  buttons reacted above the pointer. Turned on by itself when the system cannot draw borderless
  windows.

**Browser connection**
- The browser connection works when your Windows user name has letters like č, š or ř (#5).
- Download File Info starts Download Craft when it is not running and hands the download over once it
  is up (#7).
- Browsers are linked by themselves: opening Browser Integration links any browser that is not yet
  linked, so loading the extension is all you do. Chromium on Linux is linked too.

**Browser extension**
- **Downloads made inside the browser** carry the page's Referer, Origin and the player's login
  headers, which protected video servers check.
- **The video button sits on the player's top edge**, outside the picture, as IDM's does. It moves
  out of a pinned header's way instead of hiding, and hides when a pop-up (a Facebook post window)
  covers the video.
- **Facebook**: the menu's picture and title come from the video you clicked, not another one on the
  page (a frame of the video itself when it has no poster).
- No more stray console errors on sites with a Cloudflare/hCaptcha/reCAPTCHA check: the extension
  stays out of those frames.
- Brave: the browser replaying its old downloads can no longer flood the app; a burst of captures
  pauses capture for a minute.

**Linux**
- The Fedora/RPM package installs again (it asked for a package name Fedora does not have).

## Updating from 0.9.12

Use **Update now** in the app: it downloads this release, checks it and restarts. Then reload the browser
extension (chrome://extensions → reload) with the files from this release.

## Install

**Windows 10/11.** Extract `DownloadCraft-0.9.13-windows-x64-portable.zip` and run `download_craft.exe` —
nothing to install. Your queue and settings in `%APPDATA%\download_craft` carry over.

**macOS 12+.** Unzip `DownloadCraft-0.9.13-macos-Universal.zip` and move `Download Craft.app` into
Applications. This development build is not notarized: right-click → Open the first time.

**Linux (x86-64, glibc 2.28+).**
- Ubuntu, Debian, Mint, Pop!_OS: `sudo apt install ./downloadcraft_0.9.13_amd64.deb`
- Fedora: `sudo dnf install ./downloadcraft-0.9.13-1.x86_64.rpm`
- Other distributions: `DownloadCraft-0.9.13-x86_64.AppImage` (mark it executable; without FUSE run it
  with `--appimage-extract-and-run`).

**Browser extension (0.9.13).**
- Chrome / Edge / Brave: extract `DownloadCraft-Chrome-0.9.13.zip`, open `chrome://extensions`, enable
  Developer mode, **Load unpacked** → the `browser-extension` folder. Already installed? Press reload on
  the extension and refresh open tabs.
- Firefox: `about:debugging#/runtime/this-firefox` → **Load Temporary Add-on…** →
  `DownloadCraft-Firefox-0.9.13.xpi` (unsigned).
- Start Download Craft once: it links itself to your browsers.

## Media helpers

Settings → Plugins downloads and updates **yt-dlp**, **FFmpeg + ffprobe** and a portable
**JavaScript runtime** (Deno) into the app's own folder — no installers, no PATH changes, no sudo.
System copies on PATH or a custom file can be chosen instead.

RAR archives are read with the UnRAR source code (freeware licence, extraction only); see `NOTICE.md`.
