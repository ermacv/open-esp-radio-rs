//! Explicit, property-scoped applicability reviews over immutable observations.
//!
//! A review never creates a PASS. It admits an existing complete observation
//! for one current contract, and retains other failures until individually
//! resolved. Source, destination and current owner bindings are checked anew.

use super::*;
use crate::model::CapabilityDocument;
use serde::Serialize;

mod assessment;
mod contract;
mod dependencies;
mod sources;
use assessment::{assess, failed};
use contract::read;
pub(crate) use contract::{property, validate};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    UnchangedFunctionalContract,
    IdenticalImage,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct ObservationRef {
    id: String,
    image: String,
    application_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    build_record: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct InputBinding {
    #[serde(default, skip_serializing_if = "InputKind::is_bytes")]
    kind: InputKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    path: PathBuf,
    sha256: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum InputKind {
    #[default]
    Bytes,
    Procedure,
    Evidence,
}
impl InputKind {
    fn is_bytes(&self) -> bool {
        *self == Self::Bytes
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Disposition {
    Fixed,
    NotApplicable,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct FailureResolution {
    observation: String,
    disposition: Disposition,
    reason: String,
    resolving_observation: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Document {
    schema: u16,
    id: String,
    capability: String,
    scenario: String,
    property_sha256: String,
    kind: Kind,
    reviewer: String,
    reason: String,
    source: ObservationRef,
    /// Explicit proof for an old observation without embedded observer identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    observer_provenance: Vec<PathBuf>,
    /// Reviewer-owned acceptance of compiler/profile changes for functional checks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    observer_configuration: Vec<serde_json::Value>,
    destination: ObservationRef,
    inputs: Vec<InputBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dependency_roots: Vec<String>,
    #[serde(default)]
    failures: Vec<FailureResolution>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PropertyBinding {
    pub(crate) sha256: String,
    /// Exact v1 identity for checking existing records before metadata migration.
    pub(crate) legacy_sha256: String,
    pub(crate) previous_sha256: String,
    pub(crate) procedure_sha256: Option<String>,
    pub(crate) required_inputs: Vec<PathBuf>,
    pub(crate) implicit_build_inputs: Vec<PathBuf>,
    current_inputs: Vec<InputBinding>,
    image_sensitive: bool,
    pub(crate) unmapped_capabilities: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ReviewLink {
    id: String,
    path: PathBuf,
    sha256: String,
    reviewer: String,
    reason: String,
    kind: Kind,
    source_observation: String,
    destination_observation: String,
}

impl ReviewLink {
    pub(super) fn evidence_reference(&self) -> String {
        format!(
            ":review={}@{}:destination={}",
            self.id, self.sha256, self.destination_observation
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResolutionLink {
    review: ReviewLink,
    disposition: Disposition,
    reason: String,
    resolving_observation: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ReviewDecision {
    pub(crate) review: ReviewLink,
    pub(crate) scenario: String,
    pub(crate) status: String,
}

pub(crate) fn apply<'a>(
    root: &Path,
    document: &CapabilityDocument,
    declarations: &BTreeMap<String, CapabilityDocument>,
    index: &'a HilEvidenceIndex,
    catalog: &ScenarioCatalog,
) -> Result<(std::borrow::Cow<'a, HilEvidenceIndex>, Vec<ReviewDecision>)> {
    if document.hil_reviews.is_empty() {
        return Ok((std::borrow::Cow::Borrowed(index), Vec::new()));
    }
    let mut reviewed = index.clone();
    let mut decisions = Vec::new();
    for path in &document.hil_reviews {
        let review = read(root, path)?;
        let requirement = document
            .hil_requirements
            .iter()
            .find(|r| r.scenario == review.scenario)
            .map(|r| HilRequirement {
                scenario: r.scenario.clone(),
                checks: r.checks.clone(),
                minimum_repetitions: r.minimum_repetitions,
            })
            .or_else(|| {
                document
                    .hil_requirements
                    .iter()
                    .find(|r| catalog.control_for(&r.scenario) == Some(&review.scenario))
                    .map(|r| HilRequirement {
                        scenario: review.scenario.clone(),
                        checks: Vec::new(),
                        minimum_repetitions: r.minimum_repetitions,
                    })
            })
            .ok_or("review scenario is not part of the capability")?;
        let binding = property(document, declarations, &requirement, catalog, root)?;
        let link = ReviewLink {
            id: review.id.clone(),
            path: path.clone(),
            sha256: sha256_file(&root.join(path))?,
            reviewer: review.reviewer.clone(),
            reason: review.reason.clone(),
            kind: review.kind.clone(),
            source_observation: review.source.id.clone(),
            destination_observation: review.destination.id.clone(),
        };
        let status = assess(root, &review, &binding, &requirement, index, catalog)?;
        if status == "applied" {
            for observation in reviewed
                .scenarios
                .get_mut(&review.scenario)
                .into_iter()
                .flatten()
            {
                let id = observation.observation_id(&review.scenario);
                // Every existing failure of this property remains relevant to a
                // transfer until explicitly disposed. Future failures join this set.
                if id.as_deref() == Some(&review.source.id)
                    || failed(observation, &requirement, catalog)
                {
                    observation.review = Some(link.clone());
                }
                if let Some(disposition) = review
                    .failures
                    .iter()
                    .find(|f| Some(&f.observation) == id.as_ref())
                {
                    observation.resolution = Some(ResolutionLink {
                        review: link.clone(),
                        disposition: disposition.disposition.clone(),
                        reason: disposition.reason.clone(),
                        resolving_observation: disposition.resolving_observation.clone(),
                    });
                }
            }
        }
        decisions.push(ReviewDecision {
            review: link,
            scenario: review.scenario,
            status: status.into(),
        });
    }
    Ok((std::borrow::Cow::Owned(reviewed), decisions))
}

fn regular(root: &Path, path: &Path) -> Result<()> {
    if !safe_relative(path) {
        return Err("HIL review path must be repository-relative".into());
    }
    let mut absolute = root.canonicalize()?;
    let mut components = path.components().peekable();
    while let Some(c) = components.next() {
        absolute.push(c);
        let kind = fs::symlink_metadata(&absolute)?.file_type();
        if (components.peek().is_some() && !kind.is_dir())
            || (components.peek().is_none() && !kind.is_file())
        {
            return Err("HIL review path contains a symlink or special file".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

fn input_hash(bytes: &[u8], kind: &InputKind) -> Result<String> {
    let bytes = match kind {
        InputKind::Procedure => {
            let value: serde_json::Value = toml_edit::de::from_str(std::str::from_utf8(bytes)?)?;
            if value["schema"] != 5
                || !value["id"].is_string()
                || !crate::hil::has_one_family(&value)
            {
                return Err("procedure input must be a version 5 HIL scenario".into());
            }
            serde_json::to_vec(&crate::hil::procedure::normalize(&value))?
        }
        InputKind::Bytes | InputKind::Evidence => bytes.to_vec(),
    };
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
