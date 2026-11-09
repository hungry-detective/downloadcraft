# Download Craft 0.9.16

## What's new

- **Torrents and magnet links.** Paste a magnet link or a `.torrent` link,
  drop a `.torrent` file on the window, or click a magnet link in the browser:
  Download Craft downloads it like any other file, with progress, pause and
  resume. A stopped torrent continues from what is already on disk. A
  single-file torrent is saved under its own name, and a multi-file torrent
  in a folder of its own name. Seeding stops when the download is complete.
- **Remote control from a phone.** Settings → Remote control turns on a page
  that a phone or another computer on the same network opens. It shows the
  downloads, adds links and magnets, and pauses, resumes and removes them.
  Each device pairs once with a 6-digit code. Off by default.
- **VPN only, with a kill switch.** Settings → Proxy / Socks → Network holds
  every download to one network adapter, usually your VPN's. While it is not
  connected nothing downloads, and downloads it stopped start again once it is
  back. The status bar shows which adapter is in use.
- **Encrypted DNS.** Server names can be looked up over DNS-over-HTTPS or
  DNS-over-TLS (Cloudflare, Google, Quad9, AdGuard) instead of the system's
  DNS.
- **AI assistants (MCP).** `download_craft --mcp` lets Claude, Cursor, VS Code
  and other assistants list, add, pause and resume downloads. Settings → AI
  assistants sets what they may do (look only, standard, full), copies the
  setup, and opens the log of everything they did.
- **Debrid accounts.** Settings → Proxy / Socks → Debrid account: with a
  Real-Debrid, AllDebrid or Premiumize API key, links from file hosts
  (Rapidgator, 1fichier, Nitroflare, Turbobit, Katfile, Mega and others)
  download through your account at full speed.
- **Image galleries.** An album, gallery, artist page or category (Imgur,
  Flickr, DeviantArt, ArtStation, Wikimedia Commons, Pinterest, Tumblr,
  Bluesky), or any page written as `gallery:<address>`, saves every picture
  into a folder of its own, through gallery-dl.
- **Torrent extras.** Right-click a torrent → Choose torrent files… to
  download only some of its files. Settings → Torrents can keep sharing a
  finished torrent up to a ratio.
- **Theme files.** Drop a small JSON theme or a Base16 colour scheme (`.yaml`)
  into the themes folder, or use Theme → Add a theme file…. It appears in the
  Theme menu, on top of the layout it names.
- **Speed limit for one download.** A progress window's limit now applies to
  that download alone and is remembered for it. The global limit can apply
  only while you use the computer (full speed after a minute away).
- **Keyboard and screen readers.**
  - Arrow keys move through the list, Shift extends the selection,
    Home/End jump, Ctrl+N adds a download and Ctrl +/−/0 change the
    interface size.
  - Toolbar buttons and downloads have names for Narrator and NVDA.
- **More languages.** Español, Français, Deutsch, Português, Русский, हिन्दी,
  简体中文 and Bahasa Indonesia join বাংলা, for the menus, toolbar and
  dialogs that are translatable today.
- **IDM's command line.** `download_craft /d <link> /p <folder> /f <name> /n`
  (plus `/a` to only queue and `/s` to start the queue), for scripts and tools
  written for IDM.
- **Crash reports.** After a crash, the next start offers to open a GitHub
  issue filled in with the details, for you to read and send.
- **Windows installer.** Besides the portable zip, a `setup.exe` installs
  Download Craft for your user (no administrator rights needed), with Start
  menu and optional desktop shortcuts.
- **HTTPS for the phone page.** Settings → Remote control → Use HTTPS
  encrypts the page with this computer's own certificate, and shows its
  fingerprint to check on the phone.
- **Where the server is.** The progress window shows the server's country
  and how far it is ("Germany · about 24 ms (near)"). The country table
  (DB-IP Lite, CC BY 4.0) is downloaded once and looked up on this computer.
- **A download started early keeps its progress.** When the Download window
  has already fetched part of a file and you press Download, the bar and the
  progress window continue from that point instead of starting again at 0.
- Blue is the colour of every layout. Ledger's and Petal's icons follow the
  text colour, Ledger's column titles sit on the list's own background, and
  Petal's title bar runs into its toolbar.
- **Server mode and Docker.** `download_craft --server` runs Download Craft
  with no window, for a NAS or home server: the queue runs by itself and the
  remote control page is served. A `Dockerfile` and `docker-compose.yml` are
  included, and the image is published as `ghcr.io/hungry-detective/downloadcraft`.

- **Two new looks: Ledger and Petal**, each with its own layout. Ledger has a
  grey window, a tall toolbar with a coloured icon over each label and a
  compact list with grid lines. Petal has summary tiles (speed, active,
  queued, finished), every download as a card with its state, progress and
  buttons, and a rose "Add download" button. Pick them in **Theme** in the
  main window, which shows each one as you point at it, or in **Settings →
  Appearance → Look**. Each has a light and a dark side.
- **A fuller tray menu**: the running downloads with their percentages (a
  click opens the progress window), Pause all, Resume all, Add the link on the
  clipboard, Open download folder, Check for updates and Start with Windows.
- **A smooth progress bar again.** The progress window's main bar is one
  animated bar by default. The connection-by-connection bar is an option:
  **Settings → Progress windows → Show connections in the main progress bar**.
  The Part Info strip below it always shows the connections.
- **Pick downloads like files:** the tick boxes are gone. Click picks one
  download, Ctrl+click adds or removes one, Shift+click picks a range.
- **Petal's own layout:** a full-width toolbar (Add URL, Batch, Resume, Pause,
  Pause all, Delete, Clear done, Options, Scheduler, Grabber, Start queue, Stop
  queue), a sidebar with Library and Categories, compact tiles and cards, and a
  switch between cards and the table.
- **Faster when a server holds back:**
  - On a server that allows only two connections, a 50 MB file takes 12.7 s
    instead of 22 s. Download Craft keeps the connections the server accepts
    instead of halving down to one.
  - When some connections crawl, a 32 MB file takes about 5 s instead of
    15.8 s. A crawling connection is noticed within a second or two, and a
    free connection takes over its remaining part.

# Download Craft 0.9.15

## What's new

- **Starts on old graphics cards (#14):** when the graphics driver cannot open
  the window with OpenGL (an Intel HD Graphics 2000 on Windows 11, for one),
  Download Craft starts again with Direct3D 12, or Windows' built-in software
  renderer (WARP), and keeps using it. If no renderer works, it says why in a
  message and in `startup-error.log` instead of closing without a word.
- **Images get their file type (#13):** a picture saved from Civitai (or any
  site whose image address has no extension) is named `….jpg`, `….png`,
  `….webp` or `….avif` from the file type the browser reports.
- **Drag finished downloads onto a page (#15):** in the extension's Downloads
  list, drag a finished file onto an upload box, as with Chrome's own download
  bubble (files up to 100 MB).
- **Brave and Chrome from Flatpak (#16):** the browser connection is set up
  inside a Flatpak browser's own folder too, and starts the app on the host.
- **Interface size slider (#10):** any size from 75% to 200%, and the Settings
  window follows it.
- **Any font (#10):** every font installed on the computer, free fonts
  downloaded on one click (Inter, Roboto, Open Sans, Noto Sans, Poppins, Lato),
  a font file, or a link to a .ttf/.otf (or a .zip of a family).
- **Updates in Settings → Updates (#10):** check, update and restart there; the
  app now also looks for new releases every six hours while it runs, not only
  at launch.
- **Part-by-part progress bar (#10):** the progress window's bar fills each
  connection's part where it lies in the file, as IDM's does.
- **Double-click to open, remember folders (#12):** two options in Settings →
  Download Engine → Storage. One makes double-clicking a finished download open
  the file. The other makes the Download window start at the last folder used
  and list the last 8 folders in its menu. Both are off by default.
- The Queues window's name box matches its row, and toolbar tooltips no longer
  break one word per line in a narrow window (#10).
- **Faster startup on Linux:** the main window appears without waiting for
  browser native-host registration, extension synchronization, or update cleanup.
- **Progress windows return by default:** starting a download opens its own
  progress window. Turn this off in **Settings → Progress windows** to keep
  downloads in the main list.
- **Reliable captures and downloads:** browser captures keep separate quality
  choices, closing a progress window no longer pauses the transfer, and the app
  stays live while minimized.
- **Better Linux window grouping:** the main app and its dialogs appear under
  one desktop application icon.
- **Improved video filenames:** resolved titles replace generic CDN names.

## Install

**Windows 10/11.** Extract `DownloadCraft-0.9.15-windows-x64-portable.zip` and run
`download_craft.exe` — nothing to install.

**macOS 12+.** Unzip `DownloadCraft-0.9.15-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.9.15_amd64.deb`; Fedora: install
`downloadcraft-0.9.15-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.9.15-x86_64.AppImage`.

**Browser extensions.** Chrome, Edge and Brave: extract the Chrome ZIP and load
the extracted folder in `chrome://extensions`. Firefox: load
`DownloadCraft-Firefox-0.9.15.xpi` from
`about:debugging#/runtime/this-firefox`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.

Your queue and settings carry over between versions.
