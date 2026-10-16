//! Verdicts and their recomputation.

use serde::{Deserialize, Serialize};

/// One violated check, by name, with what was observed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Finding {
    pub check: String,
    pub detail: String,
}

impl Finding {
    pub fn new(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            detail: detail.into(),
        }
    }
}

/// A scenario's verdict over one trace. Infrastructure failures are not
/// verdicts: a run that could not record its trace has none.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "verdict")]
pub enum Verdict {
    Pass,
    Fail { findings: Vec<Finding> },
}

impl Verdict {
    /// `Pass` exactly when there is no finding.
    pub fn from_findings(findings: Vec<Finding>) -> Self {
        if findings.is_empty() {
            Self::Pass
        } else {
            Self::Fail { findings }
        }
    }

    pub const fn passed(&self) -> bool {
        matches!(self, Self::Pass)
    }
}

/// A recomputed verdict next to the recorded one. A difference is
/// reported; the recorded verdict is never overwritten.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Agreement {
    Same,
    Differs {
        recorded: Verdict,
        recomputed: Verdict,
    },
}

impl Agreement {
    pub fn of(recorded: &Verdict, recomputed: Verdict) -> Self {
        if *recorded == recomputed {
            Self::Same
        } else {
            Self::Differs {
                recorded: recorded.clone(),
                recomputed,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_finding_is_a_pass() {
        assert_eq!(Verdict::from_findings(Vec::new()), Verdict::Pass);
        let failed = Verdict::from_findings(vec![Finding::new("order", "terminal 2 before 1")]);
        assert!(!failed.passed());
    }

    #[test]
    fn a_changed_verdict_is_reported_beside_the_recorded_one() {
        let recorded = Verdict::Pass;
        let recomputed = Verdict::from_findings(vec![Finding::new("bound", "1200 us > 1000 us")]);
        assert_eq!(Agreement::of(&recorded, Verdict::Pass), Agreement::Same);
        assert_eq!(
            Agreement::of(&recorded, recomputed.clone()),
            Agreement::Differs {
                recorded,
                recomputed
            }
        );
    }
}
