use std::{collections::BTreeMap, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    Result,
    artifacts::symbol_inventory::{CodeBoundaryCandidateFact, CodeBoundaryFacts},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CodeBoundaryStatus {
    Unreviewed,
    Accepted,
    Rejected,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct CodeBoundaryPack {
    pub(super) schema: u32,
    pub(super) id: String,
    #[serde(default)]
    pub(super) inputs: Vec<ReviewedCodeInput>,
    #[serde(default)]
    pub(super) boundaries: Vec<ReviewedCodeBoundary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ReviewedCodeInput {
    pub(crate) source: String,
    pub(crate) artifact_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ReviewedCodeBoundary {
    pub(crate) source: String,
    pub(crate) artifact_sha256: String,
    pub(crate) member: Option<String>,
    pub(crate) section: String,
    pub(crate) entry_offset: u64,
    pub(crate) end_exclusive_offset: u64,
    pub(crate) status: CodeBoundaryStatus,
    pub(crate) name: Option<String>,
    pub(crate) reason: Option<String>,
}

impl ReviewedCodeBoundary {
    pub(super) fn unreviewed(fact: &CodeBoundaryCandidateFact) -> Self {
        Self {
            source: fact.source.clone(),
            artifact_sha256: fact.artifact_sha256.clone(),
            member: fact.member.clone(),
            section: fact.section.clone(),
            entry_offset: fact.entry_offset,
            end_exclusive_offset: fact.end_limit_offset,
            status: CodeBoundaryStatus::Unreviewed,
            name: None,
            reason: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CodeWorkspaceSummary {
    pub(crate) inputs: usize,
    pub(crate) observed_candidates: usize,
    pub(crate) accepted: usize,
    pub(crate) rejected: usize,
    pub(crate) unreviewed: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct ReviewedCodeBoundaryEntry {
    pub(crate) review: ReviewedCodeBoundary,
    pub(crate) fact: CodeBoundaryCandidateFact,
}

#[derive(Clone, Debug)]
pub(crate) struct CodeWorkspace {
    entries: Vec<ReviewedCodeBoundaryEntry>,
    summary: CodeWorkspaceSummary,
}

type BoundaryKey = (String, String, Option<String>, String, u64);

impl CodeWorkspace {
    pub(crate) fn load(
        facts: &CodeBoundaryFacts,
        pack_path: &Path,
        project_id: &str,
    ) -> Result<Self> {
        let pack = load_code_boundary_pack(pack_path)?;
        Self::from_pack(facts, pack, project_id)
    }

    pub(super) fn from_pack(
        facts: &CodeBoundaryFacts,
        pack: CodeBoundaryPack,
        project_id: &str,
    ) -> Result<Self> {
        if pack.schema != 1 {
            return Err(crate::Error::invalid(format!(
                "reviewed code-boundary pack requires schema = 1, got {}",
                pack.schema
            )));
        }
        if pack.id != project_id {
            return Err(crate::Error::invalid(format!(
                "reviewed code-boundary pack id {:?} does not match project id {project_id:?}",
                pack.id
            )));
        }
        validate_inputs(facts, &pack.inputs)?;

        let fact_map = facts
            .candidates
            .iter()
            .map(|fact| (fact_key(fact), fact))
            .collect::<BTreeMap<_, _>>();
        let mut reviews = BTreeMap::new();
        for review in pack.boundaries {
            // Accept legacy backlog entries without treating their generated
            // ranges or continued existence as human decisions.
            if review.status == CodeBoundaryStatus::Unreviewed {
                if review.name.is_some() || review.reason.is_some() {
                    return Err(crate::Error::invalid(
                        "unreviewed code boundary must not define name or reason",
                    ));
                }
                continue;
            }
            let key = review_key(&review);
            if reviews.insert(key.clone(), review).is_some() {
                return Err(crate::Error::invalid(format!(
                    "duplicate reviewed code boundary {}",
                    display_key(&key)
                )));
            }
        }
        if let Some(key) = reviews.keys().find(|key| !fact_map.contains_key(*key)) {
            return Err(crate::Error::invalid(format!(
                "stale reviewed code boundary {} is absent from generated symbol facts",
                display_key(key)
            )));
        }

        let mut names = BTreeMap::<String, BoundaryKey>::new();
        let mut entries = Vec::with_capacity(fact_map.len());
        let mut summary = CodeWorkspaceSummary {
            inputs: facts.inputs.len(),
            observed_candidates: facts.candidates.len(),
            ..CodeWorkspaceSummary::default()
        };
        for (key, fact) in fact_map {
            // Absence is generated review backlog, not a missing human fact.
            let review = reviews
                .remove(&key)
                .unwrap_or_else(|| ReviewedCodeBoundary::unreviewed(fact));
            validate_review(&review, fact)?;
            match review.status {
                CodeBoundaryStatus::Unreviewed => summary.unreviewed += 1,
                CodeBoundaryStatus::Rejected => summary.rejected += 1,
                CodeBoundaryStatus::Accepted => {
                    summary.accepted += 1;
                    let name = review.name.as_ref().expect("accepted review has a name");
                    if let Some(previous) = names.insert(name.clone(), key.clone()) {
                        return Err(crate::Error::invalid(format!(
                            "accepted code name {name:?} is shared by {} and {}",
                            display_key(&previous),
                            display_key(&key)
                        )));
                    }
                }
            }
            entries.push(ReviewedCodeBoundaryEntry {
                review,
                fact: fact.clone(),
            });
        }
        Ok(Self { entries, summary })
    }

    pub(crate) const fn summary(&self) -> CodeWorkspaceSummary {
        self.summary
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = &ReviewedCodeBoundaryEntry> {
        self.entries.iter()
    }

    pub(crate) fn accepted(&self) -> impl Iterator<Item = &ReviewedCodeBoundaryEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.review.status == CodeBoundaryStatus::Accepted)
    }
}

pub(super) fn load_code_boundary_pack(pack_path: &Path) -> Result<CodeBoundaryPack> {
    let input = fs::read_to_string(pack_path)?;
    toml_edit::de::from_str(&input).map_err(|error| {
        crate::Error::invalid(format!(
            "invalid reviewed code-boundary pack {}: {error}",
            pack_path.display()
        ))
    })
}

fn validate_inputs(facts: &CodeBoundaryFacts, reviewed: &[ReviewedCodeInput]) -> Result<()> {
    let mut expected = facts
        .inputs
        .iter()
        .map(|input| (input.source.clone(), input.artifact_sha256.clone()))
        .collect::<Vec<_>>();
    let mut actual = reviewed
        .iter()
        .map(|input| (input.source.clone(), input.artifact_sha256.clone()))
        .collect::<Vec<_>>();
    expected.sort();
    actual.sort();
    if actual.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(crate::Error::invalid(
            "reviewed code-boundary pack contains duplicate inputs",
        ));
    }
    if actual != expected {
        return Err(crate::Error::invalid(
            "reviewed code-boundary inputs or artifact SHA-256 guards do not match the generated symbol inventory",
        ));
    }
    Ok(())
}

pub(super) fn validate_review(
    review: &ReviewedCodeBoundary,
    fact: &CodeBoundaryCandidateFact,
) -> Result<()> {
    if review.end_exclusive_offset <= review.entry_offset
        || review.end_exclusive_offset > fact.end_limit_offset
    {
        return Err(crate::Error::invalid(format!(
            "reviewed range {:#x}..{:#x} for {} is outside generated candidate limit {:#x}",
            review.entry_offset,
            review.end_exclusive_offset,
            display_key(&review_key(review)),
            fact.end_limit_offset
        )));
    }
    match review.status {
        CodeBoundaryStatus::Accepted => {
            let name = review.name.as_deref().ok_or_else(|| {
                crate::Error::invalid("accepted code boundary requires a non-empty name")
            })?;
            if !valid_name(name) {
                return Err(crate::Error::invalid(format!(
                    "accepted code boundary name {name:?} must be an identifier"
                )));
            }
        }
        CodeBoundaryStatus::Rejected => {
            if review.name.is_some() {
                return Err(crate::Error::invalid(
                    "rejected code boundary must not define a name",
                ));
            }
            if review.reason.as_deref().is_none_or(str::is_empty) {
                return Err(crate::Error::invalid(
                    "rejected code boundary requires a non-empty reason",
                ));
            }
        }
        CodeBoundaryStatus::Unreviewed => {
            if review.name.is_some() || review.reason.is_some() {
                return Err(crate::Error::invalid(
                    "unreviewed code boundary must not define name or reason",
                ));
            }
        }
    }
    Ok(())
}

fn valid_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn fact_key(fact: &CodeBoundaryCandidateFact) -> BoundaryKey {
    (
        fact.source.clone(),
        fact.artifact_sha256.clone(),
        fact.member.clone(),
        fact.section.clone(),
        fact.entry_offset,
    )
}

fn review_key(review: &ReviewedCodeBoundary) -> BoundaryKey {
    (
        review.source.clone(),
        review.artifact_sha256.clone(),
        review.member.clone(),
        review.section.clone(),
        review.entry_offset,
    )
}

fn display_key(key: &BoundaryKey) -> String {
    format!(
        "source {:?}, artifact {}, member {:?}, section {:?}, offset {:#x}",
        key.0, key.1, key.2, key.3, key.4
    )
}
