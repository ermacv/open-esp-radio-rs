//! Lease budgets: parsing, display and estimation from earlier leases.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::history::{LeaseOutcome, LeaseRecord};

/// Budget of work with neither an explicit budget nor completed history.
pub const DEFAULT_BUDGET: Duration = Duration::from_secs(15 * 60);
/// Largest budget a short request may declare to be granted ahead of the head.
pub const MAX_SHORT_BUDGET: Duration = Duration::from_secs(2 * 60);
/// Completed leases of the same work considered by the estimate.
const HISTORY_SAMPLES: usize = 5;
const MIN_ESTIMATE: u64 = 60;
const ESTIMATE_GRANULARITY: u64 = 30;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum BudgetSource {
    Explicit,
    /// The longest of the most recent completed leases of the same work.
    History {
        samples: usize,
    },
    Default,
}

impl std::fmt::Display for BudgetSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Explicit => f.write_str("explicit"),
            Self::History { samples } => write!(f, "from {samples} earlier lease(s)"),
            Self::Default => f.write_str("default; no history for this work"),
        }
    }
}

/// Resolve the budget of `work`: an explicit value wins, then history, then
/// the default.
pub(crate) fn resolve(
    explicit: Option<Duration>,
    work: &str,
    history: &[LeaseRecord],
) -> (Duration, BudgetSource) {
    if let Some(budget) = explicit {
        return (budget, BudgetSource::Explicit);
    }
    let durations = history
        .iter()
        .rev()
        .filter(|record| record.work == work && record.outcome == LeaseOutcome::Released)
        .map(LeaseRecord::duration_secs)
        .take(HISTORY_SAMPLES)
        .collect::<Vec<_>>();
    match durations.iter().max() {
        Some(&longest) => {
            let rounded = longest.div_ceil(ESTIMATE_GRANULARITY) * ESTIMATE_GRANULARITY;
            (
                Duration::from_secs(rounded.max(MIN_ESTIMATE)),
                BudgetSource::History {
                    samples: durations.len(),
                },
            )
        }
        None => (DEFAULT_BUDGET, BudgetSource::Default),
    }
}

/// Parse `90s`, `15m`, `1h30m` or a plain number of minutes.
pub fn parse_duration(text: &str) -> crate::Result<Duration> {
    let text = text.trim();
    if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(Duration::from_secs(text.parse::<u64>()? * 60));
    }
    let mut total = 0u64;
    let mut number = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() {
            number.push(character);
            continue;
        }
        let unit = match character {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => {
                return Err(
                    format!("invalid duration `{text}`; use e.g. 90s, 15m or 1h30m").into(),
                );
            }
        };
        if number.is_empty() {
            return Err(format!("invalid duration `{text}`; a unit needs a number").into());
        }
        total += number.parse::<u64>()? * unit;
        number.clear();
    }
    if !number.is_empty() || total == 0 {
        return Err(format!("invalid duration `{text}`; use e.g. 90s, 15m or 1h30m").into());
    }
    Ok(Duration::from_secs(total))
}

/// `45s`, `12m30s`, `1h05m`.
pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match (seconds / 3600, seconds % 3600 / 60, seconds % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m{s:02}s"),
        (h, m, _) => format!("{h}h{m:02}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(work: &str, seconds: u64, outcome: LeaseOutcome) -> LeaseRecord {
        LeaseRecord {
            id: 0,
            owner: "wifi".into(),
            work: work.into(),
            granted_unix: 1000,
            released_unix: 1000 + seconds,
            budget_secs: 600,
            outcome,
        }
    }

    #[test]
    fn durations_round_trip_through_their_display() {
        for (text, seconds) in [("90s", 90), ("15m", 900), ("1h30m", 5400), ("20", 1200)] {
            assert_eq!(parse_duration(text).unwrap(), Duration::from_secs(seconds));
        }
        for text in ["", "m", "10x", "0s", "5m3"] {
            assert!(parse_duration(text).is_err(), "{text}");
        }
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(750)), "12m30s");
        assert_eq!(format_duration(Duration::from_secs(3900)), "1h05m");
    }

    #[test]
    fn estimates_use_the_longest_recent_completed_lease_of_the_same_work() {
        let history = [
            record("run a", 400, LeaseOutcome::Released),
            record("run a", 100, LeaseOutcome::Released),
            record("run a", 5000, LeaseOutcome::BudgetExceeded),
            record("run b", 3000, LeaseOutcome::Released),
            record("run a", 131, LeaseOutcome::Released),
        ];
        assert_eq!(
            resolve(None, "run a", &history),
            (
                Duration::from_secs(420),
                BudgetSource::History { samples: 3 }
            )
        );
        assert_eq!(
            resolve(None, "run c", &history),
            (DEFAULT_BUDGET, BudgetSource::Default)
        );
        assert_eq!(
            resolve(Some(Duration::from_secs(5)), "run a", &history).1,
            BudgetSource::Explicit
        );
        let short = [record("smoke", 3, LeaseOutcome::Released)];
        assert_eq!(resolve(None, "smoke", &short).0, Duration::from_secs(60));
    }
}
