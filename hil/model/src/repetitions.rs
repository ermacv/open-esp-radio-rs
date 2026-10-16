//! How many repetitions or samples a property needs, from the property
//! itself rather than habit (#761, principle 4).
//!
//! - A **bound** reported as the maximum of `n` samples covers the
//!   quantile `q` with confidence `c` once `qⁿ ≤ 1 − c`
//!   ([`quantile_samples`]).
//! - A **loss rate** the protocol admits is shown to be at most `p` with
//!   confidence `c` when the Clopper–Pearson upper bound of the observed
//!   failures is at most `p` ([`loss_trials`], [`loss_upper_bound`]).
//! - A **performance comparison** of two revisions needs, per arm, the
//!   repetitions that detect a relative effect at the measured coefficient
//!   of variation ([`comparison_repetitions`]).
//!
//! A deterministic property with no admitted loss needs one repetition:
//! one failure is a finding.

/// The smallest `n` with `qⁿ ≤ 1 − c`: the maximum of `n` independent
/// samples then lies at or above the `q` quantile with confidence `c`.
///
/// # Panics
///
/// When `q` or `c` is not strictly between zero and one.
pub fn quantile_samples(q: f64, c: f64) -> u64 {
    assert!(
        0.0 < q && q < 1.0 && 0.0 < c && c < 1.0,
        "a quantile and a confidence lie in (0, 1)"
    );
    let n = ((1.0 - c).ln() / q.ln()).ceil();
    // `n` is a positive finite whole number here.
    n as u64
}

/// The probability of at most `k` failures in `n` trials of failure
/// probability `p`.
fn binomial_cdf(k: u64, n: u64, p: f64) -> f64 {
    if p <= 0.0 {
        return 1.0;
    }
    if p >= 1.0 {
        return if k >= n { 1.0 } else { 0.0 };
    }
    let (ln_p, ln_q) = (p.ln(), (1.0 - p).ln());
    let mut ln_choose = 0.0;
    let mut total = 0.0;
    for i in 0..=k.min(n) {
        if i > 0 {
            ln_choose += ((n - i + 1) as f64).ln() - (i as f64).ln();
        }
        total += (ln_choose + i as f64 * ln_p + (n - i) as f64 * ln_q).exp();
    }
    total.min(1.0)
}

/// The Clopper–Pearson upper bound, at confidence `c`, of a failure rate
/// after `failures` in `trials`.
///
/// # Panics
///
/// When `failures > trials` or `c` is not in (0, 1).
pub fn loss_upper_bound(failures: u64, trials: u64, c: f64) -> f64 {
    assert!(
        failures <= trials && 0.0 < c && c < 1.0,
        "failures within trials, confidence in (0, 1)"
    );
    if failures == trials {
        return 1.0;
    }
    // The smallest p with P(X ≤ failures | trials, p) ≤ 1 − c.
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    for _ in 0..100 {
        let middle = (low + high) / 2.0;
        if binomial_cdf(failures, trials, middle) > 1.0 - c {
            low = middle;
        } else {
            high = middle;
        }
    }
    high
}

/// The smallest number of trials whose upper bound shows a failure rate of
/// at most `p` with confidence `c`, if no more than `failures` occur.
///
/// # Panics
///
/// When `p` or `c` is not in (0, 1).
pub fn loss_trials(p: f64, c: f64, failures: u64) -> u64 {
    assert!(
        0.0 < p && p < 1.0 && 0.0 < c && c < 1.0,
        "a rate and a confidence lie in (0, 1)"
    );
    let mut n = failures + 1;
    while binomial_cdf(failures, n, p) > 1.0 - c {
        n += 1;
    }
    n
}

/// The standard normal quantile (Acklam's rational approximation, relative
/// error below 1.2e-9).
fn normal_quantile(probability: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if probability < 0.024_25 {
        tail((-2.0 * probability.ln()).sqrt())
    } else if probability > 1.0 - 0.024_25 {
        -tail((-2.0 * (1.0 - probability).ln()).sqrt())
    } else {
        let q = probability - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    }
}

/// Repetitions per arm for a two-sided comparison of two revisions that
/// detects a relative effect `effect` (0.05 for 5 %) at the coefficient of
/// variation `cv` measured on earlier runs, at significance `alpha` and
/// `power`.
///
/// # Panics
///
/// When an argument is outside its range.
pub fn comparison_repetitions(cv: f64, effect: f64, alpha: f64, power: f64) -> u64 {
    assert!(
        cv > 0.0 && effect > 0.0 && 0.0 < alpha && alpha < 1.0 && 0.0 < power && power < 1.0,
        "positive variation and effect, alpha and power in (0, 1)"
    );
    let z = normal_quantile(1.0 - alpha / 2.0) + normal_quantile(power);
    let n = (2.0 * z * z * cv * cv / (effect * effect)).ceil();
    // `n` is a positive finite whole number here.
    (n as u64).max(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_maximum_of_2995_samples_covers_the_999th_permille_at_95_percent() {
        assert_eq!(quantile_samples(0.999, 0.95), 2_995);
        assert_eq!(quantile_samples(0.99, 0.95), 299);
    }

    #[test]
    fn loss_trials_match_the_zero_failure_closed_form() {
        // ln(0.05) / ln(0.8) = 13.4.
        assert_eq!(loss_trials(0.2, 0.95, 0), 14);
        // More admitted failures need more trials.
        assert!(loss_trials(0.2, 0.95, 3) > loss_trials(0.2, 0.95, 1));
    }

    #[test]
    fn the_upper_bound_is_consistent_with_the_trial_count() {
        let n = loss_trials(0.2, 0.95, 2);
        assert!(loss_upper_bound(2, n, 0.95) <= 0.2 + 1e-9);
        assert!(loss_upper_bound(2, n - 1, 0.95) > 0.2);
        assert_eq!(loss_upper_bound(5, 5, 0.95), 1.0);
    }

    #[test]
    fn the_normal_quantile_matches_reference_values() {
        assert!((normal_quantile(0.975) - 1.959_964).abs() < 1e-5);
        assert!((normal_quantile(0.8) - 0.841_621).abs() < 1e-5);
        assert!((normal_quantile(0.001) + 3.090_232).abs() < 1e-5);
    }

    #[test]
    fn comparison_repetitions_follow_the_variance() {
        // 2 (1.96 + 0.8416)² 0.1² / 0.05² = 62.8.
        assert_eq!(comparison_repetitions(0.1, 0.05, 0.05, 0.8), 63);
        assert!(comparison_repetitions(0.02, 0.05, 0.05, 0.8) < 63);
    }
}
