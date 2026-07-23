//! Reads the Claude Code OAuth token from local storage.
//!
//! Port of the Swift `ClaudeCredentials` + `CredentialProvider`. The storage
//! location differs per OS:
//!   - macOS:        Keychain generic-password item `Claude Code-credentials`
//!                   (account = the OS username).
//!   - Linux / Win:  plaintext file `~/.claude/.credentials.json`.
//!
//! All three hold the same JSON shape:
//!   { "claudeAiOauth": { "accessToken": "...", "expiresAt": <epoch_ms> } }
//!
//! # Do no harm to the CLI's session
//!
//! HPBar **only ever reads** this storage: it never writes it, never deletes it
//! and never performs an OAuth refresh (refreshing would rotate Claude Code's
//! refresh token out from under it and log the user out for real). Even so, a
//! *read* is not free on macOS — it can raise the Keychain password prompt, and
//! answering that prompt with "Always Allow" appends an entry to the ACL of
//! *Claude Code's* item, i.e. rewrites it. So the prompts are the one way this
//! read-only app can disturb a live CLI session, and a background poller that
//! re-reads on every tick turns one expired token into an endless prompt storm.
//!
//! Two things keep that from happening, belt and braces:
//!   1. On macOS the read goes through `/usr/bin/security`, which the item's ACL
//!      already trusts (Claude Code uses it too) — so it never prompts and never
//!      adds an ACL entry. See `read_raw`.
//!   2. Regardless of platform, a read only happens when it can plausibly return
//!      something new.
//!
//! So the rule here is: **only touch storage when a read can plausibly return
//! something new.** We fingerprint the item cheaply (macOS: the Keychain
//! modification date, which attribute-only queries expose *without* prompting;
//! elsewhere: file mtime + size) and re-read only when that fingerprint moves,
//! i.e. when Claude Code has actually written a new token. An expired token with
//! an unchanged fingerprint means "the CLI has not refreshed yet" — waiting is
//! the correct, silent answer.

use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Mutex;

/// Treat a token as needing replacement once it's within this many seconds of
/// expiry. It stays usable until it actually expires.
const REFRESH_MARGIN_SECS: f64 = 120.0;

/// Hard floor between two storage *data* reads, whatever else happens. Caps how
/// often HPBar can possibly raise a Keychain prompt.
const MIN_READ_INTERVAL_SECS: f64 = 60.0;

/// Back-off after a read that failed (denied prompt, malformed data, ...) before
/// we're allowed to try again.
const FAILED_READ_BACKOFF_SECS: f64 = 600.0;

/// macOS reads go through the Keychain and can raise a password prompt, so they
/// need positive evidence that a read is worth it. Elsewhere the credentials are
/// a plain file: reading is silent and free.
const READS_ARE_EXPENSIVE: bool = cfg!(target_os = "macos");

#[derive(Clone, Debug)]
pub struct ClaudeCredentials {
    pub access_token: String,
    /// Expiry as Unix epoch seconds, if known.
    pub expires_at: Option<f64>,
    /// Plan tier, e.g. "max" / "pro" / "free".
    pub subscription_type: Option<String>,
    /// Rate-limit tier, e.g. "default_claude_max_20x" — encodes the 5×/20× multiplier.
    pub rate_limit_tier: Option<String>,
}

impl ClaudeCredentials {
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => exp <= now_secs(),
            None => false,
        }
    }

    /// Within `REFRESH_MARGIN_SECS` of expiry — worth looking for a newer one,
    /// though still usable until `is_expired`.
    fn is_stale(&self) -> bool {
        match self.expires_at {
            Some(exp) => exp - now_secs() <= REFRESH_MARGIN_SECS,
            None => false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum CredError {
    NotFound,
    AccessDenied(String),
    Malformed,
    /// We hold a token that is expired or was rejected, and Claude Code hasn't
    /// written a new one — so there is nothing to re-read yet.
    Stale,
}

impl std::fmt::Display for CredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredError::NotFound => write!(
                f,
                "No Claude Code login found. Sign in with the Claude Code CLI first."
            ),
            CredError::AccessDenied(s) => {
                write!(f, "Could not read Claude Code credentials: {s}")
            }
            CredError::Malformed => write!(
                f,
                "Stored Claude Code credentials were not in the expected format."
            ),
            CredError::Stale => write!(
                f,
                "Claude Code's saved login has expired. Run any Claude Code command to refresh it."
            ),
        }
    }
}

#[derive(Default)]
struct State {
    creds: Option<ClaudeCredentials>,
    /// Fingerprint of the storage item as of our last data read. `None` means we
    /// have never read it.
    fingerprint: Option<String>,
    /// The cached token was rejected by the API (401) or has expired: unusable,
    /// but re-reading is pointless until the fingerprint moves.
    rejected: bool,
    /// Have we ever attempted a data read? The first one is always warranted.
    has_read: bool,
    /// One-shot: the user asked us to look again regardless of the fingerprint.
    force_read: bool,
    /// Don't perform a data read before this instant (epoch seconds).
    next_read_allowed: f64,
    /// Why the last data read failed, so callers keep seeing the real reason
    /// while we sit in back-off.
    last_error: Option<CredError>,
}

/// Caches credentials in memory and reads storage as rarely as possible — see
/// the module docs. On macOS every avoided read is an avoided password prompt.
pub struct CredentialCache {
    state: Mutex<State>,
    /// Optional append-only log of every storage *data* read, so a later
    /// "Claude Code logged me out" report can be correlated against exactly
    /// when (and whether) HPBar touched the item. Set once at startup.
    audit_path: Mutex<Option<PathBuf>>,
    /// Storage accessors, indirected so tests can drive the read-gating logic
    /// without a real Keychain.
    fingerprint: fn() -> Option<String>,
    load: fn() -> Result<ClaudeCredentials, CredError>,
}

impl Default for CredentialCache {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            audit_path: Mutex::default(),
            fingerprint,
            load: load_from_storage,
        }
    }
}

impl CredentialCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Point the read-audit log at `dir/credential-reads.log`. Called once from
    /// setup with the app config dir; without it, auditing is simply off.
    pub fn set_audit_dir(&self, dir: PathBuf) {
        let _ = std::fs::create_dir_all(&dir);
        *self.audit_path.lock().unwrap() = Some(dir.join("credential-reads.log"));
    }

    /// A usable credential.
    ///
    /// Storage is read only when the cache can't answer *and* the item has
    /// changed since our last read (or we've never read it). Everything else is
    /// served from memory or reported as [`CredError::Stale`].
    pub fn get(&self) -> Result<ClaudeCredentials, CredError> {
        // Hold the lock across the storage read. Otherwise concurrent startup
        // callers (usage fetch, account fetch, ambient poll, team upload) all
        // find the cache empty, all drop the lock, and all read the Keychain —
        // a cache stampede that fires one macOS password prompt *per caller*.
        let mut st = self.state.lock().unwrap();
        let now = now_secs();

        // Fast path: a cached token that is still valid and hasn't been rejected.
        let usable = !st.rejected && st.creds.as_ref().is_some_and(|c| !c.is_expired());
        if usable && !st.creds.as_ref().unwrap().is_stale() {
            return Ok(st.creds.clone().unwrap());
        }

        // Past here the cached token is missing, near expiry, expired or
        // rejected. Consider going to storage — but only if it can help.
        if now >= st.next_read_allowed {
            let fp = (self.fingerprint)();
            let changed = match (&fp, &st.fingerprint) {
                (Some(current), Some(at_last_read)) => current != at_last_read,
                // Storage exists but we have no fingerprint for it — either it
                // appeared since our last look (the user signed in to Claude
                // Code after HPBar started) or that look failed. Read it.
                (Some(_), None) => true,
                // Storage is gone (signed out): nothing to read.
                (None, Some(_)) => false,
                // Nothing there, and nothing there last time either. Where a
                // read is free (the file-backed platforms) take it anyway; on
                // macOS it can cost a password dialog, so wait for evidence.
                (None, None) => !READS_ARE_EXPENSIVE,
            };
            if !st.has_read || st.force_read || changed {
                st.next_read_allowed = now + MIN_READ_INTERVAL_SECS;
                st.has_read = true;
                st.force_read = false;
                let outcome = (self.load)();
                self.audit(&outcome, st.fingerprint.as_deref(), fp.as_deref());
                match outcome {
                    Ok(fresh) => {
                        st.creds = Some(fresh.clone());
                        st.fingerprint = fp;
                        st.last_error = None;
                        // Storage can legitimately hold an already-dead token —
                        // e.g. the first read of the day, before the CLI has
                        // been run. Remember it (so we know this fingerprint is
                        // a dead end) instead of handing it out.
                        st.rejected = fresh.is_expired();
                        if st.rejected {
                            return Err(CredError::Stale);
                        }
                        return Ok(fresh);
                    }
                    Err(e) => {
                        // Adopt the fingerprint even on failure. A denied prompt
                        // or malformed item won't fix itself while the item sits
                        // unchanged, and retrying would just re-prompt.
                        st.fingerprint = fp;
                        st.next_read_allowed = now + FAILED_READ_BACKOFF_SECS;
                        st.last_error = Some(e.clone());
                        return Err(e);
                    }
                }
            }
        }

        // Storage has nothing new for us.
        if usable {
            // Near expiry but still valid — keep using it rather than prompting.
            return Ok(st.creds.clone().unwrap());
        }
        match (&st.last_error, &st.creds) {
            (Some(e), _) => Err(e.clone()),
            // We have a token, it's just dead, and the CLI hasn't refreshed yet.
            (None, Some(_)) => Err(CredError::Stale),
            (None, None) => Err(CredError::NotFound),
        }
    }

    /// The cached token was rejected by the API (401) or has expired.
    ///
    /// This does *not* schedule a re-read: the token in storage is the same one
    /// we already hold, so re-reading would only raise another password prompt.
    /// The next `get()` picks up a new token as soon as Claude Code writes one.
    pub fn mark_rejected(&self) {
        self.state.lock().unwrap().rejected = true;
    }

    /// User-initiated "try again": permit one immediate read even though the
    /// item looks unchanged — the escape hatch for the case where storage moved
    /// without the fingerprint moving with it.
    ///
    /// A healthy cached token is still served from memory afterwards, so this
    /// can't turn an ordinary refresh click into a password prompt. Never call
    /// it from a background loop.
    pub fn allow_recheck(&self) {
        let mut st = self.state.lock().unwrap();
        st.next_read_allowed = 0.0;
        st.force_read = true;
        st.last_error = None;
    }

    fn audit(&self, outcome: &Result<ClaudeCredentials, CredError>, was: Option<&str>, now_fp: Option<&str>) {
        let Some(path) = self.audit_path.lock().unwrap().clone() else {
            return;
        };
        use std::io::Write;
        let result = match outcome {
            Ok(c) => format!(
                "ok expires_at={}",
                c.expires_at.map(|e| e as i64).unwrap_or(0)
            ),
            Err(e) => format!("error {e}"),
        };
        let line = format!(
            "{} read fingerprint {:?} -> {:?}: {result}\n",
            chrono::Utc::now().to_rfc3339(),
            was.unwrap_or("(none)"),
            now_fp.unwrap_or("(unknown)"),
        );
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Claude Code's config directory: `$CLAUDE_CONFIG_DIR` when set, else
/// `~/.claude`.
///
/// The CLI honours that variable for everything it keeps there — the
/// credentials file, session logs under `projects/` — so hardcoding `~/.claude`
/// tells anyone who sets it that they aren't signed in.
pub fn claude_config_dir() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => Some(dirs::home_dir()?.join(".claude")),
    }
}

/// The account file, `~/.claude.json`. Note the CLI composes this one
/// *differently*: with `$CLAUDE_CONFIG_DIR` set it becomes
/// `$CLAUDE_CONFIG_DIR/.claude.json`, not a sibling of the home directory.
pub fn claude_json_path() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir).join(".claude.json")),
        _ => Some(dirs::home_dir()?.join(".claude.json")),
    }
}

fn now_secs() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// A cheap, non-prompting signature of the stored item that changes whenever
/// Claude Code rewrites it. `None` if it can't be determined.
#[cfg(target_os = "macos")]
fn fingerprint() -> Option<String> {
    use security_framework::item::{ItemClass, ItemSearchOptions};

    // The ACL guards the item's *data*, not its attributes: asking only for
    // attributes returns the modification date with no password prompt, even
    // from a binary that has never been granted access. (Verified — see
    // `examples/keychain_probe.rs`.) This is what lets us poll for "did Claude
    // Code rotate the token?" without ever touching the secret.
    let user = os_username();
    let results = ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(SERVICE)
        .account(&user)
        .load_attributes(true) // NOT load_data — that would prompt
        .limit(1)
        .search()
        .ok()?;
    let dict = results.first()?.simplify_dict()?;
    dict.get("mdat").or_else(|| dict.get("cdat")).cloned()
}

#[cfg(not(target_os = "macos"))]
fn fingerprint() -> Option<String> {
    file_fingerprint(&credentials_file()?)
}

/// Fingerprint of a credentials *file* — modification time plus size.
///
/// Compiled on every platform, not just the file-backed ones, so the
/// Linux/Windows read path stays testable from a macOS dev machine — where it
/// is reachable only from the tests, hence the targeted allow.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn file_fingerprint(path: &std::path::Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(format!(
        "{}.{}:{}",
        mtime.as_secs(),
        mtime.subsec_nanos(),
        meta.len()
    ))
}

fn load_from_storage() -> Result<ClaudeCredentials, CredError> {
    let raw = read_raw()?;
    parse(&raw)
}

/// Where the credential lives; the item is stored under account = the OS
/// username (verified empirically).
#[cfg(target_os = "macos")]
const SERVICE: &str = "Claude Code-credentials";

/// Absolute path on purpose — never resolve this through `PATH`.
#[cfg(target_os = "macos")]
const SECURITY_BIN: &str = "/usr/bin/security";

/// How long to wait for `security` before giving up and killing it. It only
/// blocks if it hits a password dialog, which is precisely the case we don't
/// want to sit in — and this wait happens under the cache lock, so it also
/// bounds how long a popover fetch can stall.
#[cfg(target_os = "macos")]
const SECURITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Set once `/usr/bin/security` has been seen to block on a dialog here, after
/// which we go straight to the Keychain API and stop raising it. Deliberately
/// session-scoped, not persisted: the condition is recoverable (Claude Code
/// re-creating the item restores the trust), so each app start re-probes once.
#[cfg(target_os = "macos")]
static SECURITY_CLI_BLOCKS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The OS short name, which is the `account` on Claude Code's item. Normally
/// `USER`; the fallbacks cover a launch context with a stripped environment
/// (launchd agents do get `USER`, but a login item is not worth a broken app).
#[cfg(target_os = "macos")]
fn os_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            dirs::home_dir()?
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

/// What `/usr/bin/security` told us.
#[cfg(target_os = "macos")]
enum CliOutcome {
    Got(String),
    /// The item isn't there — nobody is signed in.
    NotFound,
    /// Report as-is; falling back would only stack a second dialog.
    Denied(String),
    /// We couldn't use the tool. Try the Keychain API instead.
    Unusable,
}

#[cfg(target_os = "macos")]
fn read_raw() -> Result<String, CredError> {
    use std::sync::atomic::Ordering;

    // Prefer `/usr/bin/security`. Claude Code stores this item by shelling out
    // to that same tool (`security add-generic-password -U` to write,
    // `find-generic-password` to read, `delete-generic-password` on logout), so
    // `security` is the item's *creating* app and its ACL trusts it —
    // `identifier "com.apple.security" and anchor apple`. The read therefore
    // returns the credential with no password dialog, from any process, on any
    // machine where Claude Code signed in.
    //
    // That matters for more than convenience. A dialog is not side-effect free:
    // "Always Allow" appends an ACL entry, which is a read-modify-write of
    // *Claude Code's* item, and the ACL matches on cdhash, so every new HPBar
    // build asks again (this machine's item had accumulated 76 HPBar entries).
    // Going through `security` means HPBar never prompts and so never causes
    // that item to be rewritten — the only way this read-only app could disturb
    // a live CLI session.
    if !SECURITY_CLI_BLOCKS.load(Ordering::Relaxed) {
        match read_via_security_cli() {
            CliOutcome::Got(raw) => return Ok(raw),
            CliOutcome::NotFound => return Err(CredError::NotFound),
            CliOutcome::Denied(m) => return Err(CredError::AccessDenied(m)),
            CliOutcome::Unusable => {}
        }
    }
    // Fallback: the direct Keychain API, as used by the other platforms' ports.
    // This one *can* raise a dialog — but only on a machine where the free path
    // didn't work, and the read gating keeps it to at most one per new token.
    read_via_keyring()
}

#[cfg(target_os = "macos")]
fn read_via_security_cli() -> CliOutcome {
    use std::sync::atomic::Ordering;

    let user = os_username();
    let out = match run_bounded(
        &["find-generic-password", "-w", "-s", SERVICE, "-a", &user],
        SECURITY_TIMEOUT,
    ) {
        Bounded::Exited(out) => out,
        Bounded::Spawn => return CliOutcome::Unusable, // no `security` on this box
        Bounded::TimedOut => {
            // It sat on a dialog (we killed it, so nothing is left on screen).
            // Either this machine doesn't trust `security` for the item, or the
            // login keychain is locked. Only the first is permanent, so only
            // that one disables the fast path.
            return if keychain_is_unlocked() {
                SECURITY_CLI_BLOCKS.store(true, Ordering::Relaxed);
                CliOutcome::Unusable
            } else {
                // Don't force an unlock dialog from a background poll.
                CliOutcome::Denied("the login keychain is locked".into())
            };
        }
    };
    classify(out.status.code(), out.stdout)
}

/// Map `security`'s exit status onto an outcome. Split out so the mapping is
/// testable without spawning anything.
#[cfg(target_os = "macos")]
fn classify(code: Option<i32>, stdout: Vec<u8>) -> CliOutcome {
    match code {
        Some(0) => match String::from_utf8(stdout) {
            Ok(s) => CliOutcome::Got(s.trim_end_matches('\n').to_string()),
            Err(_) => CliOutcome::Unusable,
        },
        // errSecItemNotFound — nobody signed in.
        Some(44) => CliOutcome::NotFound,
        // errSecUserCanceled — a dialog appeared and the user dismissed it.
        Some(128) => CliOutcome::Denied("access was denied".into()),
        _ => CliOutcome::Unusable,
    }
}

#[cfg(target_os = "macos")]
enum Bounded {
    Exited(std::process::Output),
    TimedOut,
    Spawn,
}

/// Run `/usr/bin/security` with the given args, killing it if it outlives
/// `timeout` (which only happens when it is waiting on a dialog).
#[cfg(target_os = "macos")]
fn run_bounded(args: &[&str], timeout: std::time::Duration) -> Bounded {
    run_bounded_at(SECURITY_BIN, args, timeout)
}

#[cfg(target_os = "macos")]
fn run_bounded_at(bin: &str, args: &[&str], timeout: std::time::Duration) -> Bounded {
    use std::process::{Command, Stdio};

    let mut child = match Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Bounded::Spawn,
    };

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                // Kill it so no dialog is left sitting on the user's screen.
                let _ = child.kill();
                let _ = child.wait();
                return Bounded::TimedOut;
            }
            Err(_) => return Bounded::Spawn,
        }
    }
    match child.wait_with_output() {
        Ok(out) => Bounded::Exited(out),
        Err(_) => Bounded::Spawn,
    }
}

/// Is the login keychain unlocked? Attribute-level question — never prompts.
/// Conservative: anything unexpected reads as "locked", which only costs us the
/// fast path for this session.
#[cfg(target_os = "macos")]
fn keychain_is_unlocked() -> bool {
    matches!(
        run_bounded(&["show-keychain-info"], std::time::Duration::from_secs(2)),
        Bounded::Exited(out) if out.status.success()
    )
}

#[cfg(target_os = "macos")]
fn read_via_keyring() -> Result<String, CredError> {
    let user = os_username();
    let entry =
        keyring::Entry::new(SERVICE, &user).map_err(|e| CredError::AccessDenied(e.to_string()))?;
    match entry.get_password() {
        Ok(s) => Ok(s),
        Err(keyring::Error::NoEntry) => Err(CredError::NotFound),
        Err(e) => Err(CredError::AccessDenied(e.to_string())),
    }
}

#[cfg(not(target_os = "macos"))]
fn credentials_file() -> Option<PathBuf> {
    Some(claude_config_dir()?.join(".credentials.json"))
}

#[cfg(not(target_os = "macos"))]
fn read_raw() -> Result<String, CredError> {
    let path =
        credentials_file().ok_or_else(|| CredError::AccessDenied("no home directory".into()))?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CredError::NotFound),
        Err(e) => Err(CredError::AccessDenied(e.to_string())),
    }
}

fn parse(raw: &str) -> Result<ClaudeCredentials, CredError> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "claudeAiOauth")]
        oauth: OAuth,
    }
    #[derive(Deserialize)]
    struct OAuth {
        #[serde(rename = "accessToken")]
        access_token: String,
        // Stored in epoch milliseconds.
        #[serde(rename = "expiresAt")]
        expires_at: Option<f64>,
        #[serde(rename = "subscriptionType")]
        subscription_type: Option<String>,
        #[serde(rename = "rateLimitTier")]
        rate_limit_tier: Option<String>,
    }

    let env: Envelope = serde_json::from_str(raw).map_err(|_| CredError::Malformed)?;
    Ok(ClaudeCredentials {
        access_token: env.oauth.access_token,
        expires_at: env.oauth.expires_at.map(|ms| ms / 1000.0),
        subscription_type: env.oauth.subscription_type,
        rate_limit_tier: env.oauth.rate_limit_tier,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Mock storage. Tests run in-process and share these, so the whole module
    // is serialised on one lock.
    static READS: AtomicUsize = AtomicUsize::new(0);
    static FP: Mutex<Option<String>> = Mutex::new(None);
    static TOKEN_LIFETIME: Mutex<f64> = Mutex::new(0.0);
    static SERIAL: Mutex<()> = Mutex::new(());

    fn mock_fingerprint() -> Option<String> {
        FP.lock().unwrap().clone()
    }

    fn mock_load() -> Result<ClaudeCredentials, CredError> {
        READS.fetch_add(1, Ordering::SeqCst);
        Ok(ClaudeCredentials {
            access_token: FP.lock().unwrap().clone().unwrap_or_default(),
            expires_at: Some(now_secs() + *TOKEN_LIFETIME.lock().unwrap()),
            subscription_type: None,
            rate_limit_tier: None,
        })
    }

    /// Fresh cache + mock storage holding a token that expired an hour ago.
    fn setup(fp: &str, lifetime_secs: f64) -> CredentialCache {
        READS.store(0, Ordering::SeqCst);
        *FP.lock().unwrap() = Some(fp.into());
        *TOKEN_LIFETIME.lock().unwrap() = lifetime_secs;
        CredentialCache {
            state: Mutex::default(),
            audit_path: Mutex::default(),
            fingerprint: mock_fingerprint,
            load: mock_load,
        }
    }

    fn allow_read_now(cache: &CredentialCache) {
        cache.state.lock().unwrap().next_read_allowed = 0.0;
    }

    /// The bug this whole module exists to prevent: an expired token must not
    /// make every poll re-read storage (on macOS, one password prompt each).
    #[test]
    fn expired_token_does_not_re_read_until_storage_changes() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let cache = setup("mdat-1", -3600.0);

        for _ in 0..20 {
            allow_read_now(&cache); // defeat the interval floor: fingerprint alone must hold
            assert!(matches!(cache.get(), Err(CredError::Stale)));
        }
        assert_eq!(READS.load(Ordering::SeqCst), 1, "storage read more than once");
    }

    /// ...but the moment Claude Code writes a new token, we pick it up.
    #[test]
    fn new_token_is_picked_up_when_the_item_changes() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let cache = setup("mdat-1", -3600.0);
        assert!(cache.get().is_err());

        *FP.lock().unwrap() = Some("mdat-2".into());
        *TOKEN_LIFETIME.lock().unwrap() = 3600.0;
        allow_read_now(&cache);

        let creds = cache.get().expect("should have re-read after the item changed");
        assert_eq!(creds.access_token, "mdat-2");
        assert_eq!(READS.load(Ordering::SeqCst), 2);
    }

    /// HPBar starting *before* the user has ever signed in to Claude Code: the
    /// sign-in has to be picked up without an app restart.
    #[test]
    fn signing_in_after_start_is_picked_up() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let cache = setup("mdat-1", 3600.0);
        *FP.lock().unwrap() = None; // no credential item at all yet
        fn missing() -> Result<ClaudeCredentials, CredError> {
            READS.fetch_add(1, Ordering::SeqCst);
            Err(CredError::NotFound)
        }
        let cache = CredentialCache { load: missing, ..cache };
        assert!(matches!(cache.get(), Err(CredError::NotFound)));
        for _ in 0..5 {
            allow_read_now(&cache);
            assert!(matches!(cache.get(), Err(CredError::NotFound)));
        }
        assert_eq!(READS.load(Ordering::SeqCst), 1, "polled a missing item");

        // The user signs in: the item appears.
        *FP.lock().unwrap() = Some("mdat-new".into());
        let cache = CredentialCache { load: mock_load, ..cache };
        allow_read_now(&cache);
        assert!(cache.get().is_ok(), "did not notice the sign-in");
    }

    /// Signing out (the item is deleted) must not start a hunt for it.
    #[test]
    fn signing_out_does_not_start_polling() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let cache = setup("mdat-1", 3600.0);
        assert!(cache.get().is_ok());
        cache.mark_rejected();
        *FP.lock().unwrap() = None; // `claude` logged out; item deleted
        for _ in 0..5 {
            allow_read_now(&cache);
            let _ = cache.get();
        }
        assert_eq!(READS.load(Ordering::SeqCst), 1);
    }

    /// A 401 marks the token dead but must not schedule a re-read: storage still
    /// holds the same token we just had rejected.
    #[test]
    fn rejected_token_does_not_re_read_until_storage_changes() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let cache = setup("mdat-1", 3600.0);
        assert!(cache.get().is_ok());

        cache.mark_rejected();
        for _ in 0..10 {
            allow_read_now(&cache);
            assert!(cache.get().is_err());
        }
        assert_eq!(READS.load(Ordering::SeqCst), 1);
    }

    /// Even with the item changing constantly, reads are floored to one per
    /// `MIN_READ_INTERVAL_SECS`.
    #[test]
    fn reads_are_rate_limited_even_when_the_item_keeps_changing() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let cache = setup("mdat-1", -3600.0);
        for i in 0..10 {
            *FP.lock().unwrap() = Some(format!("mdat-{i}"));
            let _ = cache.get();
        }
        assert_eq!(READS.load(Ordering::SeqCst), 1);
    }

    /// A token near expiry is still a valid token — serve it rather than
    /// prompting for a replacement that isn't there yet.
    #[test]
    fn stale_but_unexpired_token_is_served_without_re_reading() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let cache = setup("mdat-1", REFRESH_MARGIN_SECS / 2.0);
        assert!(cache.get().is_ok());
        for _ in 0..5 {
            allow_read_now(&cache);
            assert!(cache.get().is_ok());
        }
        assert_eq!(READS.load(Ordering::SeqCst), 1);
    }

    // --- the file-backed path (Linux / Windows), exercised on every platform ---

    /// Claude Code relocates everything when `$CLAUDE_CONFIG_DIR` is set, and
    /// composes the account file differently from the config-dir files. Getting
    /// either wrong reports "not signed in" to a user who is signed in.
    #[test]
    fn config_dir_honours_claude_config_dir() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("CLAUDE_CONFIG_DIR");

        std::env::remove_var("CLAUDE_CONFIG_DIR");
        let home = dirs::home_dir().expect("test host has a home dir");
        assert_eq!(claude_config_dir().unwrap(), home.join(".claude"));
        // Default: a *sibling* of ~/.claude, not a file inside it.
        assert_eq!(claude_json_path().unwrap(), home.join(".claude.json"));

        std::env::set_var("CLAUDE_CONFIG_DIR", "/somewhere/else");
        assert_eq!(claude_config_dir().unwrap(), PathBuf::from("/somewhere/else"));
        // Override: it moves *inside* the config dir.
        assert_eq!(
            claude_json_path().unwrap(),
            PathBuf::from("/somewhere/else/.claude.json")
        );

        std::env::set_var("CLAUDE_CONFIG_DIR", "");
        assert_eq!(claude_config_dir().unwrap(), home.join(".claude"), "empty = unset");

        match prev {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
    }

    /// The fingerprint that gates re-reads on the file-backed platforms must
    /// move when the file is rewritten and hold still when it isn't.
    #[test]
    fn file_fingerprint_tracks_rewrites() {
        let path = std::env::temp_dir().join(format!(
            "hpbar-credtest-{}-{}.json",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&path);
        assert!(file_fingerprint(&path).is_none(), "missing file has no fingerprint");

        std::fs::write(&path, b"{}").unwrap();
        let first = file_fingerprint(&path).expect("existing file has a fingerprint");
        assert_eq!(file_fingerprint(&path).as_deref(), Some(first.as_str()));

        // Size is part of the fingerprint, so this moves even if the filesystem's
        // mtime resolution is coarse.
        std::fs::write(&path, b"{\"claudeAiOauth\":{}}").unwrap();
        assert_ne!(file_fingerprint(&path), Some(first));

        let _ = std::fs::remove_file(&path);
    }

    /// The stored JSON is the one shape both platforms share — notably
    /// `expiresAt` is epoch *milliseconds* and we hold seconds.
    #[test]
    fn parses_the_stored_envelope() {
        let raw = r#"{"claudeAiOauth":{
            "accessToken":"sk-ant-oat-xyz",
            "expiresAt":1784804325000,
            "subscriptionType":"max",
            "rateLimitTier":"default_claude_max_20x"
        }}"#;
        let c = parse(raw).expect("well-formed envelope should parse");
        assert_eq!(c.access_token, "sk-ant-oat-xyz");
        assert_eq!(c.expires_at, Some(1_784_804_325.0), "ms should become s");
        assert_eq!(c.subscription_type.as_deref(), Some("max"));

        // Optional fields absent — still usable.
        let minimal = parse(r#"{"claudeAiOauth":{"accessToken":"t"}}"#).unwrap();
        assert_eq!(minimal.expires_at, None);
        assert!(!minimal.is_expired(), "unknown expiry is not expired");

        for bad in [
            r#"not json"#,
            r#"{}"#,                                   // no envelope
            r#"{"claudeAiOauth":{"expiresAt":1000}}"#, // no access token
        ] {
            assert!(matches!(parse(bad), Err(CredError::Malformed)), "{bad}");
        }
    }

    /// `security`'s exit codes drive whether we report, fall back, or give up —
    /// getting one wrong would either hide a login or stack a second dialog.
    #[cfg(target_os = "macos")]
    #[test]
    fn security_cli_exit_codes_are_classified() {
        assert!(matches!(
            classify(Some(0), b"{\"claudeAiOauth\":{}}\n".to_vec()),
            CliOutcome::Got(s) if s == "{\"claudeAiOauth\":{}}"
        ));
        assert!(matches!(classify(Some(44), vec![]), CliOutcome::NotFound));
        assert!(matches!(classify(Some(128), vec![]), CliOutcome::Denied(_)));
        assert!(matches!(classify(Some(1), vec![]), CliOutcome::Unusable));
        assert!(matches!(classify(None, vec![]), CliOutcome::Unusable));
    }

    /// The hang guard: a child that sits on a dialog must be killed promptly,
    /// not waited on forever — this runs under the cache lock.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_blocking_child_is_killed_at_the_deadline() {
        let started = std::time::Instant::now();
        let outcome = run_bounded_at(
            "/bin/sleep",
            &["30"],
            std::time::Duration::from_millis(300),
        );
        assert!(matches!(outcome, Bounded::TimedOut));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "did not return at the deadline"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_normal_child_is_read_back_and_a_missing_one_is_reported() {
        let outcome = run_bounded_at("/bin/echo", &["hi"], std::time::Duration::from_secs(5));
        assert!(matches!(outcome, Bounded::Exited(o) if o.stdout == b"hi\n"));
        assert!(matches!(
            run_bounded_at("/nonexistent/security", &[], std::time::Duration::from_secs(5)),
            Bounded::Spawn
        ));
    }

    /// The account name must survive a launch context with no `USER`.
    #[cfg(target_os = "macos")]
    #[test]
    fn os_username_is_never_empty_here() {
        assert!(!os_username().is_empty());
    }

    /// A user-initiated retry is the one thing allowed to force a read...
    #[test]
    fn user_recheck_reads_again() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let cache = setup("mdat-1", -3600.0);
        assert!(cache.get().is_err());
        cache.allow_recheck();
        assert!(cache.get().is_err());
        assert_eq!(READS.load(Ordering::SeqCst), 2);
    }

    /// ...but it must not read (i.e. must not prompt) when the cached token is
    /// perfectly fine — refresh clicks are frequent.
    #[test]
    fn user_recheck_serves_a_healthy_token_from_memory() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let cache = setup("mdat-1", 3600.0);
        assert!(cache.get().is_ok());
        for _ in 0..5 {
            cache.allow_recheck();
            assert!(cache.get().is_ok());
        }
        assert_eq!(READS.load(Ordering::SeqCst), 1);
    }

    /// If the fingerprint can't be read at all, macOS must not fall back to
    /// polling the Keychain (that's the prompt storm again); the file-backed
    /// platforms, where a read is silent and free, may.
    #[test]
    fn unknown_fingerprint_does_not_poll_where_reads_can_prompt() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let cache = setup("mdat-1", -3600.0);
        assert!(cache.get().is_err());

        *FP.lock().unwrap() = None; // fingerprint query broke
        for _ in 0..5 {
            allow_read_now(&cache);
            let _ = cache.get();
        }
        let expected = if READS_ARE_EXPENSIVE { 1 } else { 6 };
        assert_eq!(READS.load(Ordering::SeqCst), expected);
    }

    /// A failed read (denied prompt, malformed item) must not turn into a retry
    /// loop while the item sits unchanged.
    #[test]
    fn failed_read_does_not_retry_while_the_item_is_unchanged() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        fn deny() -> Result<ClaudeCredentials, CredError> {
            READS.fetch_add(1, Ordering::SeqCst);
            Err(CredError::AccessDenied("user denied".into()))
        }
        let cache = setup("mdat-1", 3600.0);
        let cache = CredentialCache {
            load: deny,
            ..cache
        };
        for _ in 0..10 {
            allow_read_now(&cache);
            assert!(matches!(cache.get(), Err(CredError::AccessDenied(_))));
        }
        assert_eq!(READS.load(Ordering::SeqCst), 1);
    }
}
