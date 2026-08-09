//! Reviewed interface-pack records and the resolved workspace view.

use std::{collections::BTreeSet, fs, path::Path};

use toml_edit::{Document, DocumentMut, Item};

use super::{InterfaceFactStep, InterfaceFacts, SemanticCatalogs};
use crate::Result;
use crate::{ExternalReturnModel, HarnessContractSpec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReviewStatus {
    Unreviewed,
    Reviewed,
    Ignored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackOrigin {
    Observed,
    Manual,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InterfaceRootSelector {
    RelocatedSymbol {
        member: Option<String>,
        symbol: String,
        addend: i64,
        addressing: String,
    },
    FunctionArgument {
        argument: u8,
    },
    AbsoluteAddress {
        address: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InterfaceGuard {
    ArtifactSha256 {
        sha256: String,
    },
    RuntimeValue {
        purpose: String,
        offset: i32,
        width: u8,
        mask: u64,
        value: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceSlot {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) status: ReviewStatus,
    pub(crate) origin: PackOrigin,
    pub(crate) name: Option<String>,
    pub(crate) arguments: Option<Vec<String>>,
    pub(crate) return_type: Option<String>,
    pub(crate) variadic: bool,
    pub(crate) semantic: Option<String>,
    pub(crate) execution_model: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceIndexDomain {
    pub(crate) argument: u8,
    pub(crate) min: u32,
    pub(crate) max: u32,
    pub(crate) evidence: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceAnchor {
    pub(crate) id: String,
    pub(crate) status: ReviewStatus,
    pub(crate) origin: PackOrigin,
    pub(crate) source: String,
    pub(crate) root: InterfaceRootSelector,
    pub(crate) container_path: Vec<InterfaceFactStep>,
    pub(crate) layout_version: Option<String>,
    pub(crate) pointer_width: Option<u8>,
    pub(crate) layout_size: Option<u32>,
    pub(crate) slot_stride: Option<u8>,
    pub(crate) execution_contract: Option<String>,
    pub(crate) index_domains: Vec<InterfaceIndexDomain>,
    pub(crate) guards: Vec<InterfaceGuard>,
    pub(crate) slots: Vec<InterfaceSlot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfacePack {
    pub(crate) id: String,
    pub(crate) calling_convention: String,
    pub(crate) anchors: Vec<InterfaceAnchor>,
}

struct LoadedInterfacePack {
    value: InterfacePack,
    input: String,
    document: Document<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct InterfaceWorkspaceSummary {
    pub(crate) fact_tables: usize,
    pub(crate) observed_slots: usize,
    pub(crate) observed_calls: usize,
    pub(crate) reviewed_anchors: usize,
    pub(crate) ignored_anchors: usize,
    pub(crate) unreviewed_anchors: usize,
    pub(crate) manual_anchors: usize,
    pub(crate) reviewed_slots: usize,
    pub(crate) ignored_slots: usize,
    pub(crate) unreviewed_slots: usize,
    pub(crate) manual_slots: usize,
    pub(crate) semantic_links: usize,
    pub(crate) semantic_operations: usize,
    pub(crate) execution_contracts: usize,
    pub(crate) execution_models: usize,
    pub(crate) artifact_guards: usize,
    pub(crate) runtime_guards: usize,
    pub(crate) resolved_calls: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ResolvedInterfaceArgument {
    pub(crate) index: usize,
    pub(crate) kind: String,
    pub(crate) expression: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ResolvedInterfaceCall {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) function: String,
    pub(crate) function_address: u32,
    pub(crate) site: u32,
    pub(crate) kind: String,
    pub(crate) jalr_offset: i32,
    pub(crate) slot_selector: Option<String>,
    pub(crate) slot_index: Option<u32>,
    pub(crate) slot_index_domain: Option<ResolvedInterfaceIndexDomain>,
    pub(crate) arguments: Vec<ResolvedInterfaceArgument>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ResolvedInterfaceIndexDomain {
    pub(crate) argument: u8,
    pub(crate) min: u32,
    pub(crate) max: u32,
    pub(crate) evidence: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedInterfaceSlot {
    /// Stable reviewed identity. It deliberately excludes call-site addresses.
    pub(crate) id: String,
    pub(crate) contract: String,
    pub(crate) anchor: String,
    pub(crate) source: String,
    pub(crate) layout_version: String,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) name: String,
    pub(crate) arguments: Vec<String>,
    pub(crate) return_type: String,
    pub(crate) variadic: bool,
    pub(crate) semantic: Option<String>,
    pub(crate) semantic_annotation: Option<ResolvedSemanticAnnotation>,
    pub(crate) execution_model: Option<ResolvedExternalCallExecutionModel>,
    pub(crate) functions: BTreeSet<String>,
    pub(crate) calls: Vec<ResolvedInterfaceCall>,
}

/// Discovered slot evidence that has not yet been classified by a reviewed
/// slot claim. It is deliberately descriptive only: no ABI, semantic or
/// executable behavior can be attached until the pack is reviewed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UnreviewedInterfaceObservation {
    pub(crate) id: String,
    pub(crate) contract: String,
    pub(crate) source: String,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) selector: Option<String>,
    pub(crate) functions: Vec<String>,
    pub(crate) call_sites: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedSemanticAnnotation {
    pub(crate) operation: String,
    pub(crate) domain: String,
    pub(crate) summary: String,
    pub(crate) argument_roles: Vec<String>,
    pub(crate) return_role: String,
    pub(crate) effects: Vec<String>,
    pub(crate) replacement: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedExternalCallExecutionModel {
    pub(crate) id: String,
    pub(crate) table: String,
    pub(crate) function: String,
    pub(crate) c_name: String,
    pub(crate) return_model: ExternalReturnModel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedInterfaceExecutionContract {
    pub(crate) id: String,
    pub(crate) pointer_symbol: String,
    pub(crate) backing_symbol: String,
    pub(crate) version: u32,
    pub(crate) magic: u32,
    pub(crate) size: u32,
    pub(crate) magic_offset: u32,
}

/// Reviewed layout/guard contract joined to current binary evidence.
///
/// Semantic annotations remain descriptive. Only `execution_contract` and a
/// slot's explicit `execution_model` foreign key authorize use of compiled
/// behavior supplied by a platform harness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedInterfaceContract {
    pub(crate) id: String,
    pub(crate) pack: String,
    pub(crate) anchor: String,
    pub(crate) source: String,
    pub(crate) root: InterfaceRootSelector,
    pub(crate) container_path: Vec<InterfaceFactStep>,
    pub(crate) layout_version: String,
    pub(crate) pointer_width: u8,
    pub(crate) layout_size: u32,
    pub(crate) slot_stride: u8,
    pub(crate) guards: Vec<InterfaceGuard>,
    pub(crate) execution_contract: Option<ResolvedInterfaceExecutionContract>,
    pub(crate) slots: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct InterfaceWorkspace {
    summary: InterfaceWorkspaceSummary,
    contracts: Vec<ResolvedInterfaceContract>,
    bindings: Vec<ResolvedInterfaceSlot>,
    unreviewed_observations: Vec<UnreviewedInterfaceObservation>,
}

impl InterfaceWorkspace {
    pub(crate) fn load(
        facts_path: &Path,
        pack_path: &Path,
        semantic_paths: &[impl AsRef<Path>],
        calling_convention: &str,
        execution_contracts: Option<&HarnessContractSpec>,
    ) -> Result<Self> {
        let facts = InterfaceFacts::load(facts_path)?;
        let catalogs = SemanticCatalogs::load(semantic_paths)?;
        let pack = InterfacePack::load(pack_path)?;
        let (summary, contracts, bindings, unreviewed_observations) = pack
            .value
            .validate(&facts, &catalogs, calling_convention, execution_contracts)
            .map_err(|error| {
                crate::error::WorkbenchError::manifest_source(
                    "interface pack",
                    pack_path,
                    &pack.input,
                    &error,
                    error.span(&pack.document),
                )
            })?;
        Ok(Self {
            summary,
            contracts,
            bindings,
            unreviewed_observations,
        })
    }

    pub(crate) const fn summary(&self) -> InterfaceWorkspaceSummary {
        self.summary
    }

    pub(crate) fn bindings(&self) -> &[ResolvedInterfaceSlot] {
        &self.bindings
    }

    pub(crate) fn unreviewed_observations(&self) -> &[UnreviewedInterfaceObservation] {
        &self.unreviewed_observations
    }

    pub(crate) fn contracts(&self) -> &[ResolvedInterfaceContract] {
        &self.contracts
    }

    pub(crate) fn validate_table_instance(
        &self,
        instance: &crate::execution_model::TableInstance,
    ) -> Result<()> {
        let contract = self
            .contracts
            .iter()
            .find(|contract| contract.id == instance.layout_id)
            .ok_or_else(|| {
                crate::Error::invalid(format!(
                    "runtime table instance refers to unknown reviewed layout {}",
                    instance.layout_id
                ))
            })?;
        if contract.pointer_width != 32 {
            return Err(crate::Error::invalid(format!(
                "runtime table layout {} uses unsupported {}-bit pointers",
                contract.id, contract.pointer_width
            )));
        }
        if contract.layout_size != instance.layout_size {
            return Err(crate::Error::invalid(format!(
                "runtime table layout {} requires size {:#x}, instance declares {:#x}",
                contract.id, contract.layout_size, instance.layout_size
            )));
        }
        for slot in &instance.slots {
            let slot_offset = i32::try_from(slot.offset).map_err(|_| {
                crate::Error::invalid(format!(
                    "runtime table layout {} slot {:#x} does not fit reviewed signed offsets",
                    contract.id, slot.offset
                ))
            })?;
            if !self.bindings.iter().any(|binding| {
                binding.contract == contract.id
                    && binding.offset == slot_offset
                    && binding.width == contract.pointer_width
            }) {
                return Err(crate::Error::invalid(format!(
                    "runtime table layout {} has no reviewed 32-bit slot at {:#x}",
                    contract.id, slot.offset
                )));
            }
        }
        Ok(())
    }
}

impl InterfacePack {
    #[tracing::instrument(name = "load_interface_pack", fields(path = %path.display()))]
    fn load(path: &Path) -> Result<LoadedInterfacePack> {
        let input = fs::read_to_string(path)
            .map_err(|error| crate::Error::read("interface pack", path, error))?;
        let source_document = Document::parse(input.clone()).map_err(|error| {
            crate::error::WorkbenchError::manifest_source(
                "interface pack",
                path,
                &input,
                &error,
                error.span(),
            )
        })?;
        let document: DocumentMut = source_document.clone().into_mut();
        if document.get("schema").and_then(Item::as_integer) != Some(1) {
            return Err(crate::error::WorkbenchError::manifest_source(
                "interface pack",
                path,
                &input,
                "requires schema = 1",
                source_document.get("schema").and_then(Item::span),
            ));
        }
        let value = super::pack_parse::parse(&document).map_err(|error| {
            crate::error::WorkbenchError::manifest("interface pack", path, error)
        })?;
        Ok(LoadedInterfacePack {
            value,
            input,
            document: source_document,
        })
    }
}
