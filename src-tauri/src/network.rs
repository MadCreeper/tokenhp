//! Shared outbound HTTP setup for GUI launches.
//!
//! Finder/launchd applications do not inherit proxy variables exported from a
//! login shell. Callers still try the direct route first and consult this
//! module only after an auth/permission response suggests a route difference.

use std::time::Duration;

pub fn client(proxy_url: Option<&str>, timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder().timeout(timeout);
    if let Some(url) = proxy_url {
        builder = builder.proxy(reqwest::Proxy::all(url)?);
    }
    builder.build()
}

/// Proxy exported by the user's shell, for GUI launches where launchd does not
/// carry shell startup variables. Read-only: never exports, executes, or
/// persists anything and never changes system proxy settings.
#[cfg(target_os = "macos")]
pub fn login_shell_proxy() -> Option<String> {
    for name in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(value) = std::env::var(name) {
            if let Some(url) = proxy_from_output(&value) {
                return Some(url);
            }
        }
    }

    // Do not execute an interactive shell: startup scripts are arbitrary code.
    // Read only simple exported assignments, in zsh's startup order. Later
    // files win, as they do in an actual interactive login shell.
    let home = dirs::home_dir()?;
    let mut proxy = None;
    for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
        let Ok(contents) = std::fs::read_to_string(home.join(name)) else {
            continue;
        };
        for line in contents.lines() {
            if let Some(url) = proxy_from_assignment(line) {
                proxy = Some(url);
            }
        }
    }
    proxy
}

#[cfg(not(target_os = "macos"))]
pub fn login_shell_proxy() -> Option<String> {
    None
}

fn proxy_from_output(output: &str) -> Option<String> {
    output.lines().rev().find_map(|line| {
        let value = line.trim();
        let parsed = reqwest::Url::parse(value).ok()?;
        if matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some() {
            Some(value.to_string())
        } else {
            None
        }
    })
}

fn proxy_from_assignment(line: &str) -> Option<String> {
    let assignment = line.trim().strip_prefix("export ").unwrap_or(line.trim());
    let (name, value) = assignment.split_once('=')?;
    if !matches!(
        name.trim(),
        "HTTPS_PROXY" | "https_proxy" | "ALL_PROXY" | "all_proxy"
    ) {
        return None;
    }
    let value = value.trim().trim_matches(['"', '\'']);
    proxy_from_output(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_proxy_output_ignores_noise_and_rejects_non_http_urls() {
        assert_eq!(
            proxy_from_output("shell banner\nhttp://127.0.0.1:6789\n"),
            Some("http://127.0.0.1:6789".into())
        );
        assert_eq!(proxy_from_output("file:///tmp/not-a-proxy\n"), None);
        assert_eq!(proxy_from_output("shell banner only\n"), None);
        assert_eq!(
            proxy_from_assignment(r#"export HTTPS_PROXY="http://127.0.0.1:6789""#),
            Some("http://127.0.0.1:6789".into())
        );
        assert_eq!(proxy_from_assignment("export OTHER=value"), None);
    }
}
