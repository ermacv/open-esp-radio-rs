use super::metered::{CheckedReader, hash_file, read_payload, write_json};
use super::*;
use serde::{Deserialize, Serialize};
use std::io::{BufReader, Write};

/// Payload-only capability granted to a worker. It has no metadata connection.
pub struct Staging {
    pub(crate) root: PathBuf,
    pub(crate) disk: TemporaryBudget,
}

/// Validated retained closure, bound to one run; only retain_candidate creates it.
pub struct RetainedImport {
    run: RunId,
    prepared: PreparedImport,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub schema: u32,
    pub project: ProjectId,
    pub storage_schema: u32,
    pub checked_revisions: usize,
    pub checked_images: u64,
    pub checked_analyses: u64,
    pub checked_publications: u64,
    pub unfinished_runs: Vec<RunId>,
    pub errors: Vec<Error>,
}

impl Staging {
    pub fn open(path: &Path) -> Result<Self> {
        if !path.join("objects").is_dir() || !path.join("staging").is_dir() {
            return Err(integrity("incomplete worker staging directory"));
        }
        Ok(Self {
            root: path.to_owned(),
            disk: TemporaryBudget::open(path)?,
        })
    }
    pub fn with_temporary_budget(path: &Path, disk: TemporaryBudget) -> Result<Self> {
        let mut staging = Self::open(path)?;
        staging.disk = disk;
        Ok(staging)
    }
    pub fn lease(&self, id: &ArtifactId) -> Result<ArtifactLease> {
        self.lease_controlled(id, &mut || Ok(()))
    }
    pub fn lease_controlled(
        &self,
        id: &ArtifactId,
        control: &mut dyn RunControl,
    ) -> Result<ArtifactLease> {
        let mut position = control.position();
        position.phase = RunPhase::ReadCaptured;
        position.artifact(id);
        control.set_position(position);
        control.checkpoint(0)?;
        let bytes = read_payload(&self.object_path(id), control)?;
        if ArtifactId::of_bytes_controlled(&bytes, control)? != *id {
            return Err(integrity("staged payload digest mismatch"));
        }
        Ok(ArtifactLease::new(bytes))
    }
    pub(crate) fn object_path(&self, id: &ArtifactId) -> PathBuf {
        self.root.join("objects").join(id.as_str())
    }

    /// Validate inside the resource-limited worker, then write a streaming closure
    /// index and manifest. The coordinator verifies this receipt's byte identities.
    pub fn prepare(&mut self, revision: Revision) -> Result<PreparedImport> {
        self.prepare_controlled(revision, &mut || Ok(()))
    }
    pub fn prepare_controlled(
        &mut self,
        revision: Revision,
        control: &mut dyn RunControl,
    ) -> Result<PreparedImport> {
        revision.validate_controlled(control)?;
        control.set_position(RunPosition {
            phase: RunPhase::Serialize,
            ..Default::default()
        });
        control.checkpoint(0)?;
        let mut closure = self.disk.temporary(&self.root.join("staging"))?;
        for capture in revision.captures() {
            if let Capture::Captured { artifact, length } = capture {
                verify_file(
                    &self.object_path(artifact),
                    artifact,
                    Some(*length),
                    control,
                )?;
                write_json(closure.as_file_mut(), capture, control)?;
                control.bytes(1)?;
                closure.write_all(b"\n").map_err(io)?;
            }
        }
        closure.as_file().sync_all().map_err(io)?;
        let closure_id = hash_file(closure.path(), control)?.0;
        self.persist(closure, &closure_id, control)?;
        let mut manifest = self.disk.temporary(&self.root.join("staging"))?;
        write_json(manifest.as_file_mut(), &revision, control)?;
        manifest.as_file().sync_all().map_err(io)?;
        let id = hash_file(manifest.path(), control)?.0;
        self.persist(manifest, &id, control)?;
        sync_dir(&self.root.join("objects"))?;
        let mut complete = true;
        for input in &revision.inputs {
            control.checkpoint(1)?;
            match &input.inventory {
                Some(inventory) => {
                    complete &= inventory.members_complete && inventory.diagnostics.is_empty();
                    for object in &inventory.objects {
                        control.checkpoint(1)?;
                        complete &= object.elf.is_some() && object.diagnostics.is_empty();
                    }
                }
                None => complete = false,
            }
        }
        Ok(PreparedImport {
            schema: 1,
            project: revision.project.clone(),
            parent: revision.parent.clone(),
            revision: id.as_str().parse()?,
            closure: closure_id,
            complete,
        })
    }
}

pub(crate) fn json(error: serde_json::Error) -> Error {
    integrity(error.to_string())
}

impl Project {
    pub fn run(&self, id: &RunId) -> Result<RunRecord> {
        let connection = open_connection(&self.root, false)?;
        let raw: String = connection
            .query_row(
                "SELECT record FROM runs WHERE id=?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .map_err(db)?;
        let record = decode_run(&raw)?;
        if record.id != *id {
            return Err(integrity("run row and record identity disagree"));
        }
        Ok(record)
    }

    pub fn runs(&self) -> Result<Vec<RunRecord>> {
        let connection = open_connection(&self.root, false)?;
        let schema: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db)?;
        if schema == 1 {
            return Ok(Vec::new());
        }
        let mut query = connection
            .prepare("SELECT record FROM runs ORDER BY sequence")
            .map_err(db)?;
        query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db)?
            .map(|row| decode_run(&row.map_err(db)?))
            .collect()
    }

    /// Read-only verification. No writer acquisition, migration, repair or source reads.
    pub fn doctor(path: &Path) -> Result<DoctorReport> {
        let project = Self::open(path)?;
        let memory = WorkingMemory::new(DEFAULT_WORKING_BYTES)?;
        struct Collector<'a> {
            memory: &'a WorkingMemory,
            reservations: Vec<MemoryReservation<'a>>,
            errors: Vec<Error>,
            unfinished: Vec<RunId>,
        }
        impl Collector<'_> {
            fn reserve(&mut self, bytes: u64, control: &mut dyn RunControl) -> Result<()> {
                let reservation = self.memory.reserve(bytes, control.position())?;
                self.reservations.try_reserve_exact(1).map_err(|_| {
                    Error::new(
                        ErrorCode::ResourceLimited,
                        "host allocation refused doctor result",
                    )
                })?;
                self.reservations.push(reservation);
                Ok(())
            }
        }
        impl DoctorSink for Collector<'_> {
            fn error(&mut self, error: &Error, control: &mut dyn RunControl) -> Result<()> {
                self.reserve(1024 + error.message.len() as u64 * 2, control)?;
                self.errors.try_reserve_exact(1).map_err(|_| {
                    Error::new(
                        ErrorCode::ResourceLimited,
                        "host allocation refused doctor errors",
                    )
                })?;
                self.errors.push(error.clone());
                Ok(())
            }
            fn unfinished(&mut self, id: &RunId, control: &mut dyn RunControl) -> Result<()> {
                self.reserve(1024, control)?;
                self.unfinished.try_reserve_exact(1).map_err(|_| {
                    Error::new(
                        ErrorCode::ResourceLimited,
                        "host allocation refused doctor run list",
                    )
                })?;
                self.unfinished.push(id.clone());
                Ok(())
            }
        }
        let mut sink = Collector {
            memory: &memory,
            reservations: Vec::new(),
            errors: Vec::new(),
            unfinished: Vec::new(),
        };
        let summary = project.doctor_stream(&memory, &mut || Ok(()), &mut sink)?;
        Ok(DoctorReport {
            schema: summary.schema,
            project: summary.project,
            storage_schema: summary.storage_schema,
            checked_images: summary.checked_images,
            checked_analyses: summary.checked_analyses,
            checked_publications: summary.checked_publications,
            checked_revisions: usize::try_from(summary.checked_revisions)
                .map_err(|_| integrity("revision count overflow"))?,
            unfinished_runs: sink.unfinished,
            errors: sink.errors,
        })
    }
}

impl Writer {
    pub fn register(
        &mut self,
        budget: ResourceBudget,
        owner: OwnerIdentity,
    ) -> Result<(RunRecord, PathBuf)> {
        self.register_with_stage_owner(budget, owner, |_| {})
    }
    /// Assign the stage to its capacity owner before attempting filesystem setup.
    /// The callback records ownership only; it must not mutate the path or panic.
    pub fn register_with_stage_owner(
        &mut self,
        budget: ResourceBudget,
        owner: OwnerIdentity,
        own_stage: impl FnOnce(&Path),
    ) -> Result<(RunRecord, PathBuf)> {
        self.register_operation(budget, owner, RunOperation::Import, own_stage)
    }
    pub fn register_operation(
        &mut self,
        budget: ResourceBudget,
        owner: OwnerIdentity,
        operation: RunOperation,
        own_stage: impl FnOnce(&Path),
    ) -> Result<(RunRecord, PathBuf)> {
        if let RunOperation::Knowledge { change } = &operation {
            self.project.check_knowledge_base(&change.expected_base)?;
        }
        let source_revision = match &operation {
            RunOperation::PrepareImage { revision, .. }
            | RunOperation::Investigate { revision, .. } => Some(revision),
            RunOperation::AnalyzeFunction { request } => request.revision.as_ref(),
            RunOperation::BuildIr { request } => {
                request.validate()?;
                Some(&request.scope.revision)
            }
            _ => None,
        };
        if matches!(&operation,RunOperation::AnalyzeFunction{request} if request.revision.is_none())
        {
            return Err(integrity("analysis requires frozen revision"));
        }
        if let Some(revision) = source_revision {
            let exists: bool = open_connection(&self.project.root, true)?
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM revisions WHERE id=?1)",
                    [revision.as_str()],
                    |r| r.get(0),
                )
                .map_err(db)?;
            if !exists {
                return Err(Error::new(
                    ErrorCode::NotFound,
                    "image source revision is not retained",
                ));
            }
        }
        if operation == RunOperation::Query {
            return Err(integrity("query cannot acquire durable registration"));
        }
        let connection = open_connection(&self.project.root, true)?;
        let unfinished: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE json_extract(record,'$.state') IN ('registered','running','validating'))", [], |row| row.get(0)).map_err(db)?;
        if unfinished {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "unfinished import exists; run recover explicitly",
            ));
        }
        let raw: String = connection
            .query_row("SELECT lower(hex(randomblob(32)))", [], |r| r.get(0))
            .map_err(db)?;
        budget.validate()?;
        let record = RunRecord {
            schema: 37,
            operation,
            resolved_operation: None,
            image: None,
            analysis: None,
            publication: None,
            knowledge: None,
            execution: None,
            semantic_ir: None,
            assessment: None,
            id: raw.parse()?,
            state: RunState::Registered,
            owner,
            budget,
            base: self.project.current()?,
            revision: None,
            error: None,
            diagnostics: Some(RunDiagnostics::default()),
        };
        connection
            .execute(
                "INSERT INTO runs(id,record) VALUES(?1,?2)",
                params![
                    record.id.as_str(),
                    serde_json::to_string(&record).map_err(json)?
                ],
            )
            .map_err(db)?;
        let stage = self.stage_path(&record.id);
        own_stage(&stage);
        let prepared = (|| -> Result<()> {
            fs::create_dir(&stage).map_err(io)?;
            File::create(stage.join("lease.lock")).map_err(io)?;
            fs::create_dir(stage.join("objects")).map_err(io)?;
            fs::create_dir(stage.join("staging")).map_err(io)?;
            sync_dir(&stage)?;
            sync_dir(&self.project.root.join("staging"))
        })();
        if let Err(error) = prepared {
            let mut failed = record.clone();
            failed.state = RunState::Failed;
            failed.error = Some(error.clone());
            if let Err(secondary) = self.update_run(&failed) {
                failed.diagnostics.as_mut().unwrap().secondary(secondary);
            }
            if stage.try_exists().unwrap_or(true)
                && let Err(secondary) = fs::remove_dir_all(&stage)
            {
                failed
                    .diagnostics
                    .as_mut()
                    .unwrap()
                    .secondary(io(secondary));
            }
            let _ = self.update_run(&failed);
            return Err(error);
        }
        Ok((record, stage))
    }
    pub fn update_run(&mut self, record: &RunRecord) -> Result<()> {
        let previous = self.project.run(&record.id)?;
        if previous.owner != record.owner
            || previous.budget != record.budget
            || previous.base != record.base
            || previous.operation != record.operation
            || previous.image != record.image
            || previous.analysis != record.analysis
            || previous.publication != record.publication
            || previous.knowledge != record.knowledge
            || previous.schema != record.schema
            || previous.revision != record.revision
            || previous.assessment != record.assessment
            || previous.execution != record.execution
            || previous.semantic_ir != record.semantic_ir
            || previous.resolved_operation != record.resolved_operation
        {
            return Err(integrity(
                "run update changed immutable admission or publication fields",
            ));
        }
        let allowed = previous.state == record.state
            || match previous.state {
                RunState::Registered => matches!(
                    record.state,
                    RunState::Running
                        | RunState::Cancelled
                        | RunState::TimedOut
                        | RunState::ResourceLimited
                        | RunState::Failed
                        | RunState::Abandoned
                ),
                RunState::Running => matches!(
                    record.state,
                    RunState::Validating
                        | RunState::Cancelled
                        | RunState::TimedOut
                        | RunState::ResourceLimited
                        | RunState::Failed
                        | RunState::Abandoned
                ),
                RunState::Validating => matches!(
                    record.state,
                    RunState::Cancelled
                        | RunState::TimedOut
                        | RunState::ResourceLimited
                        | RunState::Failed
                        | RunState::Abandoned
                ),
                _ => false,
            };
        if !allowed {
            return Err(integrity(
                "invalid run transition; only publication may complete a run",
            ));
        }
        let connection = open_connection(&self.project.root, true)?;
        let changed = connection
            .execute(
                "UPDATE runs SET record=?2 WHERE id=?1",
                params![
                    record.id.as_str(),
                    serde_json::to_string(record).map_err(json)?
                ],
            )
            .map_err(db)?;
        if changed != 1 {
            return Err(integrity("unknown run"));
        }
        Ok(())
    }
    pub fn stage_path(&self, id: &RunId) -> PathBuf {
        self.project.root.join("staging").join(id.as_str())
    }

    /// Streaming validation/promotion keeps large inventory out of the coordinator.
    /// Only a successfully reaped, trusted worker's prepared receipt is admissible.
    pub fn retain_candidate(
        &mut self,
        run: &RunRecord,
        prepared: &PreparedImport,
        checkpoint: &mut dyn RunControl,
    ) -> Result<RetainedImport> {
        if run.operation != RunOperation::Import
            || prepared.schema != 1
            || prepared.project != self.project.id
            || prepared.parent != run.base
        {
            return Err(integrity("worker receipt belongs to a different import"));
        }
        let stage = Staging::open(&self.stage_path(&run.id))?;
        verify_file(
            &stage.object_path(&prepared.closure),
            &prepared.closure,
            None,
            checkpoint,
        )?;
        let mut failure = None;
        let file = File::open(stage.object_path(&prepared.closure)).map_err(io)?;
        // Decode one bounded control record per line, releasing the control borrow
        // before promoting its payload.
        use std::io::BufRead;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            checkpoint.checkpoint(1)?;
            line.clear();
            loop {
                checkpoint.checkpoint(1)?;
                let available = reader.fill_buf().map_err(io)?;
                if available.is_empty() {
                    break;
                }
                let take = available
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(available.len(), |i| i + 1)
                    .min(WORK_BLOCK);
                if line.len() + take > 65536 {
                    return Err(integrity("closure record exceeds 64 KiB"));
                }
                let done = available[take - 1] == b'\n';
                line.extend_from_slice(&available[..take]);
                reader.consume(take);
                if done {
                    break;
                }
            }
            if line.is_empty() {
                break;
            }
            let item = serde_json::from_slice::<Capture>(&line);
            let Capture::Captured { artifact, length } = item.map_err(json)? else {
                return Err(integrity("invalid closure record"));
            };
            self.promote(&stage, &artifact, Some(length), checkpoint)?;
        }
        let manifest: ArtifactId = prepared.revision.as_str().parse()?;
        self.promote(&stage, &manifest, None, checkpoint)?;
        // Check the small manifest header without allocating its inventory.
        #[derive(Deserialize)]
        struct Header {
            schema: u32,
            project: ProjectId,
            parent: Option<RevisionId>,
        }
        let file = File::open(stage.object_path(&manifest)).map_err(io)?;
        let remaining = file.metadata().map_err(io)?.len();
        let parsed = serde_json::from_reader::<_, Header>(BufReader::new(CheckedReader {
            file,
            remaining,
            control: checkpoint,
            failure: &mut failure,
        }));
        if let Some(error) = failure {
            return Err(error);
        }
        checkpoint.checkpoint(0)?;
        let header = parsed.map_err(json)?;
        if header.schema != 1
            || header.project != prepared.project
            || header.parent != prepared.parent
        {
            return Err(integrity("receipt and manifest header disagree"));
        }
        sync_dir(&self.project.root.join("objects"))?;
        checkpoint.checkpoint(0)?;
        Ok(RetainedImport {
            run: run.id.clone(),
            prepared: prepared.clone(),
        })
    }
    pub(crate) fn promote(
        &self,
        stage: &Staging,
        id: &ArtifactId,
        length: Option<u64>,
        checkpoint: &mut dyn RunControl,
    ) -> Result<()> {
        let mut position = checkpoint.position();
        position.artifact(id);
        checkpoint.set_position(position);
        let source = stage.object_path(id);
        verify_file(&source, id, length, checkpoint)?;
        let destination = self.project.object_path(id);
        match fs::hard_link(&source, &destination) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                verify_file(&destination, id, length, checkpoint)
            }
            Err(e) => Err(io(e)),
        }
    }

    /// Caller linearizes cancellation before entering this transaction. No more
    /// cancellation is admitted until its durable outcome is known.
    pub fn publish_run(&mut self, record: &mut RunRecord, retained: RetainedImport) -> Result<()> {
        if record.operation != RunOperation::Import || retained.run != record.id {
            return Err(integrity("candidate belongs to another run"));
        }
        let prepared = retained.prepared;
        let mut connection = open_connection(&self.project.root, true)?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db)?;
        let current: Option<String> = tx
            .query_row(
                "SELECT current_revision FROM project WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(db)?;
        if current.as_deref() != record.base.as_ref().map(RevisionId::as_str) {
            return Err(Error::new(ErrorCode::Busy, "revision parent is stale"));
        }
        let mut completed = record.clone();
        completed.state = RunState::Completed;
        completed.revision = Some(prepared.revision.clone());
        completed.assessment = Some(ResultAssessment::covered(
            CoverageSubject::Inventory(prepared.revision.clone()),
            prepared.complete,
        ));
        tx.execute(
            "INSERT INTO revisions(id) VALUES(?1)",
            [prepared.revision.as_str()],
        )
        .map_err(db)?;
        tx.execute(
            "UPDATE project SET current_revision=?1 WHERE singleton=1",
            [prepared.revision.as_str()],
        )
        .map_err(db)?;
        tx.execute(
            "UPDATE runs SET record=?2 WHERE id=?1",
            params![
                record.id.as_str(),
                serde_json::to_string(&completed).map_err(json)?
            ],
        )
        .map_err(db)?;
        tx.commit().map_err(db)?;
        *record = completed;
        Ok(())
    }

    pub fn cleanup_stage(&self, id: &RunId) -> Result<()> {
        self.cleanup_stage_with(id, &|_| Ok(()))
    }

    fn cleanup_stage_with(&self, id: &RunId, reclaim: &dyn Fn(&Path) -> Result<()>) -> Result<()> {
        let stage = self.stage_path(id);
        if !stage.exists() {
            return Ok(());
        }
        let lease = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(stage.join("lease.lock"))
        {
            Ok(lease) => lease,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && !stage.join("guard.json").exists() =>
            {
                // Registered before directory creation; no guard can launch before
                // both its configuration and lease exist. This is interrupted setup.
                return fs::remove_dir_all(&stage).map_err(io);
            }
            Err(error) => return Err(io(error)),
        };
        lease
            .try_lock()
            .map_err(|_| Error::new(ErrorCode::Busy, "worker still holds staging lease"))?;
        reclaim(&stage)?;
        fs::remove_dir_all(&stage).map_err(io)
    }

    /// Requires exclusive writer ownership and host-specific process identity checks.
    pub fn recover(
        &mut self,
        alive: &dyn Fn(&OwnerIdentity) -> Result<bool>,
        reclaim: &dyn Fn(&Path) -> Result<()>,
    ) -> Result<Vec<RunRecord>> {
        let mut recovered = Vec::new();
        for mut run in self.project.runs()? {
            if !run.state.terminal() && alive(&run.owner)? {
                return Err(Error::new(
                    ErrorCode::Busy,
                    "recorded run owner is still alive",
                ));
            }
            // Read only after acquiring the same lease that excludes a live guard.
            let stage = self.stage_path(&run.id);
            if stage.exists() && stage.join("lease.lock").exists() {
                let lease = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(stage.join("lease.lock"))
                    .map_err(io)?;
                lease
                    .try_lock()
                    .map_err(|_| Error::new(ErrorCode::Busy, "worker still holds staging lease"))?;
                if let Some(diagnostics) = &mut run.diagnostics {
                    match read_progress(&stage, &run.id) {
                        Ok(Some(progress)) => diagnostics.progress = Some(progress),
                        Ok(None) => (),
                        Err(error) => diagnostics.secondary(error),
                    }
                }
            }
            if !run.state.terminal() {
                run.state = RunState::Abandoned;
                run.error = Some(Error::new(
                    ErrorCode::RecoveryRequired,
                    "owner exited before terminal publication",
                ));
                self.update_run(&run)?;
                recovered.push(run.clone());
            }
            self.cleanup_stage_with(&run.id, reclaim)?;
        }
        Ok(recovered)
    }
}

fn verify_file(
    path: &Path,
    expected: &ArtifactId,
    length: Option<u64>,
    checkpoint: &mut dyn RunControl,
) -> Result<()> {
    let (actual, size) = hash_file(path, checkpoint)?;
    if actual != *expected || length.is_some_and(|n| n != size) {
        return Err(integrity(format!(
            "retained object {expected} failed integrity verification"
        )));
    }
    Ok(())
}

/// Shared current-format decoder for single-run, listing and recovery paths.
/// Other journal versions are rejected without conversion.
pub(crate) fn decode_run(raw: &str) -> Result<RunRecord> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(json)?;
    let schema = value.get("schema").and_then(serde_json::Value::as_u64);
    if schema != Some(37) {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported run record schema",
        ));
    }
    let record: RunRecord = serde_json::from_value(value).map_err(json)?;
    record.budget.validate()?;
    if record.diagnostics.is_none()
        || (record.state == RunState::Completed) != record.assessment.is_some()
    {
        return Err(integrity("invalid run assessment or diagnostics"));
    }
    if matches!(record.operation, RunOperation::Scenario { .. }) {
        if (record.state == RunState::Completed) != record.resolved_operation.is_some()
            || matches!(record.effective_operation(), RunOperation::Scenario { .. })
                && record.state == RunState::Completed
        {
            return Err(integrity("invalid scenario resolution"));
        }
    } else if record.resolved_operation.is_some() {
        return Err(integrity("unexpected scenario resolution"));
    }
    if let RunOperation::Scenario { request } = &record.operation
        && let Some(resolved) = &record.resolved_operation
    {
        let valid = match (request, &**resolved) {
            (
                ScenarioRequest::ProposeEffectContract { request },
                RunOperation::Knowledge { change },
            ) => {
                change.expected_base == request.expected_base
                    && change.actor == request.actor
                    && change.reason == request.reason
                    && matches!(&change.action, KnowledgeAction::Propose {proposal} if proposal.subject == request.subject
                    && proposal.occurrence == request.contract.vendor.occurrence
                    && matches!(&proposal.claim, KnowledgeClaim::EffectContract {contract} if **contract == request.contract))
            }

            (
                ScenarioRequest::ProposeProjection { request },
                RunOperation::Knowledge { change },
            ) => {
                change.expected_base == request.expected_base
                    && change.actor == request.actor
                    && change.reason == request.reason
                    && matches!(&change.action, KnowledgeAction::Propose {proposal} if proposal.subject == request.subject
                    && proposal.occurrence == request.projection.vendor.entry.occurrence
                    && matches!(&proposal.claim, KnowledgeClaim::LayoutProjection {projection} if **projection == request.projection))
            }

            (ScenarioRequest::ProposeCallPair { request }, RunOperation::Knowledge { change }) => {
                change.expected_base == request.expected_base
                    && change.actor == request.actor
                    && change.reason == request.reason
                    && matches!(&change.action, KnowledgeAction::Propose { proposal } if proposal.subject == request.subject
                        && proposal.occurrence == request.correspondence.vendor.occurrence
                        && matches!(&proposal.claim, KnowledgeClaim::CallPair { correspondence } if **correspondence == request.correspondence))
            }

            (ScenarioRequest::ProposeData { request }, RunOperation::Knowledge { change }) => {
                change.expected_base == request.expected_base
                    && change.actor == request.actor
                    && change.reason == request.reason
                    && matches!(&change.action, KnowledgeAction::Propose { proposal } if proposal.occurrence == request.occurrence && proposal.subject == request.subject && matches!(&proposal.claim, KnowledgeClaim::IntegerTable { selector: DataSelector::Section { .. }, purpose, applicability, .. } | KnowledgeClaim::PointerTable { selector: DataSelector::Section { .. }, purpose, applicability, .. } if proposal.claim.table_layout().as_ref() == Some(&request.layout) && purpose == &request.purpose && applicability == &request.applicability))
            }
            (ScenarioRequest::ProposeConstant { request }, RunOperation::Knowledge { change }) => {
                change.expected_base == request.expected_base
                    && change.actor == request.actor
                    && change.reason == request.reason
                    && matches!(&change.action, KnowledgeAction::Propose { proposal } if proposal.subject == request.subject && proposal.claim == (KnowledgeClaim::Constant { analysis: request.analysis.clone(), record: request.record, operand: request.operand.clone(), value: request.value, purpose: request.purpose.clone(), applicability: request.applicability.clone() }) && proposal.evidence == vec![EvidenceRef::Analysis { analysis: request.analysis.clone(), record: Some(request.record) }])
            }
            (
                ScenarioRequest::Investigate { request, .. },
                RunOperation::Investigate { revision, .. },
            ) => request.revision.as_ref() == Some(revision),
            (
                ScenarioRequest::Research { request },
                RunOperation::AnalyzeFunction { request: function },
            ) => {
                function.research.as_ref() == Some(&request.options)
                    && request.publication == request.options.publication
            }
            (ScenarioRequest::ProposeRegister { request }, RunOperation::Knowledge { change }) => {
                change.expected_base == request.expected_base
                    && change.actor == request.actor
                    && change.reason == request.reason
                    && matches!(&change.action, KnowledgeAction::Propose { proposal } if proposal.subject == request.subject && proposal.claim == (KnowledgeClaim::MmioRegister { register: request.register.clone() }) && proposal.evidence == vec![EvidenceRef::Analysis { analysis: request.analysis.clone(), record: None }])
            }
            (
                ScenarioRequest::Replay { producer, .. },
                RunOperation::Execute {
                    producer: resolved, ..
                },
            ) => producer == resolved,
            _ => false,
        };
        if !valid {
            return Err(integrity(
                "scenario resolution differs from admitted action",
            ));
        }
    }
    let published = [
        record.revision.is_some(),
        record.image.is_some(),
        record.analysis.is_some(),
        record.publication.is_some(),
        record.knowledge.is_some(),
        record.execution.is_some(),
        record.semantic_ir.is_some(),
    ]
    .into_iter()
    .filter(|v| *v)
    .count();
    if published != usize::from(record.state == RunState::Completed) {
        return Err(integrity("invalid durable result cardinality"));
    }
    if let Some(assessment) = &record.assessment {
        let subject = match record.effective_operation() {
            RunOperation::Import => record.revision.clone().map(CoverageSubject::Inventory),
            RunOperation::AnalyzeFunction { .. } => {
                record.analysis.clone().map(CoverageSubject::Function)
            }
            RunOperation::Investigate { .. } => record
                .publication
                .clone()
                .map(CoverageSubject::Investigation),
            RunOperation::Execute { .. } => {
                record.execution.clone().map(CoverageSubject::Execution)
            }
            _ => None,
        };
        if assessment.coverage.as_ref().map(|c| &c.subject) != subject.as_ref()
            || assessment.check.is_some()
            || (!matches!(record.effective_operation(), RunOperation::Execute { .. })
                && assessment.comparison.is_some())
        {
            return Err(integrity(
                "assessment does not describe the published result",
            ));
        }
    }
    if record.semantic_ir.is_some()
        && !matches!(record.effective_operation(), RunOperation::BuildIr { .. })
    {
        return Err(integrity("semantic IR result belongs to another operation"));
    }
    match record.effective_operation() {
        RunOperation::BuildIr { request } => {
            request.validate()?;
            if (record.state == RunState::Completed) != record.semantic_ir.is_some() {
                return Err(integrity("invalid semantic IR outcome"));
            }
        }
        RunOperation::Execute { request, producer } => {
            request.validate()?;
            if producer.executor.is_empty()
                || producer.environment.is_empty()
                || producer.verifier.is_empty()
                || record.revision.is_some()
                || record.image.is_some()
                || record.analysis.is_some()
                || record.publication.is_some()
                || record.knowledge.is_some()
                || (record.state == RunState::Completed) != record.execution.is_some()
                || (record.state == RunState::Completed && request.replacement.is_some())
                    != record
                        .assessment
                        .as_ref()
                        .is_some_and(|a| a.comparison.is_some())
            {
                return Err(integrity("invalid execution outcome"));
            }
        }
        RunOperation::Knowledge { .. }
            if record.revision.is_some()
                || record.image.is_some()
                || record.analysis.is_some()
                || record.publication.is_some()
                || (record.state == RunState::Completed) != record.knowledge.is_some() =>
        {
            return Err(integrity("invalid knowledge outcome"));
        }
        RunOperation::Investigate { .. }
            if record.revision.is_some()
                || record.image.is_some()
                || record.analysis.is_some()
                || (record.state == RunState::Completed) != record.publication.is_some() =>
        {
            return Err(integrity("invalid investigation outcome"));
        }
        RunOperation::Import if record.image.is_some() => {
            return Err(integrity("import carries an image result"));
        }
        RunOperation::PrepareImage { .. }
            if record.revision.is_some()
                || (record.state == RunState::Completed) != record.image.is_some() =>
        {
            return Err(integrity("invalid image run outcome"));
        }
        RunOperation::AnalyzeFunction { request }
            if request.revision.is_none()
                || record.revision.is_some()
                || record.image.is_some()
                || (record.state == RunState::Completed) != record.analysis.is_some() =>
        {
            return Err(integrity("invalid analysis outcome"));
        }
        RunOperation::Query => return Err(integrity("query was persisted in durable journal")),
        _ => (),
    }
    Ok(record)
}

/// Last atomically written worker checkpoint; absence is an unknown observation.
pub fn read_progress(stage: &Path, run: &RunId) -> Result<Option<RunProgress>> {
    use std::io::Read;
    let file = match File::open(stage.join("progress.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(error)),
    };
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() > 65536 {
        return Err(integrity("progress exceeds 64 KiB"));
    }
    let record: ProgressRecord = serde_json::from_slice(&bytes).map_err(json)?;
    if record.schema != 1 || record.run != *run {
        return Err(integrity("progress identity/version mismatch"));
    }
    Ok(Some(record.progress))
}
