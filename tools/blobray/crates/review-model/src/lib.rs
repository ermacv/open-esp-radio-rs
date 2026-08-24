//! Sparse, architecture-neutral reviewed knowledge.
//!
//! Generated observations remain outside this crate. A [`ReviewPack`] records
//! only accepted human decisions and the evidence/applicability that bounds
//! them. Consumers own the vocabulary carried by `subject`, `kind` and
//! `value`; this crate owns fail-closed composition.

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use open_radio_vendor_contracts::{FactAccuracy, FactCompleteness, FactProvenance};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read reviewed-knowledge pack {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot parse reviewed-knowledge pack {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml_edit::de::Error,
    },

    #[error("invalid reviewed-knowledge pack {pack:?}: {reason}")]
    Invalid { pack: String, reason: String },

    #[error(
        "reviewed assertion conflict for {subject:?}/{kind:?}: {left:?} and {right:?} have overlapping applicability"
    )]
    Conflict {
        subject: String,
        kind: String,
        left: String,
        right: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FactClassification {
    pub provenance: FactProvenance,
    pub accuracy: FactAccuracy,
    pub completeness: FactCompleteness,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ArtifactApplicability {
    pub source: String,
    pub sha256: String,
}

/// Bounds one fact to reusable ecosystem, chip and artifact identities.
///
/// An empty dimension is a wildcard. Pack and record applicability are
/// intersected, never overridden.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Applicability {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ecosystems: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chips: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chip_revisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_lineages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactApplicability>,
}

impl Applicability {
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        Some(Self {
            ecosystems: intersect_dimension(&self.ecosystems, &other.ecosystems)?,
            chips: intersect_dimension(&self.chips, &other.chips)?,
            chip_revisions: intersect_dimension(&self.chip_revisions, &other.chip_revisions)?,
            artifact_lineages: intersect_dimension(
                &self.artifact_lineages,
                &other.artifact_lineages,
            )?,
            artifacts: intersect_dimension(&self.artifacts, &other.artifacts)?,
        })
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.intersection(other).is_some()
    }

    pub fn is_unbounded(&self) -> bool {
        self.ecosystems.is_empty()
            && self.chips.is_empty()
            && self.chip_revisions.is_empty()
            && self.artifact_lineages.is_empty()
            && self.artifacts.is_empty()
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct EvidenceReference {
    /// Stable ID of a reviewed evidence catalog entry.
    pub source: String,
    /// Location within that source, such as a function-relative site, header
    /// declaration or HIL observation ID.
    pub locator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Scalar/list values keep reviewed diffs small and deterministic. Structured
/// domain-specific data belongs in several independently identified facts.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssertionValue {
    Boolean(bool),
    Integer(i64),
    String(String),
    Strings(Vec<String>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReviewedAssertion {
    pub id: String,
    /// Stable entity reference, for example `mmio:cpu:0x20103128/32` or a
    /// revision-independent function identity.
    pub subject: String,
    /// Consumer-owned fact vocabulary, for example `register-name` or
    /// `hardware-write-semantics`.
    pub kind: String,
    pub value: AssertionValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<FactClassification>,
    #[serde(default, skip_serializing_if = "Applicability::is_unbounded")]
    pub applies_to: Applicability,
    pub evidence: Vec<EvidenceReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VendorBugStatus {
    Suspected,
    Reviewed,
    Resolved,
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct VendorBug {
    pub id: String,
    /// Stable function identity; absolute instruction addresses belong in
    /// evidence locators instead.
    pub function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub register: Option<String>,
    pub kind: String,
    pub status: VendorBugStatus,
    pub observed: String,
    pub expected: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<FactClassification>,
    #[serde(default, skip_serializing_if = "Applicability::is_unbounded")]
    pub applies_to: Applicability,
    pub evidence: Vec<EvidenceReference>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReviewPack {
    pub schema: u32,
    pub id: String,
    pub classification: FactClassification,
    #[serde(default, skip_serializing_if = "Applicability::is_unbounded")]
    pub applies_to: Applicability,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<ReviewedAssertion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vendor_bugs: Vec<VendorBug>,
}

impl ReviewPack {
    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        let pack = toml_edit::de::from_str(&input).map_err(|source| Error::Parse {
            path: path.to_owned(),
            source,
        })?;
        validate_pack(&pack)?;
        Ok(pack)
    }

    pub fn from_toml(input: &str) -> Result<Self> {
        let pack = toml_edit::de::from_str(input).map_err(|source| Error::Parse {
            path: PathBuf::from("<memory>"),
            source,
        })?;
        validate_pack(&pack)?;
        Ok(pack)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EffectiveAssertion {
    pub pack: String,
    pub id: String,
    pub subject: String,
    pub kind: String,
    pub value: AssertionValue,
    pub classification: FactClassification,
    pub applies_to: Applicability,
    pub evidence: Vec<EvidenceReference>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EffectiveVendorBug {
    pub pack: String,
    pub id: String,
    pub function: String,
    pub register: Option<String>,
    pub kind: String,
    pub status: VendorBugStatus,
    pub observed: String,
    pub expected: String,
    pub classification: FactClassification,
    pub applies_to: Applicability,
    pub evidence: Vec<EvidenceReference>,
}

/// Deterministically merged, fail-closed reviewed knowledge.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ReviewKnowledge {
    assertions: BTreeMap<String, EffectiveAssertion>,
    vendor_bugs: BTreeMap<String, EffectiveVendorBug>,
}

impl ReviewKnowledge {
    pub fn load_all(paths: &[PathBuf]) -> Result<Self> {
        let packs = paths
            .iter()
            .map(|path| ReviewPack::load(path))
            .collect::<Result<Vec<_>>>()?;
        Self::merge(packs)
    }

    pub fn merge(packs: impl IntoIterator<Item = ReviewPack>) -> Result<Self> {
        let mut packs = packs.into_iter().collect::<Vec<_>>();
        packs.sort_by(|left, right| left.id.cmp(&right.id));
        let mut pack_ids = BTreeSet::new();
        let mut record_ids = BTreeSet::new();
        let mut assertions = BTreeMap::new();
        let mut vendor_bugs = BTreeMap::new();
        let mut semantic_assertions =
            BTreeMap::<(String, String), Vec<(String, Applicability)>>::new();

        for pack in packs {
            validate_pack(&pack)?;
            if !pack_ids.insert(pack.id.clone()) {
                return invalid(&pack.id, format!("duplicate pack id {:?}", pack.id));
            }
            for assertion in pack.assertions {
                if !record_ids.insert(assertion.id.clone()) {
                    return invalid(&pack.id, format!("duplicate record id {:?}", assertion.id));
                }
                let applies_to = effective_applicability(
                    &pack.id,
                    &assertion.id,
                    &pack.applies_to,
                    &assertion.applies_to,
                )?;
                let key = (assertion.subject.clone(), assertion.kind.clone());
                if let Some((previous, _)) = semantic_assertions
                    .get(&key)
                    .into_iter()
                    .flatten()
                    .find(|(_, previous)| previous.overlaps(&applies_to))
                {
                    return Err(Error::Conflict {
                        subject: assertion.subject,
                        kind: assertion.kind,
                        left: previous.clone(),
                        right: assertion.id,
                    });
                }
                semantic_assertions
                    .entry(key)
                    .or_default()
                    .push((assertion.id.clone(), applies_to.clone()));
                assertions.insert(
                    assertion.id.clone(),
                    EffectiveAssertion {
                        pack: pack.id.clone(),
                        id: assertion.id,
                        subject: assertion.subject,
                        kind: assertion.kind,
                        value: assertion.value,
                        classification: assertion.classification.unwrap_or(pack.classification),
                        applies_to,
                        evidence: assertion.evidence,
                        note: assertion.note,
                    },
                );
            }
            for bug in pack.vendor_bugs {
                if !record_ids.insert(bug.id.clone()) {
                    return invalid(&pack.id, format!("duplicate record id {:?}", bug.id));
                }
                let applies_to =
                    effective_applicability(&pack.id, &bug.id, &pack.applies_to, &bug.applies_to)?;
                vendor_bugs.insert(
                    bug.id.clone(),
                    EffectiveVendorBug {
                        pack: pack.id.clone(),
                        id: bug.id,
                        function: bug.function,
                        register: bug.register,
                        kind: bug.kind,
                        status: bug.status,
                        observed: bug.observed,
                        expected: bug.expected,
                        classification: bug.classification.unwrap_or(pack.classification),
                        applies_to,
                        evidence: bug.evidence,
                    },
                );
            }
        }
        Ok(Self {
            assertions,
            vendor_bugs,
        })
    }

    pub fn assertions(&self) -> &BTreeMap<String, EffectiveAssertion> {
        &self.assertions
    }

    pub fn vendor_bugs(&self) -> &BTreeMap<String, EffectiveVendorBug> {
        &self.vendor_bugs
    }

    pub fn semantic_fingerprint(&self) -> String {
        let encoded = serde_json::to_vec(self).expect("review knowledge is JSON serializable");
        let mut hash = Sha256::new();
        hash.update(b"blobray/review-knowledge/v1\0");
        hash.update(encoded);
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

fn validate_pack(pack: &ReviewPack) -> Result<()> {
    if pack.schema != 1 {
        return invalid(
            &pack.id,
            format!("requires schema = 1, got {}", pack.schema),
        );
    }
    validate_stable_id(&pack.id).map_err(|reason| Error::Invalid {
        pack: pack.id.clone(),
        reason,
    })?;
    validate_classification(&pack.id, "pack", pack.classification)?;
    validate_applicability(&pack.id, "pack", &pack.applies_to)?;
    if pack.assertions.is_empty() && pack.vendor_bugs.is_empty() {
        return invalid(&pack.id, "contains no assertions or vendor bugs");
    }
    let mut ids = BTreeSet::new();
    for assertion in &pack.assertions {
        validate_record_id(&pack.id, &assertion.id, &mut ids)?;
        validate_text(&pack.id, &assertion.id, "subject", &assertion.subject)?;
        validate_token(&pack.id, &assertion.id, "kind", &assertion.kind)?;
        if let AssertionValue::Strings(values) = &assertion.value
            && (values.is_empty() || !all_unique_nonempty(values))
        {
            return invalid(
                &pack.id,
                format!(
                    "assertion {:?} has an empty or duplicate string-list value",
                    assertion.id
                ),
            );
        }
        if let Some(classification) = assertion.classification {
            validate_classification(&pack.id, &assertion.id, classification)?;
        }
        validate_applicability(&pack.id, &assertion.id, &assertion.applies_to)?;
        validate_evidence(&pack.id, &assertion.id, &assertion.evidence)?;
    }
    for bug in &pack.vendor_bugs {
        validate_record_id(&pack.id, &bug.id, &mut ids)?;
        validate_text(&pack.id, &bug.id, "function", &bug.function)?;
        if let Some(register) = &bug.register {
            validate_text(&pack.id, &bug.id, "register", register)?;
        }
        validate_token(&pack.id, &bug.id, "kind", &bug.kind)?;
        validate_text(&pack.id, &bug.id, "observed", &bug.observed)?;
        validate_text(&pack.id, &bug.id, "expected", &bug.expected)?;
        if let Some(classification) = bug.classification {
            validate_classification(&pack.id, &bug.id, classification)?;
        }
        validate_applicability(&pack.id, &bug.id, &bug.applies_to)?;
        validate_evidence(&pack.id, &bug.id, &bug.evidence)?;
    }
    Ok(())
}

fn effective_applicability(
    pack: &str,
    record: &str,
    pack_applicability: &Applicability,
    record_applicability: &Applicability,
) -> Result<Applicability> {
    pack_applicability
        .intersection(record_applicability)
        .ok_or_else(|| Error::Invalid {
            pack: pack.to_owned(),
            reason: format!("record {record:?} has applicability disjoint from its pack"),
        })
}

fn validate_record_id(pack: &str, id: &str, ids: &mut BTreeSet<String>) -> Result<()> {
    validate_stable_id(id).map_err(|reason| Error::Invalid {
        pack: pack.to_owned(),
        reason: format!("invalid record id {id:?}: {reason}"),
    })?;
    if !ids.insert(id.to_owned()) {
        return invalid(pack, format!("duplicate record id {id:?}"));
    }
    Ok(())
}

fn validate_stable_id(value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'-' | b'_' | b':' | b'/' | b'@' | b'+')
        })
        || !value.bytes().any(|byte| byte.is_ascii_alphanumeric())
    {
        return Err("must be 1..160 lower-case ASCII identity characters".to_owned());
    }
    Ok(())
}

fn validate_token(pack: &str, record: &str, field: &str, value: &str) -> Result<()> {
    validate_stable_id(value).map_err(|reason| Error::Invalid {
        pack: pack.to_owned(),
        reason: format!("record {record:?} {field} {value:?}: {reason}"),
    })
}

fn validate_text(pack: &str, record: &str, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return invalid(
            pack,
            format!("record {record:?} has invalid empty/control {field}"),
        );
    }
    Ok(())
}

fn validate_classification(
    pack: &str,
    record: &str,
    classification: FactClassification,
) -> Result<()> {
    if classification.provenance == FactProvenance::Hint {
        return invalid(
            pack,
            format!("record {record:?} is a hint; hints cannot be accepted reviewed knowledge"),
        );
    }
    Ok(())
}

fn validate_applicability(pack: &str, record: &str, value: &Applicability) -> Result<()> {
    for (name, values) in [
        ("ecosystems", &value.ecosystems),
        ("chips", &value.chips),
        ("chip-revisions", &value.chip_revisions),
        ("artifact-lineages", &value.artifact_lineages),
    ] {
        if !all_unique_nonempty(values) {
            return invalid(
                pack,
                format!("record {record:?} has empty or duplicate {name}"),
            );
        }
    }
    let mut artifacts = BTreeSet::new();
    for artifact in &value.artifacts {
        if artifact.source.trim().is_empty()
            || !is_sha256(&artifact.sha256)
            || !artifacts.insert(artifact)
        {
            return invalid(
                pack,
                format!("record {record:?} has invalid or duplicate artifact applicability"),
            );
        }
    }
    Ok(())
}

fn validate_evidence(pack: &str, record: &str, evidence: &[EvidenceReference]) -> Result<()> {
    if evidence.is_empty() {
        return invalid(pack, format!("record {record:?} requires evidence"));
    }
    let mut seen = BTreeSet::new();
    for item in evidence {
        if item.source.trim().is_empty()
            || item.locator.trim().is_empty()
            || item.source.chars().any(char::is_control)
            || item.locator.chars().any(char::is_control)
            || item
                .sha256
                .as_deref()
                .is_some_and(|digest| !is_sha256(digest))
            || !seen.insert((
                item.source.as_str(),
                item.locator.as_str(),
                item.sha256.as_deref(),
            ))
        {
            return invalid(
                pack,
                format!("record {record:?} has invalid or duplicate evidence"),
            );
        }
    }
    Ok(())
}

fn all_unique_nonempty(values: &[String]) -> bool {
    let mut seen = BTreeSet::new();
    values
        .iter()
        .all(|value| !value.trim().is_empty() && seen.insert(value))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn intersect_dimension<T: Clone + Ord>(left: &[T], right: &[T]) -> Option<Vec<T>> {
    if left.is_empty() {
        return Some(sorted_unique(right));
    }
    if right.is_empty() {
        return Some(sorted_unique(left));
    }
    let left = left.iter().collect::<BTreeSet<_>>();
    let right = right.iter().collect::<BTreeSet<_>>();
    let values = left
        .intersection(&right)
        .map(|value| (*value).clone())
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn sorted_unique<T: Clone + Ord>(values: &[T]) -> Vec<T> {
    values
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn invalid<T>(pack: &str, reason: impl Into<String>) -> Result<T> {
    Err(Error::Invalid {
        pack: pack.to_owned(),
        reason: reason.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
schema = 1
id = "esp32s31-radio"

[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"

[applies-to]
chips = ["esp32s31"]
chip-revisions = ["rev0"]

[[assertions]]
id = "ieee802154.event-status.name"
subject = "mmio:cpu:0x20103128/32"
kind = "register-name"
value = "EVENT_STATUS"

[[assertions.evidence]]
source = "ESP_IDF_IEEE802154_COMMON_LL"
locator = "ieee802154_ll_get_events"

[[assertions]]
id = "ieee802154.event-status.access"
subject = "mmio:cpu:0x20103128/32"
kind = "hardware-write-semantics"
value = "unknown"
note = "Masked self-write observed; W1C is not proven."

[[assertions.evidence]]
source = "ESP_IDF_IEEE802154_COMMON_LL"
locator = "ieee802154_ll_clear_events"

[[vendor-bugs]]
id = "vendor.ieee802154.event-clear-rmw"
function = "function:esp-idf:ieee802154_ll_clear_events"
register = "mmio:cpu:0x20103128/32"
kind = "suspect-rmw-on-status"
status = "suspected"
observed = "The accessor performs a masked self-write."
expected = "Confirm the hardware clear contract before using the accessor."

[[vendor-bugs.evidence]]
source = "ESP_IDF_IEEE802154_COMMON_LL"
locator = "ieee802154_ll_clear_events"
"#;

    #[test]
    fn sparse_pack_round_trips_and_has_stable_fingerprint() {
        let pack = ReviewPack::from_toml(BASE).unwrap();
        let first = ReviewKnowledge::merge([pack.clone()]).unwrap();
        let second = ReviewKnowledge::merge([pack]).unwrap();
        assert_eq!(first.assertions().len(), 2);
        assert_eq!(first.vendor_bugs().len(), 1);
        assert_eq!(first.semantic_fingerprint(), second.semantic_fingerprint());
    }

    #[test]
    fn applicability_is_intersection_not_override() {
        let pack = ReviewPack::from_toml(BASE).unwrap();
        let effective = ReviewKnowledge::merge([pack]).unwrap();
        assert_eq!(
            effective.assertions()["ieee802154.event-status.name"]
                .applies_to
                .chips,
            ["esp32s31"]
        );
        assert_eq!(
            effective.assertions()["ieee802154.event-status.name"]
                .applies_to
                .chip_revisions,
            ["rev0"]
        );
    }

    #[test]
    fn overlapping_semantic_assertions_fail_closed() {
        let first = ReviewPack::from_toml(BASE).unwrap();
        let second = ReviewPack::from_toml(
            &BASE
                .replace(
                    "id = \"esp32s31-radio\"",
                    "id = \"esp32s31-radio-conflict\"",
                )
                .replace(
                    "id = \"ieee802154.event-status.name\"",
                    "id = \"ieee802154.event-status.name-conflict\"",
                )
                .replace(
                    "id = \"ieee802154.event-status.access\"",
                    "id = \"ieee802154.event-status.access-conflict\"",
                )
                .replace(
                    "id = \"vendor.ieee802154.event-clear-rmw\"",
                    "id = \"vendor.ieee802154.event-clear-rmw-conflict\"",
                ),
        )
        .unwrap();
        let error = ReviewKnowledge::merge([first, second]).unwrap_err();
        assert!(matches!(error, Error::Conflict { .. }));
    }

    #[test]
    fn disjoint_artifact_revisions_may_carry_different_values() {
        let make = |id: &str, revision: &str, value: &str| {
            ReviewPack::from_toml(&format!(
                r#"
schema = 1
id = "{id}"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"
[applies-to]
chip-revisions = ["{revision}"]
[[assertions]]
id = "{id}.fact"
subject = "mmio:cpu:0x1000/32"
kind = "register-name"
value = "{value}"
[[assertions.evidence]]
source = "MANUAL"
locator = "review"
"#
            ))
            .unwrap()
        };
        let merged = ReviewKnowledge::merge([
            make("rev-a", "a", "CONTROL_A"),
            make("rev-b", "b", "CONTROL_B"),
        ])
        .unwrap();
        assert_eq!(merged.assertions().len(), 2);
    }

    #[test]
    fn hints_and_unbounded_vendor_artifact_hashes_are_rejected() {
        let hint = BASE.replace("provenance = \"reviewed\"", "provenance = \"hint\"");
        assert!(
            ReviewPack::from_toml(&hint)
                .unwrap_err()
                .to_string()
                .contains("hint")
        );

        let invalid_hash = BASE.replace(
            "chips = [\"esp32s31\"]",
            "chips = [\"esp32s31\"]\nartifacts = [{ source = \"blob\", sha256 = \"bad\" }]",
        );
        assert!(
            ReviewPack::from_toml(&invalid_hash)
                .unwrap_err()
                .to_string()
                .contains("artifact applicability")
        );
    }
}
