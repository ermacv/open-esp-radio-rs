//! Typed JSON linked-IR report rendering.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
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
struct SourceInputArtifact<'a> {
    source: &'a str,
    artifact: ArtifactIdentity,
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
    body_complete: usize,
    call_targets_complete: usize,
    transitive_effects_complete: usize,
    executable_complete: usize,
    structured: usize,
    loop_functions: usize,
    loop_regions: usize,
    counted_loop_candidates: usize,
    irreducible_loop_regions: usize,
    internal_calls: usize,
    indexed_dispatch_calls: usize,
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
            body_complete: report.body_complete_functions,
            call_targets_complete: report.call_targets_complete_functions,
            transitive_effects_complete: report.transitive_effects_complete_functions,
            executable_complete: report.executable_complete_functions,
            structured: report.structured_functions,
            loop_functions: report.loop_functions,
            loop_regions: report.loop_regions,
            counted_loop_candidates: report.counted_loop_candidates,
            irreducible_loop_regions: report.irreducible_loop_regions,
            internal_calls: report.internal_calls,
            indexed_dispatch_calls: report.indexed_dispatch_calls,
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
    indexed_dispatch_mode: &'static str,
    indexed_dispatch_completeness_claim: bool,
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
    inventories: Vec<SourceInputArtifact<'a>>,
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

const MAX_LINKED_IR_BUNDLE_BYTES: u64 = 512 * 1024 * 1024;
static NEXT_STAGE_ID: AtomicU64 = AtomicU64::new(0);

/// A complete linked-IR bundle rendered to a private sibling directory.
///
/// Keeping serialized artifacts on disk avoids retaining a second copy of a
/// potentially very large `LinkedIrReport`. Dropping an unpublished stage
/// removes it; publishing swaps it into the configured location only after
/// every required member has been written successfully.
#[derive(Debug)]
pub(crate) struct StagedLinkedIrBundle {
    root: Option<PathBuf>,
    bytes: u64,
}

impl StagedLinkedIrBundle {
    pub(crate) const fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn compare(&self, expected: &Path) -> Result<Vec<PathBuf>> {
        let root = self.root.as_deref().expect("unpublished bundle stage");
        let mut stale = Vec::new();
        for name in super::linked_ir_bundle::BUNDLE_FILES {
            let actual = expected.join(name);
            if !files_equal(&root.join(name), &actual)? {
                stale.push(actual);
            }
        }
        Ok(stale)
    }

    pub(crate) fn publish(mut self, destination: &Path) -> Result<()> {
        let stage = self.root.take().expect("unpublished bundle stage");
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let backup = backup_path(destination);
        if backup.exists() {
            if destination.exists() {
                fs::remove_dir_all(&backup)?;
            } else {
                fs::rename(&backup, destination)?;
            }
        }
        if destination.exists() {
            fs::rename(destination, &backup)?;
        }
        if let Err(error) = fs::rename(&stage, destination) {
            if backup.exists() {
                let _ = fs::rename(&backup, destination);
            }
            return Err(error.into());
        }
        if backup.exists() {
            fs::remove_dir_all(backup)?;
        }
        Ok(())
    }
}

impl Drop for StagedLinkedIrBundle {
    fn drop(&mut self) {
        if let Some(root) = self.root.take() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

fn sibling_path(destination: &Path, kind: &str) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("linked-ir");
    let id = NEXT_STAGE_ID.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{name}.{kind}-{}-{id}", std::process::id()))
}

fn backup_path(destination: &Path) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("linked-ir");
    parent.join(format!(".{name}.backup"))
}

fn files_equal(left: &Path, right: &Path) -> Result<bool> {
    let Ok(right_file) = File::open(right) else {
        return Ok(false);
    };
    let left_file = File::open(left)?;
    if left_file.metadata()?.len() != right_file.metadata()?.len() {
        return Ok(false);
    }
    let mut left = BufReader::new(left_file);
    let mut right = BufReader::new(right_file);
    let mut left_buffer = [0_u8; 64 * 1024];
    let mut right_buffer = [0_u8; 64 * 1024];
    loop {
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

struct BudgetWriter<'a> {
    inner: BufWriter<File>,
    total: &'a mut u64,
    limit: u64,
}

struct RecordWriter<'a, 'b> {
    inner: &'a mut BudgetWriter<'b>,
    written: u64,
}

impl Write for RecordWriter<'_, '_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl Write for BudgetWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let next = self.total.saturating_add(buffer.len() as u64);
        if next > self.limit {
            return Err(std::io::Error::other(format!(
                "linked-IR bundle exceeds the {} MiB safety limit",
                self.limit / (1024 * 1024)
            )));
        }
        let written = self.inner.write(buffer)?;
        *self.total += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn budget_writer<'a>(root: &Path, name: &str, total: &'a mut u64) -> Result<BudgetWriter<'a>> {
    Ok(BudgetWriter {
        inner: BufWriter::new(File::create(root.join(name))?),
        total,
        limit: MAX_LINKED_IR_BUNDLE_BYTES,
    })
}

fn manifest_projection<'a>(document: &'a LinkedIrDocument<'a>) -> LinkedIrDocument<'a> {
    LinkedIrDocument {
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
        indexed_dispatch_mode: document.indexed_dispatch_mode,
        indexed_dispatch_completeness_claim: document.indexed_dispatch_completeness_claim,
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
        inventories: document.inventories.clone(),
        companions: document.companions.clone(),
        symbol_prefix: document.symbol_prefix,
        entry_contract: document.entry_contract,
        summary: document.summary.clone(),
        data_objects: Vec::new(),
        mmio_registers: &[],
        semantic_boundaries: document.semantic_boundaries,
        trampoline_slots: document.trampoline_slots,
        functions: &[],
    }
}

/// Compact, persistent review projection.  The full function stream is the
/// lossless analysis artifact and may contain megabytes for a single
/// function; project status and the TUI must not deserialize it merely to
/// build an index.
#[derive(Serialize)]
struct FunctionOverviewDocument<'a> {
    source: &'a str,
    identity: &'a str,
    selection: &'a str,
    member: &'a Option<String>,
    symbol: &'a str,
    binding: &'a str,
    loops: &'a [crate::artifact::FunctionLoop],
    completeness: &'a crate::LinkedFunctionCompleteness,
    dependencies: &'a [String],
    direct_calls: usize,
    calls: Vec<FunctionOverviewCall<'a>>,
    mmio: Vec<FunctionOverviewMmio>,
    mmio_addresses: Vec<u32>,
    direct_context_fields: usize,
    direct_memory_fields: usize,
    direct_effects: Vec<FunctionOverviewDirectEffect>,
    diagnostics: Vec<FunctionOverviewDiagnostic<'a>>,
    effect_summary: FunctionOverviewEffectSummary<'a>,
    decode_blockers: &'a [crate::LinkedDecodeBlocker],
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct FunctionOverviewDirectEffect {
    kind: &'static str,
    site: Option<u32>,
    operation: String,
    target: String,
    width: Option<u8>,
    value: Option<String>,
    modified_mask: Option<u32>,
    preserved_mask: Option<u32>,
    forced_zero_mask: Option<u32>,
    forced_one_mask: Option<u32>,
    arguments: Vec<String>,
}

fn deduplicate_observable_effects(effects: &mut Vec<FunctionOverviewDirectEffect>) {
    let mut observed = BTreeSet::new();
    effects.retain(|effect| observed.insert(effect.clone()));
}

#[derive(Serialize)]
struct FunctionOverviewCall<'a> {
    kind: &'a str,
    target: &'a str,
    site: Option<u32>,
    direct: bool,
    project_symbol: &'a Option<String>,
    semantic_operation: &'a Option<String>,
}

#[derive(Serialize)]
struct FunctionOverviewMmio {
    address: u32,
    width: u8,
}

#[derive(Serialize)]
struct FunctionOverviewDiagnostic<'a> {
    channel: &'static str,
    root_id: &'a str,
    kind: &'a str,
    site: Option<u32>,
    rendered: &'a str,
}

#[derive(Serialize)]
struct FunctionOverviewEffectSummary<'a> {
    transitive_effects_materialized: bool,
    call_graph_closed: bool,
    context_projection_materialized: bool,
    context_projection_complete: bool,
    context_projection_blockers: &'a [String],
    context_fields: Vec<FunctionOverviewContextField>,
    memory_fields: Vec<FunctionOverviewMemoryField<'a>>,
    semantic_operations: Vec<&'a str>,
    trampoline_calls: usize,
    event_dispatches: Vec<FunctionOverviewEventDispatch<'a>>,
}

#[derive(Serialize)]
struct FunctionOverviewContextField {
    argument: u8,
    offset: i32,
    width: u8,
    reads: usize,
    writes: usize,
    write_mask: u32,
}

#[derive(Serialize)]
struct FunctionOverviewMemoryField<'a> {
    object: &'a crate::LinkedMemoryObject,
    offset: i64,
    width: u8,
    reads: usize,
    writes: usize,
    write_mask: u32,
    origins: &'a [String],
}

#[derive(Serialize)]
struct FunctionOverviewEventDispatch<'a> {
    mechanism: &'a str,
    execution_context: &'a str,
    receiver: &'a Option<String>,
    interface_complete: bool,
    bindings: Vec<FunctionOverviewEventBinding<'a>>,
}

#[derive(Serialize)]
struct FunctionOverviewEventBinding<'a> {
    role: &'a str,
    value: &'a str,
}

impl<'a> FunctionOverviewDocument<'a> {
    fn new(function: &'a LinkedIrFunction) -> Self {
        let summary = &function.effect_summary;
        let mut direct_effects = function
            .instruction_effects
            .iter()
            .filter_map(|effect| match effect {
                crate::LinkedInstructionEffect::Mmio {
                    site,
                    access,
                    width,
                    address,
                    mode,
                    value,
                    modified_mask,
                    preserved_mask,
                    forced_zero_mask,
                    forced_one_mask,
                    ..
                } => Some(FunctionOverviewDirectEffect {
                    kind: "mmio",
                    site: Some(*site),
                    operation: format!("{access}:{mode}"),
                    target: format!("{address:#010x}"),
                    width: Some(*width),
                    value: value.clone(),
                    modified_mask: *modified_mask,
                    preserved_mask: *preserved_mask,
                    forced_zero_mask: *forced_zero_mask,
                    forced_one_mask: *forced_one_mask,
                    arguments: Vec::new(),
                }),
                crate::LinkedInstructionEffect::Memory {
                    site,
                    access,
                    width,
                    object,
                    offset,
                    value,
                    value_pseudo,
                    write_mask,
                    preserved_mask,
                    forced_zero_mask,
                    forced_one_mask,
                    ..
                } if access.contains("write") => Some(FunctionOverviewDirectEffect {
                    kind: "memory",
                    site: Some(*site),
                    operation: (*access).to_owned(),
                    target: format!("{} {offset:+#x}", object.display_name()),
                    width: Some(*width),
                    value: value_pseudo.clone().or_else(|| value.clone()),
                    modified_mask: *write_mask,
                    preserved_mask: *preserved_mask,
                    forced_zero_mask: *forced_zero_mask,
                    forced_one_mask: *forced_one_mask,
                    arguments: Vec::new(),
                }),
                crate::LinkedInstructionEffect::Memory { .. } => None,
            })
            .collect::<Vec<_>>();
        direct_effects.extend(function.calls.iter().filter_map(|call| {
            call.semantic_operation
                .as_ref()
                .map(|operation| FunctionOverviewDirectEffect {
                    kind: "semantic-call",
                    site: call.site,
                    operation: operation.clone(),
                    target: call.target.clone(),
                    width: None,
                    value: None,
                    modified_mask: None,
                    preserved_mask: None,
                    forced_zero_mask: None,
                    forced_one_mask: None,
                    arguments: call.arguments.clone(),
                })
        }));
        direct_effects.sort_by(|left, right| {
            (left.site, left.kind, &left.target, &left.operation).cmp(&(
                right.site,
                right.kind,
                &right.target,
                &right.operation,
            ))
        });
        direct_effects.extend(
            function
                .delays
                .iter()
                .map(|delay| FunctionOverviewDirectEffect {
                    kind: "delay",
                    site: None,
                    operation: "delay-micros".to_owned(),
                    target: delay.path.clone(),
                    width: None,
                    value: Some(delay.micros.clone()),
                    modified_mask: None,
                    preserved_mask: None,
                    forced_zero_mask: None,
                    forced_one_mask: None,
                    arguments: Vec::new(),
                }),
        );
        direct_effects.extend(summary.event_dispatches.iter().map(|dispatch| {
            FunctionOverviewDirectEffect {
                kind: "event-dispatch",
                site: None,
                operation: dispatch.mechanism.to_owned(),
                target: dispatch
                    .receiver
                    .clone()
                    .unwrap_or_else(|| "<unknown>".to_owned()),
                width: None,
                value: Some(dispatch.execution_context.to_owned()),
                modified_mask: None,
                preserved_mask: None,
                forced_zero_mask: None,
                forced_one_mask: None,
                arguments: dispatch
                    .bindings
                    .iter()
                    .map(|binding| format!("{}={}", binding.role, binding.argument.value))
                    .collect(),
            }
        }));
        deduplicate_observable_effects(&mut direct_effects);
        Self {
            source: &function.source,
            identity: &function.identity,
            selection: function.selection,
            member: &function.member,
            symbol: &function.symbol,
            binding: function.binding,
            loops: &function.loops,
            completeness: &function.completeness,
            dependencies: &function.dependencies,
            direct_calls: function.calls.len(),
            calls: function
                .calls
                .iter()
                .map(|call| FunctionOverviewCall {
                    kind: call.kind,
                    target: &call.target,
                    site: call.site,
                    direct: call.direct,
                    project_symbol: &call.project_symbol,
                    semantic_operation: &call.semantic_operation,
                })
                .collect(),
            mmio: function
                .mmio_accesses
                .iter()
                .map(|access| FunctionOverviewMmio {
                    address: access.address,
                    width: access.width,
                })
                .collect(),
            mmio_addresses: function
                .mmio_accesses
                .iter()
                .map(|access| access.address)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            direct_context_fields: function.context_fields.len(),
            direct_memory_fields: function.memory_fields.len(),
            direct_effects,
            diagnostics: function
                .call_graph_diagnostics
                .iter()
                .map(|diagnostic| ("call-graph", diagnostic))
                .chain(
                    function
                        .direct_diagnostics
                        .iter()
                        .map(|diagnostic| ("direct", diagnostic)),
                )
                .chain(
                    function
                        .reference_diagnostics
                        .iter()
                        .map(|diagnostic| ("reference", diagnostic)),
                )
                .map(|(channel, diagnostic)| FunctionOverviewDiagnostic {
                    channel,
                    root_id: &diagnostic.root_id,
                    kind: diagnostic.kind,
                    site: diagnostic.site,
                    rendered: &diagnostic.rendered,
                })
                .collect(),
            effect_summary: FunctionOverviewEffectSummary {
                transitive_effects_materialized: summary.transitive_effects_materialized,
                call_graph_closed: summary.call_graph_closed,
                context_projection_materialized: summary.context_projection_materialized,
                context_projection_complete: summary.context_projection_complete,
                context_projection_blockers: &summary.context_projection_blockers,
                context_fields: summary
                    .context_fields
                    .iter()
                    .map(|field| FunctionOverviewContextField {
                        argument: field.argument,
                        offset: field.offset,
                        width: field.width,
                        reads: field.reads,
                        writes: field.writes,
                        write_mask: field.write_mask,
                    })
                    .collect(),
                memory_fields: summary
                    .memory_fields
                    .iter()
                    .map(|field| FunctionOverviewMemoryField {
                        object: &field.object,
                        offset: field.offset,
                        width: field.width,
                        reads: field.reads,
                        writes: field.writes,
                        write_mask: field.write_mask,
                        origins: &field.origins,
                    })
                    .collect(),
                semantic_operations: summary
                    .semantic_operations
                    .iter()
                    .map(|operation| operation.operation.as_str())
                    .collect(),
                trampoline_calls: summary.trampoline_calls.len(),
                event_dispatches: summary
                    .event_dispatches
                    .iter()
                    .map(|dispatch| FunctionOverviewEventDispatch {
                        mechanism: dispatch.mechanism,
                        execution_context: dispatch.execution_context,
                        receiver: &dispatch.receiver,
                        interface_complete: dispatch.interface_complete,
                        bindings: dispatch
                            .bindings
                            .iter()
                            .map(|binding| FunctionOverviewEventBinding {
                                role: binding.role,
                                value: &binding.argument.value,
                            })
                            .collect(),
                    })
                    .collect(),
            },
            decode_blockers: &function.decode_blockers,
        }
    }
}

#[derive(Serialize)]
struct FunctionIndexDocument<'a> {
    schema_version: u32,
    command: &'static str,
    records: Vec<FunctionIndexRecord<'a>>,
}

#[derive(Serialize)]
struct FunctionIndexRecord<'a> {
    identity: &'a str,
    source: &'a str,
    member: &'a Option<String>,
    symbol: &'a str,
    address: Option<u32>,
    offset: u64,
    length: u64,
}

#[derive(Serialize)]
struct DataObjectIndexDocument<'a> {
    schema_version: u32,
    command: &'static str,
    records: Vec<DataObjectIndexRecord<'a>>,
}

#[derive(Serialize)]
struct DataObjectIndexRecord<'a> {
    source: &'a str,
    member: &'a Option<String>,
    symbol: &'a str,
    address: &'a Option<String>,
    offset: u64,
    length: u64,
}

#[derive(Serialize)]
struct GraphDocument<'a> {
    schema_version: u32,
    command: &'static str,
    edges: Vec<GraphEdgeDocument<'a>>,
}

#[derive(Serialize)]
struct GraphEdgeDocument<'a> {
    caller: &'a str,
    callee: &'a str,
    site: Option<u32>,
    kind: &'a str,
}

#[derive(Serialize)]
struct RegisterIndexDocument<'a> {
    schema_version: u32,
    command: &'static str,
    registers: &'a [LinkedMmioRegister],
}

pub(crate) fn build_linked_ir_document<'a>(
    artifacts: &'a [IrArtifactInput],
    inventories: &'a [(String, PathBuf)],
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
        effect_summary_mode: if report
            .functions
            .iter()
            .all(|function| function.effect_summary.semantic_actions_materialized)
        {
            "reachable-inventory-origin-preserving"
        } else {
            "direct-facts-with-focused-transitive-projection"
        },
        context_projection_mode: "affine-simple-call-paths",
        memory_object_mode: "affine-argument-and-relocated-symbols",
        instruction_effect_mode: "direct-origin-sites-with-basic-blocks",
        data_object_mode: "symbol-bounded-elf-objects-with-uninterpreted-initializers-and-symbolic-relocations",
        indexed_dispatch_mode: "bounded-riscv32-relocation-tables-with-case-handler-edges",
        indexed_dispatch_completeness_claim: false,
        semantic_action_mode: if report
            .functions
            .iter()
            .all(|function| function.effect_summary.semantic_actions_materialized)
        {
            "lexical-site-paths-factorized-cfg-guards-affine-root-bindings"
        } else {
            "direct-calls-plus-call-graph-with-focused-root-projection"
        },
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
        inventories: inventories
            .iter()
            .map(|(source, path)| {
                Ok(SourceInputArtifact {
                    source,
                    artifact: ArtifactIdentity::load(path)?,
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

pub(crate) fn stage_linked_ir_bundle(
    destination: &Path,
    document: &LinkedIrDocument<'_>,
) -> Result<StagedLinkedIrBundle> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let root = sibling_path(destination, "stage");
    fs::create_dir(&root)?;
    let mut stage = StagedLinkedIrBundle {
        root: Some(root.clone()),
        bytes: 0,
    };
    let mut total = 0_u64;

    {
        let mut writer = budget_writer(&root, "manifest.json", &mut total)?;
        serde_json::to_writer(&mut writer, &manifest_projection(document))?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }

    let mut records = Vec::with_capacity(document.functions.len());
    let mut edges = Vec::new();
    {
        let mut functions = budget_writer(&root, "functions.jsonl", &mut total)?;
        let mut offset = 0_u64;
        for function in document.functions {
            let mut record = RecordWriter {
                inner: &mut functions,
                written: 0,
            };
            serde_json::to_writer(&mut record, function)?;
            let length = record.written;
            record.write_all(b"\n")?;
            records.push(FunctionIndexRecord {
                identity: &function.identity,
                source: &function.source,
                member: &function.member,
                symbol: &function.symbol,
                address: function.address,
                offset,
                length,
            });
            offset += length + 1;
            edges.extend(function.calls.iter().map(|call| GraphEdgeDocument {
                caller: &function.identity,
                callee: &call.target,
                site: call.site,
                kind: call.kind,
            }));
        }
        functions.flush()?;
    }

    {
        let mut overviews = budget_writer(&root, "function-overview.jsonl", &mut total)?;
        for function in document.functions {
            serde_json::to_writer(&mut overviews, &FunctionOverviewDocument::new(function))?;
            overviews.write_all(b"\n")?;
        }
        overviews.flush()?;
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

    let mut data_object_records = Vec::with_capacity(document.data_objects.len());
    {
        let mut objects = budget_writer(&root, "data-objects.jsonl", &mut total)?;
        let mut offset = 0_u64;
        for object in &document.data_objects {
            let mut record = RecordWriter {
                inner: &mut objects,
                written: 0,
            };
            serde_json::to_writer(&mut record, object)?;
            let length = record.written;
            record.write_all(b"\n")?;
            data_object_records.push(DataObjectIndexRecord {
                source: &object.source,
                member: &object.member,
                symbol: &object.symbol,
                address: &object.address,
                offset,
                length,
            });
            offset += length + 1;
        }
        objects.flush()?;
    }
    data_object_records.sort_by(|left, right| {
        (&left.source, &left.member, &left.symbol, &left.address).cmp(&(
            &right.source,
            &right.member,
            &right.symbol,
            &right.address,
        ))
    });

    macro_rules! write_json_file {
        ($name:literal, $value:expr) => {{
            let mut writer = budget_writer(&root, $name, &mut total)?;
            serde_json::to_writer(&mut writer, &$value)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }};
    }
    write_json_file!(
        "function-index.json",
        FunctionIndexDocument {
            schema_version: document.schema_version,
            command: "ir function index",
            records,
        }
    );
    write_json_file!(
        "graph.json",
        GraphDocument {
            schema_version: document.schema_version,
            command: "ir graph index",
            edges,
        }
    );
    write_json_file!(
        "register-index.json",
        RegisterIndexDocument {
            schema_version: document.schema_version,
            command: "ir register index",
            registers: document.mmio_registers,
        }
    );
    write_json_file!(
        "data-object-index.json",
        DataObjectIndexDocument {
            schema_version: document.schema_version,
            command: "ir data object index",
            records: data_object_records,
        }
    );

    stage.bytes = total;
    Ok(stage)
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
        body_complete_functions: 0,
        call_targets_complete_functions: 0,
        transitive_effects_complete_functions: 0,
        executable_complete_functions: 0,
        structured_functions: 0,
        loop_functions: 0,
        loop_regions: 0,
        counted_loop_candidates: 0,
        irreducible_loop_regions: 0,
        internal_calls: 0,
        indexed_dispatch_calls: 0,
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
    fn staged_bundle_replaces_the_complete_directory() {
        let path =
            std::env::temp_dir().join(format!("blobray-linked-ir-write-{}", std::process::id()));
        let stage_path = sibling_path(&path, "test-stage");
        std::fs::create_dir_all(&stage_path).unwrap();
        for name in super::super::linked_ir_bundle::BUNDLE_FILES {
            std::fs::write(stage_path.join(name), "{}\n").unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("obsolete"), "old bundle").unwrap();
        let bundle = StagedLinkedIrBundle {
            root: Some(stage_path),
            bytes: 24,
        };

        bundle.publish(&path).unwrap();

        assert!(path.join("manifest.json").is_file());
        assert!(!path.join("obsolete").exists());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn budget_writer_fails_before_exceeding_its_limit() {
        let path =
            std::env::temp_dir().join(format!("blobray-linked-ir-budget-{}", std::process::id()));
        let mut total = 0;
        let mut writer = BudgetWriter {
            inner: BufWriter::new(File::create(&path).unwrap()),
            total: &mut total,
            limit: 4,
        };
        writer.write_all(b"1234").unwrap();
        let error = writer.write_all(b"5").unwrap_err();
        assert!(error.to_string().contains("safety limit"));
        drop(writer);
        assert_eq!(total, 4);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn overview_deduplicates_path_variants_with_identical_observable_effects() {
        let effect = FunctionOverviewDirectEffect {
            kind: "mmio",
            site: Some(0x1000),
            operation: "write:static".to_owned(),
            target: "0x60000010".to_owned(),
            width: Some(32),
            value: Some("0x00000001".to_owned()),
            modified_mask: Some(1),
            preserved_mask: Some(!1),
            forced_zero_mask: Some(0),
            forced_one_mask: Some(1),
            arguments: Vec::new(),
        };
        let mut effects = vec![effect.clone(), effect.clone()];
        deduplicate_observable_effects(&mut effects);

        assert_eq!(effects, [effect]);
    }
}
