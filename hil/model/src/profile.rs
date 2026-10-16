//! The stand profile: spans measured on the stand, from which the
//! infrastructure waits (boot to Hello, flash, a fixture's readiness) and
//! the plan's image costs derive.

use core::fmt;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Factor, repetitions};

/// The quantile every span reports.
pub const QUANTILE: f64 = 0.999;
/// The confidence that the reported value is at least that quantile.
pub const CONFIDENCE: f64 = 0.95;

/// The name of a measured span, such as `boot-to-hello` or
/// `flash.application`.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SpanName(pub String);

impl SpanName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

impl fmt::Display for SpanName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A sealed run that evidence names: its ID and the SHA-256 of its
/// bundle, the reference the evidence archive checks (#761 Q7).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RunRef {
    pub run: String,
    pub sha256: String,
}

/// One measured span.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MeasuredSpan {
    /// The largest of `samples` observations, which bounds the
    /// [`QUANTILE`] with [`CONFIDENCE`] once there are enough of them.
    pub quantile_micros: u64,
    pub samples: u64,
    pub margin: Factor,
    /// Why this margin: every bound records it (#761 Q5).
    pub margin_reason: String,
    /// Set while there are too few samples: why the span is used anyway.
    /// A provisional span is labelled in every plan that uses it.
    pub provisional: Option<String>,
    /// The runs the samples come from.
    pub evidence: Vec<RunRef>,
}

impl MeasuredSpan {
    /// Whether the samples support the quantile at the confidence.
    pub fn supported(&self) -> bool {
        self.samples >= repetitions::quantile_samples(QUANTILE, CONFIDENCE)
    }

    pub fn budget_micros(&self) -> u64 {
        self.margin.apply(self.quantile_micros)
    }
}

/// The measured spans of one stand.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct StandProfile {
    pub spans: BTreeMap<SpanName, MeasuredSpan>,
}

/// A profile entry that breaks the rules.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileError {
    NoMarginReason(SpanName),
    NoEvidence(SpanName),
    /// Too few samples and no provisional label.
    Unsupported {
        span: SpanName,
        samples: u64,
        required: u64,
    },
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMarginReason(span) => {
                write!(f, "span `{span}` does not say why its margin was chosen")
            }
            Self::NoEvidence(span) => write!(f, "span `{span}` names no run"),
            Self::Unsupported {
                span,
                samples,
                required,
            } => write!(
                f,
                "span `{span}` has {samples} samples, {required} are needed, and it is not labelled provisional"
            ),
        }
    }
}

impl std::error::Error for ProfileError {}

impl StandProfile {
    /// Every rule violation, in span order.
    pub fn validate(&self) -> Vec<ProfileError> {
        let required = repetitions::quantile_samples(QUANTILE, CONFIDENCE);
        let mut errors = Vec::new();
        for (name, span) in &self.spans {
            if span.margin_reason.trim().is_empty() {
                errors.push(ProfileError::NoMarginReason(name.clone()));
            }
            if span.evidence.is_empty() {
                errors.push(ProfileError::NoEvidence(name.clone()));
            }
            if !span.supported() && span.provisional.is_none() {
                errors.push(ProfileError::Unsupported {
                    span: name.clone(),
                    samples: span.samples,
                    required,
                });
            }
        }
        errors
    }

    /// The span's budget, when it is supported or labelled provisional.
    pub fn budget_micros(&self, name: &SpanName) -> Option<u64> {
        self.spans
            .get(name)
            .filter(|span| span.supported() || span.provisional.is_some())
            .map(MeasuredSpan::budget_micros)
    }

    /// The provisional spans among `names`, for a plan to label.
    pub fn provisional<'a>(
        &'a self,
        names: impl IntoIterator<Item = &'a SpanName>,
    ) -> Vec<&'a SpanName> {
        names
            .into_iter()
            .filter(|name| {
                self.spans
                    .get(*name)
                    .is_some_and(|span| !span.supported() && span.provisional.is_some())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU32;

    use super::*;

    fn span(samples: u64, provisional: Option<&str>) -> MeasuredSpan {
        MeasuredSpan {
            quantile_micros: 25_000_000,
            samples,
            margin: Factor::new(6, NonZeroU32::new(5).unwrap()).unwrap(),
            margin_reason: "flash time grows with the image".into(),
            provisional: provisional.map(Into::into),
            evidence: vec![RunRef {
                run: "r1".into(),
                sha256: "0".repeat(64),
            }],
        }
    }

    #[test]
    fn too_few_samples_need_a_provisional_label() {
        let name = SpanName::new("flash.application");
        let mut profile = StandProfile::default();
        profile.spans.insert(name.clone(), span(201, None));
        assert_eq!(
            profile.validate(),
            vec![ProfileError::Unsupported {
                span: name.clone(),
                samples: 201,
                required: 2_995
            }]
        );
        assert_eq!(profile.budget_micros(&name), None);

        profile
            .spans
            .insert(name.clone(), span(201, Some("first stand measurement")));
        assert!(profile.validate().is_empty());
        assert_eq!(profile.budget_micros(&name), Some(30_000_000));
        assert_eq!(profile.provisional([&name]), vec![&name]);
    }

    #[test]
    fn a_span_records_why_its_margin_was_chosen_and_its_runs() {
        let name = SpanName::new("boot-to-hello");
        let mut entry = span(3_000, None);
        entry.margin_reason = " ".into();
        entry.evidence.clear();
        let profile = StandProfile {
            spans: [(name.clone(), entry)].into(),
        };
        assert_eq!(
            profile.validate(),
            vec![
                ProfileError::NoMarginReason(name.clone()),
                ProfileError::NoEvidence(name)
            ]
        );
    }
}
