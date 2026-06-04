# tokenhp

A macOS menu-bar app that turns your Claude Code usage into HP/MP/EXP bars.

- **Live quota** — the same 5-hour / weekly / extra-usage numbers Claude Code's
  `/usage` shows, pulled live from Anthropic's OAuth usage endpoint.
- **Local activity** — per-exact-model token totals (input / output / cache
  read / cache write) over a switchable **24h / 7d / 30d** window, with USD
  cost computed from a bundled pricing table that covers every current Claude
  model and a handful of common third-party models (Kimi, DeepSeek, MiniMax,
  Doubao). Click any model in the picker to inspect it; the picker defaults
  to whoever you used most.

## Install

1. Grab the latest `HPBar.zip` from
   [Releases](https://github.com/MadCreeper/tokenhp/releases) and unzip it.
2. The build isn't notarized (no paid Apple Developer Program), so macOS
   Gatekeeper will block it on first launch. Either:
   - **Right-click → Open**, confirm the dialog, _or_
   - run `xattr -dr com.apple.quarantine HPBar.app` once.
3. Launch `HPBar.app`. The first read of Claude Code's Keychain item will
   prompt for your login password — click **Always Allow**.

The bolt icon will appear in your menu bar.

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

The user file is _merged_ on top of the bundled defaults — anything you don't
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

```
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
