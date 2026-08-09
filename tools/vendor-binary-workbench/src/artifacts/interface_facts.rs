//! Stable typed JSON projection of generic indirect-call evidence.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{
    Result,
    analysis::{
        DiscoveredInterfaceCall, LinkageSymbolLocation, ProjectInterfaceDiscovery,
        interface_root_linkage,
    },
    interface_discovery::{
        InterfaceArgumentValue, InterfaceCallCandidate, InterfaceCallKind, InterfaceLoad,
        InterfacePointer, InterfaceRoot, InterfaceSlotSelector,
    },
};

#[derive(Serialize)]
#[serde(untagged)]
enum RootDocument {
    Relocated {
        kind: &'static str,
        canonical: String,
        member: Option<String>,
        symbol: String,
        addend: i64,
        addressing: &'static str,
    },
    FunctionArgument {
        kind: &'static str,
        canonical: String,
        argument: u8,
    },
    AbsoluteAddress {
        kind: &'static str,
        canonical: String,
        address: String,
    },
}

impl From<&InterfaceRoot> for RootDocument {
    fn from(root: &InterfaceRoot) -> Self {
        match root {
            InterfaceRoot::RelocatedSymbol {
                member,
                symbol,
                addend,
                addressing,
            } => Self::Relocated {
                kind: root.kind(),
                canonical: root.canonical(),
                member: member.clone(),
                symbol: symbol.clone(),
                addend: *addend,
                addressing: addressing.label(),
            },
            InterfaceRoot::FunctionArgument { index } => Self::FunctionArgument {
                kind: root.kind(),
                canonical: root.canonical(),
                argument: *index,
            },
            InterfaceRoot::AbsoluteAddress { address } => Self::AbsoluteAddress {
                kind: root.kind(),
                canonical: root.canonical(),
                address: format!("{address:#010x}"),
            },
        }
    }
}

#[derive(Serialize)]
struct LocationDocument {
    artifact: usize,
    member: Option<String>,
    address: String,
    kind: &'static str,
}

impl From<&LinkageSymbolLocation> for LocationDocument {
    fn from(location: &LinkageSymbolLocation) -> Self {
        Self {
            artifact: location.artifact,
            member: location.member.clone(),
            address: format!("{:#x}", location.address),
            kind: location.kind.label(),
        }
    }
}

#[derive(Serialize)]
struct LoadDocument {
    site: String,
    offset: i32,
    width: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    selector: Option<SelectorDocument>,
}

#[derive(Clone, Serialize)]
struct SelectorDocument {
    argument: u8,
    scale: u32,
    addend: i32,
    canonical: String,
}

impl From<&InterfaceSlotSelector> for SelectorDocument {
    fn from(selector: &InterfaceSlotSelector) -> Self {
        Self {
            argument: selector.argument,
            scale: selector.scale,
            addend: selector.addend,
            canonical: selector.canonical(),
        }
    }
}

impl From<&InterfaceLoad> for LoadDocument {
    fn from(load: &InterfaceLoad) -> Self {
        Self {
            site: format!("{:#x}", load.site),
            offset: load.offset,
            width: load.width,
            selector: load.selector.as_ref().map(Into::into),
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum ArgumentDocument {
    Unknown {
        index: usize,
        kind: &'static str,
    },
    Constant {
        index: usize,
        kind: &'static str,
        value: String,
    },
    Pointer {
        index: usize,
        kind: &'static str,
        canonical: String,
        root: RootDocument,
        loads: Vec<LoadDocument>,
        post_offset: i32,
    },
}

fn argument_document(index: usize, value: &InterfaceArgumentValue) -> ArgumentDocument {
    match value {
        InterfaceArgumentValue::Unknown => ArgumentDocument::Unknown {
            index,
            kind: "unknown",
        },
        InterfaceArgumentValue::Constant(value) => ArgumentDocument::Constant {
            index,
            kind: "constant",
            value: format!("{value:#010x}"),
        },
        InterfaceArgumentValue::Pointer(pointer) => ArgumentDocument::Pointer {
            index,
            kind: "pointer-provenance",
            canonical: pointer.canonical(),
            root: (&pointer.root).into(),
            loads: pointer.loads.iter().map(Into::into).collect(),
            post_offset: pointer.post_offset,
        },
    }
}

#[derive(Serialize)]
struct TargetDocument {
    canonical: String,
    root: RootDocument,
    loads: Vec<LoadDocument>,
    container_depth: usize,
    slot_offset: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slot_selector: Option<SelectorDocument>,
    jalr_offset: i32,
}

impl TargetDocument {
    fn new(pointer: &InterfacePointer, jalr_offset: i32) -> Self {
        Self {
            canonical: pointer.canonical(),
            root: (&pointer.root).into(),
            loads: pointer.loads.iter().map(Into::into).collect(),
            container_depth: pointer.container_loads().len(),
            slot_offset: pointer.fixed_slot().map(|load| load.offset),
            slot_selector: pointer
                .slot()
                .and_then(|load| load.selector.as_ref())
                .map(Into::into),
            jalr_offset,
        }
    }
}

#[derive(Serialize)]
struct RootLinkageDocument {
    mode: &'static str,
    symbols: BTreeSet<String>,
    resolutions: BTreeSet<&'static str>,
    candidates: Vec<LocationDocument>,
}

#[derive(Serialize)]
struct CallDocument {
    artifact: usize,
    member: Option<String>,
    function: String,
    function_address: String,
    site: String,
    kind: &'static str,
    link_register: u8,
    target: TargetDocument,
    root_linkage: RootLinkageDocument,
    arguments: Vec<ArgumentDocument>,
}

fn call_document(
    discovery: &ProjectInterfaceDiscovery,
    discovered: &DiscoveredInterfaceCall,
) -> CallDocument {
    let call = &discovered.call;
    let linkage = interface_root_linkage(discovery, discovered.artifact, &call.target.root);
    CallDocument {
        artifact: discovered.artifact,
        member: call.member.clone(),
        function: call.function.clone(),
        function_address: format!("{:#x}", call.function_address),
        site: format!("{:#x}", call.site),
        kind: call.kind.label(),
        link_register: match call.kind {
            InterfaceCallKind::Call => 1,
            InterfaceCallKind::TailJump => 0,
            InterfaceCallKind::LinkedJump(register) => register,
        },
        target: TargetDocument::new(&call.target, call.jalr_offset),
        root_linkage: RootLinkageDocument {
            mode: "association-only",
            symbols: linkage.symbols,
            resolutions: linkage.resolutions,
            candidates: linkage.candidates.iter().map(Into::into).collect(),
        },
        arguments: call
            .arguments
            .iter()
            .enumerate()
            .map(|(index, value)| argument_document(index, value))
            .collect(),
    }
}

#[derive(Serialize)]
struct ContainerStepDocument {
    offset: i32,
    width: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    selector: Option<SelectorDocument>,
}

#[derive(Serialize)]
struct SlotDocument {
    offset: i32,
    width: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    selector: Option<SelectorDocument>,
    functions: BTreeSet<String>,
    call_sites: usize,
}

#[derive(Serialize)]
struct TableGroupDocument {
    artifact: usize,
    root: RootDocument,
    container_path: Vec<ContainerStepDocument>,
    slots: Vec<SlotDocument>,
    functions: BTreeSet<String>,
    call_sites: usize,
}

fn table_group_documents(discovery: &ProjectInterfaceDiscovery) -> Vec<TableGroupDocument> {
    type StepKey = (i32, u8, Option<InterfaceSlotSelector>);
    type GroupKey = (usize, InterfaceRoot, Vec<StepKey>);
    let mut groups = BTreeMap::<GroupKey, Vec<&InterfaceCallCandidate>>::new();
    for discovered in &discovery.calls {
        if discovered.call.target.loads.is_empty() {
            continue;
        }
        groups
            .entry((
                discovered.artifact,
                discovered.call.target.root.clone(),
                discovered
                    .call
                    .target
                    .container_loads()
                    .iter()
                    .map(|load| (load.offset, load.width, load.selector.clone()))
                    .collect(),
            ))
            .or_default()
            .push(&discovered.call);
    }
    groups
        .into_iter()
        .map(|((artifact, root, container), calls)| {
            let mut slots = BTreeMap::<StepKey, Vec<&InterfaceCallCandidate>>::new();
            for call in &calls {
                if let Some(slot) = call.target.slot() {
                    slots
                        .entry((slot.offset, slot.width, slot.selector.clone()))
                        .or_default()
                        .push(call);
                }
            }
            TableGroupDocument {
                artifact,
                root: (&root).into(),
                container_path: container
                    .into_iter()
                    .map(|(offset, width, selector)| ContainerStepDocument {
                        offset,
                        width,
                        selector: selector.as_ref().map(Into::into),
                    })
                    .collect(),
                slots: slots
                    .into_iter()
                    .map(|((offset, width, selector), slot_calls)| SlotDocument {
                        offset,
                        width,
                        selector: selector.as_ref().map(Into::into),
                        functions: slot_calls
                            .iter()
                            .map(|call| call.function.clone())
                            .collect(),
                        call_sites: slot_calls.len(),
                    })
                    .collect(),
                functions: calls.iter().map(|call| call.function.clone()).collect(),
                call_sites: calls.len(),
            }
        })
        .collect()
}

#[derive(Serialize)]
struct ArtifactDocument<'a> {
    index: usize,
    path: String,
    roles: &'a [String],
    sources: &'a [String],
    sha256: String,
    container: &'static str,
    functions: usize,
    reviewed_boundaries: usize,
}

#[derive(Serialize)]
struct AnalysisScope {
    architecture: &'static str,
    calling_convention: &'static str,
    evidence: &'static str,
    relocation_evidence: [&'static str; 3],
    semantic_claim: bool,
    table_layout_claim: bool,
    linker_resolution_claim: bool,
    completeness_claim: bool,
}

#[derive(Serialize)]
struct DecodeFailureDocument<'a> {
    artifact: usize,
    member: &'a Option<String>,
    function: &'a str,
    error: &'a str,
}

#[derive(Serialize)]
pub(crate) struct InterfaceFactsDocument<'a> {
    schema_version: u32,
    command: &'static str,
    analysis_scope: AnalysisScope,
    artifacts: Vec<ArtifactDocument<'a>>,
    calls: Vec<CallDocument>,
    table_candidates: Vec<TableGroupDocument>,
    decode_failures: Vec<DecodeFailureDocument<'a>>,
}

pub(crate) fn build_interface_facts(
    discovery: &ProjectInterfaceDiscovery,
) -> Result<InterfaceFactsDocument<'_>> {
    Ok(InterfaceFactsDocument {
        schema_version: super::INTERFACE_FACTS.version,
        command: super::INTERFACE_FACTS.command,
        analysis_scope: AnalysisScope {
            architecture: "riscv32",
            calling_convention: "riscv-ilp32",
            evidence: "control-flow-merged register provenance",
            relocation_evidence: ["absolute", "pc-relative", "got"],
            semantic_claim: false,
            table_layout_claim: false,
            linker_resolution_claim: false,
            completeness_claim: false,
        },
        artifacts: discovery
            .linkage
            .artifacts
            .iter()
            .enumerate()
            .map(|(index, artifact)| {
                Ok(ArtifactDocument {
                    index,
                    path: artifact.path.display().to_string(),
                    roles: &artifact.roles,
                    sources: &artifact.sources,
                    sha256: crate::artifact_sha256(&artifact.path)?,
                    container: artifact.container.label(),
                    functions: discovery.functions[index],
                    reviewed_boundaries: discovery.reviewed_boundaries[index],
                })
            })
            .collect::<Result<Vec<_>>>()?,
        calls: discovery
            .calls
            .iter()
            .map(|call| call_document(discovery, call))
            .collect(),
        table_candidates: table_group_documents(discovery),
        decode_failures: discovery
            .failures
            .iter()
            .map(|failure| DecodeFailureDocument {
                artifact: failure.artifact,
                member: &failure.member,
                function: &failure.function,
                error: &failure.error,
            })
            .collect(),
    })
}

pub(crate) fn render_interface_facts(document: &InterfaceFactsDocument<'_>) -> Result<String> {
    Ok(serde_json::to_string_pretty(&document)? + "\n")
}
