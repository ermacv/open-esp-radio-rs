//! Immutable library publications. Membership stays streamed; one transaction
//! exposes children, the publication and its completed run together.
use super::*;
use sha2::{Digest, Sha256};
use std::io::Write;

pub struct InvestigationLease {
    pub manifest: InvestigationManifest,
    pub members: FileLease,
}
pub struct RetainedInvestigation {
    run: RunId,
    receipt: PreparedInvestigationReceipt,
    manifest: InvestigationManifest,
}
/// Hash canonical JSON directly, rejecting expansion before exceeding the record
/// envelope. A caller-provided large recipe/name cannot allocate a large buffer.
struct HashWriter<'a> {
    hash: &'a mut Sha256,
    remaining: usize,
    exceeded: bool,
}
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            self.exceeded = true;
            return Err(std::io::Error::other("investigation record exceeds 64 KiB"));
        }
        self.hash.update(bytes);
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn hash_json(hash: &mut Sha256, value: &impl serde::Serialize, limit: usize) -> Result<()> {
    let mut writer = HashWriter {
        hash,
        remaining: limit,
        exceeded: false,
    };
    let result = serde_json::to_writer(&mut writer, value);
    if writer.exceeded {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "investigation record exceeds 64 KiB",
        ));
    }
    result.map_err(jobs::json)
}
fn plan_id(recipe: &InvestigationRecipe) -> Result<InvestigationPlanId> {
    let mut hash = Sha256::new();
    hash_json(&mut hash, recipe, 65536)?;
    format!("{:x}", hash.finalize()).parse()
}
pub fn investigation_plan(recipe: InvestigationRecipe) -> Result<InvestigationPlan> {
    Ok(InvestigationPlan {
        id: plan_id(&recipe)?,
        recipe,
    })
}
pub fn validate_investigation_plan(plan: &InvestigationPlan) -> Result<()> {
    let r = &plan.recipe;
    if r.schema != 2 || r.policy != 3 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported investigation recipe",
        ));
    }
    if r.request.revision.is_none()
        || r.producer.decoder.is_empty()
        || r.producer.semantics.is_empty()
        || plan_id(r)? != plan.id
    {
        return Err(integrity("invalid investigation plan identity"));
    }
    Ok(())
}
/// Canonical entry digest shared by planning, execution and store validation.
#[derive(Default)]
pub struct EntryDigest {
    hash: Sha256,
    pub count: u64,
    pub functions: u64,
}
impl EntryDigest {
    pub fn include(&mut self, entry: &PlanEntry) -> Result<()> {
        hash_json(&mut self.hash, entry, 65535)?;
        self.hash.update(b"\n");
        self.count += 1;
        self.functions += u64::from(matches!(entry, PlanEntry::Function { .. }));
        Ok(())
    }
    pub fn finish(self) -> Result<ArtifactId> {
        format!("{:x}", self.hash.finalize()).parse()
    }
}
/// Bounded JSONL reading from a verified lease. Caller admits a fixed 1 MiB
/// envelope covering the 64 KiB line and one deserialized record at a time.
pub fn visit_jsonl<T: serde::de::DeserializeOwned>(
    source: &dyn ByteSource,
    control: &mut dyn RunControl,
    mut sink: impl FnMut(T, &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    let mut cursor = JsonlCursor::new(source)?;
    while let Some(value) = cursor.next(control)? {
        sink(value, control)?;
    }
    Ok(())
}
/// Pull reader for scoped object groups; same record bound as `visit_jsonl`.
pub struct JsonlCursor<'a> {
    source: &'a dyn ByteSource,
    offset: u64,
    chunk: [u8; WORK_BLOCK],
    start: usize,
    end: usize,
    line: Vec<u8>,
}
impl<'a> JsonlCursor<'a> {
    pub fn new(source: &'a dyn ByteSource) -> Result<Self> {
        let mut line = Vec::new();
        line.try_reserve_exact(65536).map_err(|_| {
            Error::new(ErrorCode::ResourceLimited, "JSONL buffer allocation failed")
        })?;
        Ok(Self {
            source,
            offset: 0,
            chunk: [0; WORK_BLOCK],
            start: 0,
            end: 0,
            line,
        })
    }
    pub fn next<T: serde::de::DeserializeOwned>(
        &mut self,
        c: &mut dyn RunControl,
    ) -> Result<Option<T>> {
        self.line.clear();
        loop {
            if self.start == self.end {
                if self.offset == self.source.len() {
                    return if self.line.is_empty() {
                        Ok(None)
                    } else {
                        serde_json::from_slice(&self.line)
                            .map(Some)
                            .map_err(jobs::json)
                    };
                }
                c.checkpoint(1)?;
                self.end = (self.source.len() - self.offset).min(WORK_BLOCK as u64) as usize;
                self.source
                    .read_at(self.offset, &mut self.chunk[..self.end], c)?;
                self.offset += self.end as u64;
                self.start = 0;
            }
            let byte = self.chunk[self.start];
            self.start += 1;
            if byte == b'\n' {
                return serde_json::from_slice(&self.line)
                    .map(Some)
                    .map_err(jobs::json);
            }
            if self.line.len() == 65535 {
                return Err(integrity("JSONL record exceeds 64 KiB"));
            }
            self.line.push(byte);
        }
    }
}

pub(crate) fn decode(
    source: &dyn ByteSource,
    control: &mut dyn RunControl,
) -> Result<InvestigationManifest> {
    if source.len() > 65536 {
        return Err(integrity("investigation manifest exceeds 64 KiB"));
    }
    let mut bytes = vec![0; source.len() as usize];
    source.read_at(0, &mut bytes, control)?;
    let manifest: InvestigationManifest = serde_json::from_slice(&bytes).map_err(jobs::json)?;
    if manifest.schema != 1 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported publication schema",
        ));
    }
    validate_investigation_plan(&manifest.plan)?;
    Ok(manifest)
}
fn verify_members(
    source: &dyn ByteSource,
    manifest: &InvestigationManifest,
    control: &mut dyn RunControl,
    mut child: impl FnMut(
        &FunctionAnalysisId,
        &FunctionRequest,
        &ArtifactId,
        bool,
        &mut dyn RunControl,
    ) -> Result<()>,
) -> Result<()> {
    let mut digest = EntryDigest::default();
    let mut coverage = InvestigationCoverage::default();
    visit_jsonl(source, control, |member: InvestigationMember, control| {
        digest.include(&member.entry)?;
        match (&member.entry, &member.outcome) {
            (
                PlanEntry::Function {
                    request, payload, ..
                },
                InvestigationOutcome::Analyzed { analysis, complete },
            ) => child(analysis, request, payload, *complete, control)?,
            (PlanEntry::Function { .. }, InvestigationOutcome::Blocked { error })
                if matches!(
                    error.code,
                    ErrorCode::NeedsExtent
                        | ErrorCode::Incompatible
                        | ErrorCode::InvalidRequest
                        | ErrorCode::Unavailable
                ) => {}
            (
                PlanEntry::Input { .. }
                | PlanEntry::Object { .. }
                | PlanEntry::Gap { .. }
                | PlanEntry::Image { .. }
                | PlanEntry::ImageGap { .. },
                InvestigationOutcome::Recorded,
            ) => (),
            _ => return Err(integrity("invalid investigation member outcome")),
        }
        coverage.include(&member);
        Ok(())
    })?;
    if digest.count != manifest.plan.recipe.entry_count
        || digest.functions != manifest.plan.recipe.functions
        || digest.finish()? != manifest.plan.recipe.entries
        || coverage != manifest.coverage
    {
        return Err(integrity(
            "publication membership differs from plan or coverage",
        ));
    }
    Ok(())
}
fn verify_child(
    manifest: &FunctionManifest,
    plan: &InvestigationPlan,
    request: &FunctionRequest,
    payload: &ArtifactId,
    complete: bool,
) -> Result<()> {
    let r = &manifest.recipe;
    if r.project != plan.recipe.project
        || Some(&r.revision) != plan.recipe.request.revision.as_ref()
        || request.revision.as_ref() != Some(&r.revision)
        || r.source != request.source
        || r.symbol != request.symbol
        || &r.payload != payload
        || r.decoder != plan.recipe.producer.decoder
        || r.semantics.as_ref() != Some(&plan.recipe.producer.semantics)
        || r.user_extent != request.extent.is_some()
        || request.extent.is_some_and(|e| e != r.extent)
        || complete
            != (manifest.coverage.complete() && manifest.semantics.is_some_and(|s| s.complete))
    {
        return Err(integrity(
            "publication child differs from admitted function",
        ));
    }
    Ok(())
}
impl Staging {
    pub fn staged_function(
        &self,
        id: &FunctionAnalysisId,
        control: &mut dyn RunControl,
    ) -> Result<FunctionManifest> {
        functions::decode(&self.open_payload(&id.as_str().parse()?, control)?, control)
    }
    pub fn investigation_receipt(
        &self,
        manifest: &InvestigationManifest,
        control: &mut dyn RunControl,
    ) -> Result<PreparedInvestigationReceipt> {
        validate_investigation_plan(&manifest.plan)?;
        let bytes = serde_json::to_vec(manifest).map_err(jobs::json)?;
        if bytes.len() > 65536 {
            return Err(integrity("publication manifest exceeds 64 KiB"));
        }
        let mut file = self.disk.temporary(&self.root.join("staging"))?;
        file.write_all(&bytes).map_err(io)?;
        let id = self.retain_temporary(file, control)?;
        Ok(PreparedInvestigationReceipt {
            schema: 1,
            project: manifest.plan.recipe.project.clone(),
            revision: manifest.plan.recipe.request.revision.clone().unwrap(),
            plan: manifest.plan.id.clone(),
            publication: id.as_str().parse()?,
        })
    }
}
impl Project {
    pub fn publications(
        &self,
        control: &mut dyn RunControl,
        sink: &mut dyn FnMut(&PublicationId, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        let conn = open_connection(&self.root, false)?;
        let schema: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db)?;
        if schema < 5 {
            return Ok(());
        }
        let mut query = conn
            .prepare("SELECT id FROM publications ORDER BY sequence")
            .map_err(db)?;
        let mut rows = query.query([]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            control.checkpoint(1)?;
            let id: String = row.get(0).map_err(db)?;
            sink(&id.parse()?, control)?;
        }
        Ok(())
    }
    pub fn publication(
        &self,
        id: &PublicationId,
        control: &mut dyn RunControl,
    ) -> Result<InvestigationLease> {
        use rusqlite::OptionalExtension;
        let conn = open_connection(&self.root, false)?;
        let schema: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db)?;
        if schema < 5 {
            return Err(Error::new(
                ErrorCode::NotFound,
                "project has no publications",
            ));
        }
        let revision: Option<String> = conn
            .query_row(
                "SELECT revision FROM publications WHERE id=?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        let revision =
            revision.ok_or_else(|| Error::new(ErrorCode::NotFound, "publication not found"))?;
        let manifest = decode(&self.open_payload(&id.as_str().parse()?, control)?, control)?;
        for reference in &manifest.plan.recipe.request.reviewed_extents {
            let entry = self.knowledge_entry(&reference.revision, &reference.assertion, control)?;
            if entry.state != AssertionState::Accepted
                || !matches!(entry.proposal.claim, KnowledgeClaim::FunctionExtent { .. })
                || manifest.plan.recipe.request.revision.as_ref()
                    != Some(&entry.proposal.occurrence.revision)
            {
                return Err(integrity(
                    "publication reviewed extent is not retained and accepted for its source revision",
                ));
            }
        }
        if manifest.plan.recipe.project != self.id
            || manifest
                .plan
                .recipe
                .request
                .revision
                .as_ref()
                .map(RevisionId::as_str)
                != Some(revision.as_str())
        {
            return Err(integrity("publication row and manifest disagree"));
        }
        self.open_payload(&revision.parse()?, control)?;
        let members = self.open_payload(&manifest.members, control)?;
        verify_members(
            &members,
            &manifest,
            control,
            |id, request, payload, complete, c| {
                let child = self.analysis(id, c)?;
                verify_child(&child.manifest, &manifest.plan, request, payload, complete)
            },
        )?;
        Ok(InvestigationLease { manifest, members })
    }
    pub fn investigation_status(
        &self,
        control: &mut dyn RunControl,
    ) -> Result<InvestigationStatus> {
        let mut conn = open_connection(&self.root, false)?;
        let tx = conn.transaction().map_err(db)?;
        let revision: Option<String> = tx
            .query_row(
                "SELECT current_revision FROM project WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(db)?;
        let schema: u32 = tx
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db)?;
        use rusqlite::OptionalExtension;
        let publication: Option<String> = if schema >= 5 {
            tx.query_row(
                "SELECT id FROM current_publication WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?
        } else {
            None
        };
        let revision = revision.map(|s| s.parse()).transpose()?;
        let publication: Option<PublicationId> = publication.map(|s| s.parse()).transpose()?;
        let manifest = publication
            .as_ref()
            .map(|id| decode(&self.open_payload(&id.as_str().parse()?, control)?, control))
            .transpose()?;
        let publication_revision = manifest
            .as_ref()
            .and_then(|m| m.plan.recipe.request.revision.clone());
        Ok(InvestigationStatus {
            knowledge: crate::knowledge::current(&tx)?,
            current: publication.is_some() && revision == publication_revision,
            revision,
            publication,
            publication_revision,
            coverage: manifest.map(|m| m.coverage),
        })
    }
}
impl Writer {
    pub fn retain_investigation(
        &self,
        run: &RunRecord,
        receipt: &PreparedInvestigationReceipt,
        plan: &InvestigationPlan,
        control: &mut dyn RunControl,
    ) -> Result<RetainedInvestigation> {
        let RunOperation::Investigate {
            revision,
            plan: admitted,
        } = run.effective_operation()
        else {
            return Err(integrity("publication belongs to another operation"));
        };
        if receipt.schema != 1
            || receipt.project != self.project.id
            || &receipt.revision != revision
            || &receipt.plan != admitted
            || &plan.id != admitted
        {
            return Err(integrity("publication receipt differs from admission"));
        }
        let stage = Staging::open(&self.stage_path(&run.id))?;
        let id = receipt.publication.as_str().parse()?;
        let manifest = decode(&stage.open_payload(&id, control)?, control)?;
        if &manifest.plan != plan
            || manifest.plan.recipe.project != receipt.project
            || manifest.plan.recipe.request.revision.as_ref() != Some(&receipt.revision)
        {
            return Err(integrity("publication plan differs from admission"));
        }
        self.project
            .open_payload(&revision.as_str().parse()?, control)?;
        let members = stage.open_payload(&manifest.members, control)?;
        verify_members(
            &members,
            &manifest,
            control,
            |id, request, payload, complete, c| {
                let artifact = id.as_str().parse()?;
                let child = functions::decode(&stage.open_payload(&artifact, c)?, c)?;
                verify_child(&child, plan, request, payload, complete)?;
                self.promote(&stage, &child.records, None, c)?;
                self.promote(&stage, &artifact, None, c)
            },
        )?;
        self.promote(&stage, &manifest.members, None, control)?;
        self.promote(&stage, &id, None, control)?;
        sync_dir(&self.project.root.join("objects"))?;
        Ok(RetainedInvestigation {
            run: run.id.clone(),
            receipt: receipt.clone(),
            manifest,
        })
    }
    pub fn publish_investigation(
        &mut self,
        run: &mut RunRecord,
        retained: RetainedInvestigation,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if retained.run != run.id
            || retained.receipt.project != self.project.id
            || !matches!(run.effective_operation(),RunOperation::Investigate{revision,plan} if revision==&retained.receipt.revision && plan==&retained.receipt.plan)
            || run.state != RunState::Validating
        {
            return Err(integrity("retained publication belongs to another run"));
        }
        let mut conn = open_connection(&self.project.root, true)?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db)?;
        let source = self
            .project
            .open_payload(&retained.manifest.members, control)?;
        visit_jsonl(&source, control, |member: InvestigationMember, c| {
            c.checkpoint(1)?;
            if let InvestigationOutcome::Analyzed { analysis, .. } = member.outcome {
                tx.execute(
                    "INSERT INTO analyses(id,revision) VALUES(?1,?2) ON CONFLICT(id) DO NOTHING",
                    params![analysis.as_str(), retained.receipt.revision.as_str()],
                )
                .map_err(db)?;
            }
            Ok(())
        })?;
        tx.execute(
            "INSERT INTO publications(id,revision) VALUES(?1,?2) ON CONFLICT(id) DO NOTHING",
            params![
                retained.receipt.publication.as_str(),
                retained.receipt.revision.as_str()
            ],
        )
        .map_err(db)?;
        tx.execute("INSERT INTO current_publication(singleton,id) SELECT 1,?1 FROM project WHERE current_revision=?2 ON CONFLICT(singleton) DO UPDATE SET id=excluded.id",params![retained.receipt.publication.as_str(),retained.receipt.revision.as_str()]).map_err(db)?;
        let mut completed = run.clone();
        completed.state = RunState::Completed;
        completed.publication = Some(retained.receipt.publication);
        completed.assessment = Some(ResultAssessment::covered(
            CoverageSubject::Investigation(completed.publication.clone().unwrap()),
            retained.manifest.coverage.complete(),
        ));
        control.checkpoint(0)?;
        if let Some(progress) = control.progress() {
            completed
                .diagnostics
                .get_or_insert_with(Default::default)
                .progress = Some(progress);
        }
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
