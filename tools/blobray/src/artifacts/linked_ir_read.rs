//! Strict typed consumer for the persistent linked-IR schema.

pub(super) mod schema;

pub(crate) use schema::{
    GuardedReturnClassification, GuardedReturnMatch, LinkedIrStoredDocument, StoredCall,
    StoredDataObject, StoredFlowValue, StoredFunction, StoredInstructionEffect,
    StoredLocalValueFlow, StoredMemoryObject, StoredMmioAccess, StoredMmioRegister,
    StoredReviewCall, StoredReviewDirectEffect,
};

use crate::Result;
use std::collections::BTreeSet;

pub(crate) fn parse_linked_ir(input: &str) -> Result<LinkedIrStoredDocument> {
    super::expect_identity(input, super::LINKED_IR)?;
    let document: LinkedIrStoredDocument = serde_json::from_str(input)?;
    if document.completeness_claim || document.mmio_field_semantics_claim {
        return Err(crate::Error::invalid(
            "linked-IR artifact makes an unsupported completeness or field-semantics claim",
        ));
    }
    let artifacts = document
        .artifacts
        .iter()
        .map(|artifact| (artifact.source.as_str(), artifact.artifact.sha256.as_str()))
        .collect::<BTreeSet<_>>();
    for function in &document.functions {
        if !artifacts.contains(&(function.source.as_str(), function.artifact_sha256.as_str())) {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {:?} refers to an undeclared source artifact {}@{}",
                function.identity, function.source, function.artifact_sha256
            )));
        }
        schema::validate_function_loops(&function.identity, &function.loops)?;
        schema::validate_call_arguments(&function.identity, &function.calls)?;
        schema::validate_return_frontiers(function)?;
    }
    Ok(document)
}
