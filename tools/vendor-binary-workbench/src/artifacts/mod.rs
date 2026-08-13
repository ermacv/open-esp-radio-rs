//! Version and command identities for persistent workbench artifacts.
//!
//! Producers and consumers share these identities. Command-result schemas are
//! intentionally separate because they describe one invocation rather than a
//! reusable project artifact.

mod interface_facts;
mod interface_facts_read;
mod json;
mod linked_ir;
mod linked_ir_bundle;
mod linked_ir_document;
mod linked_ir_read;
mod mmio_facts;
mod mmio_facts_read;
mod replay_evidence;
mod replay_evidence_read;
pub(crate) mod symbol_inventory;

pub(crate) use interface_facts::build_interface_facts;
pub(crate) use interface_facts_read::{
    StoredInterfaceArgument, StoredInterfaceFacts, StoredInterfaceRoot, StoredInterfaceSelector,
    StoredInterfaceStep, parse_interface_facts,
};
pub(crate) use linked_ir::inspect_linked_ir;
#[cfg(test)]
pub(crate) use linked_ir_bundle::write_fixture_bundle;
pub(crate) use linked_ir_bundle::{
    GraphSearchLimits, LinkedIrReader, LinkedIrReviewProjection, StoredGraphEdge, bundle_files,
    load_linked_ir_functions,
};
#[cfg(test)]
pub(crate) use linked_ir_document::render_linked_ir_fixture;
pub(crate) use linked_ir_document::{
    StagedLinkedIrBundle, build_linked_ir_document, stage_linked_ir_bundle,
};
pub(crate) use linked_ir_read::{
    LinkedIrStoredDocument, StoredCall, StoredDataObject, StoredFlowValue, StoredFunction,
    StoredInstructionEffect, StoredLocalValueFlow, StoredMemoryObject, StoredMmioAccess,
    StoredMmioRegister, StoredReviewCall, StoredReviewDirectEffect, parse_linked_ir,
};
pub(crate) use mmio_facts::{MmioFactsDocument, build_mmio_facts};
pub(crate) use mmio_facts_read::parse_mmio_facts;
pub(crate) use replay_evidence::{
    ReplayCompletionDocument, ReplayEvidenceDocument, ReplayMemoryObservationDocument,
    ReplayMemoryWriteDocument, ReplayPhaseEvidence, build_replay_evidence, render_replay_evidence,
};
pub(crate) use replay_evidence_read::{
    StoredFifoLifecycleEvent, StoredReplayCompletion, StoredReplayPhase, parse_replay_evidence,
};
pub(crate) use symbol_inventory::{
    LinkUnitOriginFact, StoredSymbolInventory, SymbolInventoryDocument,
    build_symbol_inventory_document, inspect_symbol_inventory, load_link_unit_origins,
    parse_symbol_inventory,
};

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactSchema {
    pub(crate) version: u32,
    pub(crate) command: &'static str,
}

pub(crate) const SYMBOL_INVENTORY: ArtifactSchema = ArtifactSchema {
    version: 4,
    command: "symbols inventory",
};

pub(crate) const MMIO_FACTS: ArtifactSchema = ArtifactSchema {
    version: 5,
    command: "mmio discover",
};

pub(crate) const INTERFACE_FACTS: ArtifactSchema = ArtifactSchema {
    version: 5,
    command: "interfaces discover",
};

pub(crate) const LINKED_IR: ArtifactSchema = ArtifactSchema {
    version: 53,
    command: "ir export",
};

pub(crate) const REPLAY_EVIDENCE: ArtifactSchema = ArtifactSchema {
    version: 2,
    command: "execute replay",
};

#[derive(Deserialize)]
struct ArtifactIdentityDocument {
    schema_version: u32,
    command: String,
}

fn expect_identity(input: &str, schema: ArtifactSchema) -> crate::Result<()> {
    let identity: ArtifactIdentityDocument = serde_json::from_str(input)?;
    if identity.schema_version != schema.version || identity.command != schema.command {
        return Err(crate::Error::invalid(format!(
            "expected schema_version {} and command {:?}",
            schema.version, schema.command
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_schema_identities_are_explicit_and_distinct() {
        assert_eq!(
            SYMBOL_INVENTORY,
            ArtifactSchema {
                version: 4,
                command: "symbols inventory",
            }
        );
        assert_eq!(
            MMIO_FACTS,
            ArtifactSchema {
                version: 5,
                command: "mmio discover",
            }
        );
        assert_eq!(
            INTERFACE_FACTS,
            ArtifactSchema {
                version: 5,
                command: "interfaces discover",
            }
        );
        assert_eq!(
            LINKED_IR,
            ArtifactSchema {
                version: 53,
                command: "ir export",
            }
        );
        assert_eq!(
            REPLAY_EVIDENCE,
            ArtifactSchema {
                version: 2,
                command: "execute replay",
            }
        );
    }
}
