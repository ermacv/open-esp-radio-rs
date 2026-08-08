//! Typed consumer projection for the persistent schema-v35 linked-IR document.
//!
//! The writer owns the complete document. Review workspaces deliberately read
//! only the stable evidence they consume; unknown presentation/enrichment
//! fields remain forward-incompatible through the explicit schema identity.

use serde::Deserialize;

use crate::Result;

#[derive(Debug, Deserialize)]
pub(crate) struct LinkedIrStoredDocument {
    schema_version: u32,
    command: String,
    pub(crate) completeness_claim: bool,
    pub(crate) mmio_field_semantics_claim: bool,
    pub(crate) artifacts: Vec<StoredSourceArtifact>,
    pub(crate) mmio_registers: Vec<StoredMmioRegister>,
    pub(crate) functions: Vec<StoredFunction>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredSourceArtifact {
    pub(crate) source: String,
    pub(crate) artifact: StoredArtifactIdentity,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredArtifactIdentity {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredFunction {
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    pub(crate) selection: String,
    pub(crate) object_offset: u32,
    pub(crate) complete: bool,
    pub(crate) calls: Vec<StoredCall>,
    pub(crate) mmio_accesses: Vec<StoredMmioAccess>,
    pub(crate) scenario_suggestions: Vec<StoredScenarioSuggestion>,
    pub(crate) effect_summary: StoredEffectSummary,
    pub(crate) pseudo: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredCall {
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) semantic_operation: Option<String>,
    pub(crate) site: Option<u32>,
    pub(crate) arguments: Vec<String>,
    pub(crate) guard_paths: Option<Vec<StoredGuardPath>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredGuardPath {
    pub(crate) guards: Vec<StoredGuard>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredGuard {
    pub(crate) condition: String,
    pub(crate) taken: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredMmioAccess {
    pub(crate) address: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredEffectSummary {
    pub(crate) call_graph_closed: bool,
    pub(crate) reachable_functions: Vec<String>,
    pub(crate) context_projection_complete: bool,
    pub(crate) context_projection_blockers: Vec<String>,
    pub(crate) context_fields: Vec<StoredContextField>,
    pub(crate) memory_fields: Vec<StoredMemoryField>,
    pub(crate) semantic_operations: Vec<StoredSemanticOperation>,
    pub(crate) trampoline_calls: Vec<StoredIgnored>,
    pub(crate) event_dispatches: Vec<StoredIgnored>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredIgnored {}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredContextField {
    pub(crate) argument: u8,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredMemoryField {
    pub(crate) object: StoredMemoryObject,
    pub(crate) offset: i64,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum StoredMemoryObject {
    Argument {
        index: u8,
    },
    Global {
        member: Option<String>,
        symbol: String,
    },
    DereferencedGlobal {
        member: Option<String>,
        symbol: String,
        pointer_offset: i64,
    },
    Absolute {
        address_space: String,
        address: u32,
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredSemanticOperation {
    pub(crate) operation: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredScenarioSuggestion {
    pub(crate) kind: String,
    pub(crate) site: Option<u32>,
    pub(crate) evidence: String,
    pub(crate) variants: Vec<StoredScenarioVariant>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredScenarioVariant {
    pub(crate) name: String,
    pub(crate) arguments: Vec<StoredScenarioArgument>,
    pub(crate) mmio_reads: Vec<StoredScenarioMmioRead>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredScenarioArgument {
    pub(crate) index: u8,
    pub(crate) value: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredScenarioMmioRead {
    pub(crate) address: u32,
    pub(crate) mask: u32,
    pub(crate) expected: u32,
    pub(crate) values: Vec<u32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredMmioRegister {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) names: Vec<String>,
    pub(crate) functions: Vec<String>,
    pub(crate) field_candidates: Vec<StoredFieldCandidate>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredFieldCandidate {
    pub(crate) least_significant_bit: u8,
    pub(crate) most_significant_bit: u8,
    pub(crate) mask: u32,
    pub(crate) write_shapes: usize,
    pub(crate) predicate_shapes: usize,
    pub(crate) poll_shapes: usize,
    pub(crate) functions: Vec<String>,
    pub(crate) access_functions: Vec<String>,
    pub(crate) predicate_functions: Vec<String>,
    pub(crate) predicate_evidence: Vec<StoredPredicateEvidence>,
    pub(crate) semantic_operations: Vec<String>,
    pub(crate) semantic_roots: Vec<String>,
    pub(crate) semantic_evidence: Vec<StoredSemanticEvidence>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredPredicateEvidence {
    pub(crate) kind: String,
    pub(crate) function: String,
    pub(crate) producer_path: Vec<String>,
    pub(crate) condition: String,
    pub(crate) effective_operation: Option<String>,
    pub(crate) register_comparison_value: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StoredSemanticEvidence {
    pub(crate) kind: String,
    pub(crate) root: String,
    pub(crate) operation: String,
    pub(crate) action_target: String,
    pub(crate) action_origin: String,
    pub(crate) predicate_function: String,
    pub(crate) path_expression: String,
    pub(crate) residual_path_expression: String,
    pub(crate) condition: String,
    pub(crate) effective_operation: String,
}

pub(crate) fn parse_linked_ir(input: &str) -> Result<LinkedIrStoredDocument> {
    let document: LinkedIrStoredDocument = serde_json::from_str(input)?;
    if document.schema_version != super::LINKED_IR.version
        || document.command != super::LINKED_IR.command
    {
        return Err(crate::Error::invalid(format!(
            "expected schema-v{} {} artifact",
            super::LINKED_IR.version,
            super::LINKED_IR.command
        )));
    }
    if document.completeness_claim || document.mmio_field_semantics_claim {
        return Err(crate::Error::invalid(
            "linked-IR artifact makes an unsupported completeness or field-semantics claim",
        ));
    }
    Ok(document)
}
