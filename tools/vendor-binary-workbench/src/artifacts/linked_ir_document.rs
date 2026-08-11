//! Typed JSON linked-IR report rendering.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    path::PathBuf,
};

use serde::Serialize;

use crate::{
    EntryContractRef, LinkedIrFunction, LinkedIrReport, LinkedMmioRegister, LinkedTrampolineSlot,
    Result, SemanticBoundary,
    linked_ir_export::{IrArtifactInput, field_candidate_summary, provenance_summary},
};

#[derive(Clone, Serialize)]
struct ArtifactIdentity {
    path: String,
    sha256: String,
}

impl ArtifactIdentity {
    fn load(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.display().to_string(),
            sha256: crate::artifact_sha256(path)?,
        })
    }
}

#[derive(Clone, Serialize)]
struct SourceArtifact<'a> {
    source: &'a str,
    artifact: ArtifactIdentity,
    reviewed_code_boundaries: Vec<ReviewedCodeBoundaryDocument<'a>>,
}

#[derive(Clone, Serialize)]
struct ReviewedCodeBoundaryDocument<'a> {
    member: &'a Option<String>,
    section: &'a str,
    name: &'a str,
    start_offset: String,
    end_offset: String,
}

#[derive(Clone, Serialize)]
struct ReportSummary {
    artifacts: usize,
    reviewed_code_boundaries: usize,
    functions: usize,
    decode_blocker_functions: usize,
    decode_blockers: usize,
    root_functions: usize,
    included_reachable_functions: usize,
    exported: usize,
    local: usize,
    mmio_registers: usize,
    mmio_functions: usize,
    mmio_access_shapes: usize,
    instruction_effects: usize,
    mmio_field_candidate_registers: usize,
    mmio_field_candidates: usize,
    direct_mmio_predicates: usize,
    direct_mmio_predicate_sources: usize,
    delay_functions: usize,
    delay_shapes: usize,
    context_functions: usize,
    context_fields: usize,
    context_accesses: usize,
    memory_functions: usize,
    memory_fields: usize,
    memory_accesses: usize,
    semantic_operations: usize,
    semantic_calls: usize,
    trampoline_slots: usize,
    trampoline_calls: usize,
    complete: usize,
    structured: usize,
    internal_calls: usize,
    external_calls: usize,
    call_argument_shapes: usize,
    project_linked_calls: usize,
    ambiguous_project_calls: usize,
    unresolved_calls: usize,
    closed_effect_summaries: usize,
    recursive_effect_summaries: usize,
    complete_context_projections: usize,
    projected_context_fields: usize,
    projected_memory_fields: usize,
    exact_return_functions: usize,
    return_source_ranges: usize,
    mmio_return_sources: usize,
    guard_mmio_links: usize,
    transitive_guard_mmio_links: usize,
    scenario_suggestions: usize,
    data_objects: usize,
    initialized_data_objects: usize,
    data_object_relocations: usize,
    data_object_xrefs: usize,
}

impl ReportSummary {
    fn new(
        artifacts: &[IrArtifactInput],
        report: &LinkedIrReport,
        data_objects: &[DataObjectDocument],
    ) -> Self {
        let root_functions = report
            .functions
            .iter()
            .filter(|function| function.selection == "symbol-prefix-root")
            .count();
        let (
            exact_return_functions,
            return_source_ranges,
            mmio_return_sources,
            guard_mmio_links,
            transitive_guard_mmio_links,
        ) = provenance_summary(report);
        let (
            mmio_field_candidate_registers,
            mmio_field_candidates,
            direct_mmio_predicates,
            direct_mmio_predicate_sources,
        ) = field_candidate_summary(report);
        Self {
            artifacts: artifacts.len(),
            reviewed_code_boundaries: artifacts
                .iter()
                .map(|artifact| artifact.reviewed_code.len())
                .sum(),
            functions: report.functions.len(),
            decode_blocker_functions: report
                .functions
                .iter()
                .filter(|function| !function.decode_blockers.is_empty())
                .count(),
            decode_blockers: report
                .functions
                .iter()
                .map(|function| function.decode_blockers.len())
                .sum(),
            root_functions,
            included_reachable_functions: report.functions.len() - root_functions,
            exported: report.exported_functions,
            local: report.local_functions,
            mmio_registers: report.mmio_registers.len(),
            mmio_functions: report.mmio_functions,
            mmio_access_shapes: report.mmio_access_shapes,
            instruction_effects: report
                .functions
                .iter()
                .map(|function| function.instruction_effects.len())
                .sum(),
            mmio_field_candidate_registers,
            mmio_field_candidates,
            direct_mmio_predicates,
            direct_mmio_predicate_sources,
            delay_functions: report.delay_functions,
            delay_shapes: report.delay_shapes,
            context_functions: report.context_functions,
            context_fields: report.context_fields,
            context_accesses: report.context_accesses,
            memory_functions: report.memory_functions,
            memory_fields: report.memory_fields,
            memory_accesses: report.memory_accesses,
            semantic_operations: report.semantic_boundaries.len(),
            semantic_calls: report.semantic_calls,
            trampoline_slots: report.trampoline_slots.len(),
            trampoline_calls: report.trampoline_calls,
            complete: report.complete_functions,
            structured: report.structured_functions,
            internal_calls: report.internal_calls,
            external_calls: report.external_calls,
            call_argument_shapes: report.call_argument_shapes,
            project_linked_calls: report.project_linked_calls,
            ambiguous_project_calls: report.ambiguous_project_calls,
            unresolved_calls: report.unresolved_calls,
            closed_effect_summaries: report.closed_effect_summaries,
            recursive_effect_summaries: report.recursive_effect_summaries,
            complete_context_projections: report.complete_context_projections,
            projected_context_fields: report.projected_context_fields,
            projected_memory_fields: report.projected_memory_fields,
            exact_return_functions,
            return_source_ranges,
            mmio_return_sources,
            guard_mmio_links,
            transitive_guard_mmio_links,
            scenario_suggestions: report.scenario_suggestions,
            data_objects: data_objects.len(),
            initialized_data_objects: data_objects
                .iter()
                .filter(|object| object.initialized)
                .count(),
            data_object_relocations: data_objects
                .iter()
                .map(|object| object.relocations.len())
                .sum(),
            data_object_xrefs: data_objects.iter().map(|object| object.xrefs.len()).sum(),
        }
    }
}

#[derive(Clone, Serialize)]
struct DataObjectRelocationDocument {
    offset: String,
    elf_type: Option<u32>,
    target: String,
    addend: i64,
}

#[derive(Clone, Serialize)]
struct DataObjectXrefDocument {
    function: String,
    reads: usize,
    writes: usize,
    offsets: Vec<String>,
    indexed_by: Vec<String>,
}

#[derive(Clone, Serialize)]
struct DataObjectDocument {
    source: String,
    member: Option<String>,
    section: String,
    symbol: String,
    aliases: Vec<String>,
    address: Option<String>,
    object_offset: String,
    size: u64,
    writable: bool,
    initialized: bool,
    synthetic_from_anchor: bool,
    exported: bool,
    initializer_hex: Option<String>,
    relocations: Vec<DataObjectRelocationDocument>,
    xrefs: Vec<DataObjectXrefDocument>,
}

type GlobalAccess<'a> = (Option<&'a str>, &'a str, Option<(u8, i64)>);

fn global_access(object: &crate::LinkedMemoryObject) -> Option<GlobalAccess<'_>> {
    match object {
        crate::LinkedMemoryObject::Global { member, symbol } => {
            Some((member.as_deref(), symbol.as_str(), None))
        }
        crate::LinkedMemoryObject::Indexed {
            object,
            argument,
            stride,
        } => global_access(object)
            .map(|(member, symbol, _)| (member, symbol, Some((*argument, *stride)))),
        _ => None,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        write!(output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn load_data_objects(
    artifacts: &[IrArtifactInput],
    report: &LinkedIrReport,
) -> Result<Vec<DataObjectDocument>> {
    let mut xrefs = BTreeMap::<
        (String, Option<String>, String, String),
        (usize, usize, BTreeSet<i64>, BTreeSet<String>),
    >::new();
    for function in &report.functions {
        for access in &function.memory_accesses {
            let Some((member, symbol, index)) = global_access(&access.object) else {
                continue;
            };
            let entry = xrefs
                .entry((
                    function.source.clone(),
                    member.map(str::to_owned),
                    symbol.to_owned(),
                    function.identity.clone(),
                ))
                .or_default();
            match access.access {
                "read" => entry.0 += 1,
                "write" => entry.1 += 1,
                _ => {}
            }
            entry.2.insert(access.offset);
            if let Some((argument, stride)) = index {
                entry.3.insert(format!("arg{argument} * {stride:+#x}"));
            }
        }
    }

    let mut output = Vec::new();
    for artifact in artifacts {
        for object in crate::artifact::load_data_objects(&artifact.path)? {
            let object_xrefs = xrefs
                .iter()
                .filter(|((source, member, symbol, _), _)| {
                    source == &artifact.source
                        && member == &object.member
                        && (symbol == &object.name || object.aliases.contains(symbol))
                })
                .map(
                    |((_, _, _, function), (reads, writes, offsets, indexed_by))| {
                        DataObjectXrefDocument {
                            function: function.clone(),
                            reads: *reads,
                            writes: *writes,
                            offsets: offsets
                                .iter()
                                .map(|offset| format!("{offset:+#x}"))
                                .collect(),
                            indexed_by: indexed_by.iter().cloned().collect(),
                        }
                    },
                )
                .collect();
            output.push(DataObjectDocument {
                source: artifact.source.clone(),
                member: object.member,
                section: object.section,
                symbol: object.name,
                aliases: object.aliases,
                address: object.address.map(|address| format!("{address:#010x}")),
                object_offset: format!("{:#x}", object.object_offset),
                size: object.size,
                writable: object.writable,
                initialized: object.initialized,
                synthetic_from_anchor: object.synthetic_from_anchor,
                exported: object.exported,
                initializer_hex: object.initialized.then(|| encode_hex(&object.initializer)),
                relocations: object
                    .relocations
                    .into_iter()
                    .map(|relocation| DataObjectRelocationDocument {
                        offset: format!("{:#x}", relocation.offset),
                        elf_type: relocation.elf_type,
                        target: relocation.target,
                        addend: relocation.addend,
                    })
                    .collect(),
                xrefs: object_xrefs,
            });
        }
    }
    Ok(output)
}

#[derive(Serialize)]
pub(crate) struct LinkedIrDocument<'a> {
    schema_version: u32,
    command: &'static str,
    analysis_mode: &'static str,
    linkage_mode: &'static str,
    project_call_linkage: &'static str,
    selection_mode: &'static str,
    include_reachable: bool,
    effect_summary_mode: &'static str,
    context_projection_mode: &'static str,
    memory_object_mode: &'static str,
    instruction_effect_mode: &'static str,
    data_object_mode: &'static str,
    semantic_action_mode: &'static str,
    event_dispatch_mode: &'static str,
    event_dispatch_effect_completeness_claim: bool,
    event_dispatch_receiver_inference_mode: &'static str,
    mmio_field_candidate_mode: &'static str,
    direct_mmio_predicate_completeness_claim: bool,
    scenario_suggestion_mode: &'static str,
    scenario_suggestion_proof_claim: bool,
    mmio_field_semantics_claim: bool,
    cfg_guard_completeness_claim: bool,
    completeness_claim: bool,
    artifacts: Vec<SourceArtifact<'a>>,
    companions: Vec<ArtifactIdentity>,
    symbol_prefix: &'a str,
    entry_contract: &'a str,
    summary: ReportSummary,
    data_objects: Vec<DataObjectDocument>,
    mmio_registers: &'a [LinkedMmioRegister],
    semantic_boundaries: &'a [SemanticBoundary],
    trampoline_slots: &'a [LinkedTrampolineSlot],
    functions: &'a [LinkedIrFunction],
}

#[derive(Debug)]
pub(crate) struct LinkedIrBundle {
    pub(crate) manifest: String,
    pub(crate) functions: String,
    pub(crate) function_index: String,
    pub(crate) graph: String,
    pub(crate) register_index: String,
    pub(crate) data_objects: String,
    pub(crate) data_object_index: String,
}

#[derive(Serialize)]
struct FunctionIndexDocument {
    schema_version: u32,
    command: &'static str,
    records: Vec<FunctionIndexRecord>,
}

#[derive(Serialize)]
struct FunctionIndexRecord {
    identity: String,
    source: String,
    member: Option<String>,
    symbol: String,
    address: Option<u32>,
    offset: u64,
    length: u64,
}

#[derive(Serialize)]
struct DataObjectIndexDocument {
    schema_version: u32,
    command: &'static str,
    records: Vec<DataObjectIndexRecord>,
}

#[derive(Serialize)]
struct DataObjectIndexRecord {
    source: String,
    member: Option<String>,
    symbol: String,
    address: Option<String>,
    offset: u64,
    length: u64,
}

#[derive(Serialize)]
struct GraphDocument {
    schema_version: u32,
    command: &'static str,
    edges: Vec<GraphEdgeDocument>,
}

#[derive(Serialize)]
struct GraphEdgeDocument {
    caller: String,
    callee: String,
    site: Option<u32>,
    kind: String,
}

#[derive(Serialize)]
struct RegisterIndexDocument<'a> {
    schema_version: u32,
    command: &'static str,
    registers: &'a [LinkedMmioRegister],
}

pub(crate) fn build_linked_ir_document<'a>(
    artifacts: &'a [IrArtifactInput],
    companions: &[PathBuf],
    symbol_prefix: &'a str,
    entry_contract: EntryContractRef,
    report: &'a LinkedIrReport,
    include_reachable: bool,
) -> Result<LinkedIrDocument<'a>> {
    let data_objects = load_data_objects(artifacts, report)?;
    Ok(LinkedIrDocument {
        schema_version: crate::artifacts::LINKED_IR.version,
        command: crate::artifacts::LINKED_IR.command,
        analysis_mode: "best-effort",
        linkage_mode: if artifacts.len() > 1 {
            "independent-artifacts"
        } else {
            "primary-with-companions"
        },
        project_call_linkage: if artifacts.len() > 1 {
            "unique-exported-symbol-only"
        } else {
            "primary-resolver"
        },
        selection_mode: match (symbol_prefix.is_empty(), include_reachable) {
            (true, true) => "all-symbols-with-reachable-internal-callees",
            (true, false) => "all-symbols-only",
            (false, true) => "symbol-prefix-with-reachable-internal-callees",
            (false, false) => "symbol-prefix-only",
        },
        include_reachable,
        effect_summary_mode: "reachable-inventory-origin-preserving",
        context_projection_mode: "affine-simple-call-paths",
        memory_object_mode: "affine-argument-and-relocated-symbols",
        instruction_effect_mode: "direct-origin-sites-with-basic-blocks",
        data_object_mode: "named-elf-objects-with-uninterpreted-initializers-and-symbolic-relocations",
        semantic_action_mode: "lexical-site-paths-factorized-cfg-guards-affine-root-bindings",
        event_dispatch_mode: "reviewed-contract-declared-role-projection",
        event_dispatch_effect_completeness_claim: false,
        event_dispatch_receiver_inference_mode: "none",
        mmio_field_candidate_mode: "contiguous-subregister-write-poll-and-direct-guard-evidence",
        direct_mmio_predicate_completeness_claim: false,
        scenario_suggestion_mode: "structural-candidates-require-concrete-replay",
        scenario_suggestion_proof_claim: false,
        mmio_field_semantics_claim: false,
        cfg_guard_completeness_claim: false,
        completeness_claim: false,
        artifacts: artifacts
            .iter()
            .map(|artifact| {
                Ok(SourceArtifact {
                    source: &artifact.source,
                    artifact: ArtifactIdentity::load(&artifact.path)?,
                    reviewed_code_boundaries: artifact
                        .reviewed_code
                        .iter()
                        .map(|range| ReviewedCodeBoundaryDocument {
                            member: &range.member,
                            section: &range.section,
                            name: &range.name,
                            start_offset: format!("{:#x}", range.start_offset),
                            end_offset: format!("{:#x}", range.end_offset),
                        })
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?,
        companions: companions
            .iter()
            .map(|path| ArtifactIdentity::load(path))
            .collect::<Result<Vec<_>>>()?,
        symbol_prefix,
        entry_contract: entry_contract.id(),
        summary: ReportSummary::new(artifacts, report, &data_objects),
        data_objects,
        mmio_registers: &report.mmio_registers,
        semantic_boundaries: &report.semantic_boundaries,
        trampoline_slots: &report.trampoline_slots,
        functions: &report.functions,
    })
}

#[cfg(test)]
fn render_linked_ir(document: &LinkedIrDocument<'_>) -> Result<String> {
    Ok(serde_json::to_string(document)? + "\n")
}

pub(crate) fn render_linked_ir_bundle(document: &LinkedIrDocument<'_>) -> Result<LinkedIrBundle> {
    let manifest = LinkedIrDocument {
        schema_version: document.schema_version,
        command: document.command,
        analysis_mode: document.analysis_mode,
        linkage_mode: document.linkage_mode,
        project_call_linkage: document.project_call_linkage,
        selection_mode: document.selection_mode,
        include_reachable: document.include_reachable,
        effect_summary_mode: document.effect_summary_mode,
        context_projection_mode: document.context_projection_mode,
        memory_object_mode: document.memory_object_mode,
        instruction_effect_mode: document.instruction_effect_mode,
        data_object_mode: document.data_object_mode,
        semantic_action_mode: document.semantic_action_mode,
        event_dispatch_mode: document.event_dispatch_mode,
        event_dispatch_effect_completeness_claim: document.event_dispatch_effect_completeness_claim,
        event_dispatch_receiver_inference_mode: document.event_dispatch_receiver_inference_mode,
        mmio_field_candidate_mode: document.mmio_field_candidate_mode,
        direct_mmio_predicate_completeness_claim: document.direct_mmio_predicate_completeness_claim,
        scenario_suggestion_mode: document.scenario_suggestion_mode,
        scenario_suggestion_proof_claim: document.scenario_suggestion_proof_claim,
        mmio_field_semantics_claim: document.mmio_field_semantics_claim,
        cfg_guard_completeness_claim: document.cfg_guard_completeness_claim,
        completeness_claim: document.completeness_claim,
        artifacts: document.artifacts.clone(),
        companions: document.companions.clone(),
        symbol_prefix: document.symbol_prefix,
        entry_contract: document.entry_contract,
        summary: document.summary.clone(),
        data_objects: Vec::new(),
        mmio_registers: &[],
        semantic_boundaries: document.semantic_boundaries,
        trampoline_slots: document.trampoline_slots,
        functions: &[],
    };
    let mut functions = String::new();
    let mut records = Vec::with_capacity(document.functions.len());
    let mut edges = Vec::new();
    for function in document.functions {
        let offset = functions.len() as u64;
        let encoded = serde_json::to_string(function)?;
        let length = encoded.len() as u64;
        functions.push_str(&encoded);
        functions.push('\n');
        records.push(FunctionIndexRecord {
            identity: function.identity.clone(),
            source: function.source.clone(),
            member: function.member.clone(),
            symbol: function.symbol.clone(),
            address: function.address,
            offset,
            length,
        });
        edges.extend(function.calls.iter().map(|call| GraphEdgeDocument {
            caller: function.identity.clone(),
            callee: call.target.clone(),
            site: call.site,
            kind: call.kind.to_owned(),
        }));
    }
    records.sort_by(|left, right| {
        (&left.source, &left.identity).cmp(&(&right.source, &right.identity))
    });
    edges.sort_by(|left, right| {
        (&left.caller, left.site, &left.kind, &left.callee).cmp(&(
            &right.caller,
            right.site,
            &right.kind,
            &right.callee,
        ))
    });
    let mut data_objects = String::new();
    let mut data_object_records = Vec::with_capacity(document.data_objects.len());
    for object in &document.data_objects {
        let offset = data_objects.len() as u64;
        let encoded = serde_json::to_string(object)?;
        let length = encoded.len() as u64;
        data_objects.push_str(&encoded);
        data_objects.push('\n');
        data_object_records.push(DataObjectIndexRecord {
            source: object.source.clone(),
            member: object.member.clone(),
            symbol: object.symbol.clone(),
            address: object.address.clone(),
            offset,
            length,
        });
    }
    data_object_records.sort_by(|left, right| {
        (&left.source, &left.member, &left.symbol, &left.address).cmp(&(
            &right.source,
            &right.member,
            &right.symbol,
            &right.address,
        ))
    });
    Ok(LinkedIrBundle {
        manifest: serde_json::to_string(&manifest)? + "\n",
        functions,
        function_index: serde_json::to_string(&FunctionIndexDocument {
            schema_version: document.schema_version,
            command: "ir function index",
            records,
        })? + "\n",
        graph: serde_json::to_string(&GraphDocument {
            schema_version: document.schema_version,
            command: "ir graph index",
            edges,
        })? + "\n",
        register_index: serde_json::to_string(&RegisterIndexDocument {
            schema_version: document.schema_version,
            command: "ir register index",
            registers: document.mmio_registers,
        })? + "\n",
        data_objects,
        data_object_index: serde_json::to_string(&DataObjectIndexDocument {
            schema_version: document.schema_version,
            command: "ir data object index",
            records: data_object_records,
        })? + "\n",
    })
}

pub(crate) fn write_linked_ir_bundle(path: &Path, bundle: &LinkedIrBundle) -> Result<()> {
    std::fs::create_dir_all(path)?;
    for (name, contents) in [
        ("manifest.json", &bundle.manifest),
        ("functions.jsonl", &bundle.functions),
        ("function-index.json", &bundle.function_index),
        ("graph.json", &bundle.graph),
        ("register-index.json", &bundle.register_index),
        ("data-objects.jsonl", &bundle.data_objects),
        ("data-object-index.json", &bundle.data_object_index),
    ] {
        std::fs::write(path.join(name), contents)?;
    }
    let mut legacy = path.as_os_str().to_os_string();
    legacy.push(".json");
    let legacy = std::path::PathBuf::from(legacy);
    if legacy.is_file() {
        std::fs::remove_file(legacy)?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn render_linked_ir_fixture(
    functions: Vec<LinkedIrFunction>,
    mmio_registers: Vec<LinkedMmioRegister>,
) -> String {
    use crate::EntryContractSpec;

    static ENTRY: EntryContractSpec = EntryContractSpec {
        id: "none",
        function_table: None,
        pointer_symbols: &[],
        data_pointer_binding: None,
    };
    let report = LinkedIrReport {
        functions,
        mmio_registers,
        mmio_functions: 0,
        mmio_access_shapes: 0,
        delay_functions: 0,
        delay_shapes: 0,
        semantic_boundaries: Vec::new(),
        semantic_calls: 0,
        trampoline_slots: Vec::new(),
        trampoline_calls: 0,
        exported_functions: 0,
        local_functions: 0,
        context_functions: 0,
        context_accesses: 0,
        context_fields: 0,
        memory_functions: 0,
        memory_accesses: 0,
        memory_fields: 0,
        complete_functions: 0,
        structured_functions: 0,
        internal_calls: 0,
        external_calls: 0,
        call_argument_shapes: 0,
        project_linked_calls: 0,
        ambiguous_project_calls: 0,
        unresolved_calls: 0,
        closed_effect_summaries: 0,
        recursive_effect_summaries: 0,
        complete_context_projections: 0,
        projected_context_fields: 0,
        projected_memory_fields: 0,
        scenario_suggestions: 0,
    };
    let document = build_linked_ir_document(
        &[],
        &[],
        "",
        crate::EntryContractRef::new(&ENTRY),
        &report,
        false,
    )
    .unwrap();
    render_linked_ir(&document).unwrap()
}

#[cfg(test)]
mod bundle_write_tests {
    use super::*;

    #[test]
    fn bundle_write_removes_the_obsolete_monolithic_sidecar() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-linked-ir-write-{}",
            std::process::id()
        ));
        let mut legacy = path.as_os_str().to_os_string();
        legacy.push(".json");
        let legacy = std::path::PathBuf::from(legacy);
        std::fs::write(&legacy, "stale monolithic IR").unwrap();
        let bundle = LinkedIrBundle {
            manifest: "{}\n".to_owned(),
            functions: String::new(),
            function_index: "{}\n".to_owned(),
            graph: "{}\n".to_owned(),
            register_index: "{}\n".to_owned(),
            data_objects: String::new(),
            data_object_index: "{}\n".to_owned(),
        };

        write_linked_ir_bundle(&path, &bundle).unwrap();

        assert!(path.join("manifest.json").is_file());
        assert!(!legacy.exists());
        std::fs::remove_dir_all(path).unwrap();
    }
}
