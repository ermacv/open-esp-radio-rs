//! Durable concrete evidence. Runs index publications; interpretation stays outside store.
use super::*;
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
    if manifest.schema != 1 {
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
        let RunOperation::Execute { request, producer } = &record.operation else {
            return Err(integrity("execution row has another operation"));
        };
        if record.state != RunState::Completed
            || manifest.project != self.id
            || request != &manifest.request
            || producer.executor != manifest.producer.executor
            || producer.environment != manifest.producer.environment
            || producer.verifier != manifest.producer.verifier
            || record.complete != Some(manifest.complete)
            || record.verdict != manifest.verdict
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
        let RunOperation::Execute { request, producer } = &run.operation else {
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
        validate_execution_records(&manifest, &stage.open_payload(&manifest.records, c)?, c)?;
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
        if run.id != retained.run || !matches!(run.operation, RunOperation::Execute { .. }) {
            return Err(integrity("execution retained for another run"));
        }
        let mut connection = open_connection(&self.project.root, true)?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db)?;
        let mut completed = run.clone();
        completed.state = RunState::Completed;
        completed.execution = Some(retained.id);
        completed.verdict = retained.verdict;
        completed.complete = Some(retained.complete);
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
    c: &mut dyn RunControl,
) -> Result<()> {
    let mut case = 0u32;
    let mut side = false;
    let mut events = 0u32;
    let mut outcome = false;
    let mut complete = true;
    let mut verdict = manifest.verdict.map(|_| ComparisonVerdict::Match);
    visit_jsonl::<ExecutionEvidence>(source, c, |record, _| {
        if case as usize >= manifest.request.cases.len() {
            return Err(integrity("extra execution case"));
        }
        match record {
            ExecutionEvidence::Event {
                case: i,
                replacement,
                ..
            } => {
                if i != case || replacement != side || outcome {
                    return Err(integrity("execution event order differs"));
                }
                events += 1;
                if events > manifest.request.max_events {
                    return Err(integrity("event capacity exceeded"));
                }
            }
            ExecutionEvidence::Outcome {
                case: i,
                replacement,
                stop,
                ..
            } => {
                if i != case || replacement != side || outcome {
                    return Err(integrity("execution outcome order differs"));
                }
                complete &= matches!(stop, ExecutionStop::Returned { .. });
                if manifest.request.replacement.is_none() {
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
                verdict = Some(match (verdict.unwrap(), result.verdict) {
                    (ComparisonVerdict::Diff, _) | (_, ComparisonVerdict::Diff) => {
                        ComparisonVerdict::Diff
                    }
                    (ComparisonVerdict::Incomplete, _) | (_, ComparisonVerdict::Incomplete) => {
                        ComparisonVerdict::Incomplete
                    }
                    _ => ComparisonVerdict::Match,
                });
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
    {
        return Err(integrity("execution evidence summary differs"));
    }
    Ok(())
}
