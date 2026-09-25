//! One saved-fact build owner. Native navigation supplies selection and physical links.
use crate::*;
use serde::{Deserialize, Serialize};
use std::io::Write;
mod profiles;
mod provenance;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrWork {
    pub schema: u32,
    pub run: RunId,
    pub project: OriginPath,
    pub request: IrBuildRequest,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}
struct Node<'m> {
    function: NavigationFunction,
    manifest: FunctionManifest,
    name: Option<Vec<u8>>,
    roots: u32,
    profiles: u32,
    _capacity: MemoryReservation<'m>,
}
struct Call<'m> {
    record: NavigationRecord,
    _capacity: MemoryReservation<'m>,
}
struct Output {
    file: blobray_store::TemporaryFile,
    count: u64,
}
impl Output {
    fn emit(&mut self, record: &SemanticIrRecord, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(1)?;
        write_control_message(&mut self.file, record)?;
        self.file.write_all(b"\n").map_err(storage_io)?;
        self.count += 1;
        Ok(())
    }
}
fn invalid(s: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, s)
}
fn bits(mask: u32) -> Vec<u8> {
    (0u8..32).filter(|i| mask & (1 << i) != 0).collect()
}
fn position(nodes: &[Node<'_>], id: &FunctionAnalysisId, c: &mut dyn RunControl) -> Result<usize> {
    c.checkpoint(nodes.len().max(1).ilog2() as u64 + 1)?;
    nodes
        .binary_search_by(|n| n.function.analysis.cmp(id))
        .map_err(|_| invalid("IR function is outside the explicit scope"))
}
pub fn prepare_ir_worker(
    stage: &Path,
    work: &IrWork,
    c: &mut dyn RunControl,
) -> Result<PreparedIrReceipt> {
    if work.schema != 1 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported IR worker protocol",
        ));
    }
    work.request.validate()?;
    let memory = WorkingMemory::new(
        work.budget
            .working_memory_bytes
            .ok_or_else(|| invalid("working capacity missing"))?,
    )?;
    let disk = blobray_store::TemporaryBudget::open(stage)?;
    let mut control = blobray_store::TemporaryControl::new(c, &disk);
    let result = build(stage, work, &memory, &disk, &mut control);
    control.memory_phases(&memory.phase_observations());
    control.working_memory(memory.observation());
    result
}
fn build(
    stage: &Path,
    work: &IrWork,
    memory: &WorkingMemory,
    disk: &blobray_store::TemporaryBudget,
    c: &mut dyn RunControl,
) -> Result<PreparedIrReceipt> {
    let _construction = memory.reserve(4 * 1024 * 1024, c.position())?;
    let project = Project::open(&work.project.to_path()?)?;
    let staging = Staging::with_temporary_budget(stage, disk.clone())?;
    let mut output = Output {
        file: disk.temporary(&stage.join("staging"))?,
        count: 0,
    };
    let mut nodes = AdmittedVec::new(memory);
    let mut calls = AdmittedVec::new(memory);
    let navigation = NavigationQuery {
        scope: work.request.scope.clone(),
        filter: NavigationFilter::Calls {
            function: None,
            direction: CallDirection::Callees,
        },
    };
    let selection = crate::navigation::query_observed(
        &project,
        &navigation,
        memory,
        c,
        &mut |function, manifest, name, c| {
            let capacity = memory.reserve(
                function.allocated_bytes()
                    + manifest.recipe.allocated_bytes()
                    + name.map_or(0, |n| n.len() as u64)
                    + 256,
                c.position(),
            )?;
            nodes.push(
                Node {
                    function: function.clone(),
                    manifest: manifest.clone(),
                    name: name.map(<[u8]>::to_vec),
                    roots: 0,
                    profiles: 0,
                    _capacity: capacity,
                },
                c.position(),
            )
        },
        &mut |record, c| {
            match record {
                NavigationRecord::Call { .. } => {
                    let capacity =
                        memory.reserve(crate::navigation::call_bytes(record), c.position())?;
                    calls.push(
                        Call {
                            record: record.clone(),
                            _capacity: capacity,
                        },
                        c.position(),
                    )?;
                }
                NavigationRecord::Unavailable { .. } => output.emit(
                    &SemanticIrRecord::Unavailable {
                        record: Box::new(record.clone()),
                    },
                    c,
                )?,
                _ => (),
            }
            Ok(())
        },
    )?;
    c.phase(RunPhase::ComposeResearch)?;
    let summaries = profiles::select(&work.request.profiles, &mut nodes, &calls, memory, c)?;
    let mut proof = provenance::Closure::new(memory);
    let mut selected = 0;
    let mut reader = project.analysis_reader(memory);
    for node in nodes.iter().filter(|n| n.profiles != 0) {
        c.checkpoint(1)?;
        selected += 1;
        proof.selected(&node.function.analysis, c)?;
        output.emit(
            &SemanticIrRecord::Function {
                function: node.function.clone(),
                manifest: Box::new(node.manifest.clone()),
                name: node.name.clone(),
                profiles: bits(node.profiles),
                roots: bits(node.roots),
                provenance_only: false,
            },
            c,
        )?;
        let lease = reader.analysis(&node.function.analysis, c)?;
        proof.observe(&lease.records, c)?;
    }
    for call in &*calls {
        if let NavigationRecord::Call { caller, .. } = &call.record
            && nodes[position(&nodes, &caller.analysis, c)?].profiles != 0
        {
            output.emit(
                &SemanticIrRecord::Call {
                    record: Box::new(call.record.clone()),
                },
                c,
            )?;
        }
    }
    drop(calls);
    drop(nodes);
    let knowledge = project.knowledge_snapshot(work.request.scope.knowledge.as_ref(), memory, c)?;
    for entry in knowledge
        .entries()
        .filter(|e| e.proposal.occurrence.revision == work.request.scope.revision)
    {
        c.checkpoint(1)?;
        for evidence in &entry.proposal.evidence {
            if let EvidenceRef::Analysis { analysis, .. } = evidence {
                proof.require(analysis, c)?;
            }
        }
        output.emit(
            &SemanticIrRecord::Knowledge {
                entry: Box::new(entry.clone()),
            },
            c,
        )?;
    }
    drop(knowledge);
    let provenance = proof.finish(&mut reader, &work.request.scope.revision, &mut output, c)?;
    let record_count = output.count;
    let records = staging.retain_temporary(output.file, c)?;
    staging.ir_receipt(
        &SemanticIrManifest {
            schema: SEMANTIC_IR_SCHEMA,
            policy: SEMANTIC_IR_POLICY,
            project: project.id().clone(),
            request: work.request.clone(),
            records,
            record_count,
            functions: selected,
            provenance_functions: provenance,
            unavailable_entries: selection.unavailable_entries,
            profiles: summaries,
        },
        c,
    )
}

/// Expand immutable original streams without acquiring source paths or running analysis.
pub(crate) fn read(
    project: &Project,
    id: &ArtifactId,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&SemanticIrRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<SemanticIrManifest> {
    let _envelope = memory.reserve(2 * 1024 * 1024, c.position())?;
    let ir = project.semantic_ir(id, memory, c)?;
    let mut reader = project.analysis_reader(memory);
    blobray_store::visit_jsonl(&ir.records, c, |record: SemanticIrRecord, c| {
        emit(&record, c)?;
        if let SemanticIrRecord::Function { function, .. } = &record {
            let lease = reader.analysis(&function.analysis, c)?;
            let mut ordinal = 0;
            blobray_store::visit_jsonl(&lease.records, c, |fact: FunctionRecord, c| {
                emit(
                    &SemanticIrRecord::Fact {
                        analysis: function.analysis.clone(),
                        record: ordinal,
                        fact: Box::new(fact),
                    },
                    c,
                )?;
                ordinal += 1;
                Ok(())
            })?;
        }
        Ok(())
    })?;
    Ok(ir.manifest)
}
