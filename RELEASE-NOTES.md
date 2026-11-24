# Download Craft 0.11.14

## What's new

- **Add download opens in its own window**, like Settings. The main window
  stays usable and is no longer dimmed while it is open.
- **Settings has its own title bar** instead of Windows' frame, like the
  rest of the app ("Use the Windows title bar" brings Windows' frame back).
- **Batch is back** in the Craft and Classic toolbars. In Classic the order
  is now Delete Completed, Batch, Scheduler, Options.
- **YouTube titles in Add download**: a pasted YouTube link (youtu.be too)
  is named after the video. The refresh button beside the name reads it
  again.
- **Edge Add-ons**: Settings → Browser Integration and Get ready open the
  extension's page on Microsoft Edge Add-ons, as they do for Firefox.
- **Browser extension 0.11.15**:
  - **Instagram, TikTok, X and Reddit**: one card for the post on screen,
    named after its caption, with Best, 1080p, 720p, 480p, 360p and MP3.
    The app downloads the whole video with its sound.
  - **Facebook Reels**: only the reel you are watching, with its own title.
  - The list **follows the page while the popup is open** (the next reel
    you scroll to appears).
  - **Download in browser** is always there, folded until you open it, and
    it remembers whether you did.
  - It **remembers the quality you last downloaded** and picks it next time.
  - Cards without a picture get one from the video on screen; MP4 cards
    show the video's length; the toolbar badge counts YouTube videos too.

## Fixes

- **Windows 10 (#29)**: Settings no longer shows Windows' old pale title bar
  with boxed buttons, nor a white flash as it opens. The other windows that
  keep Windows' title bar draw in a way that keeps it from appearing too.
- **Add download**: the quality list is no longer cut off, and its arrow no
  longer shows as a square.
- **Extension**:
  - Previews, GIF clips and 10-second pieces of the playing video are no
    longer listed: only the full video, as a file or a stream.
  - Page items that are not videos (a `.php` player script) are left out.
  - "Download in browser" no longer saves a short preview instead of the
    full video.
  - Instagram no longer lists dozens of "MP4" pieces of one reel, nor names
    every reel "Instagram".
  - The quality list on a card is no longer squeezed into an arrow.
  - The "Failed to fetch" error on the extension's Errors page is gone (it
    came from the page's own blocked ad requests).
  - The popup shows its list at once instead of waiting for a picture.

## Install

**Windows 10/11.** Run `DownloadCraft-0.11.14-windows-x64-setup.exe` (installs
for your user, no administrator rights), or extract
`DownloadCraft-0.11.14-windows-x64-portable.zip` and run `download_craft.exe`.
Already installed: About → Check again.

**macOS 12+.** Unzip `DownloadCraft-0.11.14-macos-Universal.zip` and move
`Download Craft.app` into Applications. This build is not notarized: right-click
→ Open the first time.

**Linux (x86-64, glibc 2.28+).** Ubuntu/Debian/Mint: install
`downloadcraft_0.11.14_amd64.deb`; Fedora: install
`downloadcraft-0.11.14-1.x86_64.rpm`; other distributions can use
`DownloadCraft-0.11.14-x86_64.AppImage`.

**Browser extensions.** Firefox: install
[Download Craft from addons.mozilla.org](https://addons.mozilla.org/firefox/addon/download-craft/),
or load `DownloadCraft-Firefox-0.11.15.xpi` from
`about:debugging#/runtime/this-firefox`. Edge: install
[Download Craft from Edge Add-ons](https://microsoftedge.microsoft.com/addons/detail/download-craft/bmjjepniglflidaclilkibodbkdogoio).
Chrome and Brave: extract `DownloadCraft-Chrome-0.11.15.zip` and load the
extracted folder in `chrome://extensions`. With the app running, the
extension updates itself.

**Server (NAS, home server).** `docker run -d -p 8765:8765 -p 6881:6881 -v dc-data:/data
-v /path/to/downloads:/downloads ghcr.io/hungry-detective/downloadcraft:0.11.14`, then
`docker logs` for the address and pairing code. Or run `download_craft --server`.

Settings and queued downloads carry over between versions. Settings → Plugins
provides yt-dlp, FFmpeg and the JavaScript runtime when needed.

# Earlier: Download Craft 0.11.13

## What's new

- **Full episodes from embedded players**: a series that plays in a player
  inside another site (VidSrc on IMDb, for example) downloads in full. The
  extension used to pick the site's own 11-second trailer preview, and the
  player's server refused the download (it checks that requests come from
  the player's site, which is now sent).
- **One card per video** in the extension (0.11.8): the video playing now,
  with its length and a list of its qualities. Trailer previews, short
  clips, earlier episodes and copies from other servers are left out, and
  the "Download in browser" list shows only when the app is not running
  (each card's ⋮ menu still offers it).
- **The player's download button picks the playing video**: the one as
  long as the player's, from the player's own frame.
- **Stream length and size before it starts**: Download File Info reads a
  stream's length and estimates its size from its best quality
  ("49:27 • ~1.5 GB"), with a turning ring while it reads.
- **Part Info for streams, like IDM**: a stream arrives in hundreds of
  pieces; Part Info now lists the pieces arriving now and says how many are
  done ("212 of 600 pieces done"), not every piece.
- **Classic and Petal in Craft's colors**: Petal's title bar and toolbar
  are one band with tools that fill the row; Classic's tools have even
  spacing, lighter strokes, and all fit the default window.

## Fixes

- **Add download**: the name is caught for links pasted one after another
  (it kept the previous link's), and the name and size boxes show a
  loading ring while they are read.

# Earlier: Download Craft 0.11.12

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
