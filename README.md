<div align="center">

# ♥ HPBar

**Your Claude subscription usage, as a health bar in the menu bar / system tray.**

**把你的 Claude 订阅用量，做成菜单栏 / 系统托盘里的一条血条。**

Cross-platform (macOS · Linux · Windows) · Tauri 2 · tiny (~6 MB)

**English** · [中文](#zh)

<br/>

<table>
  <tr>
    <td align="center"><b>Minecraft</b></td>
    <td align="center"><b>Classic</b></td>
    <td align="center"><b>Arknights</b></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/minecraft-live.png" width="230" alt="Minecraft · live quota"/></td>
    <td><img src="docs/screenshots/classic-live.png" width="230" alt="Classic · live quota"/></td>
    <td><img src="docs/screenshots/arknights-live.png" width="230" alt="Arknights · live quota"/></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/minecraft-local.png" width="230" alt="Minecraft · local activity"/></td>
    <td><img src="docs/screenshots/classic-local.png" width="230" alt="Classic · local activity"/></td>
    <td><img src="docs/screenshots/arknights-local.png" width="230" alt="Arknights · local activity"/></td>
  </tr>
</table>

<sub>Top: live quota — draining Minecraft hearts · classic green→red HP ramp · Arknights 理智 / 源石 readout.<br/>Bottom: local activity — per-model token &amp; cost breakdown. Switch themes any time.</sub>

</div>

---

<a id="en"></a>

## ✨ Features

- **Lives in the menu bar / tray.** A ♥ icon; click for the popover, right-click for the menu. No window, no Dock icon.
- **A living menu-bar heart** — the tray ♥ *is* the gauge: it drains with your most-depleted live window, **in whichever theme you've picked** (Minecraft red · Classic's green→amber→red HP ramp · Arknights 理智 azure), so you read your HP at a glance without clicking. When you're low it also shows the exact `%` beside it (macOS), which stays legible on any wallpaper.
- **Live quota** — your 5-hour, weekly, and extra-usage windows, with reset countdowns.
- **Burn-rate warning** — when your recent pace would exhaust a window *before* it resets, the bar flags **⚠ hits limit in ~35m**, so a surprise rate-limit doesn't catch you mid-task.
- **Quota alerts** — an opt-out native notification the first time a window goes low / critical (toggle in the tray menu).
- **Local activity** — per-model token + cost breakdown from your local Claude Code session logs (`~/.claude/projects`), over 24h / 7d / 30d.
- **Three switchable themes**, remembered across launches:
  | Theme | Live quota | Local activity |
  |-------|-----------|----------------|
  | 🟩 **Minecraft** | draining pixel hearts | Bedrock-style XP bars |
  | 🍏 **Classic** | green→yellow→red HP ramp | neutral magnitude bars |
  | 🎮 **Arknights** | 理智 (Sanity) hero plate · 源石 · 合成玉 | skewed game-UI bars |
- **Open at Login** toggle in the tray menu.
- **Tiny & native** — uses each OS's built-in webview (no bundled Chromium), so installers are ~3–6 MB and idle RAM stays near a native app's.

## 📦 Install

Pre-built bundles are attached to each [release](../../releases) (tags like `tauri-v0.1.1`). They are **unsigned**, so every OS shows an "unidentified developer" warning on first launch — how to get past each is below.

HPBar reads the token from your existing Claude Code login, so **sign in with the Claude Code CLI first** (`claude`). Then look for the ♥ in your menu bar / tray.

<details open>
<summary><b>macOS</b> — <code>.dmg</code></summary>

1. Download `HPBar_<version>_universal.dmg` (Intel + Apple Silicon), open it, drag **HPBar** to Applications.
2. First launch is blocked by Gatekeeper (unsigned). Do one of:
   - **Right-click** HPBar.app → **Open** → **Open**, or
   - `xattr -dr com.apple.quarantine /Applications/HPBar.app`, or
   - after the block: System Settings → Privacy & Security → **Open Anyway**.
3. **On notched Macs**, a crowded menu bar can hide the ♥ *behind the notch* — hold **⌘ and drag** menu-bar icons to reorder (or use [Ice](https://github.com/jordanbaird/Ice) / Bartender) to surface it.

</details>

<details>
<summary><b>Linux</b> — <code>.deb</code> / <code>.rpm</code> / <code>.AppImage</code></summary>

Needs a webkit2gtk 4.1 runtime and an AppIndicator-style tray.

```sh
# Debian / Ubuntu
sudo apt install ./HPBar_<version>_amd64.deb
# (deps if missing) sudo apt install libwebkit2gtk-4.1-0 libayatana-appindicator3-1

# Fedora / RHEL
sudo dnf install ./HPBar-<version>-1.x86_64.rpm

# AppImage (any distro)
chmod +x HPBar_<version>_amd64.AppImage && ./HPBar_<version>_amd64.AppImage
```

GNOME needs the [AppIndicator extension](https://extensions.gnome.org/extension/615/appindicator-support/) for the tray icon. Token is read from `~/.claude/.credentials.json` — **no prompt**.

</details>

<details>
<summary><b>Windows</b> — <code>.msi</code> / <code>.exe</code></summary>

1. Run `HPBar_<version>_x64-setup.exe` (NSIS) or `HPBar_<version>_x64_en-US.msi`.
2. Unsigned → SmartScreen shows "Windows protected your PC": **More info → Run anyway**.
3. Needs the **WebView2 runtime** — preinstalled on Windows 11; the installer pulls it in on Windows 10.

Token is read from `%USERPROFILE%\.claude\.credentials.json` — **no prompt**.

</details>

## 🔑 Where the token comes from

| OS | Source | Prompt? |
|----|--------|---------|
| macOS | Keychain item `Claude Code-credentials` (via `keyring`) | once per token rotation |
| Linux / Windows | `~/.claude/.credentials.json` plaintext file | **never** |

HPBar never logs in itself — it reuses your Claude Code login and only reads it.

## 🛠 Develop

Prereqs: Node + npm, and the Rust toolchain (`rustup`).

```sh
npm install
npm run tauri dev      # tray app with hot-reload
npm run tauri build    # .dmg / .deb+.rpm+.AppImage / .msi+.exe
```

If `cargo` downloads fail with an HTTP/2 framing error: `CARGO_HTTP_MULTIPLEXING=false npm run tauri dev`.
Headless data-path checks (no GUI): `cargo run --example check` (live) · `cargo run --example local_check` (local).

**Theme showcase / mock mode** — preview every theme with canned data; no Keychain, no network, no Tauri:

```sh
npm run dev    # vite only, then open in a browser:
```

- Gallery of all themes × tabs: <http://localhost:1420/showcase.html>
- A single panel: `http://localhost:1420/?mock=1&theme=arknights&source=local`
  (`theme` = `minecraft` \| `classic` \| `arknights`, `source` = `live` \| `local`)

These screenshots were captured this way.

<details>
<summary>Project layout</summary>

```
src/                    Frontend (vanilla TS + Vite)
  hearts.ts             Minecraft hearts (pixel-grid SVG)
  xpbar.ts              Minecraft XP bars
  classicbar.ts         Classic HP/neutral bars
  arknights.ts          理智/源石/合成玉 rows + sanity hero plate
  theme.ts              theme state (persisted, body.theme-<id>)
  mock.ts               canned data for the showcase/test mode
  main.ts               state, fetch, render dispatch
public/ak/              Arknights icon assets
showcase.html           theme gallery (mock mode)
src-tauri/src/
  credentials.rs        read Claude Code token (Keychain / file)
  usage.rs              GET /api/oauth/usage
  localstats.rs         scan ~/.claude/projects + pricing.rs
  heart_icon.rs         rasterise the live-HP tray heart (RGBA, no deps)
  burn.rs               burn-rate → "hits limit before reset?" (pure math)
  ambient.rs            background poll: live tray icon + % title + alerts + history
  lib.rs                tray, popover window, commands
  examples/tray_preview.rs   dev aid: render the tray heart at every level to a PNG
```

</details>

## 📝 Status

Three themes complete and verified on macOS; Linux/Windows bundles build in CI. Not yet done: code signing/notarization. The Arknights icon assets are sourced from community wikis and bundled for this app.

**Platform notes for the live heart.** The draining/colour-changing tray icon, quota alerts, and the burn-rate warning are cross-platform. Two pieces are macOS-specific by design: the inline `%` annunciator (the tray *title* is unsupported on Windows and inconsistent on Linux) and template handling. The trade-off works out — only macOS's menu-bar vibrancy can wash the icon's hue, and that's exactly where the `%` title fills in; Windows and Linux render the heart's colour faithfully. The icon + alerts are verified on macOS; the Win/Linux paths are correct-by-construction (Tauri no-ops the macOS-only calls) but not yet run on those OSes.

---
<br/>

<a id="zh"></a>

<div align="center">

# ♥ HPBar · 中文

**把你的 Claude 订阅用量，做成菜单栏 / 系统托盘里的一条血条。**

跨平台（macOS · Linux · Windows）· 基于 Tauri 2 · 体积小（约 6 MB）

[English](#en) · **中文**

</div>

## ✨ 功能

- **常驻菜单栏 / 托盘**：一个 ♥ 图标，左键点开弹窗，右键打开菜单；没有窗口，也没有 Dock 图标。
- **会动的菜单栏血条**：托盘上的 ♥ 本身就是血条——它会随你「最吃紧」的实时额度逐格扣血，并**按你选择的主题着色**（我的世界红 · 经典 绿→黄→红 血条 · 明日方舟 理智蓝），不点开也能一眼看出血量；额度偏低时还会在旁边显示精确的 `%`（macOS），任何壁纸下都清晰可读。
- **实时额度（Live quota）**：5 小时、每周、额外用量三个窗口，并显示重置倒计时。
- **耗尽预警（Burn-rate）**：当按你最近的用量速度、某个窗口会在重置**之前**就被用光时，血条会标出 **⚠ 约 35m 后触顶**，免得任务进行到一半被突然限流。
- **额度提醒（Quota alerts）**：窗口首次进入「偏低 / 临界」时弹一条系统通知（默认开启，托盘菜单可关）。
- **本地活动（Local activity）**：从本地 Claude Code 会话日志（`~/.claude/projects`）统计每个模型的 token 用量与花费，支持 24 小时 / 7 天 / 30 天。
- **三种可切换主题**（自动记住上次选择）：
  | 主题 | 实时额度 | 本地活动 |
  |------|---------|---------|
  | 🟩 **我的世界** | 逐格扣血的像素红心 | 基岩版风格经验条 |
  | 🍏 **经典** | 绿→黄→红 血条渐变 | 单色用量条 |
  | 🎮 **明日方舟** | 理智读数 · 源石 · 合成玉 | 斜切游戏 UI 进度条 |
- 托盘菜单内可开启 **开机自启**。
- **小巧且原生**：复用各系统自带的 webview（不打包 Chromium），安装包仅约 3–6 MB，空闲内存接近原生应用。

## 📦 安装

每个 [release](../../releases) 都附带预编译安装包（标签形如 `tauri-v0.1.1`）。安装包**未签名**，首次启动时系统会提示「未受信任的开发者」，各平台绕过方法见下。

HPBar 复用你已有的 Claude Code 登录，请**先用 Claude Code CLI 登录**（`claude`），随后在菜单栏 / 托盘找到 ♥ 图标。

<details open>
<summary><b>macOS</b> — <code>.dmg</code></summary>

1. 下载 `HPBar_<version>_universal.dmg`（同时支持 Intel 与 Apple 芯片），打开后把 **HPBar** 拖入「应用程序」。
2. 首次启动会被 Gatekeeper 拦截（未签名），任选其一：
   - **右键**点 HPBar.app → **打开** → **打开**；或
   - 终端执行 `xattr -dr com.apple.quarantine /Applications/HPBar.app`；或
   - 被拦截后到「系统设置 → 隐私与安全性」点 **仍要打开**。
3. **刘海屏 Mac**：菜单栏图标过多时 ♥ 可能被藏在*刘海后面* —— 按住 **⌘ 拖动**菜单栏图标重新排列即可让它显示出来（或用 [Ice](https://github.com/jordanbaird/Ice) / Bartender 管理）。

</details>

<details>
<summary><b>Linux</b> — <code>.deb</code> / <code>.rpm</code> / <code>.AppImage</code></summary>

需要 webkit2gtk 4.1 运行库，以及支持 AppIndicator 的托盘。

```sh
# Debian / Ubuntu
sudo apt install ./HPBar_<version>_amd64.deb

# Fedora / RHEL
sudo dnf install ./HPBar-<version>-1.x86_64.rpm

# AppImage（任意发行版）
chmod +x HPBar_<version>_amd64.AppImage && ./HPBar_<version>_amd64.AppImage
```

GNOME 需要安装 [AppIndicator 扩展](https://extensions.gnome.org/extension/615/appindicator-support/)才会显示托盘图标。令牌从 `~/.claude/.credentials.json` 读取，**不会弹窗**。

</details>

<details>
<summary><b>Windows</b> — <code>.msi</code> / <code>.exe</code></summary>

1. 运行 `HPBar_<version>_x64-setup.exe`（NSIS）或 `HPBar_<version>_x64_en-US.msi`。
2. 未签名 → SmartScreen 提示「Windows 已保护你的电脑」：点 **更多信息 → 仍要运行**。
3. 需要 **WebView2 运行时** —— Windows 11 已自带；Windows 10 上安装程序会自动拉取。

令牌从 `%USERPROFILE%\.claude\.credentials.json` 读取，**不会弹窗**。

</details>

## 🔑 令牌来源

| 系统 | 来源 | 是否弹窗 |
|------|------|---------|
| macOS | 钥匙串项 `Claude Code-credentials`（通过 `keyring`） | 每次令牌轮换时一次 |
| Linux / Windows | `~/.claude/.credentials.json` 明文文件 | **从不** |

HPBar 自身不做任何登录，只复用并读取你的 Claude Code 登录信息。

## 🛠 开发

依赖：Node + npm，以及 Rust 工具链（`rustup`）。

```sh
npm install
npm run tauri dev      # 带热重载的托盘应用
npm run tauri build    # 打包各平台安装包
```

若 `cargo` 下载报 HTTP/2 framing 错误，加前缀：`CARGO_HTTP_MULTIPLEXING=false npm run tauri dev`。

**主题预览 / mock 模式** —— 用预置假数据预览所有主题，无需钥匙串、网络或 Tauri：执行 `npm run dev`（仅 vite），然后在浏览器打开 <http://localhost:1420/showcase.html>（所有主题画廊），或 `http://localhost:1420/?mock=1&theme=arknights&source=local` 查看单个面板。本文档的截图即由此生成。

## 📝 状态

三种主题均已完成并在 macOS 上验证；Linux/Windows 安装包由 CI 构建。尚未完成：代码签名 / 公证。明日方舟主题图标来自社区 wiki，随应用一并打包。

**关于「会动的血条」的平台说明。** 会扣血 / 变色的托盘图标、额度提醒、耗尽预警都是跨平台的。有两处是特意只在 macOS 生效：旁边的 `%` 读数（托盘**标题**在 Windows 不支持、在 Linux 表现不一）以及 template（模板图标）处理。这个取舍正好合适——只有 macOS 的菜单栏 vibrancy 会把图标颜色冲淡，而那正是 `%` 读数补位的地方；Windows 与 Linux 会如实显示血条颜色。图标 + 提醒已在 macOS 验证；Win/Linux 路径在代码层面是正确的（Tauri 会把 macOS 专有调用变为空操作），但尚未在这两个系统上实跑。
