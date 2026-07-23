# Design: how HPBar reads Claude Code's credential

Status: **implemented** (`src-tauri/src/credentials.rs`).

HPBar has no login of its own. To call the OAuth usage endpoint it reads the
token Claude Code already stored — on macOS a Keychain generic-password item
named `Claude Code-credentials`, on Linux/Windows the plaintext
`~/.claude/.credentials.json`. This note records how that read is performed and
why, because the obvious implementation is the wrong one and we shipped it for
six months before noticing.

## The invariant

**HPBar only ever reads.** It never writes the item, never deletes it, and never
performs an OAuth refresh. Refreshing would rotate Claude Code's refresh token
out from under the CLI and log the user out for real — so a self-refreshing
"copy the token into our own Keychain item" design is permanently off the table,
however tempting it is as a way to stop the password prompts.

## The design choice: use the owning app's own access path

> **A keychain ACL trusts the process making the request.** When you piggyback on
> another app's credential storage, use the same access path that app uses and
> you inherit its trust.

Claude Code stores this item by shelling out to `/usr/bin/security`
(`add-generic-password -U` to write, `find-generic-password` to read,
`delete-generic-password` on logout — verifiable with `strings` on the CLI
binary). That makes `security` the item's *creating* app, so it sits permanently
in the item's ACL:

```text
applications (81):
    …
    80: /usr/bin/security (OK)
        requirement: identifier "com.apple.security" and anchor apple
```

HPBar therefore reads by spawning `/usr/bin/security find-generic-password -w`.
The requesting process is `security`, which the item already trusts, so the read
returns the credential with **no password dialog, from any process, on any
machine where Claude Code signed in** — including unsigned dev builds.

### Why not the direct Keychain API

That was the original implementation (`SecItemCopyMatching` in the Swift app,
`keyring::get_password` in the Rust port), and it is still the fallback. It works
— but it makes HPBar a *stranger* to an item it only reads, so macOS prompts for
the password. The prompt is not side-effect free:

- Answering it with **Always Allow** appends an entry to the ACL of *Claude
  Code's* item — i.e. a read-modify-write of the credential the CLI depends on.
- The ACL matches on **cdhash**, so every rebuild and every released version
  needs a fresh grant. The item on the author's machine had accumulated **76
  HPBar entries** this way.

That ACL rewrite is the only mechanism by which this read-only app can disturb a
live CLI session — an ACL write racing Claude Code's own write of a freshly
rotated token. Using `security` removes it entirely.

### Platforms

`security` is a macOS binary, so **every line of that path is behind
`#[cfg(target_os = "macos")]`** — the constant, the subprocess runner, the
timeout, the exit-code mapping, the keychain-lock probe, and the tests that
cover them. Linux and Windows compile none of it: `read_raw` there is a plain
`fs::read_to_string` of `.credentials.json`, and `fingerprint` is the file's
mtime + size. Nothing is ever spawned, and no prompt is possible.

Claude Code uses no OS keyring on either platform — no libsecret/gnome-keyring,
no Windows Credential Manager or DPAPI (verifiable by their absence from the CLI
binary's strings). The token is plain JSON on disk, so protect it with file
permissions; HPBar reads it and nothing else.

**Both platforms honour `$CLAUDE_CONFIG_DIR`**, and so must we: the CLI puts the
credentials file and `projects/` under it, but composes the account file
differently (`$CLAUDE_CONFIG_DIR/.claude.json` when set, `~/.claude.json`
otherwise). `credentials::claude_config_dir()` and `claude_json_path()` are the
single source of truth for both rules — hardcoding `~/.claude` tells anyone who
sets that variable they aren't signed in.

The gating logic itself is shared. `READS_ARE_EXPENSIVE` is `false` off macOS,
so a fingerprint that can't be read there falls through to "just read it" — free
and silent for a file — while macOS waits for positive evidence rather than risk
a dialog.

Verify the split without a Linux or Windows machine by type-checking the module
for those targets (`--all-targets` covers the test module too):

```sh
rustup target add x86_64-unknown-linux-gnu x86_64-pc-windows-msvc
cargo check --target x86_64-unknown-linux-gnu --all-targets
```

The full Tauri app can't be cross-compiled from macOS (Linux needs gtk/webkit via
`pkg-config`, Windows needs the MSVC linker) — CI's release matrix is what proves
those builds.

## Layered protections

The access path is the main fix; these bound the damage if it ever fails.

| Layer | What it does |
|---|---|
| Fingerprint gating | Reads the item's Keychain modification date via an **attribute-only** query — which the ACL does not guard, so it never prompts — and performs a data read only when that date moves (i.e. Claude Code actually wrote a new token). An expired token with an unchanged fingerprint means "the CLI hasn't refreshed yet"; waiting is the correct, silent answer. |
| Rate floor / back-off | At most one data read per 60s, and a 10-minute back-off after a failed read. A failed or denied read does not retry while the item is unchanged. |
| Bounded subprocess | `security` is killed after 5s if it ever blocks on a dialog, so nothing is left on screen and the cache lock is never held for long. If it blocked because the login keychain is locked (transient), the fast path stays enabled; if it blocked because this machine doesn't trust `security` (permanent), the fast path is disabled for the session and the Keychain API takes over. |
| User-only recheck | `recheck_credentials` — wired to the popover's refresh click — is the only way to force a read of an apparently-unchanged item. Background polls never do this. A healthy cached token is still served from memory, so an ordinary refresh click cannot cause a dialog. |
| Read audit log | Every data read is appended to `credential-reads.log` in the app config dir with a timestamp, the fingerprint transition, and the outcome — so a report of "the CLI logged me out" can be checked against whether HPBar touched the item anywhere near that time. |

## Verifying

`cargo run --example keychain_probe` exercises the whole path from a binary that
has never been granted access to the item. It should print the attribute
fingerprint and read the credential without any dialog. A real
`npm run tauri dev` run should leave both the item's `mdat` and its ACL entry
count unchanged:

```sh
security find-generic-password -s "Claude Code-credentials" | grep mdat
security dump-keychain -a | awk '/"svce"<blob>="Claude Code-credentials"/{f=1} f' | grep -m1 "applications ("
```

Both commands read attributes only and never prompt.

## The generalisation

This is not really a Keychain lesson. When you read another tool's state, prefer
its own documented or observable access path over a parallel one — you inherit
its permissions instead of having to acquire your own, and you stop being an
actor the owning tool has to tolerate.
