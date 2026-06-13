# Design: multi-tool usage (Claude Code + Codex)

Status: **implemented**, aligned to the two-axis model from the PR review.
Branch `feat/multi-tool-usage`.

> **Post-review update.** This now follows the settled two-axis design:
> **Subscription** (Live, provider-selectable quota) + **API** (Local, multi-tool
> usage tagged `equivalent`/`real`). `LocalReport` is `{ apps, combined }` and
> each tool is a `ToolAdapter` (Claude Code + Codex implemented). Token usage
> sums per-turn `last_token_usage`, not the cumulative field the draft below
> proposed. Field names / frontend may still need reconciliation with the
> in-flight `feat/openclaw-usage` adapter refactor — see the PR thread.

## Goal

Today HPBar only knows about Claude Code. We want it to also show **Codex
(OpenAI Codex CLI)** usage, and let the user switch between tools, while keeping
the UI as simple as possible (one extra toggle).

## How the data works today

Two independent sources (see `src-tauri/src`):

| View | Module | Source | Meaning |
|---|---|---|---|
| **Live quota** | `usage.rs` | GET `https://api.anthropic.com/api/oauth/usage` | Claude account's rolling subscription limits (5h / weekly / extra), as %. Account-wide, server truth. |
| **Local activity** | `localstats.rs` | scans `~/.claude/projects/**/*.jsonl` | Per-model token totals + $ from **Claude Code's** own session transcripts on this machine. |

Key point: "Local activity" is **Claude Code's** usage, not "all AI tools on
this machine". Codex writes its own logs elsewhere and is currently invisible.

## What Codex logs (verified on this machine, 2026-06)

- Codex CLI **does run on Linux** — `~/.codex/` exists here (model
  `gpt-5.1-codex-max`), so this is not a portability blocker.
- Sessions live at `~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl`.
- The rollout JSONL is event-shaped (`type` ∈ `session_meta`, `turn_context`,
  `response_item`, `event_msg`, `token_count`, …) — **a different schema from
  Claude Code's `message.usage` rows.**
- **Recent Codex** versions emit, per turn, an `event_msg` of type
  `token_count` whose payload carries:
  - `info.total_token_usage` → `input_tokens` / `output_tokens` /
    `cached_input_tokens` / `reasoning_output_tokens` (token usage)
  - `rate_limits` → primary/secondary windows with `used_percent` + reset
    info (a **live-quota equivalent**, sourced from API response metadata)
- **Caveat (this machine):** the local sessions are all from an *old* Codex
  build (March); they contain neither `token_count` nor `rate_limits`. So Codex
  support requires a reasonably recent Codex version, and the schema is
  **undocumented and version-dependent** — we must parse defensively and treat
  every field as optional.

### Consequence

Contrary to the first guess, Codex usage *and* a quota-style view are both
feasible **purely from local logs, no network call** — IF the user is on a
recent Codex. We never hit an OpenAI endpoint; we read what Codex already wrote.

## Proposed design

### 1. Provider dimension (kept minimal)

Introduce one new axis: **provider** = `claude` | `codex`. Per the "one toggle"
preference, surface it as a single switch (e.g. a small `Claude ⇄ Codex` toggle
in the header). Everything else (Live/Local segmented control, themes, window)
stays as-is.

Behaviour per provider:

| Provider | Live quota | Local activity |
|---|---|---|
| Claude | OAuth usage endpoint (today) | `~/.claude/projects` scan (today) |
| Codex | `rate_limits` parsed from latest `token_count` in `~/.codex/sessions` (if present) | `~/.codex/sessions` scan → per-model tokens/$ |

If Codex Live data isn't available (old version / not found), the Live tab for
Codex shows a clear "not available — needs a recent Codex" state instead of a
fake bar. Never fabricate a quota.

### 2. Backend

- New module `codexstats.rs`, mirroring `localstats.rs`: walk
  `~/.codex/sessions/**/*.jsonl`, sum `total_token_usage` per `model`, window by
  timestamp, reuse the existing `ModelUsageDTO` / `LocalReport` shapes so the
  frontend renders it with zero new types.
- New module (or extend `usage.rs`) for Codex "live": read the most recent
  `token_count` event's `rate_limits` and map to `UsageWindow`s. Optional /
  best-effort.
- **Pricing**: `pricing.json` is Claude-only. Add an OpenAI price table (per
  model id, incl. cached-input rate) so Codex $ estimates work. Models without a
  price entry show tokens only (no $), exactly like unknown Claude ids today.
- Commands: `fetch_local` / `fetch_usage` gain a `provider` arg, or add parallel
  `fetch_codex_local` / `fetch_codex_usage`. (Leaning toward a `provider` arg to
  keep the command surface small.)

### 3. Frontend

- `state.provider: "claude" | "codex"`, persisted like theme.
- Header toggle; on change, re-fetch and re-render.
- Account footer: already Live-only and Claude-specific — also gate it to
  `provider === "claude"` (a Codex/OpenAI login isn't the Claude account).

## Open questions / risks

1. **Codex schema drift** — undocumented; pin to the user's installed version
   and parse every field as optional. Need a real recent-Codex log sample to
   finalize field names (this machine's are too old).
2. **Codex "account identity"** — is there an `~/.codex/`-local email/plan
   analogous to `~/.claude.json`? TBD; footer may stay Claude-only.
3. **Pricing accuracy/maintenance** — OpenAI prices change; bundle a table and
   accept staleness, same trade-off as the Claude table.
4. **Multi-account / org attribution** — out of scope for v1; this stays a
   per-machine local view.

## Suggested first step (when we move to code)

Ship **Codex Local activity only** (low risk, high value for cost-splitting),
behind the provider toggle, with Codex Live deferred until we have a recent-Codex
`rate_limits` sample to build against.
