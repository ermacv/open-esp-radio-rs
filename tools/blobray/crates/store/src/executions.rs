//! Durable concrete evidence. Runs index publications; interpretation stays outside store.
use super::*;
use crate::execution_observation::{EvidencePart, MemoryState, difference_valid};
use std::io::Write;
pub struct ExecutionLease<'a> {
    _capacity: MemoryReservation<'a>,
    pub manifest: ExecutionManifest,
    pub records: FileLease,
}
pub struct RetainedExecution {
    run: RunId,
    id: ArtifactId,
    complete: bool,
    verdict: Option<ComparisonVerdict>,
}
fn decode(source: &dyn ByteSource, c: &mut dyn RunControl) -> Result<ExecutionManifest> {
    if source.len() > 65536 {
        return Err(integrity("execution manifest exceeds 64 KiB"));
    }
    let mut bytes = vec![0; source.len() as usize];
    source.read_at(0, &mut bytes, c)?;
    let manifest: ExecutionManifest = serde_json::from_slice(&bytes).map_err(jobs::json)?;
    if manifest.schema != EXECUTION_SCHEMA {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported execution manifest",
        ));
    }
    manifest.request.validate()?;
    if manifest.producer.executor.is_empty()
        || manifest.producer.environment.is_empty()
        || manifest.producer.verifier.is_empty()
        || manifest.request.replacement.is_some() != manifest.verdict.is_some()
    {
        return Err(integrity("invalid execution identities or verdict"));
    }
    Ok(manifest)
}
impl Project {
    pub fn execution<'a>(
        &self,
        id: &ArtifactId,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionLease<'a>> {
        let connection = open_connection(&self.root, false)?;
        let mut query = connection
            .prepare("SELECT sequence FROM runs ORDER BY sequence")
            .map_err(db)?;
        let mut rows = query.query([]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            c.checkpoint(1)?;
            let sequence: i64 = row.get(0).map_err(db)?;
            let blob = connection
                .blob_open(rusqlite::MAIN_DB, "runs", "record", sequence, true)
                .map_err(db)?;
            let length = blob.len();
            let _capacity = memory.reserve(
                (length as u64)
                    .checked_mul(64)
                    .and_then(|n| n.checked_add(4096))
                    .ok_or_else(|| integrity("run capacity overflow"))?,
                c.position(),
            )?;
            let mut raw = memory.bytes(length, c.position())?;
            for (i, chunk) in raw.chunks_mut(WORK_BLOCK).enumerate() {
                c.bytes(chunk.len())?;
                blob.read_at_exact(chunk, i * WORK_BLOCK).map_err(db)?;
            }
            let run = jobs::decode_run(
                std::str::from_utf8(&raw).map_err(|_| integrity("run is not UTF-8"))?,
            )?;
            let identity = connection
                .blob_open(rusqlite::MAIN_DB, "runs", "id", sequence, true)
                .map_err(db)?;
            if identity.len() != 64 {
                return Err(integrity("run row identity length differs"));
            }
            let mut raw_id = [0; 64];
            c.bytes(64)?;
            identity.read_at_exact(&mut raw_id, 0).map_err(db)?;
            if raw_id != run.id.as_str().as_bytes() {
                return Err(integrity("run row identity differs"));
            }
            if run.execution.as_ref() == Some(id) {
                return self.execution_from_run(&run, memory, c);
            }
        }
        Err(Error::new(
            ErrorCode::NotFound,
            "execution result not retained",
        ))
    }
    pub(crate) fn execution_from_run<'a>(
        &self,
        record: &RunRecord,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionLease<'a>> {
        let id = record
            .execution
            .as_ref()
            .ok_or_else(|| integrity("run has no execution publication"))?;
        let source = self.open_payload(id, c)?;
        let capacity = memory.reserve(
            source
                .len()
                .checked_mul(64)
                .and_then(|n| n.checked_add(4096))
                .ok_or_else(|| integrity("execution capacity overflow"))?,
            c.position(),
        )?;
        let manifest = decode(&source, c)?;
        let RunOperation::Execute { request, producer } = record.effective_operation() else {
            return Err(integrity("execution row has another operation"));
        };
        if record.state != RunState::Completed
            || manifest.project != self.id
            || request != &manifest.request
            || producer.executor != manifest.producer.executor
            || producer.environment != manifest.producer.environment
            || producer.verifier != manifest.producer.verifier
            || record.assessment
                != Some(ResultAssessment::execution(
                    id.clone(),
                    manifest.complete,
                    manifest.verdict,
                ))
        {
            return Err(integrity("execution manifest and admitted run differ"));
        }
        for target in std::iter::once(&request.vendor).chain(&request.replacement) {
            self.open_payload(&target.revision.as_str().parse()?, c)?;
            if let FunctionSource::Image { image } = &target.source {
                let image = self.image(image, c)?;
                if image.manifest.plan.recipe.revision != target.revision {
                    return Err(integrity("execution image belongs to another revision"));
                }
            }
        }
        self.validate_execution_interfaces(request, memory, c)?;
        Ok(ExecutionLease {
            _capacity: capacity,
            records: self.open_payload(&manifest.records, c)?,
            manifest,
        })
    }
}
impl Staging {
    pub fn execution_receipt(
        &self,
        manifest: &ExecutionManifest,
        c: &mut dyn RunControl,
    ) -> Result<PreparedExecutionReceipt> {
        let bytes = serde_json::to_vec(manifest).map_err(jobs::json)?;
        if bytes.len() > 65536 {
            return Err(integrity("execution manifest exceeds 64 KiB"));
        }
        let mut file = self.disk.temporary(&self.root.join("staging"))?;
        file.write_all(&bytes).map_err(io)?;
        Ok(PreparedExecutionReceipt {
            schema: 1,
            project: manifest.project.clone(),
            execution: self.retain_temporary(file, c)?,
        })
    }
}
impl Writer {
    pub fn retain_execution(
        &self,
        run: &RunRecord,
        receipt: &PreparedExecutionReceipt,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<RetainedExecution> {
        let RunOperation::Execute { request, producer } = run.effective_operation() else {
            return Err(integrity("execution receipt belongs to another operation"));
        };
        let stage = Staging::open(&self.stage_path(&run.id))?;
        let source = stage.open_payload(&receipt.execution, c)?;
        let _capacity = memory.reserve(
            source
                .len()
                .checked_mul(64)
                .and_then(|n| n.checked_add(4096))
                .ok_or_else(|| integrity("execution capacity overflow"))?,
            c.position(),
        )?;
        let manifest = decode(&source, c)?;
        if receipt.schema != 1
            || receipt.project != self.project.id
            || manifest.project != self.project.id
            || &manifest.request != request
            || manifest.producer.executor != producer.executor
            || manifest.producer.environment != producer.environment
            || manifest.producer.verifier != producer.verifier
        {
            return Err(integrity("execution receipt differs from admission"));
        }
        self.project
            .validate_execution_interfaces(request, memory, c)?;
        validate_execution_records(
            &manifest,
            &stage.open_payload(&manifest.records, c)?,
            memory,
            c,
        )?;
        for id in [&manifest.records, &receipt.execution] {
            self.promote(&stage, id, None, c)?;
        }
        sync_dir(&self.project.root.join("objects"))?;
        Ok(RetainedExecution {
            run: run.id.clone(),
            id: receipt.execution.clone(),
            complete: manifest.complete,
            verdict: manifest.verdict,
        })
    }
    pub fn publish_execution(
        &mut self,
        run: &mut RunRecord,
        retained: RetainedExecution,
    ) -> Result<()> {
        if run.id != retained.run
            || !matches!(run.effective_operation(), RunOperation::Execute { .. })
        {
            return Err(integrity("execution retained for another run"));
        }
        let mut connection = open_connection(&self.project.root, true)?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db)?;
        let mut completed = run.clone();
        completed.state = RunState::Completed;
        completed.execution = Some(retained.id);
        completed.assessment = Some(ResultAssessment::execution(
            completed.execution.clone().unwrap(),
            retained.complete,
            retained.verdict,
        ));
        tx.execute(
            "UPDATE runs SET record=?2 WHERE id=?1",
            params![
                run.id.as_str(),
                serde_json::to_string(&completed).map_err(jobs::json)?
            ],
        )
        .map_err(db)?;
        tx.commit().map_err(db)?;
        *run = completed;
        Ok(())
    }
}
/// Structural evidence validation, independent of execution/comparison algorithms.
pub fn validate_execution_records(
    manifest: &ExecutionManifest,
    source: &dyn ByteSource,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    manifest.request.validate()?;
    let mut case = 0u32;
    let mut side = false;
    let mut events = 0u32;
    let mut outcome = false;
    let mut complete = true;
    let mut blocked = false;
    let mut phase_complete = true;
    let mut models = [
        crate::execution_models::Models::new(),
        crate::execution_models::Models::new(),
    ];
    let mut calls = [
        crate::execution_calls::Calls::new(),
        crate::execution_calls::Calls::new(),
    ];
    let mut tables = [
        crate::execution_tables::Tables::new(),
        crate::execution_tables::Tables::new(),
    ];
    let mut services = [
        crate::execution_services::Services::new(memory),
        crate::execution_services::Services::new(memory),
    ];
    let mut prepared = None;
    let mut part = EvidencePart::Events;
    let mut memory_state = [MemoryState::new(), MemoryState::new()];
    let mut capture = [
        crate::execution_capture::CaptureState::new(),
        crate::execution_capture::CaptureState::new(),
    ];
    let mut relation_complete = true;
    let mut environment_complete = true;
    let mut verdict = manifest.verdict.map(|_| ComparisonVerdict::Match);
    visit_jsonl::<ExecutionEvidence>(source, c, |record, c| {
        if case as usize >= manifest.request.cases.len() {
            return Err(integrity("extra execution case"));
        }
        let phase = &manifest.request.cases[case as usize];
        let must_block = phase.reset == SessionReset::Warm && blocked;
        let close_chain = manifest
            .request
            .cases
            .get(case as usize + 1)
            .is_none_or(|next| next.reset == SessionReset::Cold);
        if prepared != Some(case) {
            relation_complete = true;
            capture = [
                crate::execution_capture::CaptureState::new(),
                crate::execution_capture::CaptureState::new(),
            ];
            memory_state[0].begin(phase.relation.as_ref(), false);
            memory_state[1].begin(phase.relation.as_ref(), true);
            services[0].begin(
                &phase.vendor,
                &manifest.request.vendor.stack,
                phase.reset,
                must_block,
                c,
            )?;
            tables[0].begin(&phase.vendor.tables, phase.reset, must_block, c)?;
            calls[0].begin(&phase.vendor.calls, phase.reset, must_block, c)?;
            models[0].begin(&phase.vendor.models, phase.reset, must_block, c)?;
            if !must_block {
                services[0].validate_bindings(&phase.vendor, &tables[0], c)?;
            }
            if let Some(replacement) = &phase.replacement {
                services[1].begin(
                    replacement,
                    &manifest.request.replacement.as_ref().unwrap().stack,
                    phase.reset,
                    must_block,
                    c,
                )?;
                tables[1].begin(&replacement.tables, phase.reset, must_block, c)?;
                calls[1].begin(&replacement.calls, phase.reset, must_block, c)?;
                models[1].begin(&replacement.models, phase.reset, must_block, c)?;
                if !must_block {
                    services[1].validate_bindings(replacement, &tables[1], c)?;
                }
            }
            prepared = Some(case);
        }
        match record {
            ExecutionEvidence::FinalMemory {
                case: i,
                replacement,
                chunk,
            } => {
                if i != case
                    || replacement != side
                    || outcome
                    || part == EvidencePart::Environment
                    || must_block
                {
                    return Err(integrity("final-memory evidence order differs"));
                }
                part = EvidencePart::Memory;
                let input = if side {
                    phase.replacement.as_ref().unwrap()
                } else {
                    &phase.vendor
                };
                memory_state[usize::from(side)].chunk(input, &chunk)?;
            }
            ExecutionEvidence::FifoService {
                case: i,
                replacement,
                observation,
            } => {
                if i != case || replacement != side || outcome {
                    return Err(integrity("FIFO evidence order differs"));
                }
                part = EvidencePart::Environment;
                services[usize::from(side)].observe(&observation, close_chain, must_block)?;
                environment_complete &= observation.status != ModelStatus::Incomplete;
            }
            ExecutionEvidence::RuntimeTable {
                case: i,
                replacement,
                observation,
            } => {
                if i != case || replacement != side || outcome {
                    return Err(integrity("runtime table evidence order differs"));
                }
                part = EvidencePart::Environment;
                tables[usize::from(side)].observe(&observation, close_chain, must_block)?;
                environment_complete &= observation.status != ModelStatus::Incomplete;
            }
            ExecutionEvidence::CallModel {
                case: i,
                replacement,
                observation,
            } => {
                if i != case || replacement != side || outcome {
                    return Err(integrity("call model evidence order differs"));
                }
                part = EvidencePart::Environment;
                calls[usize::from(side)].observe(&observation, close_chain, must_block, c)?;
                environment_complete &= observation.status != ModelStatus::Incomplete;
            }
            ExecutionEvidence::Model {
                case: i,
                replacement,
                observation,
            } => {
                if i != case || replacement != side || outcome {
                    return Err(integrity("model evidence order differs"));
                }
                part = EvidencePart::Environment;
                models[usize::from(side)].observe(&observation, close_chain, must_block, c)?;
                environment_complete &= observation.status != ModelStatus::Incomplete;
            }
            ExecutionEvidence::Event {
                case: i,
                replacement,
                event,
            } => {
                if i != case || replacement != side || outcome || part != EvidencePart::Events {
                    return Err(integrity("execution event order differs"));
                }
                let input = if side {
                    phase.replacement.as_ref().unwrap()
                } else {
                    &phase.vendor
                };
                c.checkpoint(
                    input
                        .observe_calls
                        .as_ref()
                        .map_or(0, |p| p.overrides.len()) as u64
                        + 1,
                )?;
                capture[usize::from(side)].event(input, &event)?;
                if let ExecutionEvent::RuntimeTable { instance, event } = &event {
                    tables[usize::from(side)].event(*instance, event, c)?;
                }
                services[usize::from(side)].event(&event, &tables[usize::from(side)], c)?;
                calls[usize::from(side)].event(&event, c)?;
                events += 1;
                if events > manifest.request.max_events {
                    return Err(integrity("event capacity exceeded"));
                }
            }
            ExecutionEvidence::Outcome {
                case: i,
                replacement,
                stop,
                steps,
            } => {
                if i != case || replacement != side || outcome {
                    return Err(integrity("execution outcome order differs"));
                }
                let phase = &manifest.request.cases[case as usize];
                let input = if side {
                    phase
                        .replacement
                        .as_ref()
                        .ok_or_else(|| integrity("outcome has no replacement input"))?
                } else {
                    &phase.vendor
                };
                let must_block = phase.reset == SessionReset::Warm && blocked;
                if matches!(stop, ExecutionStop::BlockedByPriorPhase) != must_block
                    || (must_block && (steps != 0 || events != 0))
                {
                    return Err(integrity("execution dependency blocking differs"));
                }
                let address_valid = |pc: u32| pc & 1 == 0 && pc < u32::MAX - 1;
                let goal_valid = match (&stop, &input.goal) {
                    (ExecutionStop::Returned { .. }, ExecutionGoal::Return) => true,
                    (
                        ExecutionStop::ObservedDequeue { .. },
                        ExecutionGoal::ObserveDequeue { .. },
                    ) => true,
                    (ExecutionStop::ReachedSymbol { pc }, ExecutionGoal::ReachSymbol { .. }) => {
                        address_valid(*pc)
                    }
                    (
                        ExecutionStop::ObservedCall { pc, target, tail },
                        ExecutionGoal::ObserveCall { include_tail, .. },
                    ) => address_valid(*pc) && address_valid(*target) && (!tail || *include_tail),
                    (ExecutionStop::GoalNotReached { .. }, goal) => {
                        !matches!(goal, ExecutionGoal::Return)
                    }
                    (ExecutionStop::Incomplete { .. } | ExecutionStop::BlockedByPriorPhase, _) => {
                        true
                    }
                    _ => false,
                };
                if !goal_valid || !services[usize::from(side)].goal_valid(&stop) {
                    return Err(integrity(
                        "execution outcome does not match its declared goal",
                    ));
                }
                capture[usize::from(side)].finish(input, &stop)?;
                if phase.relation.as_ref().is_some_and(|r| r.calls) {
                    relation_complete &= capture[usize::from(side)].known;
                }
                memory_state[usize::from(side)].finish(input, must_block)?;
                relation_complete &= memory_state[usize::from(side)].known;
                if let Some(r) = &phase.relation {
                    for (word, selected) in [r.returns.low, r.returns.high].into_iter().enumerate()
                    {
                        if selected {
                            relation_complete &= match &stop {
                                ExecutionStop::Returned { low, high } => {
                                    [*low, *high][word].is_some()
                                }
                                _ => false,
                            };
                        }
                    }
                }
                models[usize::from(side)].finish_side()?;
                calls[usize::from(side)].finish_side()?;
                tables[usize::from(side)].finish_side()?;
                services[usize::from(side)].finish_side()?;
                complete &= stop.completed() && environment_complete;
                phase_complete &= stop.completed() && environment_complete;
                part = EvidencePart::Events;
                environment_complete = true;
                if manifest.request.replacement.is_none() {
                    blocked = !phase_complete;
                    phase_complete = true;
                    case += 1;
                    events = 0;
                } else if !side {
                    side = true;
                    events = 0;
                } else {
                    outcome = true;
                }
            }
            ExecutionEvidence::Comparison { case: i, result } => {
                if i != case || !outcome || verdict.is_none() {
                    return Err(integrity("comparison order differs"));
                }
                if !difference_valid(&result, phase, manifest.request.max_events)
                    || (result.verdict == ComparisonVerdict::Match
                        && (!phase_complete || !relation_complete))
                {
                    return Err(integrity("MATCH has unmet execution/model obligations"));
                }
                verdict = Some(match (verdict.unwrap(), result.verdict) {
                    (ComparisonVerdict::Diff, _) | (_, ComparisonVerdict::Diff) => {
                        ComparisonVerdict::Diff
                    }
                    (ComparisonVerdict::Incomplete, _) | (_, ComparisonVerdict::Incomplete) => {
                        ComparisonVerdict::Incomplete
                    }
                    _ => ComparisonVerdict::Match,
                });
                blocked = !phase_complete;
                phase_complete = true;
                case += 1;
                side = false;
                events = 0;
                outcome = false;
            }
        }
        Ok(())
    })?;
    if case as usize != manifest.request.cases.len()
        || side
        || outcome
        || events != 0
        || complete != manifest.complete
        || verdict != manifest.verdict
        || !models.iter().all(crate::execution_models::Models::closed)
        || !calls.iter().all(crate::execution_calls::Calls::closed)
        || !tables.iter().all(crate::execution_tables::Tables::closed)
        || !services
            .iter()
            .all(crate::execution_services::Services::closed)
    {
        return Err(integrity("execution evidence summary differs"));
    }
    Ok(())
}
