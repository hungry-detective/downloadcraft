# Download Craft 0.11.10

Use [the current release instructions](RELEASE-NOTES.md) for Windows, universal
Mac, Fedora/RPM, Debian-family DEB and AppImage installation and media helpers.

`VERSION` contains the public app version; Cargo, UI and CLI use it (0.11.10).

The browser extension keeps its own version, raised only when something in
`browser-extension/` changes: `version` in both manifests and `EXT_VER` in
`background.js` and `content.js`, all four the same. An app release that leaves
the extension alone keeps its number, and the release workflow then skips the
Firefox and Edge store uploads (the stores refuse a version they already have).

Build native binaries with Cargo. Package with `scripts/package.py`, or build
Linux glibc-2.28 binaries using cargo-zigbuild and run `scripts/package_linux.py`.
Mac cross-packaging accepts `--signer <rcodesign>` to sign the app's resources as
well as its universal executable. Native Mac packaging uses system codesign.

Paused/failed downloads may retain `.dcmeta` for resuming; successful jobs
remove it. What is still open is listed in [docs/ROADMAP.md](docs/ROADMAP.md).
