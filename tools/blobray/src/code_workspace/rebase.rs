use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::{
    CodeBoundaryPack, CodeBoundaryStatus, CodeWorkspace, ReviewedCodeBoundary, ReviewedCodeInput,
    load_code_boundary_pack, render_code_boundary_pack, validate_review,
};
use crate::{
    Result,
    artifacts::symbol_inventory::{CodeBoundaryCandidateFact, CodeBoundaryFacts},
};

type StableBoundaryKey = (String, Option<String>, String, u64);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CodeRebaseSummary {
    pub(crate) current: bool,
    pub(crate) safe_to_apply: bool,
    pub(crate) inputs_added: usize,
    pub(crate) inputs_removed: usize,
    pub(crate) preserved: usize,
    pub(crate) changed: usize,
    pub(crate) added: usize,
    pub(crate) removed: usize,
}

pub(crate) struct CodeRebaseCandidate {
    pack: CodeBoundaryPack,
    contents: String,
    summary: CodeRebaseSummary,
}

impl CodeRebaseCandidate {
    pub(crate) fn prepare(
        facts: &CodeBoundaryFacts,
        pack_path: &Path,
        project_id: &str,
    ) -> Result<Self> {
        let old = load_code_boundary_pack(pack_path)?;
        validate_header(&old, project_id)?;

        let old_inputs = old
            .inputs
            .iter()
            .map(|input| input.source.as_str())
            .collect::<BTreeSet<_>>();
        let old_input_guards = old
            .inputs
            .iter()
            .map(|input| (input.source.clone(), input.artifact_sha256.clone()))
            .collect::<BTreeSet<_>>();
        let new_input_guards = facts
            .inputs
            .iter()
            .map(|input| (input.source.clone(), input.artifact_sha256.clone()))
            .collect::<BTreeSet<_>>();
        let new_inputs = facts
            .inputs
            .iter()
            .map(|input| input.source.as_str())
            .collect::<BTreeSet<_>>();
        let inputs_added = new_inputs.difference(&old_inputs).count();
        let inputs_removed = old_inputs.difference(&new_inputs).count();

        let mut old_boundaries = BTreeMap::new();
        for review in old.boundaries {
            let key = stable_review_key(&review);
            if old_boundaries.insert(key.clone(), review).is_some() {
                return Err(crate::Error::invalid(format!(
                    "reviewed code-boundary pack contains duplicate stable boundary {}",
                    display_key(&key)
                )));
            }
        }

        let mut preserved = 0;
        let mut changed = 0;
        let mut added = 0;
        let mut boundary_guards_current = true;
        let mut boundaries = Vec::with_capacity(facts.candidates.len());
        for fact in &facts.candidates {
            let key = stable_fact_key(fact);
            let review = match old_boundaries.remove(&key) {
                Some(mut review) => {
                    boundary_guards_current &= review.artifact_sha256 == fact.artifact_sha256;
                    review.artifact_sha256.clone_from(&fact.artifact_sha256);
                    if validate_review(&review, fact).is_ok() {
                        preserved += 1;
                        review
                    } else {
                        changed += 1;
                        unreviewed(fact)
                    }
                }
                None => {
                    boundary_guards_current = false;
                    added += 1;
                    unreviewed(fact)
                }
            };
            boundaries.push(review);
        }
        let removed = old_boundaries.len();
        let inputs = facts
            .inputs
            .iter()
            .map(|input| ReviewedCodeInput {
                source: input.source.clone(),
                artifact_sha256: input.artifact_sha256.clone(),
            })
            .collect::<Vec<_>>();
        let pack = CodeBoundaryPack {
            schema: 1,
            id: project_id.to_owned(),
            inputs,
            boundaries,
        };
        // Validate the complete rebased document, including duplicate accepted names.
        CodeWorkspace::from_pack(facts, pack.clone(), project_id)?;

        let current = old_input_guards == new_input_guards
            && boundary_guards_current
            && changed == 0
            && added == 0
            && removed == 0;
        // Input guards protect the reviewed boundaries, not the mere presence
        // of an artifact in the project. Adding or removing an input which
        // contributes no boundary candidate cannot invalidate a human
        // decision. Any affected candidate is still counted as added,
        // removed, or changed below and keeps the rebase fail-closed.
        let safe_to_apply = changed == 0 && added == 0 && removed == 0;
        let contents = render_code_boundary_pack(&pack, facts);
        Ok(Self {
            pack,
            contents,
            summary: CodeRebaseSummary {
                current,
                safe_to_apply,
                inputs_added,
                inputs_removed,
                preserved,
                changed,
                added,
                removed,
            },
        })
    }

    pub(crate) const fn summary(&self) -> CodeRebaseSummary {
        self.summary
    }

    pub(crate) fn contents(&self) -> &str {
        &self.contents
    }

    pub(crate) fn validate(&self, facts: &CodeBoundaryFacts, project_id: &str) -> Result<()> {
        CodeWorkspace::from_pack(facts, self.pack.clone(), project_id).map(|_| ())
    }
}

fn validate_header(pack: &CodeBoundaryPack, project_id: &str) -> Result<()> {
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
    Ok(())
}

fn unreviewed(fact: &CodeBoundaryCandidateFact) -> ReviewedCodeBoundary {
    ReviewedCodeBoundary {
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

fn stable_fact_key(fact: &CodeBoundaryCandidateFact) -> StableBoundaryKey {
    (
        fact.source.clone(),
        fact.member.clone(),
        fact.section.clone(),
        fact.entry_offset,
    )
}

fn stable_review_key(review: &ReviewedCodeBoundary) -> StableBoundaryKey {
    (
        review.source.clone(),
        review.member.clone(),
        review.section.clone(),
        review.entry_offset,
    )
}

fn display_key(key: &StableBoundaryKey) -> String {
    format!(
        "source {:?}, member {:?}, section {:?}, offset {:#x}",
        key.0, key.1, key.2, key.3
    )
}
