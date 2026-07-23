//! Probe: can we learn *whether Claude Code's Keychain item changed* without
//! reading its data (and therefore without triggering the macOS password
//! prompt)?
//!
//!   cargo run --example keychain_probe
//!
//! The ACL on a generic-password item guards its *data*, not its *attributes*,
//! so a `SecItemCopyMatching` that asks only for attributes should return the
//! modification date silently even from a binary that has never been granted
//! access. If this hangs on a password dialog, the assumption is wrong.
//!
//! Prints the item's attribute fingerprint. Run it twice around a Claude Code
//! token refresh: the fingerprint must change when — and only when — CC rewrites
//! the item.

fn main() {
    #[cfg(target_os = "macos")]
    {
        use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};

        let user = std::env::var("USER").unwrap_or_default();
        println!("querying attributes for account={user:?} (no data requested)");

        let results = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service("Claude Code-credentials")
            .account(&user)
            .load_attributes(true) // NOT load_data — that is what would prompt
            .limit(1)
            .search();

        match results {
            Ok(items) => {
                for item in &items {
                    match item {
                        SearchResult::Dict(_) => {
                            let dict = item.simplify_dict().unwrap_or_default();
                            let mut keys: Vec<_> = dict.keys().cloned().collect();
                            keys.sort();
                            for k in keys {
                                println!("  {k} = {}", dict[&k]);
                            }
                            println!(
                                "\nfingerprint (mdat) = {:?}",
                                dict.get("mdat").or_else(|| dict.get("cdat"))
                            );
                        }
                        other => println!("  unexpected result kind: {other:?}"),
                    }
                }
                if items.is_empty() {
                    println!("  no matching item found");
                }
            }
            Err(e) => println!("  search failed: {e}"),
        }

        // End-to-end: the real cache, against the real Keychain. This binary has
        // never been granted access to the item, so if anything here prompts,
        // the `/usr/bin/security` read path is not working as intended.
        println!("\nreading through CredentialCache (must not prompt):");
        let cache = hpbar_lib::credentials::CredentialCache::new();
        match cache.get() {
            Ok(c) => println!(
                "  ok — token {} chars, expires in {:.0} min",
                c.access_token.len(),
                c.expires_at
                    .map(|e| (e - std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs_f64())
                        / 60.0)
                    .unwrap_or(f64::NAN),
            ),
            Err(e) => println!("  {e}"),
        }
        // Second call must be served from memory — no second storage read.
        match cache.get() {
            Ok(_) => println!("  second get(): served from cache"),
            Err(e) => println!("  second get(): {e}"),
        }
    }

    #[cfg(not(target_os = "macos"))]
    println!("macOS only");
}
