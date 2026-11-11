# Download Craft 0.9.17

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

## Install

**Windows 10/11.** Run `DownloadCraft-0.9.17-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.9.17-windows-x64-portable.zip` and run `download_craft.exe`.

**macOS 12+.** Unzip `DownloadCraft-0.9.17-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.9.17_amd64.deb`; Fedora: install
`downloadcraft-0.9.17-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.9.17-x86_64.AppImage`.

**Browser extensions.** Chrome, Edge and Brave: extract the Chrome ZIP and load
the extracted folder in `chrome://extensions`. Firefox: load
`DownloadCraft-Firefox-0.9.17.xpi` from
`about:debugging#/runtime/this-firefox`.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.9.17`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.


# Earlier: Download Craft 0.9.16

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
