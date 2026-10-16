# Automatic device-share fitting

HPBar estimates the part of an account-wide quota used by this machine from
local session logs. Providers do not report device shares. API-equivalent cost
is a proxy, not the provider's subscription metering formula.

Code: [`src-tauri/src/share.rs`](../src-tauri/src/share.rs).
Run `cargo run --example share_check` for local fit diagnostics.

## No setup or calibration switch

Fitting runs automatically during ordinary use, including background polling.
The old **Only Device Here** control has been removed; previously saved assertions
cannot force confidence. Changing local activity and idle periods provide the
variation needed to distinguish local usage from background account usage.

Contiguous observations are accumulated into at least ten-minute blocks to
reduce polling lag and quota rounding. A robust Theil–Sen regression fits:

```
quota per minute ≈ local API-equivalent dollars per minute × slope
                   + background account quota per minute
window budget Q ≈ 1 / slope
this machine ≈ local window cost / Q
other usage ≈ reported utilization − this machine
```

A visible split requires at least eight blocks, four with local spend, ten
percentage points of observed quota movement, substantial variation in local
activity, low prediction error, and agreement between independent fits of the
earlier and later halves. Only the latest 96 blocks, within two days or one
quota cycle (whichever is shorter), contribute. Constant activity cannot
identify the split regardless of how many samples are collected.

**Learning device split** means usable observations exist but the fit is not
identifiable or stable yet. **Split unavailable** means fresh, complete
observations are missing. Neither displays an invented percentage. The displayed
percentages are fractions of the whole quota window, not just consumed usage.

## Data alignment and change detection

- Both sides describe the same observation time and reset window, including
  the event at a Codex quota snapshot's timestamp.
- Pending local spend includes subsequent idle polls' quota increases.
- Blocks never cross reset IDs, counter decreases, non-increasing timestamps,
  or gaps longer than 30 minutes.
- History is isolated by provider, account, window duration, pricing fingerprint
  and estimator version. Updating accounting/rates starts fresh history.
- Only usage attributed to the current account is included. Unknown historical
  attribution, unknown prices and unclassified tokens prevent a split. After
  installation or account-observation gaps, the window may need to roll over
  before local coverage is complete.
- Observations older than 15 minutes, expired/implausible resets, saturated quota
  and a live reading that disagrees with recorded data are rejected.
- Impossible local shares and contradictory recent slopes suppress the split.
  Fits must agree across recent halves, so a changed budget requires new evidence.
- Up to 64 days of observations are retained without thinning interval endpoints.

## Remaining uncertainty

The regression models background usage as approximately steady within its
recent evidence. Irregular concurrent usage, changed model mixes or delayed
provider accounting can prevent a fit. Other devices working in proportion to
this machine are fundamentally indistinguishable from a different quota budget.
Web/mobile use also appears as inferred other usage. Even a stable fit is an
estimate, never a billing measurement or proof of which device consumed quota.

Codex cumulative deltas prevent duplicate token events; cached input, cache
writes and reasoning output are not counted twice. Claude messages are
deduplicated before request-level pricing. These accounting safeguards apply
to both the fit and the usage calendar.
