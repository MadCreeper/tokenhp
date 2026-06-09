# HPBar (Tauri) — cross-platform port

A cross-platform (macOS / Linux / Windows) rewrite of the macOS-only SwiftUI
HPBar menu-bar app, built on **Tauri 2** (Rust backend + vanilla-TS/Vite
frontend). It shows live Claude subscription quota as draining Minecraft
hearts in the system tray / menu bar.

## Why Tauri

The Swift logic (~700 lines) was already portable, but the UI (menu-bar tray +
SwiftUI hearts) is Apple-only. Tauri gives a single codebase with a real system
tray on all three OSes and tiny binaries (uses the OS-native webview, not a
bundled Chromium like Electron).

## Install

Pre-built bundles are attached to each [release](../../releases) (tags like
`tauri-v0.1.0`). They are **unsigned**, so every OS shows a "this is from an
unidentified developer" warning on first launch — how to get past each is below.

HPBar reads the token from your existing Claude Code login, so **sign in with the
Claude Code CLI first** (`claude` / `claude login`). It then lives in the system
tray / menu bar — click the ♥ for the popover, right-click for the menu.

### macOS (`.dmg`)

1. Download `HPBar_<version>_universal.dmg` (Intel + Apple Silicon), open it,
   and drag **HPBar** to Applications.
2. First launch is blocked by Gatekeeper (unsigned). Do one of:
   - **Right-click** HPBar.app → **Open** → **Open**, or
   - `xattr -dr com.apple.quarantine /Applications/HPBar.app`, or
   - after the block, System Settings → Privacy & Security → **Open Anyway**.
3. The heart appears in the menu bar. **On notched Macs**, a crowded menu bar can
   hide the heart *behind the notch* — if you don't see it, hold **⌘ and drag**
   menu-bar icons to reorder (or use [Ice](https://github.com/jordanbaird/Ice) /
   Bartender) to surface it.

### Linux (`.deb` / `.AppImage`)

Needs a webkit2gtk 4.1 runtime and an AppIndicator-style system tray.

- **Debian / Ubuntu (`.deb`):**
  ```sh
  sudo apt install ./HPBar_<version>_amd64.deb
  # if deps are missing: sudo apt install libwebkit2gtk-4.1-0 libayatana-appindicator3-1
  ```
- **Fedora / RHEL (`.rpm`):**
  ```sh
  sudo dnf install ./HPBar-<version>-1.x86_64.rpm
  ```
- **AppImage (any distro):**
  ```sh
  chmod +x HPBar_<version>_amd64.AppImage
  ./HPBar_<version>_amd64.AppImage
  ```
- **Tray not showing?** GNOME needs the
  [AppIndicator extension](https://extensions.gnome.org/extension/615/appindicator-support/);
  KDE and most others work out of the box.
- The token is read from `~/.claude/.credentials.json` — **no prompt**.

### Windows (`.msi` / `.exe`)

1. Run `HPBar_<version>_x64-setup.exe` (NSIS) or `HPBar_<version>_x64_en-US.msi`.
2. Unsigned → SmartScreen shows "Windows protected your PC": click
   **More info → Run anyway**.
3. Needs the **WebView2 runtime** — preinstalled on Windows 11; the installer
   pulls it in automatically on Windows 10.
4. The token is read from `%USERPROFILE%\.claude\.credentials.json` — **no prompt**.

## Layout

```
tauri/
  index.html, src/        Frontend (vanilla TS + Vite)
    hearts.ts             Pixel-perfect port of HeartPixel / MinecraftHeartsBar (SVG)
    main.ts               Fetches usage, renders bars, listens for the "refresh" event
    styles.css            Minecraft gray panel + Monocraft @font-face
  public/Monocraft.ttf    Bundled pixel font (SIL OFL 1.1)
  src-tauri/
    src/credentials.rs    Reads the Claude Code token (macOS Keychain / file elsewhere)
    src/usage.rs          GET /api/oauth/usage, parses the 5h/weekly/extra windows
    src/lib.rs            Tray icon, popover window, fetch_usage command
    examples/check.rs     Headless data-path smoke test (no GUI)
```

## Credential source per OS

| OS | Where the Claude Code token is read |
|----|--------------------------------------|
| macOS | Keychain item `Claude Code-credentials` (via `keyring`) — prompts once per token rotation, same as the Swift app |
| Linux / Windows | plaintext `~/.claude/.credentials.json` — **no prompt** |

## Develop

Prereqs: Node + npm, and the Rust toolchain (`rustup`).

```sh
npm install
npm run tauri dev      # launches the tray app with hot-reload
```

If `cargo` downloads fail with an HTTP/2 framing error, force HTTP/1.1:

```sh
CARGO_HTTP_MULTIPLEXING=false npm run tauri dev
```

Headless check of the credential + usage path (no GUI, prints live hearts):

```sh
cargo run --manifest-path src-tauri/Cargo.toml --example check
```

## Build a release bundle

```sh
npm run tauri build    # produces a .app / .dmg (macOS), .deb/.AppImage (Linux), .msi (Windows)
```

Cross-platform bundles are also built in CI: push a `tauri-v*` tag (e.g.
`tauri-v0.1.0`) and `.github/workflows/tauri-release.yml` builds macOS
(universal), Linux, and Windows artifacts into a draft prerelease.

## Features

- **Live quota** — 5h / weekly / extra-usage windows as draining Minecraft
  hearts (pixel-exact port of `MinecraftHeartsBar`), with reset captions.
- **Local activity** — per-model token + cost breakdown from
  `~/.claude/projects/**/*.jsonl`, over 24h / 7d / 30d, as Minecraft XP bars
  (port of `LocalStatsDataSource` + `Pricing` + `MinecraftXPBar`). Model picker
  via a stone dropdown.
- **Open at Login** toggle in the tray's right-click menu (off by default).
- Popover anchors under the tray icon and resizes to content height.

## Status

Feature-complete vs. the Swift app's Minecraft theme and verified against live
data on macOS. Not yet done: the "Classic" (non-Minecraft) visual theme, code
signing/notarization, and real-world testing of the Linux/Windows bundles
(they compile via CI but haven't been run on those OSes).

## Regenerating icons

```sh
npm run tauri icon ./app-icon.png   # regenerates src-tauri/icons/*
# tray.png (menu-bar template heart) is generated separately — see the heart pattern in src/hearts.ts
```
