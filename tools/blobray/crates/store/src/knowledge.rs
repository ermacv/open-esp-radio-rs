//! Immutable review events. SQLite indexes publication order; CAS owns evidence.
use super::*;
use rusqlite::OptionalExtension;
use std::io::Write;

pub struct RetainedKnowledge {
    run: RunId,
    receipt: PreparedKnowledgeReceipt,
    manifest: KnowledgeManifest,
}
fn decode(source: &dyn ByteSource, control: &mut dyn RunControl) -> Result<KnowledgeManifest> {
    if source.len() > 65536 {
        return Err(integrity("knowledge manifest exceeds 64 KiB"));
    }
    let mut bytes = vec![0; source.len() as usize];
    source.read_at(0, &mut bytes, control)?;
    let value: KnowledgeManifest = serde_json::from_slice(&bytes).map_err(jobs::json)?;
    if value.schema != 2 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported knowledge manifest",
        ));
    }
    Ok(value)
}
pub(crate) fn current(connection: &Connection) -> Result<Option<KnowledgeRevisionId>> {
    let schema: u32 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db)?;
    if schema < 6 {
        return Ok(None);
    }
    let raw: Option<String> = connection
        .query_row(
            "SELECT id FROM knowledge_revisions ORDER BY sequence DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(db)?;
    raw.map(|s| s.parse()).transpose()
}
fn sequence(connection: &Connection, id: &KnowledgeRevisionId) -> Result<i64> {
    connection
        .query_row(
            "SELECT sequence FROM knowledge_revisions WHERE id=?1",
            [id.as_str()],
            |r| r.get(0),
        )
        .optional()
        .map_err(db)?
        .ok_or_else(|| Error::new(ErrorCode::NotFound, "knowledge revision not retained"))
}
impl Project {
    pub fn current_knowledge(&self) -> Result<Option<KnowledgeRevisionId>> {
        current(&open_connection(&self.root, false)?)
    }
    pub fn check_knowledge_base(&self, base: &Option<KnowledgeRevisionId>) -> Result<()> {
        if &self.current_knowledge()? != base {
            return Err(Error::new(
                ErrorCode::Conflict,
                "knowledge base is stale; review against an explicit current revision",
            ));
        }
        Ok(())
    }
    pub fn knowledge_manifest(
        &self,
        id: &KnowledgeRevisionId,
        control: &mut dyn RunControl,
    ) -> Result<KnowledgeManifest> {
        let connection = open_connection(&self.root, false)?;
        sequence(&connection, id)?;
        let value = decode(&self.open_payload(&id.as_str().parse()?, control)?, control)?;
        if value.project != self.id {
            return Err(integrity("knowledge belongs to another project"));
        }
        let row: (Option<String>, String, String, Option<String>) = connection
            .query_row(
                "SELECT parent,assertion,action,supersedes FROM knowledge_revisions WHERE id=?1",
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(db)?;
        let (action, replaced) = action_fields(&value.change.action);
        if row.0.as_deref()
            != value
                .change
                .expected_base
                .as_ref()
                .map(KnowledgeRevisionId::as_str)
            || row.1 != value.assertion.as_str()
            || row.2 != action
            || row.3.as_deref() != replaced
        {
            return Err(integrity("knowledge index and immutable event disagree"));
        }
        for root in &value.evidence_roots {
            self.open_payload(root, control)?;
        }
        Ok(value)
    }
    /// None denotes the frozen empty history. Caller resolves the current head at admission.
    pub fn knowledge_history(
        &self,
        at: Option<&KnowledgeRevisionId>,
        control: &mut dyn RunControl,
        sink: &mut dyn FnMut(
            &KnowledgeRevisionId,
            &KnowledgeManifest,
            &mut dyn RunControl,
        ) -> Result<()>,
    ) -> Result<()> {
        let Some(at) = at else { return Ok(()) };
        let connection = open_connection(&self.root, false)?;
        let cutoff = sequence(&connection, at)?;
        let mut query = connection
            .prepare("SELECT id FROM knowledge_revisions WHERE sequence<=?1 ORDER BY sequence")
            .map_err(db)?;
        let mut rows = query.query([cutoff]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            control.checkpoint(1)?;
            let id: KnowledgeRevisionId = row.get::<_, String>(0).map_err(db)?.parse()?;
            let manifest = self.knowledge_manifest(&id, control)?;
            sink(&id, &manifest, control)?;
        }
        Ok(())
    }
    pub fn knowledge_entry(
        &self,
        at: &KnowledgeRevisionId,
        id: &AssertionId,
        control: &mut dyn RunControl,
    ) -> Result<KnowledgeEntry> {
        let connection = open_connection(&self.root, false)?;
        let cutoff = sequence(&connection, at)?;
        let raw:Option<String>=connection.query_row("SELECT id FROM knowledge_revisions WHERE assertion=?1 AND action='propose' AND sequence<=?2 ORDER BY sequence LIMIT 1",params![id.as_str(),cutoff],|r|r.get(0)).optional().map_err(db)?;
        let proposed_in: KnowledgeRevisionId = raw
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    "assertion not proposed at selected knowledge revision",
                )
            })?
            .parse()?;
        let manifest = self.knowledge_manifest(&proposed_in, control)?;
        let KnowledgeAction::Propose { proposal } = manifest.change.action else {
            return Err(integrity("proposal index points at review"));
        };
        let mut entry = KnowledgeEntry {
            id: id.clone(),
            proposal,
            proposed_in: proposed_in.clone(),
            last_revision: proposed_in,
            state: AssertionState::Proposed,
        };
        let mut query=connection.prepare("SELECT id,assertion,action,supersedes FROM knowledge_revisions WHERE sequence<=?2 AND (assertion=?1 OR supersedes=?1) ORDER BY sequence").map_err(db)?;
        let mut rows = query.query(params![id.as_str(), cutoff]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            control.checkpoint(1)?;
            let raw: String = row.get(0).map_err(db)?;
            let action: String = row.get(2).map_err(db)?;
            let replaced: Option<String> = row.get(3).map_err(db)?;
            entry.last_revision = raw.parse()?;
            self.knowledge_manifest(&entry.last_revision, control)?;
            entry.state = if replaced.as_deref() == Some(id.as_str()) {
                AssertionState::Superseded
            } else {
                match action.as_str() {
                    "propose" => AssertionState::Proposed,
                    "accept" => AssertionState::Accepted,
                    "reject" => AssertionState::Rejected,
                    _ => return Err(integrity("unknown review action")),
                }
            };
        }
        Ok(entry)
    }
    /// Materialize a frozen review history once. Every event and evidence root is
    /// verified before the snapshot can be used, including superseded proposals.
    pub fn knowledge_snapshot<'a>(
        &self,
        at: Option<&KnowledgeRevisionId>,
        memory: &'a WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<KnowledgeSnapshot<'a>> {
        let _scratch = memory.reserve(1024 * 1024, control.position())?;
        let mut slots = AdmittedVec::new(memory);
        if let Some(at) = at {
            let connection = open_connection(&self.root, false)?;
            let cutoff = sequence(&connection, at)?;
            let mut query = connection.prepare("SELECT assertion FROM knowledge_revisions WHERE action='propose' AND sequence<=?1 ORDER BY assertion").map_err(db)?;
            let mut rows = query.query([cutoff]).map_err(db)?;
            while let Some(row) = rows.next().map_err(db)? {
                control.checkpoint(1)?;
                let capacity = memory.reserve(64, control.position())?;
                let id: AssertionId = row.get::<_, String>(0).map_err(db)?.parse()?;
                slots.push((id, None, capacity), control.position())?;
            }
        }
        control.measure(WorkMetric::KnowledgeHistoryPasses, 1);
        self.knowledge_history(at, control, &mut |revision, manifest, control| {
            control.checkpoint((slots.len().max(1).ilog2() + 1) as u64)?;
            let index = slots
                .binary_search_by(|(id, _, _)| id.cmp(&manifest.assertion))
                .map_err(|_| integrity("history refers to an absent proposal"))?;
            match &manifest.change.action {
                KnowledgeAction::Propose { proposal } => {
                    if slots[index].1.is_some() {
                        return Err(integrity("duplicate assertion proposal"));
                    }
                    let capacity = memory
                        .reserve(proposal_heap_bytes(proposal) + 3 * 64, control.position())?;
                    slots[index].1 = Some((
                        KnowledgeEntry {
                            id: manifest.assertion.clone(),
                            proposal: proposal.clone(),
                            proposed_in: revision.clone(),
                            last_revision: revision.clone(),
                            state: AssertionState::Proposed,
                        },
                        capacity,
                    ));
                }
                KnowledgeAction::Review {
                    decision,
                    supersedes,
                    ..
                } => {
                    let (entry, _) = slots[index]
                        .1
                        .as_mut()
                        .ok_or_else(|| integrity("review precedes proposal"))?;
                    entry.last_revision = revision.clone();
                    entry.state = match decision {
                        ReviewDecision::Accept => AssertionState::Accepted,
                        ReviewDecision::Reject => AssertionState::Rejected,
                    };
                    if let Some(replaced) = supersedes {
                        control.checkpoint((slots.len().max(1).ilog2() + 1) as u64)?;
                        let index = slots
                            .binary_search_by(|(id, _, _)| id.cmp(replaced))
                            .map_err(|_| integrity("superseded assertion missing"))?;
                        let (entry, _) = slots[index]
                            .1
                            .as_mut()
                            .ok_or_else(|| integrity("supersession precedes proposal"))?;
                        entry.last_revision = revision.clone();
                        entry.state = AssertionState::Superseded;
                    }
                }
            }
            Ok(())
        })?;
        Ok(KnowledgeSnapshot { slots })
    }
    pub fn knowledge_entries(
        &self,
        at: Option<&KnowledgeRevisionId>,
        control: &mut dyn RunControl,
        sink: &mut dyn FnMut(&KnowledgeEntry, &mut dyn RunControl) -> Result<()>,
    ) -> Result<KnowledgeStatus> {
        let mut status = KnowledgeStatus {
            revision: at.cloned(),
            proposed: 0,
            accepted: 0,
            rejected: 0,
            superseded: 0,
        };
        let Some(at) = at else { return Ok(status) };
        let connection = open_connection(&self.root, false)?;
        let cutoff = sequence(&connection, at)?;
        let mut query=connection.prepare("SELECT assertion FROM knowledge_revisions WHERE action='propose' AND sequence<=?1 ORDER BY sequence").map_err(db)?;
        let mut rows = query.query([cutoff]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            control.checkpoint(1)?;
            let id: AssertionId = row.get::<_, String>(0).map_err(db)?.parse()?;
            let entry = self.knowledge_entry(at, &id, control)?;
            match entry.state {
                AssertionState::Proposed => status.proposed += 1,
                AssertionState::Accepted => status.accepted += 1,
                AssertionState::Rejected => status.rejected += 1,
                AssertionState::Superseded => status.superseded += 1,
            };
            sink(&entry, control)?;
        }
        Ok(status)
    }
}
fn action_fields(action: &KnowledgeAction) -> (&'static str, Option<&str>) {
    match action {
        KnowledgeAction::Propose { .. } => ("propose", None),
        KnowledgeAction::Review {
            decision,
            supersedes,
            ..
        } => (
            match decision {
                ReviewDecision::Accept => "accept",
                ReviewDecision::Reject => "reject",
            },
            supersedes.as_ref().map(AssertionId::as_str),
        ),
    }
}
impl Staging {
    pub fn knowledge_receipt(
        &self,
        manifest: &KnowledgeManifest,
        control: &mut dyn RunControl,
    ) -> Result<PreparedKnowledgeReceipt> {
        let bytes = serde_json::to_vec(manifest).map_err(jobs::json)?;
        if bytes.len() > 65536 {
            return Err(integrity("knowledge manifest exceeds 64 KiB"));
        }
        let mut file = self.disk.temporary(&self.root.join("staging"))?;
        file.write_all(&bytes).map_err(io)?;
        let id = self.retain_temporary(file, control)?;
        Ok(PreparedKnowledgeReceipt {
            schema: 1,
            project: manifest.project.clone(),
            revision: id.as_str().parse()?,
        })
    }
}
impl Writer {
    pub fn retain_knowledge(
        &self,
        run: &RunRecord,
        receipt: &PreparedKnowledgeReceipt,
        control: &mut dyn RunControl,
    ) -> Result<RetainedKnowledge> {
        let RunOperation::Knowledge { change } = run.effective_operation() else {
            return Err(integrity("knowledge receipt belongs to another operation"));
        };
        self.project.check_knowledge_base(&change.expected_base)?;
        if receipt.schema != 1 || receipt.project != self.project.id {
            return Err(integrity("knowledge receipt identity differs"));
        }
        let stage = Staging::open(&self.stage_path(&run.id))?;
        let id: ArtifactId = receipt.revision.as_str().parse()?;
        let manifest = decode(&stage.open_payload(&id, control)?, control)?;
        if manifest.project != receipt.project || &manifest.change != change {
            return Err(integrity("knowledge event differs from admitted change"));
        }
        if let KnowledgeAction::Review { assertion, .. } = &change.action
            && assertion != &manifest.assertion
        {
            return Err(integrity("reviewed assertion differs"));
        }
        for root in &manifest.evidence_roots {
            self.project.open_payload(root, control)?;
        }
        self.promote(&stage, &id, None, control)?;
        sync_dir(&self.project.root.join("objects"))?;
        Ok(RetainedKnowledge {
            run: run.id.clone(),
            receipt: receipt.clone(),
            manifest,
        })
    }
    pub fn publish_knowledge(
        &mut self,
        run: &mut RunRecord,
        retained: RetainedKnowledge,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if retained.run != run.id || run.state != RunState::Validating {
            return Err(integrity(
                "knowledge publication requires its validating run",
            ));
        }
        let mut connection = open_connection(&self.project.root, true)?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db)?;
        if current(&tx)? != retained.manifest.change.expected_base {
            return Err(Error::new(
                ErrorCode::Conflict,
                "knowledge head changed before publication",
            ));
        }
        let (action, replaced) = action_fields(&retained.manifest.change.action);
        tx.execute("INSERT INTO knowledge_revisions(id,parent,assertion,action,supersedes) VALUES(?1,?2,?3,?4,?5)",params![retained.receipt.revision.as_str(),retained.manifest.change.expected_base.as_ref().map(KnowledgeRevisionId::as_str),retained.manifest.assertion.as_str(),action,replaced]).map_err(db)?;
        control.checkpoint(1)?;
        let mut completed = run.clone();
        completed.state = RunState::Completed;
        completed.knowledge = Some(retained.receipt.revision);
        completed.assessment = Some(ResultAssessment::default());
        if let Some(progress) = control.progress() {
            completed.diagnostics.as_mut().unwrap().progress = Some(progress);
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

/// Operation-local materialization; reservations live as long as the owned entries.
type KnowledgeSlot<'a> = (
    AssertionId,
    Option<(KnowledgeEntry, MemoryReservation<'a>)>,
    MemoryReservation<'a>,
);
pub struct KnowledgeSnapshot<'a> {
    slots: AdmittedVec<'a, KnowledgeSlot<'a>>,
}
impl KnowledgeSnapshot<'_> {
    pub fn entries(&self) -> impl Iterator<Item = &KnowledgeEntry> {
        self.slots
            .iter()
            .filter_map(|(_, entry, _)| entry.as_ref().map(|(entry, _)| entry))
    }
}
fn proposal_heap_bytes(p: &KnowledgeProposal) -> u64 {
    let claim = match &p.claim {
        KnowledgeClaim::IntegerTable {
            purpose,
            applicability,
            ..
        }
        | KnowledgeClaim::PointerTable {
            purpose,
            applicability,
            ..
        } => purpose.len() + applicability.len() + 64,
        KnowledgeClaim::Constant {
            purpose,
            applicability,
            ..
        } => purpose.len() + applicability.len() + 64,
        KnowledgeClaim::MmioRegister { register } => {
            register.name.len()
                + register.fields.len() * std::mem::size_of::<MmioField>()
                + register.fields.iter().map(|f| f.name.len()).sum::<usize>()
        }
        KnowledgeClaim::MmioRegion { region } => region.name.len(),
        KnowledgeClaim::Name { name } => name.len(),
        KnowledgeClaim::Hypothesis { text } => text.len(),
        KnowledgeClaim::Binding
        | KnowledgeClaim::FunctionExtent { .. }
        | KnowledgeClaim::ExecutableRange { .. } => 0,
    };
    (p.subject.as_str().len()
        + 2 * 64
        + usize::from(matches!(p.occurrence.source, FunctionSource::Image { .. })) * 64
        + usize::from(p.occurrence.symbol.is_some()) * 64
        + claim
        + p.evidence.len() * (std::mem::size_of::<EvidenceRef>() + 64)
        + p.note.as_ref().map_or(0, String::len)) as u64
}
