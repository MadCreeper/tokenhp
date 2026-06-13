# Data Collection & Privacy

How HPBar gathers the numbers it shows, and what happens to that data.

**Short version:** HPBar is a local, read-only viewer. It reads usage data that
already exists on your machine — written by Claude Code, Codex, OpenClaw, etc. —
plus your existing Claude login token. It makes **exactly one network request**:
to Anthropic's own API, to fetch your *Claude* subscription quota. Everything
else — all per-tool token usage, **and even Codex's quota** — is read from local
files; **nothing else is sent anywhere**. There is no telemetry, no analytics, no
third-party server, and no account with HPBar. Your prompts and responses are
never read or transmitted.

---

## The two axes

The UI has two axes, and this doc follows them:

- **Subscription** — a provider's plan quota as draining bars (Claude, Codex, …).
- **API** — per-model token usage from local logs, where each tool is tagged
  **`equivalent`** (a subscription priced at API rates, for comparison — Claude
  Code, Codex) or **`real`** (actual metered API-key spend — OpenClaw).

Across *both* axes, HPBar reads from local files on disk. The **only** network
call anywhere is fetching Claude's subscription quota.

---

## What HPBar reads

### 1. Subscription quota

**Claude — the one and only network request:**

- **Request:** `GET https://api.anthropic.com/api/oauth/usage`
  ([src-tauri/src/usage.rs](src-tauri/src/usage.rs))
- **Sent with it:** `Authorization: Bearer <your Claude OAuth token>`,
  `anthropic-beta: oauth-2025-04-20`, `User-Agent: claude-code/<version>`,
  `Content-Type: application/json`.
- **Body:** none — no identifiers beyond the token above.
- **Response:** your quota windows (percent used + reset times). The same
  endpoint and data Claude Code's own `/usage` view uses. The destination is
  **Anthropic — your provider** — not HPBar or any third party.

**Codex — read locally, no network call.** Codex writes its plan's `rate_limits`
snapshots into its own session logs; HPBar reads the most recent one from
`~/.codex/sessions` and renders it as the same bars
([src-tauri/src/codexstats.rs](src-tauri/src/codexstats.rs)). **HPBar never
contacts an OpenAI endpoint** — there is no network call for Codex at all.

### 2. API token usage — read from disk, never leaves the machine

The **API** view aggregates per-model token counts from the session logs your
CLI tools already write locally. Each tool is a small adapter
([src-tauri/src/tools.rs](src-tauri/src/tools.rs)):

| Tool | `kind` | Files read | Records parsed |
|------|--------|-----------|----------------|
| Claude Code | equivalent | `~/.claude/projects/**/*.jsonl` | assistant messages → `message.usage` |
| Codex | equivalent | `~/.codex/sessions/**/*.jsonl` | `token_count` events → `last_token_usage` |
| OpenClaw | real | `~/.openclaw/agents/**/sessions/*.trajectory.jsonl` | `model.completed` events → `data.usage` |

From each record HPBar extracts **only**:

- token counts (input, output, cache-read, cache-write),
- the model identifier (e.g. `claude-opus-4-8`, `gpt-5.5`, `deepseek-v4-pro`),
- the timestamp (to filter by your selected 24h / 7d / 30d window).

> **HPBar does not read, store, log, or transmit any prompt or response
> content.** The parsers pull out the usage numbers above and discard everything
> else in the line. These logs are read, aggregated in memory, priced with a
> bundled rate table, and rendered in the popover — they **never leave your
> computer**.

New tools (Hermes, …) plug in as additional local adapters under the same rule:
local files in, token counts out, nothing transmitted.

### 3. Account identity (footer) — read locally, shown locally

On the Subscription view, the footer can show which login a machine uses
([src-tauri/src/account.rs](src-tauri/src/account.rs)) — read from local files
and **only displayed on-screen**, never transmitted:

- **Claude** — email from `~/.claude.json` (`oauthAccount.emailAddress`); plan
  from the stored credential's `subscriptionType` / `rateLimitTier` (e.g.
  "Max 20×").
- **Codex** — email + plan from the `id_token` JWT claims in `~/.codex/auth.json`.
  HPBar decodes the token's claims **for display only — no signature
  verification, no network**.

It exists to help when several people share one subscription and want to confirm
whose usage a machine is reporting.

---

## Where the Claude token comes from

HPBar never logs in itself — it reuses the token Claude Code already saved and
**only reads** it ([src-tauri/src/credentials.rs](src-tauri/src/credentials.rs)):

| OS | Source | Prompt? |
|----|--------|---------|
| macOS | Keychain item `Claude Code-credentials` (via the OS keychain) | once per token rotation |
| Linux / Windows | `~/.claude/.credentials.json` (plaintext, written by Claude Code) | never |

- The token is **read-only**. HPBar never modifies, re-writes, or refreshes it
  (when it expires, you refresh it by using Claude Code).
- It is held **in memory only**, re-read from storage just when it's missing or
  near expiry — never copied to a file by HPBar.
- It is sent **only** as the `Authorization` header to `api.anthropic.com`
  (Subscription #1) and nowhere else.

The Codex `id_token` in `~/.codex/auth.json` is likewise read-only and used only
to display the ChatGPT login's email/plan locally — it is never sent anywhere.

---

## What HPBar does **not** do

- ❌ No telemetry, analytics, crash reporting, or "phone-home."
- ❌ No HPBar account, server, or cloud — there is no HPBar backend.
- ❌ No third-party network destinations. The only host contacted is
  `api.anthropic.com`. (Codex quota and all token usage are read from local files.)
- ❌ No reading or transmitting of prompt/response content from your sessions.
- ❌ No writing of your usage data anywhere — it's computed on the fly per view.

## What HPBar writes to disk

Only two small, local things, both at your request:

- **"Open at Login"** toggle — registers/unregisters a normal OS autostart entry
  (LaunchAgent on macOS, the equivalent on Linux/Windows) when you tick it.
- **In-memory only:** the credential cache and all aggregated usage are kept in
  memory and discarded when the app quits.

It also *optionally reads* a user-provided pricing override at
`…/HPBar/pricing.json` if you create one ([src-tauri/src/pricing.rs](src-tauri/src/pricing.rs));
HPBar reads it but never writes it.

---

## Data-flow at a glance

```
  SUBSCRIPTION axis                       API axis  (every source is local)
  ────────────────                        ──────────────────────────────────
  Claude:  Keychain / .credentials.json   ~/.claude/projects/*.jsonl   equivalent
           └─ Bearer token ──┐            ~/.codex/sessions/*.jsonl    equivalent
  Codex:   ~/.codex/sessions │            ~/.openclaw/**/*.trajectory  real
           (rate_limits)     │                     │ token counts / model / ts only
           └─ read locally   │                     ▼
                  │          │            aggregate → price → render  (on-device)
   the ONLY network call ────┘
   GET api.anthropic.com/api/oauth/usage
   (Claude quota only — Codex quota is local)
                  ▼
        quota %s + reset times → render
```

The entire API axis — and Codex's Subscription quota — stay on-device. The only
thing that crosses into the network is the Claude quota request, carrying just
your token.

---

## Verify it yourself

HPBar is open source and unsigned by design — you can check every claim here:

- **Network:** the entire `reqwest` usage is in
  [src-tauri/src/usage.rs](src-tauri/src/usage.rs); there are no other network
  calls in the codebase. Confirm with a packet inspector (e.g. Little Snitch,
  `mitmproxy`, `tcpdump`) — you'll see one host: `api.anthropic.com`.
- **Local parsing:** the adapters in [src-tauri/src/tools.rs](src-tauri/src/tools.rs)
  (→ `localstats.rs`, `codexstats.rs`, `openclawstats.rs`) show exactly which
  fields are read (token counts, model id, timestamp) and that content fields are
  never touched.
- **Headless check:** `cargo run --example local_check` prints the aggregated
  per-tool / per-model token totals straight from your local logs, and
  `cargo run --example codex_check` does the same for Codex — both with no network
  access, the same numbers the API view shows.

*This document describes HPBar as of v0.2.2. If the behavior and this document
ever disagree, the code is the source of truth — please open an issue.*
