# Data Collection & Privacy

How HPBar gathers the numbers it shows, and what happens to that data.

**Short version:** HPBar is local-first. It reads usage data already written by
Claude Code, Codex, OpenClaw, etc., plus your existing Claude login token. Codex
quota and every local-activity number come from local files. HPBar has no
telemetry, analytics, hosted backend, or HPBar account, and it never extracts,
stores, or transmits prompt/response content.

Network access is limited to clearly scoped features: Anthropic for Claude
quota; GitHub when you check/download an update; and, only when you enable Team,
the SSH/Postgres host you configured. Team is off by default.

---

## The two axes

The UI has two axes, and this doc follows them:

- **Subscription** — a provider's plan quota as draining bars (Claude, Codex, …).
- **API** — per-model token usage from local logs, where each tool is tagged
  **`equivalent`** (a subscription priced at API rates, for comparison — Claude
  Code, Codex) or **`real`** (actual metered API-key spend — OpenClaw).

Across both axes HPBar primarily reads local files. The exceptions are listed
under [Network destinations](#network-destinations).

---

## What HPBar reads

### 1. Subscription quota

**Claude — provider quota request:**

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

### 2. Local token usage — read from disk

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
- the timestamp (to filter by your selected 24h / 7d / 30d window),
- the working-directory label needed for the optional project breakdown.

> **HPBar does not extract, store, log, or transmit any prompt or response
> content.** The parsers pull out the usage numbers above and discard everything
> else in the line. These logs are read, aggregated in memory, priced with a
> bundled rate table, and rendered in the popover. Aggregate fields leave the
> computer only if you explicitly enable Team sharing.

New tools (Hermes, …) plug in as additional local adapters under the same rule:
local files in; token-count aggregates out.

### 3. Account identity and attribution

On the Subscription view, the footer can show which login a machine uses
([src-tauri/src/account.rs](src-tauri/src/account.rs)), read from local files:

- **Claude** — email from `~/.claude.json` (`oauthAccount.emailAddress`); plan
  from the stored credential's `subscriptionType` / `rateLimitTier` (e.g.
  "Max 20×").
- **Codex** — email + plan from the `id_token` JWT claims in `~/.codex/auth.json`.
  HPBar decodes the token's claims **for display only — no signature
  verification, no network**.

HPBar also keeps local account observation epochs in
`…/HPBar/account-history.json`. Each epoch contains a deterministic hashed
account/billing identifier, the display label, provider, and observation
timestamps — never an OAuth token. (A deterministic hash groups the same account
across machines; it should be treated as pseudonymous, not anonymous.) A
historical event is assigned only when its timestamp falls inside an observed
epoch. History from before HPBar observed the login, offline gaps, and ambiguous
switches are shown as **Unknown account**, rather than guessed.

### 4. Optional Team sharing

Team is disabled by default. If enabled, HPBar opens an SSH tunnel to the host
you configure and writes aggregate rows to your Postgres database:

- installation member UUID and display name;
- UTC day, provider, model, aggregate token components and optional estimated
  cost;
- optional project label;
- optional hashed account/billing keys and account label.

No prompts, responses, OAuth tokens, or Codex tokens are uploaded. `member_id`
is generated per installation and is not derived from the shared login, so two
people using one account remain separate members. Account labels default to a
masked email when account sharing is enabled; settings can choose full, masked,
or hidden. Account sharing itself defaults off. Turning it off suppresses both
labels and stable account/billing hashes. Saving a changed sharing scope pushes
a replacement snapshot immediately, so an earlier privacy choice does not leave
stale `Hidden account` rows in the UI.

During a rolling upgrade, v1 members remain visible with a legacy marker and
continue using the original aggregate table. Their model totals are available,
but account-level splitting is deliberately unavailable because v1 never
recorded account attribution. v2 keeps account-aware rows separately and writes
a collapsed compatibility mirror for old clients. Once a v2 installation names
its v1 predecessor, the new UI suppresses that predecessor row to avoid counting
the same backfilled history twice.

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

The Codex `id_token` in `~/.codex/auth.json` is likewise read-only and is never
sent anywhere. Only decoded, non-secret identity fields may enter the local
account history and, subject to Team privacy settings, aggregate Team rows.

---

## Network destinations

- **`api.anthropic.com`** — automatic Claude quota polling, authorized with the
  existing Claude token.
- **`api.github.com` and GitHub release asset hosts** — only when you open the
  Update page to check, or explicitly download an installer.
- **Your configured SSH/Postgres host** — only when Team is enabled or you press
  Test Connection. The tunnel encrypts the Postgres connection.

HPBar does not send telemetry or analytics to its maintainers and has no hosted
HPBar backend.

---

## What HPBar does **not** do

- ❌ No telemetry, analytics, crash reporting, or "phone-home."
- ❌ No HPBar account or hosted HPBar server.
- ❌ No extracting, storing, or transmitting prompt/response content from sessions.
- ❌ No uploading aggregate usage unless Team is explicitly enabled.

## What HPBar writes to disk

HPBar writes small local state needed for its features:

- UI preferences in the webview's local storage (theme, selected view/provider,
  pin state, update channel, and animation baselines);
- quota/burn-rate and device-share sample histories;
- `account-history.json`, described above;
- `team-config.json` when Team settings are saved. SSH-key use stores no secret,
  but an optional SSH password is stored there in plaintext if you enter one;
- a downloaded installer when you explicitly choose Download & Install;
- an OS autostart entry when you enable Open at Login.

Raw session content and OAuth tokens are not copied into these files. Aggregate
local usage is otherwise computed on demand.

It also *optionally reads* a user-provided pricing override at
`…/HPBar/pricing.json` if you create one ([src-tauri/src/pricing.rs](src-tauri/src/pricing.rs));
HPBar reads it but never writes it.

---

## Data-flow at a glance

```
  SUBSCRIPTION axis                       LOCAL axis
  ────────────────                        ──────────────────────────────────
  Claude:  Keychain / .credentials.json   ~/.claude/projects/*.jsonl   equivalent
           └─ Bearer token ──┐            ~/.codex/sessions/*.jsonl    equivalent
  Codex:   ~/.codex/sessions │            ~/.openclaw/**/*.trajectory  real
           (rate_limits)     │                     │ token counts / model / ts only
           └─ read locally   │                     ▼
                  │          │            aggregate → price → render  (on-device)
   provider quota request ───┘
   GET api.anthropic.com/api/oauth/usage
   (Claude quota only — Codex quota is local)
                  ▼
        quota %s + reset times → render

  Optional Team (off by default):
  aggregate day/member/provider/account/model rows
                  └─ SSH tunnel ──> your Postgres
```

Codex quota stays on-device. Local aggregates cross the network only through the
opt-in Team path above. Update checks are separate and carry no usage data.

---

## Verify it yourself

HPBar is open source and unsigned by design — you can check every claim here:

- **Network:** provider quota HTTP is in
  [src-tauri/src/usage.rs](src-tauri/src/usage.rs), GitHub update HTTP is in
  [src-tauri/src/update.rs](src-tauri/src/update.rs), and Team SSH/Postgres code
  is in [src-tauri/src/team/db.rs](src-tauri/src/team/db.rs).
- **Local parsing:** the adapters in [src-tauri/src/tools.rs](src-tauri/src/tools.rs)
  (→ `localstats.rs`, `codexstats.rs`, `openclawstats.rs`) show exactly which
  fields are read (token counts, model id, timestamp) and that content fields are
  never touched.
- **Headless check:** `cargo run --example local_check` prints the aggregated
  per-tool / per-model token totals straight from your local logs, and
  `cargo run --example codex_check` does the same for Codex — both with no network
  access, the same numbers the API view shows.

If behavior and this document ever disagree, the code is the source of truth —
please open an issue.
