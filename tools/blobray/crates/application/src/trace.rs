//! Trace query owner: load one selected profile, release its facts before the other side.
use crate::*;
use blobray_analysis::trace::{
    Canonical, Collected, Emitter, TraceFunction, TraceInput, TraceLink,
};
struct Function<'m> {
    id: FunctionAnalysisId,
    manifest: FunctionManifest,
    records: RecordBuffer<'m>,
    _capacity: MemoryReservation<'m>,
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
struct Selection<'a> {
    target: &'a TraceTarget,
    observation: &'a TraceObservation,
    side: TraceSide,
}
fn side<'m>(
    project: &Project,
    selection: Selection<'_>,
    memory: &'m WorkingMemory,
    canonical: &mut Canonical<'m>,
    c: &mut dyn RunControl,
    emit: &mut Emitter<'_>,
) -> Result<Collected<'m>> {
    let Selection {
        target,
        observation,
        side,
    } = selection;
    let _envelope = memory.reserve(2 * 1024 * 1024, c.position())?;
    let ir = project.semantic_ir(&target.ir, memory, c)?;
    let profile = ir
        .manifest
        .profiles
        .iter()
        .position(|p| p.name == target.profile)
        .ok_or_else(|| invalid("trace profile is absent from the saved IR"))?;
    let mut functions = AdmittedVec::new(memory);
    let mut reader = project.analysis_reader(memory);
    blobray_store::visit_jsonl(&ir.records, c, |row: SemanticIrRecord, c| {
        if let SemanticIrRecord::Function {
            function,
            manifest,
            profiles,
            ..
        } = row
            && profiles.contains(&(profile as u8))
        {
            let capacity = memory.reserve(
                function.analysis.allocated_bytes() + manifest.recipe.allocated_bytes() + 256,
                c.position(),
            )?;
            let lease = reader.analysis(&function.analysis, c)?;
            let records = crate::research::load_records(&lease.records, memory, c)?;
            functions.push(
                Function {
                    id: function.analysis,
                    manifest: *manifest,
                    records,
                    _capacity: capacity,
                },
                c.position(),
            )?;
        }
        Ok(())
    })?;
    drop(reader);
    c.checkpoint(functions.len() as u64 * (functions.len().max(1).ilog2() as u64 + 1))?;
    functions.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    let position = |id: &FunctionAnalysisId, c: &mut dyn RunControl| -> Result<Option<usize>> {
        c.checkpoint(functions.len().max(1).ilog2() as u64 + 1)?;
        Ok(functions.binary_search_by(|f| f.id.cmp(id)).ok())
    };
    let entry = position(&target.entry, c)?
        .ok_or_else(|| invalid("trace entry is outside the selected profile"))?;
    let mut links = AdmittedVec::new(memory);
    blobray_store::visit_jsonl(&ir.records, c, |row: SemanticIrRecord, c| {
        if let SemanticIrRecord::Call { record } = row
            && let NavigationRecord::Call {
                caller,
                offset,
                candidates,
                issue,
                ..
            } = *record
            && let Some(caller) = position(&caller.analysis, c)?
        {
            let (callee, issue) = if issue.is_none() && candidates.len() == 1 {
                (position(&candidates[0].analysis, c)?, None)
            } else {
                (None, Some(TraceBlocker::UnresolvedCall))
            };
            links.push(
                TraceLink {
                    caller,
                    offset,
                    callee,
                    issue,
                },
                c.position(),
            )?;
        }
        Ok(())
    })?;
    drop(ir);
    c.checkpoint(links.len() as u64 * (links.len().max(1).ilog2() as u64 + 1))?;
    links.sort_unstable_by_key(|l| (l.caller, l.offset));
    let mut borrowed = AdmittedVec::new(memory);
    for function in &*functions {
        borrowed.push(
            TraceFunction {
                id: &function.id,
                manifest: &function.manifest,
                records: &function.records,
            },
            c.position(),
        )?;
    }
    c.phase(RunPhase::AnalyzeValues)?;
    blobray_analysis::trace::extract(
        &TraceInput {
            functions: &borrowed,
            links: &links,
            entry,
            target,
            observation,
            side,
        },
        memory,
        canonical,
        c,
        emit,
    )
}
pub(crate) fn query(
    project: &Project,
    request: &TraceRequest,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut Emitter<'_>,
) -> Result<TraceSummary> {
    request.validate()?;
    let mut canonical = Canonical::new(memory);
    let left = side(
        project,
        Selection {
            target: &request.left,
            observation: &request.observation,
            side: TraceSide::Left,
        },
        memory,
        &mut canonical,
        c,
        emit,
    )?;
    let right = request
        .right
        .as_ref()
        .map(|target| {
            side(
                project,
                Selection {
                    target,
                    observation: &request.observation,
                    side: TraceSide::Right,
                },
                memory,
                &mut canonical,
                c,
                emit,
            )
        })
        .transpose()?;
    c.phase(RunPhase::Compare)?;
    let verdict = right
        .as_ref()
        .map(|right| blobray_analysis::trace::compare(&left, right, c, emit))
        .transpose()?;
    Ok(TraceSummary {
        schema: 1,
        policy: STATIC_TRACE_POLICY,
        request: request.clone(),
        left: left.outcome,
        right: right.map(|r| r.outcome),
        verdict,
    })
}
