//! Resolve old uploaded prices at read time, without rewriting anyone's history.
use crate::pricing::Pricing;

#[derive(Debug)]
pub(super) struct ResolvedCost {
    pub value: Option<f64>,
    pub complete: bool,
    pub estimated: bool,
}

pub(super) fn resolve(
    pricing: &Pricing,
    model: &str,
    components: [i64; 4],
    tokens: i64,
    unattributed: i64,
    stored: f64,
    shared: bool,
) -> ResolvedCost {
    let unavailable = || ResolvedCost {
        value: None,
        complete: false,
        estimated: false,
    };
    if !shared || components.iter().any(|n| *n < 0) || tokens < 0 || unattributed < 0 {
        return unavailable();
    }
    // A legacy dollar value for an unknown model is not a verified rate. In
    // particular, auto-review must not keep an old client's guessed GPT price.
    let Some(base) = pricing.cost(
        model,
        components[0],
        components[1],
        components[2],
        components[3],
        0,
    ) else {
        return unavailable();
    };
    if stored.is_finite() && stored > 0.0 {
        return ResolvedCost {
            value: Some(stored),
            complete: unattributed == 0,
            estimated: false,
        };
    }
    let known: i128 = components.iter().map(|n| *n as i128).sum();
    if known == 0 && tokens > 0 || known > tokens as i128 {
        return unavailable();
    }
    // Daily aggregates do not retain request context lengths, cache TTLs or
    // peak/off-peak timestamps. Use base rates and explicitly mark the estimate;
    // never treat an entire day's tokens as one long-context request, and never
    // invent an input/output split for unattributed tokens.
    ResolvedCost {
        value: Some(base.total()),
        complete: unattributed == 0 && known == tokens as i128,
        estimated: tokens > 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_zero_cost_recovers_known_components_at_base_rates() {
        let p = Pricing::bundled();
        let c = resolve(
            &p,
            "gpt-6.1-sol",
            [1_000_000, 100_000, 2_000_000, 0],
            3_100_000,
            0,
            0.0,
            true,
        );
        assert!((c.value.unwrap() - 3.2).abs() < 1e-9);
        assert!(c.complete && c.estimated);
        let c = resolve(
            &p,
            "ATXC44Y9/gpt-6-astra",
            [1_000_000, 0, 0, 0],
            1_000_000,
            0,
            0.0,
            true,
        );
        assert_eq!(c.value, Some(10.0));
    }

    #[test]
    fn partial_cost_is_retained_and_missing_components_are_never_invented() {
        let p = Pricing::bundled();
        let c = resolve(
            &p,
            "gpt-6.1-sol",
            [1_000_000, 0, 0, 0],
            1_000_123,
            123,
            0.0,
            true,
        );
        assert_eq!(c.value, Some(2.0));
        assert!(!c.complete && c.estimated);
        assert!(resolve(&p, "gpt-6.1-sol", [0; 4], 123, 123, 0.0, true)
            .value
            .is_none());
        assert!(!resolve(&p, "gpt-6.1-sol", [100, 0, 0, 0], 123, 0, 0.0, true).complete);
    }

    #[test]
    fn recorded_request_prices_survive_but_hidden_and_unknown_costs_do_not() {
        let p = Pricing::bundled();
        let c = resolve(
            &p,
            "gpt-6-astra",
            [1_000_000, 0, 0, 0],
            1_000_000,
            0,
            20.0,
            true,
        );
        assert_eq!(c.value, Some(20.0));
        assert!(c.complete && !c.estimated);
        let partial = resolve(
            &p,
            "gpt-6-astra",
            [1_000_000, 0, 0, 0],
            1_000_123,
            123,
            20.0,
            true,
        );
        assert_eq!(partial.value, Some(20.0));
        assert!(!partial.complete && !partial.estimated);
        assert!(
            resolve(&p, "codex-auto-review", [1_000; 4], 4_000, 0, 28.6, true)
                .value
                .is_none()
        );
        assert!(resolve(&p, "gpt-6-astra", [1_000; 4], 4_000, 0, 0.0, false)
            .value
            .is_none());
        assert_eq!(
            resolve(&p, "gpt-6-astra", [0; 4], 0, 0, 12.0, true).value,
            Some(12.0)
        );
        assert!(resolve(&p, "gpt-6-astra", [-1, 0, 0, 0], 0, 0, 0.0, true)
            .value
            .is_none());
    }
}
