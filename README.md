# tokenhp

> **Status: alpha.** Works for me, but it's a side project — expect rough
> edges and breaking changes. Read [the limitations](#caveats) before
> relying on it.

A macOS menu-bar app that turns your Claude Code usage into HP/MP/EXP bars.

- **Live quota** — the same 5-hour / weekly / extra-usage numbers Claude Code's
  `/usage` shows, pulled live from Anthropic's OAuth usage endpoint.
- **Local activity** — per-exact-model token totals (input / output / cache
  read / cache write) over a switchable **24h / 7d / 30d** window, with USD
  cost computed from a bundled pricing table that covers every current Claude
  model and a handful of common third-party models (Kimi, DeepSeek, MiniMax,
  Doubao). Click any model in the picker to inspect it; the picker defaults
  to whoever you used most.

## Install (macOS 14 Sonoma / 15 Sequoia)

1. **Download.** Get `HPBar.zip` from the latest
   [Release](https://github.com/MadCreeper/tokenhp/releases) — double-click
   to unzip.

2. **Move to `/Applications`.** Drag `HPBar.app` into your Applications
   folder. (Optional but recommended.)

3. **Bypass Gatekeeper.** This build isn't notarized (see [Caveats](#caveats)),
   so macOS will refuse to open it the first time. Pick one:

   **Option A — Terminal (one line, works on every macOS version):**

   ```bash
   xattr -dr com.apple.quarantine /Applications/HPBar.app
   ```

   **Option B — System Settings (macOS 15 Sequoia):**

   - Try to open `HPBar.app` from Finder → macOS shows a "blocked" dialog.
     Dismiss it.
   - Open **System Settings → Privacy & Security**, scroll down to the
     **Security** section.
   - Click **"Open Anyway"** next to HPBar, then confirm the follow-up
     prompt with Touch ID / password.

   *(On macOS 14 Sonoma you could also right-click → **Open**. Apple
   removed that shortcut on Sequoia for unsigned apps — use A or B.)*

4. **Launch HPBar.** A ⚡ bolt icon appears in the menu bar. Click it.

5. **First Keychain prompt.** The first time HPBar reads Claude Code's
   stored OAuth token, macOS will ask for your **login password** to
   authorize access. Click **"Always Allow"**. *(See [Caveats](#caveats) —
   this prompt comes back every few hours and there's currently no fix.)*

### Auto-launch on login

System Settings → **General → Login Items → Open at Login** → click `+`,
add `HPBar.app`.

### Updating

1. Quit HPBar (click the menu bar icon → bottom of the menu → **Quit**, or
   just `pkill -x HPBar`).
2. Download the new `HPBar.zip`, replace `HPBar.app` in `/Applications`.
3. Re-run the `xattr` line from step 3 above — the new bundle is
   quarantined too. Then launch.

## Caveats

This is alpha-quality software for personal use:

- **Apple Silicon only.** CI builds for `arm64`; the release zip won't run
  on Intel Macs. [Build from source](#build-from-source) if you need
  x86_64.
- **Requires macOS 14+** (Sonoma or later).
- **The Keychain prompt re-appears every few hours.** Claude Code rotates
  its OAuth token, which rewrites the Keychain item and resets its ACL —
  wiping the "Always Allow" you clicked. Working around this requires
  HPBar to run its own OAuth flow, which is on the maybe-someday list.
- **Non-Anthropic prices are best-effort.** Bundled rates for Kimi /
  DeepSeek / MiniMax / Doubao are looked up from vendor docs and can
  drift. Override via your own `pricing.json` if you have your real
  billed rates.
- **Reads Claude Code's Keychain item directly.** If Anthropic changes how
  the CLI stores credentials, HPBar will break until it's updated.
- **No code signing / no notarization.** No $99/yr Apple Developer
  membership for a side project — hence the one-time Gatekeeper bypass.

## Themes

Open the popover, click the **paintbrush** icon (top right) to switch.

| Theme | Live quota | Local activity |
| --- | --- | --- |
| **Classic** | Continuous green→yellow→red bars that drain as quota is consumed | Neutral accent-color magnitude bars |
| **Minecraft** | 10 pixel hearts per quota, half-empty mid-cell (the 8th heart at 76%) | XP-bar style: dark slate track, bright green fill, segmented, level number in pixel font overlapping the bar top |

The Minecraft theme uses the **Press Start 2P** pixel font (OFL licensed,
bundled). The heart and XP-bar sprites are drawn programmatically in
SwiftUI Canvas — no Mojang textures shipped.

## Pricing customization

All model prices live in JSON. Defaults ship in
[`HPBarKit/Sources/HPBarKit/Resources/pricing.json`](HPBarKit/Sources/HPBarKit/Resources/pricing.json).
To override an entry or add a new model without rebuilding, drop a JSON file
at `~/Library/Application Support/HPBar/pricing.json`:

```json
{
  "claude-opus-4-8": { "input": 4.5, "output": 22 },
  "minimax-m3.0": {
    "input": 0.35, "output": 1.3,
    "cache_read": 0.07, "cache_create": 0.45
  }
}
```

All prices are USD per million tokens. Fields:

- `input`, `output` — required
- `cache_read` — defaults to 0 if omitted
- `cache_create` — Anthropic's 5-minute cache write rate (or the only cache
  write rate for vendors without a TTL distinction)
- `cache_create_1h` — optional, only Anthropic charges separately for 1-hour
  cache writes (2× input). If omitted, 1-hour writes use `cache_create`.

The user file is *merged* on top of the bundled defaults — anything you don't
override stays at the built-in value.

Restart the app to pick up changes.

## Build from source

Requires Xcode 16+ (Swift 6) and [xcodegen](https://github.com/yonaskolb/XcodeGen):

```bash
brew install xcodegen
git clone https://github.com/MadCreeper/tokenhp.git
cd tokenhp
./run.sh              # kills old instances, generates the project, builds, launches
```

For a release build:

```bash
xcodegen generate
xcodebuild -project HPBar.xcodeproj -scheme HPBar -configuration Release build
```

To run tests:

```bash
swift test --package-path HPBarKit
```

## Architecture

```text
HPBarKit/Sources/HPBarKit/
├── HealthBar.swift           # thin View that delegates to the theme
├── HealthBarTheme.swift      # protocol: Classic + Neutral + Minecraft variants
├── HealthBarStyle.swift      # continuous green→yellow→red color ramp
├── MinecraftThemes.swift     # 7×7 pixel-heart Canvas + segmented XP bar
├── Resources/
│   ├── pricing.json          # bundled price table
│   └── PressStart2P-Regular.ttf
└── Services/
    ├── ClaudeCredentials.swift  # Keychain reader for Claude Code's OAuth token
    ├── CredentialProvider.swift # in-memory cache to limit Keychain reads
    ├── OAuthUsageDataSource.swift  # /api/oauth/usage client
    ├── LocalStatsDataSource.swift  # scans ~/.claude/projects/**/*.jsonl
    ├── Pricing.swift             # JSON loader with user-file overlay
    ├── FontRegistry.swift        # registers bundled pixel font at startup
    ├── UsageReport.swift         # report models (windows vs per-model)
    └── UsageViewModel.swift      # @Observable VM with per-tab/window cache

HPBar/
└── HPBarApp.swift            # MenuBarExtra scene + popover
```

## CI / Releases

CI runs on every push and PR (`macos-15` runner): runs unit tests, generates
the Xcode project with xcodegen, builds the app ad-hoc signed, and uploads
the zip as an artifact.

A `v*` tag (e.g. `v0.1.0`) additionally creates a GitHub Release with
`HPBar.zip` attached. Tag a version locally and push:

```bash
git tag v0.1.0
git push origin v0.1.0
```

## License

No license declared yet. The bundled Press Start 2P font is OFL-1.1.
