//! The stand profile: spans measured on the stand, from which the
//! infrastructure waits (boot to Hello, flash, a fixture's readiness) and
//! the plan's image costs derive, and the bounds images of each recipe
//! reported.

use core::fmt;
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

use crate::{
    Bounds, BoundsSource, Factor, Provisional, ReportedBounds, Resolved, Sha256, SpanName,
    WaitError,
    repetitions::{self, Confidence, Probability},
};

/// The quantile every span reports.
pub const QUANTILE: Probability = match Probability::new(0.999) {
    Some(quantile) => quantile,
    None => panic!("0.999 is a probability"),
};
/// The confidence that the reported value is at least that quantile.
pub const CONFIDENCE: Confidence = match Confidence::new(0.95) {
    Some(confidence) => confidence,
    None => panic!("0.95 is a confidence"),
};

/// A run's identifier in the run store: decimal milliseconds, a hyphen and
/// lowercase hexadecimal digits, such as `1788729094522-000d170b`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RunId(String);

impl RunId {
    pub fn parse(text: &str) -> Option<Self> {
        let (millis, suffix) = text.split_once('-')?;
        let digits = !millis.is_empty() && millis.bytes().all(|b| b.is_ascii_digit());
        let hex = !suffix.is_empty()
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        (digits && hex).then(|| Self(text.to_owned()))
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for RunId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RunId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| D::Error::custom("a run ID is `<millis>-<hex>`"))
    }
}

/// A sealed run that evidence names: its ID and the SHA-256 of its
/// bundle, the reference the evidence archive checks (#761 Q7).
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RunRef {
    pub run: RunId,
    pub sha256: Sha256,
}

/// Whether a span's samples support its quantile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "support")]
pub enum Support {
    /// At least the samples [`QUANTILE`] at [`CONFIDENCE`] needs.
    Measured,
    /// Too few samples so far; why the span is used anyway. Every plan that
    /// uses it carries the label.
    Provisional { reason: String },
}

/// One measured span.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MeasuredSpan {
    /// The largest of `samples` observations, which bounds the
    /// [`QUANTILE`] with [`CONFIDENCE`] once there are enough of them.
    pub maximum_micros: u64,
    pub samples: u64,
    pub margin: Factor,
    /// Why this margin: every bound records it (#761 Q5).
    pub margin_reason: String,
    pub support: Support,
    /// The runs the samples come from.
    pub evidence: Vec<RunRef>,
}

/// The bounds an image of one recipe reported, and the run that recorded
/// them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RecordedBounds {
    pub bounds: ReportedBounds,
    pub evidence: RunRef,
}

/// A profile entry that breaks the rules.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileError {
    NoMarginReason(SpanName),
    NoEvidence(SpanName),
    NoProvisionalReason(SpanName),
    /// Too few samples for `Measured`.
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
            Self::NoProvisionalReason(span) => {
                write!(f, "span `{span}` is provisional without a reason")
            }
            Self::Unsupported {
                span,
                samples,
                required,
            } => {
                write!(
                    f,
                    "span `{span}` has {samples} samples and claims support; {required} are needed"
                )
            }
        }
    }
}

/// Every violation of a profile, in span order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileErrors(pub Vec<ProfileError>);

impl fmt::Display for ProfileErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            error.fmt(f)?;
        }
        Ok(())
    }
}

impl std::error::Error for ProfileErrors {}

/// The profile as written, before its rules are checked.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawStandProfile {
    pub spans: BTreeMap<SpanName, MeasuredSpan>,
    /// By image recipe digest.
    pub bounds: BTreeMap<Sha256, RecordedBounds>,
}

/// The measured spans of one stand and the bounds its images reported. It
/// exists only after its rules hold ([`TryFrom<RawStandProfile>`]).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "RawStandProfile", into = "RawStandProfile")]
pub struct StandProfile {
    spans: BTreeMap<SpanName, MeasuredSpan>,
    bounds: BTreeMap<Sha256, RecordedBounds>,
}

impl From<StandProfile> for RawStandProfile {
    fn from(profile: StandProfile) -> Self {
        Self {
            spans: profile.spans,
            bounds: profile.bounds,
        }
    }
}

impl TryFrom<RawStandProfile> for StandProfile {
    type Error = ProfileErrors;

    fn try_from(raw: RawStandProfile) -> Result<Self, Self::Error> {
        let required = repetitions::quantile_samples(QUANTILE, CONFIDENCE);
        let mut errors = Vec::new();
        for (name, span) in &raw.spans {
            if span.margin_reason.trim().is_empty() {
                errors.push(ProfileError::NoMarginReason(name.clone()));
            }
            if span.evidence.is_empty() {
                errors.push(ProfileError::NoEvidence(name.clone()));
            }
            match &span.support {
                Support::Measured if span.samples < required => {
                    errors.push(ProfileError::Unsupported {
                        span: name.clone(),
                        samples: span.samples,
                        required,
                    })
                }
                Support::Provisional { reason } if reason.trim().is_empty() => {
                    errors.push(ProfileError::NoProvisionalReason(name.clone()));
                }
                _ => {}
            }
        }
        if errors.is_empty() {
            Ok(Self {
                spans: raw.spans,
                bounds: raw.bounds,
            })
        } else {
            Err(ProfileErrors(errors))
        }
    }
}

impl StandProfile {
    /// A stand nothing has been measured on.
    pub fn empty() -> Self {
        Self {
            spans: BTreeMap::new(),
            bounds: BTreeMap::new(),
        }
    }

    /// A span's budget: its maximum times its margin, labelled when it is
    /// provisional.
    pub fn budget(&self, name: &SpanName) -> Result<Resolved, WaitError> {
        let span = self
            .spans
            .get(name)
            .ok_or_else(|| WaitError::NoSpan(name.clone()))?;
        let provisional = match &span.support {
            Support::Measured => None,
            Support::Provisional { reason } => Some(Provisional::Span {
                span: name.clone(),
                reason: reason.clone(),
            }),
        };
        Ok(Resolved {
            micros: span.margin.apply(span.maximum_micros)?,
            provisional,
        })
    }

    /// The bounds for an image of `recipe`: those its recorded run
    /// reported, or else the contract's declared constants, which make
    /// every wait on them provisional.
    pub fn bounds_for(
        &self,
        recipe: &Sha256,
        contract: &'static str,
        declared: ReportedBounds,
    ) -> Bounds {
        match self.bounds.get(recipe) {
            Some(recorded) => Bounds {
                values: recorded.bounds.clone(),
                source: BoundsSource::Recorded(recorded.evidence.clone()),
            },
            None => Bounds {
                values: declared,
                source: BoundsSource::Declared { contract },
            },
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use core::num::NonZeroU32;

    use super::*;
    use crate::name;

    pub(crate) fn run_ref() -> RunRef {
        RunRef {
            run: RunId::parse("1788729094522-000d170b").unwrap(),
            sha256: Sha256([0; 32]),
        }
    }

    pub(crate) fn span(maximum_micros: u64, support: Support) -> MeasuredSpan {
        MeasuredSpan {
            maximum_micros,
            samples: 201,
            margin: Factor::new(6, NonZeroU32::new(5).unwrap()).unwrap(),
            margin_reason: "flash time grows with the image".into(),
            support,
            evidence: vec![run_ref()],
        }
    }

    fn profile(spans: Vec<(SpanName, MeasuredSpan)>) -> Result<StandProfile, ProfileErrors> {
        StandProfile::try_from(RawStandProfile {
            spans: spans.into_iter().collect(),
            bounds: BTreeMap::new(),
        })
    }

    #[test]
    fn too_few_samples_cannot_claim_support() {
        let flash = name!(SpanName, "flash.application");
        assert_eq!(
            profile(vec![(flash.clone(), span(25_000_000, Support::Measured))]),
            Err(ProfileErrors(vec![ProfileError::Unsupported {
                span: flash.clone(),
                samples: 201,
                required: 2_995
            }]))
        );
        let provisional = Support::Provisional {
            reason: "first stand measurement".into(),
        };
        let profile = profile(vec![(flash.clone(), span(25_000_000, provisional))]).unwrap();
        let resolved = profile.budget(&flash).unwrap();
        assert_eq!(resolved.micros, 30_000_000);
        assert!(matches!(
            resolved.provisional,
            Some(Provisional::Span { .. })
        ));
    }

    #[test]
    fn a_span_records_why_its_margin_was_chosen_and_its_runs() {
        let boot = name!(SpanName, "boot-to-hello");
        let mut entry = span(900_000, Support::Provisional { reason: " ".into() });
        entry.margin_reason = " ".into();
        entry.evidence.clear();
        assert_eq!(
            profile(vec![(boot.clone(), entry)]),
            Err(ProfileErrors(vec![
                ProfileError::NoMarginReason(boot.clone()),
                ProfileError::NoEvidence(boot.clone()),
                ProfileError::NoProvisionalReason(boot),
            ]))
        );
    }

    #[test]
    fn a_profile_is_read_only_through_its_rules() {
        let text = r#"{"spans":{"boot-to-hello":{"maximum-micros":1,"samples":1,"margin":[1,1],
            "margin-reason":"r","support":{"support":"measured"},"evidence":[]}},"bounds":{}}"#;
        assert!(serde_json::from_str::<StandProfile>(text).is_err());
    }

    #[test]
    fn a_new_recipe_uses_the_declared_bounds() {
        let drain = name!(BoundName, "lower-mac.drain");
        let recipe = Sha256([7; 32]);
        let mut raw = RawStandProfile::default();
        raw.bounds.insert(
            recipe,
            RecordedBounds {
                bounds: ReportedBounds::default().with(drain.clone(), 3_000),
                evidence: run_ref(),
            },
        );
        let profile = StandProfile::try_from(raw).unwrap();
        let declared = ReportedBounds::default().with(drain.clone(), 5_000);
        let known = profile.bounds_for(&recipe, "toy::Port", declared.clone());
        assert_eq!(known.values.get(&drain), Some(3_000));
        assert!(matches!(known.source, BoundsSource::Recorded(_)));
        let new = profile.bounds_for(&Sha256([8; 32]), "toy::Port", declared);
        assert_eq!(new.values.get(&drain), Some(5_000));
        assert_eq!(
            new.source,
            BoundsSource::Declared {
                contract: "toy::Port"
            }
        );
    }
}
