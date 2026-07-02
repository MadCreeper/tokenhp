//! Self-update via GitHub Releases (no signing infrastructure).
//!
//! The frontend can't fetch GitHub directly — the popover's CSP is `default-src
//! 'self'` — so the HTTP work lives here. We list the repo's releases, pick the
//! newest one eligible for the chosen channel, and (if it beats the running
//! build) hand back the platform-specific installer asset. "Install" means
//! download the asset and open it with the OS: the builds are unsigned, so a
//! silent in-place replace isn't safe/possible without an Apple/Windows cert and
//! Tauri's signed-updater manifest. The user finishes the install from the
//! mounted .dmg / .msi / .AppImage, the same as a first install.
//!
//! Channels are inclusive, matching the usual alpha/beta/stable convention:
//!   stable → final releases only        (vX.Y.Z)
//!   beta   → beta + stable              (…-beta, vX.Y.Z)
//!   alpha  → alpha + beta + stable      (…-alpha, …-beta, vX.Y.Z)
//! The channel is derived from the tag suffix (see `pre_rank`).

use serde::{Deserialize, Serialize};

const REPO: &str = "MadCreeper/tokenhp";
const USER_AGENT: &str = "HPBar-Updater";

/// The OS-specific installer extension we prefer, most-preferred first. macOS
/// builds are universal; the Windows/Linux CI builds are x86_64 only.
#[cfg(target_os = "macos")]
const ASSET_EXTS: &[&str] = &[".dmg", ".app.tar.gz"];
#[cfg(target_os = "windows")]
const ASSET_EXTS: &[&str] = &[".msi", "-setup.exe", ".exe"];
#[cfg(target_os = "linux")]
const ASSET_EXTS: &[&str] = &[".AppImage", ".deb", ".rpm"];

/// What the Update section renders. snake_case to match the other DTOs.
#[derive(Serialize)]
pub struct UpdateInfo {
    /// The running build's version (Cargo package version).
    current: String,
    /// The chosen release's version, display form (tag without the leading `v`).
    latest: String,
    /// The raw tag, e.g. `v0.5.1-beta`.
    latest_tag: String,
    /// True when `latest` is newer than `current` for this channel.
    available: bool,
    /// Echoed back so the UI can label the result.
    channel: String,
    /// How many releases are eligible for this channel (inclusive).
    count: u32,
    /// The release body (changelog), as Markdown text.
    notes: String,
    /// The release page on GitHub (fallback when there's no platform asset).
    html_url: String,
    /// ISO-8601 publish time, or empty.
    published_at: String,
    /// The installer asset for this platform, if the release has one.
    asset_name: String,
    asset_url: String,
    asset_size: u64,
    /// False when an update exists but ships no installer for this OS.
    has_asset: bool,
}

// --- GitHub API shapes (only the fields we use) ------------------------------

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    html_url: String,
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

// --- version + channel ordering ----------------------------------------------

/// Rank a tag's prerelease suffix so a plain release sorts above its prereleases
/// (stable > beta > alpha), and unknown/old tag shapes sort below everything.
fn pre_rank(pre: &str) -> u8 {
    if pre.is_empty() {
        3 // final release
    } else if pre.starts_with("beta") {
        2
    } else if pre.starts_with("alpha") {
        1
    } else {
        0 // rc/dev/legacy — never eligible for a named channel
    }
}

/// Parse a tag like `v0.5.1-beta` (or `tauri-v0.1.0`) into a comparable tuple.
/// Returns None for tags we can't make sense of, which excludes them.
fn parse_ver(tag: &str) -> Option<(u64, u64, u64, u8)> {
    let t = tag.trim_start_matches("tauri-").trim_start_matches('v');
    let (core, pre) = t.split_once('-').unwrap_or((t, ""));
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch, pre_rank(pre)))
}

/// The lowest prerelease rank a channel accepts (inclusive): stable→3, beta→2,
/// alpha→1. A release is eligible when its own rank is >= this.
fn channel_floor(channel: &str) -> u8 {
    match channel {
        "alpha" => 1,
        "beta" => 2,
        _ => 3, // "stable" and anything unexpected → final releases only
    }
}

// --- commands ----------------------------------------------------------------

/// The running build's version string (e.g. "0.5.1").
#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// List releases, pick the newest one in `channel`, and report whether it beats
/// the running build. Errors (network, rate limit, no eligible release) come
/// back as human-readable strings the UI shows verbatim.
#[tauri::command]
pub async fn check_update(channel: String) -> Result<UpdateInfo, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=40");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    match resp.status().as_u16() {
        200 => {}
        403 => return Err("GitHub rate limit reached — try again in a little while.".into()),
        404 => return Err("Release feed not found.".into()),
        code => return Err(format!("GitHub returned HTTP {code}.")),
    }

    let releases: Vec<GhRelease> = resp
        .json()
        .await
        .map_err(|_| "Couldn't read the release feed.".to_string())?;

    let floor = channel_floor(&channel);
    // Eligible releases: parseable tag, not a draft, rank within the channel.
    let eligible: Vec<((u64, u64, u64, u8), GhRelease)> = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| parse_ver(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| v.3 >= floor)
        .collect();
    let count = eligible.len() as u32;

    // Newest eligible release drives the "install" decision.
    let (latest_ver, release) = eligible
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .ok_or_else(|| format!("No releases found for the {channel} channel."))?;

    let current = env!("CARGO_PKG_VERSION").to_string();
    let current_ver = parse_ver(&current).unwrap_or((0, 0, 0, 3));
    let available = latest_ver > current_ver;

    // Pick the best-matching installer asset for this platform, by extension
    // preference order. macOS .app.tar.gz only matches once .dmg is ruled out.
    let asset = ASSET_EXTS.iter().find_map(|ext| {
        release
            .assets
            .iter()
            .find(|a| a.name.to_lowercase().ends_with(&ext.to_lowercase()))
    });

    Ok(UpdateInfo {
        current,
        latest: release.tag_name.trim_start_matches('v').to_string(),
        latest_tag: release.tag_name.clone(),
        available,
        channel,
        count,
        notes: release.body.unwrap_or_default(),
        html_url: release.html_url,
        published_at: release.published_at.unwrap_or_default(),
        asset_name: asset.map(|a| a.name.clone()).unwrap_or_default(),
        asset_url: asset.map(|a| a.browser_download_url.clone()).unwrap_or_default(),
        asset_size: asset.map(|a| a.size).unwrap_or(0),
        has_asset: asset.is_some(),
    })
}

/// Download an installer asset to the Downloads folder and open it with the OS
/// (mounts the .dmg / launches the .msi / opens the .AppImage). Returns the
/// saved path so the UI can tell the user where it landed.
#[tauri::command]
pub async fn download_and_install_update(url: String, name: String) -> Result<String, String> {
    if url.is_empty() {
        return Err("No installer to download.".into());
    }
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}.", resp.status().as_u16()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;

    // Sanitize the asset name to a bare filename so a crafted release can't
    // write outside Downloads.
    let file_name = std::path::Path::new(&name)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Bad asset name.".to_string())?;
    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| "Couldn't find a Downloads folder.".to_string())?;
    let path = dir.join(&file_name);
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| format!("Couldn't save the download: {e}"))?;

    os_open(&path.to_string_lossy())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Open a URL (or file path) in the user's default app/browser. Used by the
/// About links and to launch a downloaded installer.
#[tauri::command]
pub fn open_external(target: String) -> Result<(), String> {
    os_open(&target)
}

/// Hand a URL or path to the OS opener. Spawned (not waited on) so the popover
/// stays responsive while Finder/Explorer/the browser comes up.
fn os_open(target: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(target);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        // Empty "" is start's window-title arg, so a quoted target isn't eaten.
        c.args(["/C", "start", "", target]);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(target);
        c
    };
    cmd.spawn().map_err(|e| format!("Couldn't open it: {e}"))?;
    Ok(())
}
