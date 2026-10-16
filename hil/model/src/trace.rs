//! The header every conformance trace starts with: the bounds the image
//! reports and the clock its times are on.

use serde::{Deserialize, Serialize};

use crate::{BoundName, ClockName, ReportedBounds};

/// The clock a trace's times are on: a backend's monotonic clock of one
/// generation (#563 decision 6). A clock starts a new generation when it
/// restarts, at a boot or a port's restart, so times of different
/// generations are never compared directly.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ClockDomain {
    pub clock: ClockName,
    pub generation: u32,
}

/// What every conformance trace of every block starts with.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TraceHeader {
    /// The backend's actual bounds, as the image reports them.
    pub bounds: ReportedBounds,
    pub clock: ClockDomain,
}

/// What is wrong with the headers of one run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "header")]
pub enum HeaderFinding {
    /// The backend reports a bound above the contract's ceiling.
    AboveCeiling {
        bound: BoundName,
        reported: u64,
        ceiling: u64,
    },
    /// The contract has a ceiling the backend does not report.
    Unreported { bound: BoundName },
    /// The backend reports a bound the contract does not have.
    Uncontracted { bound: BoundName },
    /// The cases of one run carry different headers.
    Changed,
    /// The run has no trace at all.
    NoTrace,
}

impl TraceHeader {
    /// Every way the reported bounds break the contract's ceilings.
    pub fn against(&self, ceilings: &ReportedBounds) -> Vec<HeaderFinding> {
        let mut findings = Vec::new();
        for (bound, ceiling) in ceilings.iter() {
            match self.bounds.get(bound) {
                None => findings.push(HeaderFinding::Unreported {
                    bound: bound.clone(),
                }),
                Some(reported) if reported > ceiling => {
                    findings.push(HeaderFinding::AboveCeiling {
                        bound: bound.clone(),
                        reported,
                        ceiling,
                    });
                }
                Some(_) => {}
            }
        }
        for (bound, _) in self.bounds.iter() {
            if ceilings.get(bound).is_none() {
                findings.push(HeaderFinding::Uncontracted {
                    bound: bound.clone(),
                });
            }
        }
        findings
    }

    /// The one header of a run's traces, checked against the ceilings: a
    /// run whose cases carry different headers, or that has none, is a
    /// finding.
    pub fn of_run<'a>(
        headers: impl IntoIterator<Item = &'a Self>,
        ceilings: &ReportedBounds,
    ) -> Vec<HeaderFinding> {
        let mut headers = headers.into_iter();
        let Some(first) = headers.next() else {
            return vec![HeaderFinding::NoTrace];
        };
        let mut findings = first.against(ceilings);
        if headers.any(|header| header != first) {
            findings.push(HeaderFinding::Changed);
        }
        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name;

    fn header(terminal: u64, generation: u32) -> TraceHeader {
        TraceHeader {
            bounds: ReportedBounds::default().with(name!(BoundName, "toy.terminal"), terminal),
            clock: ClockDomain {
                clock: name!(ClockName, "esp32s31.systimer"),
                generation,
            },
        }
    }

    fn ceilings() -> ReportedBounds {
        ReportedBounds::default().with(name!(BoundName, "toy.terminal"), 1_000)
    }

    #[test]
    fn a_bound_above_its_ceiling_is_a_finding() {
        assert!(header(800, 0).against(&ceilings()).is_empty());
        assert_eq!(
            header(1_200, 0).against(&ceilings()),
            vec![HeaderFinding::AboveCeiling {
                bound: name!(BoundName, "toy.terminal"),
                reported: 1_200,
                ceiling: 1_000
            }]
        );
    }

    #[test]
    fn missing_and_extra_bounds_are_findings() {
        let mut extra = header(800, 0);
        extra.bounds = ReportedBounds::default().with(name!(BoundName, "toy.other"), 1);
        assert_eq!(
            extra.against(&ceilings()),
            vec![
                HeaderFinding::Unreported {
                    bound: name!(BoundName, "toy.terminal")
                },
                HeaderFinding::Uncontracted {
                    bound: name!(BoundName, "toy.other")
                },
            ]
        );
    }

    #[test]
    fn a_run_has_one_header() {
        assert!(TraceHeader::of_run([&header(800, 3), &header(800, 3)], &ceilings()).is_empty());
        assert_eq!(
            TraceHeader::of_run([&header(800, 3), &header(800, 4)], &ceilings()),
            vec![HeaderFinding::Changed]
        );
        assert_eq!(
            TraceHeader::of_run([], &ceilings()),
            vec![HeaderFinding::NoTrace]
        );
    }
}
