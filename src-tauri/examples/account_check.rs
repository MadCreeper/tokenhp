//! Headless check for the footer account identity. Reads the local Claude Code
//! state (email + plan from ~/.claude.json's cached profile, falling back to
//! the OAuth credential for the plan) and prints what the footer will show.
//! Run with:
//!
//!     cargo run --example account_check

use hpbar_lib::account;
use hpbar_lib::credentials::CredentialCache;

fn main() {
    let cache = CredentialCache::new();
    let info = account::fetch(&cache);
    println!("email: {:?}", info.email);
    println!("plan:  {:?}", info.plan);
}
