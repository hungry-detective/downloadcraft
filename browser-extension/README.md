## Draggable video download bar

Drag the grip at the left or drag the Download this video button to reposition it. A normal click still downloads or opens the quality menu; the small X dismisses the bar. Focus the grip and use arrow keys for keyboard movement (Shift moves farther).

The last position is shared through browser-session extension storage. It survives reloads, new pages and background-worker restarts, and resets when the browser fully exits. Extension reload/disable/update also resets session storage. Nothing is stored in website storage or synced between devices. Positions stay within the viewport when resizing. In embedded players, movement is bounded by the frame.

Restore a dismissed bar through the extension popup's Show video download buttons again command. Reload existing video tabs after updating the extension.

> Version 0.9.6.1: automatic takeover and the video-dismiss X are shared by
> Chrome/Brave and Firefox on Windows, macOS and Linux. Portable packages now
> include a ready-to-load `browser-extension-firefox` folder and Firefox XPI.
> `python3 scripts/package_extensions.py` builds both extension variants on any OS.
> Firefox has an additional attachment-response handoff; the X follows fullscreen.

# Browser extension — "Send to Download Craft" (zero-config)

Every download is captured automatically — no pasting IDs,
no per-file clicks.

## 1. Load it once (Chrome / Edge)
1. Open `chrome://extensions`, enable **Developer mode**.
2. **Load unpacked** → select this `browser-extension/` folder.
3. Done. The extension carries a pinned key, so its ID is always
   `pgdhmfgefjnfijmlheboncdmfhcnoikk` — nothing to copy.

(If you loaded an older version before the pinned key, remove it and load
again so the stable ID applies.)

## 1b. Load it once (Firefox)

Firefox cannot load the Chrome build: Manifest V3 service workers
(`background.service_worker`) are Chrome-only, so the Firefox build uses an
event page (`background.scripts`) and its own manifest. Everything else is
the same code base — `rank.js`, `background.js` and `content.js` are shared,
and the promise-style API calls go through `api` (`browser` when it exists,
`chrome` otherwise) so one file serves both engines.

1. Build the add-on (Firefox needs the manifest named `manifest.json`, which
   the script stages for you — no bundler, no minifier):

       powershell -ExecutionPolicy Bypass -File browser-extension/build-firefox.ps1

   That writes `DownloadCraft-Firefox-<version>.xpi` in the repository root.
   Validate it with Mozilla's own linter if you like:

       npx web-ext lint --source-dir "%TEMP%\download-craft-firefox-stage"

2. In Firefox open `about:debugging#/runtime/this-firefox`, press
   **Load Temporary Add-on…** and pick the `.xpi` (or the staged folder's
   `manifest.json`). The add-on appears as *Send to Download Craft* with the
   ID `send-to-download-craft@hungry-detective.github.io` — the same ID the app allows in
   `%APPDATA%\download_craft\native-host\firefox.json`.
3. Then do step 2 below (**Install firefox** in Settings).

A temporary add-on disappears when Firefox closes: that is Firefox's signing
rule, not this add-on. For a permanent install either sign it once with your
own Mozilla account (unlisted channel) —

    npx web-ext sign --api-key <key> --api-secret <secret>

— and install the signed `.xpi`, or use Developer Edition / Nightly where
`xpinstall.signatures.required = false` allows unsigned builds.

If captures stop reaching the app, check `about:addons` → this extension →
**Permissions** and make sure *Access your data for all websites* is still
allowed: Firefox lets the user revoke host permissions per extension.

## 2. Connect it from the app (once)
1. Run Download Craft → **Settings** → **Browser integration**.
2. Click **Install Chrome manifest** (or Edge / Firefox).
3. That's it — no IDs, no packets to send by hand.

## 3. Use it
- Download anything normally in the browser — the extension cancels the
  browser's own download and Download Craft takes over automatically.
- If the app isn't reachable, the file falls back to a normal browser
  download so nothing is ever lost (badge shows `!`).
- Right-click any link / video / audio → **Download with Download Craft**.
- Toolbar icon: toggle automatic capture, paste a URL, or pick from the
  files and videos detected on the current page.
- Firefox: build + load the `.xpi` as described in **1b** above.

## YouTube videos (in-page quality panel)

On `youtube.com/watch`, `youtu.be/*`, and Shorts pages the extension adds a
compact circular button at the player's top-right. Clicking it opens a
message-bubble panel next to it with:

- **Send video link to app** — sends the page URL itself (the app resolves
  it), for when no direct stream URL is available.
- **Video Files** — the direct qualities found on the page (`MP4 · 1080p`,
  `WEBM · 1440p`, …) with sizes, the highest one pre-selected. Picking a
  row and pressing **Download** sends that stream; when the source is
  video-only the best audio track travels with it, so the app merges both.
- **Other Files** — subtitle tracks (VTT, one chip per language) and
  audio-only tracks (e.g. `♪ M4A 128k`).
- A theme toggle (☀ / ☾) in the send row, plus **Download / Cancel / ✕**.

The same button appears on **Facebook** reels (`facebook.com/reel/…`), where
the panel lists the reel's HD/SD files with its caption, duration and any
audio-only stream captured from the player. The caption is read from the page
you are looking at (Facebook is an app-like page, so the metadata left over
from the first load is not trusted) and it names the saved file — a reel
saved as `আীবার সপক বজায় রেখে….mp4`, not `facebook.mp4` — and its thumbnail
comes from the player itself when the page exposes no `og:image`.

Scrolling from one reel to the next keeps the panel on the reel you are
watching: the reel key from the URL, the player that is actually on screen and
the recency of the captured streams all decide which legs are offered, so the
reel you scrolled past — whose URLs the page's embedded JSON still holds — is
never the one that downloads. A video-only adaptive leg is sent together with
the newest audio leg of the same reel, so a band-captured row still saves with
sound instead of as a silent (or picture-less) file.

While a video plays, streams the player fetches are merged into the panel:
a stale quality row flips from "via app" to a real download as soon as its
deciphered URL shows up.

The same qualities also appear in the toolbar popup under the video title.

Why the extension and not pasting links? The page runs in your real
browser session (real cookies/TLS), so YouTube serves it direct stream
URLs that server-side fetching from a flagged network cannot get.

## After updating the app or extension

1. In Download Craft → **Settings** → **Browser integration**, reinstall
   the host for your browser (this also cleans up legacy registrations).
2. In `chrome://extensions`, hit **reload** on the unpacked extension.
   In Firefox, rebuild the `.xpi` (see **1b**) and load it again — temporary
   add-ons cannot be hot-reloaded in place.
3. **Reload your open YouTube tabs too** (Ctrl+R). This step is mandatory:
   Chrome keeps the old page script running after an extension reload, and
   the old script can no longer talk to the app. If you ever see
   "Extension context invalidated" or a version-mismatch toast, it means
   this step was skipped — the current script detects it and, in most
   cases, refreshes the tab's script itself when you click again.
