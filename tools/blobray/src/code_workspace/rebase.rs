use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::{
    CodeBoundaryPack, CodeBoundaryStatus, CodeWorkspace, ReviewedCodeBoundary, ReviewedCodeInput,
    load_code_boundary_pack, render_code_boundary_decisions, render_code_boundary_pack,
    validate_review,
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
            // Legacy packs may contain generated backlog. It carries no review
            // decision and must not turn candidate discovery into a pack edit.
            if review.status == CodeBoundaryStatus::Unreviewed {
                if review.name.is_some() || review.reason.is_some() {
                    return Err(crate::Error::invalid(
                        "unreviewed code boundary must not define name or reason",
                    ));
                }
                continue;
            }
            if !old_input_guards.contains(&(review.source.clone(), review.artifact_sha256.clone()))
            {
                return Err(crate::Error::invalid(
                    "reviewed code boundary has no matching source and artifact SHA-256 input guard",
                ));
            }
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
        let mut boundaries = Vec::with_capacity(old_boundaries.len());
        let mut pending_reviews = Vec::new();
        for fact in &facts.candidates {
            let key = stable_fact_key(fact);
            let Some(review) = old_boundaries.remove(&key) else {
                continue;
            };
            // A matching range does not authenticate the semantics of changed
            // bytes. Without body-level evidence, a new digest requires review.
            if review.artifact_sha256 == fact.artifact_sha256
                && validate_review(&review, fact).is_ok()
            {
                preserved += 1;
                boundaries.push(review);
            } else {
                changed += 1;
                pending_reviews.push(review);
            }
        }
        let removed = old_boundaries.len();
        pending_reviews.extend(old_boundaries.into_values());
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

        let current = old_input_guards == new_input_guards && changed == 0 && removed == 0;
        // Discovery changes alone are generated state. Only invalidated human
        // decisions prevent automatic rebasing of the sparse overlay.
        let safe_to_apply = changed == 0 && removed == 0;
        let mut contents = render_code_boundary_pack(&pack);
        if !pending_reviews.is_empty() {
            // Keep the former intent visible in a review candidate without
            // making it active against a different artifact revision.
            contents.push_str("\n# Previous decisions requiring fresh review (inactive):\n");
            for line in render_code_boundary_decisions(&pending_reviews).lines() {
                contents.push_str("# ");
                contents.push_str(line);
                contents.push('\n');
            }
        }
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
                added: 0,
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
