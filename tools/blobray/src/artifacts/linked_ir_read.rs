//! Strict typed consumer for the persistent linked-IR schema.

pub(super) mod schema;

pub(crate) use schema::{
    DataObjectAssociation, GuardedReturnClassification, GuardedReturnMatch, LinkedIrStoredDocument,
    StoredCall, StoredDataObject, StoredFlowValue, StoredFunction, StoredInstructionEffect,
    StoredLocalValueFlow, StoredMemoryObject, StoredMmioAccess, StoredMmioRegister,
    StoredReviewCall, StoredReviewDirectEffect,
};

use crate::Result;
use std::collections::BTreeSet;

/// Companion definitions are qualified by the analysis source that loaded them.
pub(super) fn declares_source_artifact(
    document: &LinkedIrStoredDocument,
    source: &str,
    digest: &str,
) -> bool {
    document.artifacts.iter().any(|artifact| {
        artifact.source == source
            && (artifact.artifact.sha256 == digest
                || artifact
                    .companions
                    .iter()
                    .any(|companion| companion.sha256 == digest))
    })
}

pub(crate) fn parse_linked_ir(input: &str) -> Result<LinkedIrStoredDocument> {
    super::expect_identity(input, super::LINKED_IR)?;
    let document: LinkedIrStoredDocument = serde_json::from_str(input)?;
    if document.completeness_claim || document.mmio_field_semantics_claim {
        return Err(crate::Error::invalid(
            "linked-IR artifact makes an unsupported completeness or field-semantics claim",
        ));
    }
    for blocker in &document.root_blockers {
        if !declares_source_artifact(&document, &blocker.source, &blocker.artifact_sha256)
            || blocker
                .code_identity
                .artifact_sha256()
                .is_some_and(|digest| digest != blocker.artifact_sha256)
        {
            return Err(crate::Error::invalid(
                "linked-IR root blocker has inconsistent physical source identity",
            ));
        }
    }
    for function in &document.functions {
        if !declares_source_artifact(&document, &function.source, &function.artifact_sha256) {
            return Err(crate::Error::invalid(format!(
                "linked-IR function {:?} refers to an undeclared source artifact {}@{}",
                function.identity, function.source, function.artifact_sha256
            )));
        }
        crate::artifact_occurrence::validate_function(
            &function.code_identity,
            &function.source,
            &function.artifact_sha256,
            &function.locator,
            &function.occurrence,
            function.semantic.as_deref(),
        )?;
        schema::validate_function_loops(&function.identity, &function.loops)?;
        schema::validate_call_arguments(&function.identity, &function.calls)?;
        schema::validate_return_frontiers(function)?;
    }
    let mut data_identities = BTreeSet::new();
    for object in &document.data_objects {
        if !data_identities.insert((&object.source, &object.data_identity)) {
            return Err(crate::Error::invalid(
                "duplicate linked-IR data object identity",
            ));
        }
        if !declares_source_artifact(&document, &object.source, &object.artifact_sha256) {
            return Err(crate::Error::invalid(format!(
                "linked-IR data object {}:{} refers to an undeclared source artifact {}@{}",
                object.source, object.symbol, object.source, object.artifact_sha256
            )));
        }
        crate::artifact_occurrence::validate_data(
            &object.data_identity,
            &object.source,
            &object.artifact_sha256,
            &object.locator,
            &object.occurrence,
            object.semantic.as_deref(),
        )?;
    }
    Ok(document)
}
