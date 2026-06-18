# Device-share usage fitting

How HPBar estimates **"what % of this subscription window is *this* machine using"**
versus other devices on the same account — without any device id.

Code: [`src-tauri/src/share.rs`](../src-tauri/src/share.rs) ·
inspect a live fit with `cargo run --example share_check`.

## The problem

The provider only tells us **account-wide** utilization `U` (0..1) for each window
(Claude 5-Hour / Weekly; Codex 5-Hour / Weekly / Monthly). Our local logs only see
**this machine's** usage. There's no per-device breakdown anywhere, so other
devices (a second laptop, claude.ai, the mobile app) can only be *inferred*.

We need a bridge between "utilization %" and "tokens", because the limit isn't a
raw token count. Luckily both Claude and Codex meter by **model-weighted token
cost**, so this machine's **dollar cost** `C` (priced from `pricing.json`) is a sound
proxy. Define one unknown per window:

> **`Q` = the dollar-cost that equals 100% of the window** ("the budget").

Then, with `C_others` the (unseen) cost from other devices:

```
U ≈ (C + C_others) / Q
this_machine = C / Q          others = U − this_machine
```

Everything reduces to estimating the single number **`Q`**. `Q` quietly absorbs
every conversion constant (credits↔dollars, compute-hours↔tokens), so we never
need to know the provider's exact formula.

## The key idea

At any moment, look at the ratio

```
r = C / U      (this machine's cost ÷ account-wide utilization)
```

Other devices only ever **add** to `U`, never to `C`. So `r` is always **≤ Q**, and
`r = Q` exactly when this machine was the *only* contributor. In fact
`r = Q × (this machine's share at that moment)`.

So the budget is the **upper edge** of the observed ratios:

> **`Q` ≈ a high percentile (80th) of the recent `r` values.**

The high percentile catches the moments when this machine dominated (`r ≈ Q`) and
ignores the moments when other devices were also busy (`r` well below `Q`) — with no
need to know *which* moments were which. Using the cumulative ratio `C/U` (rather
than per-interval slopes `ΔU/ΔC`) makes it steady and robust: it isn't fooled by
this machine's own non-linearity (e.g. cheap cache tokens that cost `$` but barely
move the limit).

## The algorithm (per provider × window)

Every 5 minutes the background poller records a sample `(timestamp, U, C)` to
`share_history.json`. To produce an estimate:

1. Take all samples with `U ≥ 3%` and `C > 0`; compute `r = C / U` for each.
2. Weight each by **recency** (half-life = one window length) so the estimate
   re-learns after a limit change or a free reset; weight **calibrated** samples
   (see below) ×4.
3. **`Q` = 80th weighted percentile of `r`.**
4. `this_machine = clamp(C_now / Q, 0, U)`, `others = U − this_machine`.
5. **Confidence** ∈ [0,1] = `enough-samples × ratio-is-steady × fresh`. The UI
   shows the split only above ~0.35, shows "estimating…" below it, and hides it
   entirely with no data. (Cold start, < 3 samples: assume sole device, `Q = C/U`,
   low confidence.)

**Calibration shortcut.** The tray's *"Only Device Here"* toggle marks samples as
trusted sole-device data (`r = Q` by assertion). A few minutes of that makes the
fit confident almost immediately.

## How fast does it converge?

Convergence hinges on one thing: **how often is this machine the dominant user?**
The 80th percentile needs some samples where your share was high. Human usage
naturally bursts (devices rarely run in lockstep), so this usually happens on its
own. Rough times, assuming **2–4 devices** on the account and ~5-minute samples:

| Window | This machine is the *main* device | Genuinely balanced across devices |
|---|---|---|
| **5-Hour** | shows in **~15–30 min** of active use | **~1–3 h** (needs a few of your own busy bursts) |
| **Weekly** | **~1–3 days** | up to a week |
| **Monthly** (Codex) | **~1–2 weeks** | longer |
| **Any** with *"Only Device Here"* on | **~10–15 min** (one or two polls) | same |

It never "finishes" — with the recency half-life it keeps re-estimating, so it
tracks changing usage and adapts within ~2 window cycles to limit changes.

## Where it breaks (honest limits)

- **No solo moments.** If several devices run in near-constant proportion and *this*
  machine is never dominant, the ratios never reach the true `Q`, so it over-credits
  this machine and stays low-confidence. Identifiability is fundamentally limited
  without device ids; the calibration toggle is the escape hatch.
- **Different model mix per device.** The split assumes devices use a similar model
  mix; if one runs mostly Opus and another mostly Haiku, the cost-weighting skews it.
- **Local logs only.** claude.ai web, mobile, IDE extensions and API-key tools all
  count as "others" — by design.
- **Coarse data.** 5-minute polling and percent-granular `U` mean tiny usage produces
  no estimate; near 100% / overage the absolute split is unreliable.

## Is there a name for this? Better versions?

There's no single famous algorithm — it's a pragmatic combination — but it sits at
the intersection of a few well-studied ideas:

- **The problem** is *usage disaggregation* / *blind source separation*: split one
  aggregate signal into per-source parts. The classic instance is **Non-Intrusive
  Load Monitoring (NILM)** / *energy disaggregation* — inferring each appliance's
  draw from a single household meter. Our version is easier: we observe **one** of
  the sources (this machine) directly.
- **The estimator** — "the budget is the upper edge of `C/U`" — is **frontier /
  endpoint estimation**: estimating the boundary of a distribution's support, made
  robust with a **high quantile (order statistic)** instead of the raw maximum.
  Related named methods: *stochastic frontier analysis (SFA)*, *data envelopment
  analysis (DEA)*, *quantile regression*, and *extreme-value endpoint estimation*.
  (An earlier version used the **lower envelope of `ΔU/ΔC` slopes** — that's robust
  regression-through-origin, à la *Theil–Sen* / least-quantile regression — but the
  cumulative ratio proved steadier.)

**Better versions, roughly in order of payoff:**

1. **Cooperative disaggregation (exact).** If other devices also run HPBar and report
   their local cost to a shared store — the repo already has the opt-in team DB for
   exactly this kind of sharing — you *know* each cooperating device's contribution
   and only fit the truly-unobserved remainder (web/mobile). No identifiability
   problem, near-exact.
2. **Bayesian state-space filter (Kalman / particle).** Model `Q_t` as a slowly
   drifting latent state and `C_other,t` as a non-negative latent process, and infer
   both jointly online. This is the principled version that our recency-weighted
   quantile hack approximates: cleaner non-stationarity handling, proper uncertainty,
   faster convergence.
3. **Quantile regression of `U` on cumulative `C`** — a more standard estimator than
   "80th percentile of the ratio," with a real confidence interval.
4. **Learn the weights (non-negative least squares).** Instead of using `$` cost as a
   fixed bridge, fit per-token-type weights (`input`/`output`/`cache`) so the proxy
   matches the provider's actual metering — at the cost of needing more data.
5. **Explicit change-point detection** for limit changes, instead of leaning only on
   the recency half-life.
