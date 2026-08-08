//! Structural, provenance, completeness, and review-coverage validation.

use std::collections::BTreeSet;

use super::validation::{ValidationError, ValidationResult};
use super::{FunctionFacts, FunctionPack, FunctionWorkspaceSummary};

mod contexts;
mod primitives;
mod types;

use contexts::{validate_function, validate_inputs};
use primitives::validate_id;
use types::validate_types;

pub(super) fn validate(
    pack: &FunctionPack,
    facts: &FunctionFacts,
) -> ValidationResult<FunctionWorkspaceSummary> {
    validate_id(&pack.id, "function pack id")
        .map_err(|message| ValidationError::pack("id", message))?;
    validate_inputs(&pack.inputs, &facts.inputs)?;
    let mut summary = FunctionWorkspaceSummary {
        inputs: pack.inputs.len(),
        observed_functions: facts.root_functions().count(),
        ..FunctionWorkspaceSummary::default()
    };
    let mut keys = BTreeSet::new();
    let mut reviewed_names = BTreeSet::new();
    for reviewed in &pack.functions {
        let key = (&reviewed.profile, &reviewed.source, &reviewed.identity);
        if !keys.insert(key) {
            return Err(ValidationError::function(
                reviewed,
                "identity",
                format!(
                    "duplicate reviewed function {}:{}",
                    reviewed.profile, reviewed.identity
                ),
            ));
        }
        let fact = facts
            .function(&reviewed.profile, &reviewed.source, &reviewed.identity)
            .ok_or_else(|| {
                ValidationError::function(
                    reviewed,
                    "identity",
                    format!(
                        "stale reviewed function {}:{}:{}",
                        reviewed.profile, reviewed.source, reviewed.identity
                    ),
                )
            })?;
        validate_function(reviewed, fact, &mut summary, &mut reviewed_names)?;
    }
    for fact in facts.root_functions() {
        if !keys.contains(&(&fact.profile, &fact.source, &fact.identity)) {
            summary.unreviewed_functions += 1;
            summary.unreviewed_fields += fact.context_fields.len();
            summary.unreviewed_contexts += fact
                .context_fields
                .iter()
                .map(|field| field.argument)
                .collect::<BTreeSet<_>>()
                .len();
        }
    }
    validate_types(&pack.types, facts, &mut summary)?;
    Ok(summary)
}
