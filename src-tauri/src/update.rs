//! Self-update via GitHub Releases (no signing infrastructure).
//!
//! The frontend can't fetch GitHub directly — the popover's CSP is `default-src
//! 'self'` — so the HTTP work lives here. We list the repo's releases, pick the
//! newest one eligible for the chosen channel, and (if it beats the running
//! build) hand back the platform-specific installer asset. "Install" means
//! download the asset and open it with the OS. Automatic in-place replacement
//! is not implemented; the user finishes installing from the downloaded bundle.
//!
//! Channels are inclusive, matching the usual alpha/beta/stable convention:
//!   stable → final releases only        (vX.Y.Z)
//!   beta   → beta + stable              (…-beta, vX.Y.Z)
//!   alpha  → alpha + beta + stable      (…-alpha, …-beta, vX.Y.Z)
//! The channel is derived from the tag suffix (see `pre_rank`).

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const REPO: &str = "MadCreeper/tokenhp";
const USER_AGENT: &str = "HPBar-Updater";

/// What the Update section renders. snake_case to match the other DTOs.
#[derive(Serialize)]
pub struct UpdateInfo {
    /// The release tag's version, falling back to Cargo for local builds.
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
    asset_digest: Option<String>,
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
    #[serde(default)]
    prerelease: bool,
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
    #[serde(default)]
    digest: Option<String>,
}

// --- version + channel ordering ----------------------------------------------

/// Rank a tag's prerelease suffix so a plain release sorts above its prereleases
/// (stable > beta > alpha), and unknown/old tag shapes sort below everything.
fn pre_rank(pre: &str) -> u8 {
    if pre.is_empty() {
        3 // final release
    } else if pre == "beta" || pre.starts_with("beta.") {
        2
    } else if pre == "alpha" || pre.starts_with("alpha.") {
        1
    } else {
        0 // rc/dev/legacy — never eligible for a named channel
    }
}

/// Accept current and legacy tag prefixes, retaining numeric prerelease ordering.
fn parse_ver(tag: &str) -> Option<Version> {
    Version::parse(
        tag.strip_prefix("tauri-")
            .unwrap_or(tag)
            .trim_start_matches('v'),
    )
    .ok()
}

fn installer_exts(os: &str) -> &'static [&'static str] {
    match os {
        "macos" => &[".dmg"],
        "windows" => &[".msi", "-setup.exe", ".exe"],
        "linux" => &[".AppImage", ".deb", ".rpm"],
        _ => &[],
    }
}

fn asset_matches_arch(name: &str, os: &str, arch: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if os == "macos" && name.contains("_universal.") {
        return true;
    }
    let markers: &[&str] = match arch {
        "aarch64" => &["aarch64", "arm64"],
        "x86_64" => &["x86_64", "amd64", "x64"],
        _ => return false,
    };
    let parts: Vec<_> = name
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .collect();
    markers.iter().any(|m| {
        parts.iter().any(|p| {
            *p == *m
                || p.ends_with(&format!("_{m}"))
                || p.starts_with(&format!("{m}_"))
                || p.contains(&format!("_{m}_"))
        })
    })
}

fn select_asset<'a>(assets: &'a [GhAsset], os: &str, arch: &str) -> Option<&'a GhAsset> {
    installer_exts(os).iter().find_map(|ext| {
        assets.iter().find(|a| {
            a.name.starts_with("HPBar")
                && a.name
                    .to_ascii_lowercase()
                    .ends_with(&ext.to_ascii_lowercase())
                && asset_matches_arch(&a.name, os, arch)
        })
    })
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
    option_env!("HPBAR_RELEASE_VERSION")
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_string()
}

/// List releases, pick the newest one in `channel`, and report whether it beats
/// the running build. Errors (network, rate limit, no eligible release) come
/// back as human-readable strings the UI shows verbatim.
#[tauri::command]
pub async fn check_update(channel: String) -> Result<UpdateInfo, String> {
    let releases = fetch_releases().await?;
    release_info(
        releases,
        &channel,
        &app_version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

fn update_client(timeout: Duration) -> Result<reqwest::Client, String> {
    crate::network::client(crate::network::login_shell_proxy().as_deref(), timeout)
        .map_err(|e| format!("Couldn't connect: {e}"))
}

async fn fetch_releases() -> Result<Vec<GhRelease>, String> {
    let client = update_client(Duration::from_secs(30))?;
    let mut releases = Vec::new();
    for page in 1..=10 {
        let resp = client
            .get(format!(
                "https://api.github.com/repos/{REPO}/releases?per_page=100&page={page}"
            ))
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| format!("Network error: {e}"))?;
        match resp.status().as_u16() {
            200 => {}
            403 | 429 => {
                return Err("GitHub rate limit reached — try again in a little while.".into())
            }
            404 => return Err("Release feed not found.".into()),
            code => return Err(format!("GitHub returned HTTP {code}.")),
        }
        let batch: Vec<GhRelease> = resp
            .json()
            .await
            .map_err(|_| "Couldn't read the release feed.".to_string())?;
        let done = batch.len() < 100;
        releases.extend(batch);
        if done {
            return Ok(releases);
        }
    }
    Err("Release feed is too large to check completely.".into())
}

fn release_info(
    releases: Vec<GhRelease>,
    channel: &str,
    current: &str,
    os: &str,
    arch: &str,
) -> Result<UpdateInfo, String> {
    if !matches!(channel, "stable" | "beta" | "alpha") {
        return Err("Unknown update channel.".into());
    }
    let floor = channel_floor(channel);
    // Eligible releases: parseable tag, not a draft, rank within the channel.
    let eligible: Vec<(Version, GhRelease)> = releases
        .into_iter()
        .filter(|r| !r.draft && (channel != "stable" || !r.prerelease))
        .filter_map(|r| parse_ver(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| pre_rank(v.pre.as_str()) >= floor)
        .collect();
    let count = eligible.len() as u32;

    // Newest eligible release drives the "install" decision.
    let (latest_ver, release) = eligible
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .ok_or_else(|| format!("No releases found for the {channel} channel."))?;

    let current_ver = parse_ver(current).ok_or("Invalid running app version.")?;
    let available = latest_ver > current_ver;

    let asset = select_asset(&release.assets, os, arch);

    Ok(UpdateInfo {
        current: current.to_string(),
        latest: latest_ver.to_string(),
        latest_tag: release.tag_name.clone(),
        available,
        channel: channel.to_string(),
        count,
        notes: release.body.unwrap_or_default(),
        html_url: release.html_url,
        published_at: release.published_at.unwrap_or_default(),
        asset_name: asset.map(|a| a.name.clone()).unwrap_or_default(),
        asset_url: asset
            .map(|a| a.browser_download_url.clone())
            .unwrap_or_default(),
        asset_size: asset.map(|a| a.size).unwrap_or(0),
        asset_digest: asset.and_then(|a| a.digest.clone()),
        has_asset: asset.is_some(),
    })
}

/// Download an installer asset to the Downloads folder and open it with the OS
/// (mounts the .dmg / launches the .msi / opens the .AppImage). Returns the
/// saved path so the UI can tell the user where it landed.
#[tauri::command]
pub async fn download_and_install_update(
    url: String,
    name: String,
    size: u64,
    digest: Option<String>,
) -> Result<String, String> {
    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| "Couldn't find a Downloads folder.".to_string())?;
    let asset = GhAsset {
        name,
        browser_download_url: url,
        size,
        digest,
    };
    let path = download_asset(&asset, &dir).await?;
    let target = path.to_string_lossy().into_owned();
    let open_target = target.clone();
    tokio::task::spawn_blocking(move || os_open(&open_target))
        .await
        .map_err(|e| format!("Couldn't open the installer: {e}"))?
        .map_err(|e| format!("{e} Installer saved to {target}."))?;
    Ok(target)
}

fn validate_asset(asset: &GhAsset) -> Result<(), String> {
    let url =
        reqwest::Url::parse(&asset.browser_download_url).map_err(|_| "Invalid installer URL.")?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url
            .path()
            .starts_with(&format!("/{REPO}/releases/download/"))
    {
        return Err("Installer must come from HPBar's GitHub releases.".into());
    }
    if asset.name.contains(['/', '\\'])
        || !asset.name.starts_with("HPBar")
        || !installer_exts(std::env::consts::OS).iter().any(|ext| {
            asset
                .name
                .to_ascii_lowercase()
                .ends_with(&ext.to_ascii_lowercase())
        })
        || !asset_matches_arch(&asset.name, std::env::consts::OS, std::env::consts::ARCH)
    {
        return Err("Invalid installer for this platform.".into());
    }
    if asset.size == 0 || asset.size > 512 * 1024 * 1024 {
        return Err("Invalid installer size.".into());
    }
    Ok(())
}

fn verify_download(asset: &GhAsset, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 != asset.size {
        return Err("Incomplete download — please retry.".into());
    }
    if let Some(digest) = &asset.digest {
        let actual = format!("sha256:{:x}", Sha256::digest(bytes));
        if !actual.eq_ignore_ascii_case(digest) {
            return Err("Installer checksum mismatch — please retry.".into());
        }
    }
    Ok(())
}

async fn download_asset(asset: &GhAsset, dir: &Path) -> Result<PathBuf, String> {
    validate_asset(asset)?;
    let client = update_client(Duration::from_secs(300))?;
    let resp = client
        .get(&asset.browser_download_url)
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
    verify_download(asset, &bytes)?;
    // Each attempt gets its own directory; an existing user's download is never overwritten.
    let staging = dir.join(format!("HPBar-update-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&staging)
        .await
        .map_err(|e| format!("Couldn't save the download: {e}"))?;
    let path = staging.join(&asset.name);
    if let Err(e) = tokio::fs::write(&path, &bytes).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(format!("Couldn't save the download: {e}"));
    }
    #[cfg(target_os = "linux")]
    if asset.name.ends_with(".AppImage") {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|e| format!("Couldn't make the installer executable: {e}"))?;
    }
    Ok(path)
}

/// Open a URL (or file path) in the user's default app/browser. Used by the
/// About links and to launch a downloaded installer.
#[tauri::command]
pub async fn open_external(target: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || os_open(&target))
        .await
        .map_err(|e| format!("Couldn't open it: {e}"))?
}

/// Wait for the OS handoff so a failed opener cannot report success. Installer
/// downloads call this on a blocking worker, keeping the popover responsive.
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
    let status = cmd.status().map_err(|e| format!("Couldn't open it: {e}"))?;
    if !status.success() {
        return Err(format!("Couldn't open the installer or link ({status})."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GhAsset {
        GhAsset {
            name: name.into(),
            browser_download_url: format!(
                "https://github.com/{REPO}/releases/download/v1.0.0/{name}"
            ),
            size: 4,
            digest: Some(format!("sha256:{:x}", Sha256::digest(b"test"))),
        }
    }
    fn release(tag: &str) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            body: None,
            draft: false,
            prerelease: tag.contains('-'),
            html_url: format!("https://github.com/{REPO}/releases/tag/{tag}"),
            published_at: None,
            assets: vec![asset("HPBar_1.0.0_universal.dmg")],
        }
    }
    fn info(tags: &[&str], channel: &str, current: &str) -> UpdateInfo {
        release_info(
            tags.iter().map(|t| release(t)).collect(),
            channel,
            current,
            "macos",
            "aarch64",
        )
        .unwrap()
    }

    #[test]
    fn semantic_versions_preserve_prerelease_sequence_and_final_transition() {
        let result = info(&["v1.0.0-beta.2", "v1.0.0-beta.10"], "beta", "1.0.0-beta.2");
        assert_eq!(result.latest, "1.0.0-beta.10");
        assert!(result.available);
        assert!(info(&["v1.0.0"], "beta", "1.0.0-beta.10").available);
        assert!(!info(&["v1.0.0-beta.10"], "beta", "1.0.0").available);
        assert!(!info(&["v1.0.0"], "stable", "1.0.0").available);
        assert!(!info(&["v1.0.0"], "stable", "1.1.0").available);
        assert!(parse_ver("1.0.0.4").is_none());
        assert_eq!(parse_ver("tauri-v1.2.3").unwrap().to_string(), "1.2.3");
    }

    #[test]
    fn channels_exclude_drafts_unknown_suffixes_and_github_prereleases() {
        let tags = [
            "v1.0.0",
            "v1.1.0-beta",
            "v1.2.0-alpha.1",
            "v9.0.0-betamax",
            "v9.0.0-rc.1",
        ];
        assert_eq!(info(&tags, "stable", "0.1.0").latest, "1.0.0");
        assert_eq!(info(&tags, "beta", "0.1.0").latest, "1.1.0-beta");
        assert_eq!(info(&tags, "alpha", "0.1.0").latest, "1.2.0-alpha.1");
        let mut draft = release("v5.0.0");
        draft.draft = true;
        let mut pre = release("v4.0.0");
        pre.prerelease = true;
        let result = release_info(
            vec![release("v1.0.0"), draft, pre],
            "stable",
            "0.1.0",
            "macos",
            "aarch64",
        )
        .unwrap();
        assert_eq!(result.latest, "1.0.0");
        assert_eq!(result.count, 1);
        assert!(release_info(vec![], "stable", "0.1.0", "macos", "aarch64").is_err());
    }

    #[test]
    fn installers_match_os_architecture_and_preferred_format() {
        let assets = vec![
            asset("HPBar_1.0.0_x64.dmg"),
            asset("HPBar_1.0.0_arm64-setup.exe"),
            asset("HPBar_1.0.0_aarch64.dmg"),
            asset("HPBar_1.0.0_x64-setup.exe"),
            asset("HPBar_1.0.0_amd64.deb"),
            asset("HPBar_1.0.0_amd64.AppImage"),
            asset("HPBar_1.0.0_x64_en-US.msi"),
            asset("HPBar-1.0.0-1.x86_64.rpm"),
        ];
        assert_eq!(
            select_asset(&assets, "macos", "aarch64").unwrap().name,
            "HPBar_1.0.0_aarch64.dmg"
        );
        assert_eq!(
            select_asset(&assets, "windows", "x86_64").unwrap().name,
            "HPBar_1.0.0_x64_en-US.msi"
        );
        assert_eq!(
            select_asset(&assets, "windows", "aarch64").unwrap().name,
            "HPBar_1.0.0_arm64-setup.exe"
        );
        assert_eq!(
            select_asset(&assets, "linux", "x86_64").unwrap().name,
            "HPBar_1.0.0_amd64.AppImage"
        );
        assert!(select_asset(&assets, "linux", "aarch64").is_none());
        assert!(select_asset(&[asset("HPBar_universal.app.tar.gz")], "macos", "aarch64").is_none());
        for arch in ["aarch64", "x86_64"] {
            assert!(select_asset(&[asset("HPBar_1.0.0_universal.dmg")], "macos", arch).is_some());
        }
        let result = release_info(
            vec![release("v1.0.0")],
            "stable",
            "0.1.0",
            "linux",
            "aarch64",
        )
        .unwrap();
        assert!(result.available);
        assert!(!result.has_asset);
    }

    #[test]
    fn rejects_truncated_or_corrupted_downloads() {
        let mut a = asset("HPBar_1.0.0_universal.dmg");
        assert!(verify_download(&a, b"test").is_ok());
        assert!(verify_download(&a, b"tes")
            .unwrap_err()
            .contains("Incomplete"));
        assert!(verify_download(&a, b"oops")
            .unwrap_err()
            .contains("checksum"));
        a.digest = None; // Older GitHub assets do not all expose a digest.
        assert!(verify_download(&a, b"test").is_ok());
        assert!(verify_download(&a, b"").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_non_release_urls_paths_and_wrong_arch_before_downloading() {
        let mut a = asset("HPBar_1.0.0_universal.dmg");
        assert!(validate_asset(&a).is_ok());
        a.browser_download_url =
            "https://github.com.evil.test/MadCreeper/tokenhp/releases/download/v1.0.0/a.dmg".into();
        assert!(validate_asset(&a).is_err());
        a = asset("../HPBar_1.0.0_universal.dmg");
        assert!(validate_asset(&a).is_err());
        a = asset("HPBar_1.0.0_universal.dmg");
        a.size = 0;
        assert!(validate_asset(&a).is_err());
        a = asset("HPBar_1.0.0_universal.dmg");
        a.browser_download_url = "file:///tmp/test.dmg".into();
        assert!(validate_asset(&a).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn failed_os_handoff_is_reported() {
        assert!(os_open(&format!(
            "/tmp/hpbar-nonexistent-{}.dmg",
            uuid::Uuid::new_v4()
        ))
        .is_err());
    }

    /// Real network + installer handoff, deliberately opt-in. Never replaces or
    /// launches HPBar; downloads into a temporary directory and removes it.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "downloads a GitHub release and opens its DMG in Finder"]
    async fn live_release_download_and_mount() {
        let releases = fetch_releases().await.unwrap();
        for channel in ["stable", "beta", "alpha"] {
            let result = check_update(channel.to_string()).await.unwrap();
            eprintln!(
                "{channel}: running {}, latest {}, available {}, asset {}",
                result.current, result.latest, result.available, result.asset_name
            );
            assert!(result.has_asset);
        }
        let selected =
            release_info(releases, "beta", "0.0.0", "macos", std::env::consts::ARCH).unwrap();
        assert!(selected.available);
        let a = GhAsset {
            name: selected.asset_name,
            browser_download_url: selected.asset_url,
            size: selected.asset_size,
            digest: selected.asset_digest,
        };
        let root = std::env::temp_dir().join(format!("hpbar-update-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let mount = self.0.join("mounted");
                let _ = std::process::Command::new("hdiutil")
                    .arg("detach")
                    .arg(&mount)
                    .output();
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let path = download_asset(&a, &root).await.unwrap();
        let result = std::process::Command::new("hdiutil")
            .arg("verify")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let mount = root.join("mounted");
        let result = std::process::Command::new("hdiutil")
            .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
            .arg(&mount)
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let binary = mount.join("HPBar.app/Contents/MacOS/hpbar");
        assert!(binary.is_file());
        let archs = std::process::Command::new("lipo")
            .arg("-archs")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(archs.status.success());
        assert!(String::from_utf8_lossy(&archs.stdout).contains("arm64"));
        os_open(&path.to_string_lossy()).unwrap();
        eprintln!(
            "Verified {} ({} bytes), SHA-256, DMG checksum, universal app, and Finder handoff.",
            a.name, a.size
        );
    }
}
