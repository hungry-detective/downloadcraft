# Roadmap

What is known to be unfinished, in no particular order. Remove an entry when it ships.

## Layouts

- **Classic: an IDM-style Options window.** Classic has the main window's layout
  (toolbar order, Categories tree, list columns), but Settings is still Download
  Craft's own window with its side rail. Classic should get a tabbed Options
  dialog laid out like IDM's (General, File types, Save to, Downloads,
  Connection, Proxy/Socks, Site logins, Dial up/VPN, Sounds).
- **Classic: the menu bar.** Classic still shows File / Tasks / Tools / Theme /
  Help; IDM's are Tasks / File / Downloads / View / Help.
- **Classic: "Grabber projects" in the Categories tree**, opening the Grabber
  with a saved project.
- **Classic: a higher-resolution look at very narrow widths.** Below the default
  window width the last toolbar buttons drop off (they stay in the menus).

## Issues still open from reports

- **#24: text misalignment on the Proxy / Socks page.** Waiting for a screenshot.
- **#24: the Download window stutters while a download is captured from the
  browser.** Not reproduced yet; needs the site it happens on.

## Browser stores

- **Edge Add-ons: first submission.** Register in Microsoft Partner Center (free)
  and submit `DownloadCraft-Edge-<version>.zip` (the Chrome package without the
  pinned `key`). Then:
  - add the ID Edge assigns to `STORE_EXTENSION_IDS` in `src/app.rs`, or the
    native host will not answer the store copy;
  - add the GitHub secrets `EDGE_PRODUCT_ID`, `EDGE_CLIENT_ID` and
    `EDGE_API_KEY`, so each release tag publishes the update
    (`publish-stores` in `.github/workflows/portable.yml`).
- **Firefox Add-ons: automatic updates.** Add the GitHub secrets `AMO_JWT_ISSUER`
  and `AMO_JWT_SECRET` (addons.mozilla.org → Developer Hub → Manage API Keys).
- **Chrome Web Store** (one-time USD 5 registration) if wanted.

## Speed

- **Benchmark against MDM** on the same files and servers, and record the numbers.
- More connections by default (8 now) when a server allows them.
- Keep a connection's TCP/TLS session when its part is split, instead of
  opening a new one for the next piece.
- Less locking per received chunk (one shared map lock today), and socket
  receive buffers sized for fast links.

## Smaller items

- **The command line cannot download a torrent while the app runs:** both want
  the same DHT port ("Only one usage of each socket address"). The CLI should
  hand torrents to the running app, or use another port.
- **macOS icon:** `assets/icon.png` is 256×256, so the largest Dock and Finder
  sizes are soft. A 1024×1024 source would fix it.
- **Not yet checked in the running app:** the four layout thumbnails in
  Settings → Appearance at small widths, the torrent peer table's Client column,
  the restore icon of a maximized window, and the search box when focused.
