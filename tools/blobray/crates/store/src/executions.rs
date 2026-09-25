//! Durable concrete evidence. Runs index publications; interpretation stays outside store.
use super::*;
use crate::execution_observation::{EvidencePart, MemoryState, difference_valid};
use std::io::Write;
pub struct ExecutionLease<'a> {
    _capacity: MemoryReservation<'a>,
    pub manifest: ExecutionManifest,
    /// Decoded retained request identified by `manifest.request`.
    pub request: ExecutionRequest,
    pub records: FileLease,
}
pub struct RetainedExecution {
    run: RunId,
    id: ArtifactId,
    complete: bool,
    verdict: Option<ComparisonVerdict>,
}
fn decode(source: &dyn ByteSource, c: &mut dyn RunControl) -> Result<ExecutionManifest> {
    if source.len() > CONTROL_MESSAGE_BYTES as u64 {
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
    if manifest.producer.executor.is_empty()
        || manifest.producer.environment.is_empty()
        || manifest.producer.verifier.is_empty()
    {
        return Err(integrity("invalid execution identities"));
    }
    Ok(manifest)
}
/// A comparison request yields a verdict; a single implementation does not.
fn check_verdict(manifest: &ExecutionManifest, request: &ExecutionRequest) -> Result<()> {
    if request.replacement.is_some() != manifest.verdict.is_some() {
        return Err(integrity("execution verdict differs from its request"));
    }
    Ok(())
}
impl Project {
    pub fn execution<'a>(
        &self,
        id: &ArtifactId,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionLease<'a>> {
        let connection = open_connection(&self.root, false)?;
        // The indexed column only selects the candidate row; its decoded record
        // and identity below remain the authority.
        let mut query = connection
            .prepare("SELECT sequence FROM runs WHERE execution=?1 ORDER BY sequence")
            .map_err(db)?;
        let mut rows = query.query([id.as_str()]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            c.checkpoint(1)?;
            let sequence: i64 = row.get(0).map_err(db)?;
            let blob = connection
                .blob_open(rusqlite::MAIN_DB, "runs", "record", sequence, true)
                .map_err(db)?;
            let length = blob.len();
            let _capacity = memory.reserve(
                (length as u64)
                    .checked_mul(DECODE_EXPANSION)
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
                .checked_mul(DECODE_EXPANSION)
                .and_then(|n| n.checked_add(4096))
                .ok_or_else(|| integrity("execution capacity overflow"))?,
            c.position(),
        )?;
        let manifest = decode(&source, c)?;
        let RunOperation::Execute {
            request: request_id,
            compare,
            producer,
        } = record.effective_operation()
        else {
            return Err(integrity("execution row has another operation"));
        };
        let request = self.execution_request(request_id, memory, c)?;
        check_verdict(&manifest, &request)?;
        if record.state != RunState::Completed
            || manifest.project != self.id
            || request_id != &manifest.request
            || *compare != request.replacement.is_some()
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
            self.require_revision(&target.revision)?;
            if let FunctionSource::Image { image } = &target.source {
                let (manifest, _) = self.image_manifest(image, c)?;
                if manifest.plan.recipe.revision != target.revision {
                    return Err(integrity("execution image belongs to another revision"));
                }
            }
        }
        self.validate_execution_interfaces(&request, memory, c)?;
        self.validate_execution_call_pairs(&manifest, &request, memory, c)?;
        self.validate_execution_projections(&manifest, &request, memory, c)?;
        self.validate_execution_effects(&manifest, &request, memory, c)?;
        Ok(ExecutionLease {
            _capacity: capacity,
            records: self.open_payload(&manifest.records, c)?,
            manifest,
            request,
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
        if bytes.len() > CONTROL_MESSAGE_BYTES {
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
        let RunOperation::Execute {
            request: request_id,
            compare,
            producer,
        } = run.effective_operation()
        else {
            return Err(integrity("execution receipt belongs to another operation"));
        };
        let stage = Staging::open(&self.stage_path(&run.id))?;
        let source = stage.open_payload(&receipt.execution, c)?;
        let _capacity = memory.reserve(
            source
                .len()
                .checked_mul(DECODE_EXPANSION)
                .and_then(|n| n.checked_add(4096))
                .ok_or_else(|| integrity("execution capacity overflow"))?,
            c.position(),
        )?;
        let manifest = decode(&source, c)?;
        if receipt.schema != 1
            || receipt.project != self.project.id
            || manifest.project != self.project.id
            || &manifest.request != request_id
            || manifest.producer.executor != producer.executor
            || manifest.producer.environment != producer.environment
            || manifest.producer.verifier != producer.verifier
        {
            return Err(integrity("execution receipt differs from admission"));
        }
        let request = stage.execution_request(&self.project, request_id, memory, c)?;
        check_verdict(&manifest, &request)?;
        if *compare != request.replacement.is_some() {
            return Err(integrity("execution receipt differs from admission"));
        }
        self.project
            .validate_execution_interfaces(&request, memory, c)?;
        self.project
            .validate_execution_call_pairs(&manifest, &request, memory, c)?;
        self.project
            .validate_execution_projections(&manifest, &request, memory, c)?;
        self.project
            .validate_execution_effects(&manifest, &request, memory, c)?;
        validate_execution_records(
            &manifest,
            &request,
            &stage.open_payload(&manifest.records, c)?,
            memory,
            c,
        )?;
        // A replay's request is already retained by the project.
        if stage.has_payload(request_id) {
            self.promote(&stage, request_id, None, c)?;
        }
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
    request: &ExecutionRequest,
    source: &dyn ByteSource,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    validate_execution_records_with(manifest, request, source, memory, c, &mut |_, _| Ok(()))
}

/// Validate every record and hand each one to `visit` in the same pass, so a
/// reader decodes a large evidence set once. A later validation failure fails
/// the whole read; records already visited must not be published.
pub fn validate_execution_records_with(
    manifest: &ExecutionManifest,
    request: &ExecutionRequest,
    source: &dyn ByteSource,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    visit: &mut dyn FnMut(&ExecutionEvidence, &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    request.validate()?;
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
    let mut relation_index = [
        CallRelationIndex::new(None, &manifest.call_pairs, false, c)?,
        CallRelationIndex::new(None, &manifest.call_pairs, true, c)?,
    ];
    let mut effects: [Option<EffectTracker<'_>>; 2] = [None, None];
    // A contract rule may depend on the side's next concrete effect, so each
    // effect is classified once its successor, or the side's outcome, arrives.
    let mut pending_effect: Option<(ExecutionEvent, u32)> = None;
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
        visit(&record, c)?;
        if case as usize >= request.cases.len() {
            return Err(integrity("extra execution case"));
        }
        let phase = &request.cases[case as usize];
        c.checkpoint(manifest.projections.len() as u64 + 1)?;
        let projection = selected_projection(phase.relation.as_ref(), &manifest.projections)?
            .map(|p| &p.projection);
        let must_block = phase.reset == SessionReset::Warm && blocked;
        let close_chain = request
            .cases
            .get(case as usize + 1)
            .is_none_or(|next| next.reset == SessionReset::Cold);
        if prepared != Some(case) {
            relation_complete = true;
            c.checkpoint(manifest.effect_contracts.len() as u64 + 1)?;
            effects = match selected_effect_contract(
                phase.relation.as_ref(),
                &manifest.effect_contracts,
            )? {
                Some(selected) => {
                    c.checkpoint((selected.contract.rules.len() as u64 + 1).saturating_pow(2))?;
                    selected.contract.validate_use(request, case as usize)?;
                    [
                        Some(EffectTracker::new(&selected.contract, false, c)?),
                        Some(EffectTracker::new(&selected.contract, true, c)?),
                    ]
                }
                None => [None, None],
            };
            relation_index = [
                CallRelationIndex::new(phase.relation.as_ref(), &manifest.call_pairs, false, c)?,
                CallRelationIndex::new(phase.relation.as_ref(), &manifest.call_pairs, true, c)?,
            ];
            capture = [
                crate::execution_capture::CaptureState::new(),
                crate::execution_capture::CaptureState::new(),
            ];
            memory_state[0].begin(phase.relation.as_ref(), false);
            memory_state[1].begin(phase.relation.as_ref(), true);
            if let Some(p) = projection {
                c.checkpoint((p.fields.len() + p.branches.len() + 1).pow(2) as u64)?;
                p.validate_use(request, case as usize)?;
                memory_state[0].begin_projection(p, &phase.vendor, false, c)?;
                memory_state[1].begin_projection(
                    p,
                    phase.replacement.as_ref().unwrap(),
                    true,
                    c,
                )?;
            }
            services[0].begin(
                &phase.vendor,
                &request.vendor.stack,
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
                    &request.replacement.as_ref().unwrap().stack,
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
                memory_state[usize::from(side)].chunk(input, &chunk, c)?;
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
                crate::execution_timeline::validate(input, &event)?;
                if let (
                    Some(p),
                    ExecutionEvent::Branch {
                        site,
                        target,
                        fallthrough,
                        ..
                    },
                ) = (projection, &event)
                    && phase
                        .relation
                        .as_ref()
                        .is_some_and(|r| r.events.timeline.branches)
                {
                    c.checkpoint(p.branches.len() as u64 + 1)?;
                    relation_complete &= p
                        .branch_location(
                            BranchLocation {
                                site: *site,
                                target: *target,
                                fallthrough: *fallthrough,
                            },
                            side,
                        )
                        .is_some();
                }
                if phase
                    .relation
                    .as_ref()
                    .is_some_and(|r| r.events.selects(&event))
                    && let Some(transaction) = event.normal_memory()
                {
                    relation_complete &= transaction.known();
                    if let Some(p) = projection {
                        c.checkpoint(p.fields.len() as u64 + 1)?;
                        relation_complete &= p.memory_location(transaction, side)?.is_some();
                    }
                }
                capture[usize::from(side)].event(
                    input,
                    &event,
                    Some(&relation_index[usize::from(side)]),
                    c,
                )?;
                if let ExecutionEvent::RuntimeTable { instance, event } = &event {
                    tables[usize::from(side)].event(*instance, event, c)?;
                }
                services[usize::from(side)].event(&event, &tables[usize::from(side)], c)?;
                calls[usize::from(side)].event(&event, c)?;
                if is_contract_effect(&event)
                    && let Some(tracker) = &mut effects[usize::from(side)]
                {
                    if let Some((prior, ordinal)) = pending_effect.take() {
                        tracker.observe(&prior, Some(&event), ordinal, c)?;
                    }
                    pending_effect = Some((event.clone(), events));
                }
                events += 1;
                if events > request.max_events {
                    return Err(integrity("event capacity exceeded"));
                }
            }
            ExecutionEvidence::Outcome {
                case: i,
                replacement,
                stop,
                steps,
            } => {
                if let Some((prior, ordinal)) = pending_effect.take()
                    && let Some(tracker) = &mut effects[usize::from(side)]
                {
                    tracker.observe(&prior, None, ordinal, c)?;
                }
                if i != case || replacement != side || outcome {
                    return Err(integrity("execution outcome order differs"));
                }
                let phase = &request.cases[case as usize];
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
                if phase
                    .relation
                    .as_ref()
                    .is_some_and(ComparisonRelation::observes_calls)
                {
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
                if request.replacement.is_none() {
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
                crate::execution_effects::validate_result(&result, &effects)?;
                if !difference_valid(&result, phase, request.max_events, projection)
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
    if case as usize != request.cases.len()
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

/// Test view of a manifest together with the request its identity names.
/// Its own `request` field shadows the manifest's identity field.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct TestExecution {
    pub manifest: ExecutionManifest,
    pub request: ExecutionRequest,
}
#[cfg(test)]
impl TestExecution {
    pub fn new(mut manifest: ExecutionManifest, request: ExecutionRequest) -> Self {
        manifest.request = encode_execution_request(&request)
            .map(|(id, _)| id)
            .unwrap_or_else(|_| ArtifactId::of_bytes(b"invalid test request"));
        Self { manifest, request }
    }
    pub fn validate_records(
        &self,
        source: &dyn ByteSource,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        validate_execution_records(&self.manifest, &self.request, source, memory, c)
    }
}
#[cfg(test)]
impl std::ops::Deref for TestExecution {
    type Target = ExecutionManifest;
    fn deref(&self) -> &ExecutionManifest {
        &self.manifest
    }
}
#[cfg(test)]
impl std::ops::DerefMut for TestExecution {
    fn deref_mut(&mut self) -> &mut ExecutionManifest {
        &mut self.manifest
    }
}
