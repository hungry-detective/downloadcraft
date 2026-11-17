# Store listing: Send to Download Craft

The text and answers to paste into each browser's extension store. The extension is the folder `browser-extension/`; the Firefox build comes from `browser-extension/build-firefox.ps1`.

## Short description (132 characters max)

> Send downloads and videos to Download Craft, the free open-source download manager. It can also download them right in the browser.

## Full description

Download Craft is a free, open-source (MIT) download manager for Windows, macOS and Linux. This extension connects it to your browser:

- **Captures downloads.** Files you download go to Download Craft instead of the browser. It splits each file over up to 32 connections, resumes after a cut or a restart, and can refresh an expired link.
- **Saves videos.** A button appears on pages that play video, including YouTube, Vimeo, HLS and DASH streams. Pick a quality and the app downloads and merges it.
- **Works without the app.** If Download Craft is not installed or not running, the extension downloads the file or video itself, in the browser.
- **Right-click menu.** Download a link, every link on a page, or the selected links with Download Craft.
- **Keeps your session.** It passes the page's cookies, referrer and headers to the app, so downloads that need a login work like they do in the browser. These stay on your computer.

No account, no ads, no tracking. The source code is at https://github.com/hungry-detective/downloadcraft.

## Category

Productivity (Chrome / Edge / Opera); Download Management (Firefox).

## Single purpose (Chrome Web Store)

> Hand browser downloads and page videos to the Download Craft desktop app, or download them in the browser when the app is not available.

## Permission justifications

| Permission | Why it is needed |
|---|---|
| `nativeMessaging` | Talks to the Download Craft desktop app on the same computer. This is the extension's main job. |
| `downloads`, `downloads.open`, `downloads.ui` | Notices new browser downloads so they can be handed to the app. Starts browser downloads itself when the app is not available, and opens finished files. Hides the browser's download bar when the app took over. |
| `contextMenus` | Adds the "Download with Download Craft" right-click items. |
| `cookies` | Reads the cookies of the site a download comes from, so the app can download files that need a login. The cookies go only to the local app, never to a server. |
| `webRequest` | Reads the headers and server address of download and media requests, so the app's request matches the browser's. Also finds video streams (HLS/DASH manifests) as the page loads them. Nothing is blocked or changed. |
| `proxy` | Reads (does not change) the browser's proxy setting, so the app uses the same proxy for a handed-off download. |
| `scripting` | Adds the video button to tabs that were already open when the extension was installed or updated. |
| `tabs` | Reads the current tab's URL and title for the right-click menu, the popup's link list, and file naming. |
| `offscreen` | Joins the video and audio parts of an in-browser video download into one file. |
| `storage` | Saves the extension's settings. |
| `notifications` | Says when an in-browser download finished or failed. |
| Host access `http://*/*`, `https://*/*` | Downloads and videos can come from any site. The content scripts look for video players and download links on the page; they do not read or send page content anywhere else. |

## Data use (Chrome "Privacy practices" tab)

- **Data collected:** none. The extension sends nothing to any server run by the project.
- **Website content and authentication information:** the URL, cookies and request headers of a download are passed to the Download Craft app **on the same computer** through native messaging, only to perform that download.
- Certify: no sale of data, no use beyond the single purpose, no use for credit decisions.

## Privacy policy (paste as a page or into the listing)

> Send to Download Craft does not collect, store or send any personal data to the developers or to third parties. When you download a file, the extension passes the file's address, the site's cookies and the request headers to the Download Craft program installed on your own computer, so it can download that file the way your browser would. This data never leaves your computer except in the download request itself, which goes to the website you were downloading from. Settings are saved in your browser's extension storage. The extension has no analytics and no ads.

## Assets

- Icon: `browser-extension/icons/icon.png` (128×128; Edge and Opera also take 300×300, so export it again at that size).
- Screenshots (1280×800 or 640×400): crop from `docs/screenshot-extension-popup.png`, `docs/screenshot-video-bar.jpg`, `docs/screenshot-extension-downloads.png`, `docs/screenshot-main-window.png`.

## Per store

### Chrome Web Store
- Upload a ZIP of `browser-extension/` **without** the `*.test.js` files, `testdata/`, `manifest-firefox.json` or `build-firefox.ps1`.
- The `key` in `manifest.json` fixes the extension ID that the app's native host allows (`allowed_origins`). The Web Store keeps an uploaded item's ID only if the same key is used. Once the item exists, check that its ID matches `EXTENSION_ID` in the app.

### Microsoft Edge Add-ons
- Same ZIP. Edge gives the item its own ID: add that ID to `STORE_EXTENSION_IDS` in `src/app.rs` (the native host then allows it) and ship that app build before the listing goes live.

### Opera Add-ons
- Same ZIP. Opera reads the native host from Chrome's registry key, which the app registers at every start, so no extra install step is needed. Opera also assigns its own ID: add it to `STORE_EXTENSION_IDS` as for Edge.
- Opera users can also install the Chrome Web Store version through "Install Chrome Extensions".

### Firefox (AMO)
- Build with `scripts/package_extensions.py` and upload the `.xpi`. The add-on ID is `send-to-download-craft@hungry-detective.github.io` (`browser_specific_settings.gecko.id` in `manifest-firefox.json`); the app's Firefox host allows it and the older `send-to-download-craft@example` of self-installed builds (`FIREFOX_EXTENSION_IDS` in `src/app.rs`).
- AMO asks for the source of minified code. None of the code is minified, so upload as is.

## Review notes (for the reviewer field)

> The extension needs the Download Craft desktop app (free, MIT, https://github.com/hungry-detective/downloadcraft) for its main feature. Without the app it still works: downloads and videos are saved by the browser itself. To test: install the extension, open any page with a video or a file link, and use the button or the right-click menu.
