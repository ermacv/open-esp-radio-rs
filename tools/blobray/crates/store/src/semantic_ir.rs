//! Durable indexes over immutable original semantic streams; no analysis authority.
use super::*;
use std::io::Write;

pub struct SemanticIrLease<'a> {
    _capacity: MemoryReservation<'a>,
    pub manifest: SemanticIrManifest,
    pub records: FileLease,
}
pub struct RetainedIr {
    run: RunId,
    id: ArtifactId,
}
fn decode(source: &dyn ByteSource, c: &mut dyn RunControl) -> Result<SemanticIrManifest> {
    if source.len() > 65536 {
        return Err(integrity("semantic IR manifest exceeds 64 KiB"));
    }
    let mut bytes = vec![0; source.len() as usize];
    source.read_at(0, &mut bytes, c)?;
    let manifest: SemanticIrManifest = serde_json::from_slice(&bytes).map_err(jobs::json)?;
    if manifest.schema != SEMANTIC_IR_SCHEMA || manifest.policy != SEMANTIC_IR_POLICY {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported semantic IR format",
        ));
    }
    manifest.request.validate()?;
    if manifest.profiles.len() != manifest.request.profiles.len()
        || manifest
            .profiles
            .iter()
            .zip(&manifest.request.profiles)
            .any(|(result, request)| {
                result.name != request.name
                    || result.roots == 0
                    || result.roots > result.functions
                    || result.partial_functions > result.functions
            })
    {
        return Err(integrity(
            "semantic IR profile summaries differ from request",
        ));
    }
    Ok(manifest)
}
impl Project {
    pub fn semantic_ir<'a>(
        &self,
        id: &ArtifactId,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<SemanticIrLease<'a>> {
        use rusqlite::OptionalExtension;
        let connection = open_connection(&self.root, false)?;
        c.checkpoint(1)?;
        let sequence: Option<i64> = connection.query_row(
            "SELECT runs.sequence FROM semantic_ir JOIN runs ON runs.id=semantic_ir.run WHERE semantic_ir.id=?1",
            [id.as_str()], |r| r.get(0)).optional().map_err(db)?;
        let sequence = sequence.ok_or_else(|| {
            Error::new(
                ErrorCode::NotFound,
                "semantic IR not published or index has no run",
            )
        })?;
        let blob = connection
            .blob_open(rusqlite::MAIN_DB, "runs", "record", sequence, true)
            .map_err(db)?;
        let _run_capacity = memory.reserve(
            (blob.len() as u64)
                .checked_mul(64)
                .and_then(|n| n.checked_add(4096))
                .ok_or_else(|| integrity("IR run capacity overflow"))?,
            c.position(),
        )?;
        let mut raw = memory.bytes(blob.len(), c.position())?;
        for (i, chunk) in raw.chunks_mut(WORK_BLOCK).enumerate() {
            c.bytes(chunk.len())?;
            blob.read_at_exact(chunk, i * WORK_BLOCK).map_err(db)?;
        }
        let run = jobs::decode_run(
            std::str::from_utf8(&raw).map_err(|_| integrity("IR run is not UTF-8"))?,
        )?;
        let identity = connection
            .blob_open(rusqlite::MAIN_DB, "runs", "id", sequence, true)
            .map_err(db)?;
        if identity.len() != 64 {
            return Err(integrity("IR run identity length differs"));
        }
        let mut run_id = [0; 64];
        identity.read_at_exact(&mut run_id, 0).map_err(db)?;
        if run_id != run.id.as_str().as_bytes()
            || run.state != RunState::Completed
            || run.semantic_ir.as_ref() != Some(id)
            || run.assessment != Some(ResultAssessment::default())
        {
            return Err(integrity("IR index does not identify a completed build"));
        }
        let RunOperation::BuildIr { request } = run.effective_operation() else {
            return Err(integrity("IR index identifies another operation"));
        };
        let source = self.open_payload(id, c)?;
        let capacity = memory.reserve(8 * source.len().min(65536) + 65536, c.position())?;
        let manifest = decode(&source, c)?;
        if &manifest.request != request {
            return Err(integrity("IR manifest differs from admitted request"));
        }
        if manifest.project != self.id {
            return Err(integrity("foreign semantic IR project"));
        }
        let records = self.open_payload(&manifest.records, c)?;
        validate_ir_records(self, &manifest, &records, memory, c)?;
        Ok(SemanticIrLease {
            _capacity: capacity,
            manifest,
            records,
        })
    }
}
impl Staging {
    pub fn ir_receipt(
        &self,
        manifest: &SemanticIrManifest,
        c: &mut dyn RunControl,
    ) -> Result<PreparedIrReceipt> {
        let bytes = serde_json::to_vec(manifest).map_err(jobs::json)?;
        if bytes.len() > 65536 {
            return Err(integrity("semantic IR manifest exceeds 64 KiB"));
        }
        let mut output = self.disk.temporary(&self.root.join("staging"))?;
        output.write_all(&bytes).map_err(io)?;
        Ok(PreparedIrReceipt {
            schema: 1,
            project: manifest.project.clone(),
            semantic_ir: self.retain_temporary(output, c)?,
        })
    }
}
impl Writer {
    pub fn retain_ir(
        &self,
        run: &RunRecord,
        receipt: &PreparedIrReceipt,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<RetainedIr> {
        let RunOperation::BuildIr { request } = run.effective_operation() else {
            return Err(integrity(
                "semantic IR receipt belongs to another operation",
            ));
        };
        let stage = Staging::open(&self.stage_path(&run.id))?;
        let source = stage.open_payload(&receipt.semantic_ir, c)?;
        let _capacity = memory.reserve(8 * source.len().min(65536) + 65536, c.position())?;
        let manifest = decode(&source, c)?;
        if receipt.schema != 1
            || receipt.project != self.project.id
            || manifest.project != self.project.id
            || &manifest.request != request
        {
            return Err(integrity("semantic IR receipt differs from admission"));
        }
        validate_ir_records(
            &self.project,
            &manifest,
            &stage.open_payload(&manifest.records, c)?,
            memory,
            c,
        )?;
        for id in [&manifest.records, &receipt.semantic_ir] {
            self.promote(&stage, id, None, c)?;
        }
        sync_dir(&self.project.root.join("objects"))?;
        Ok(RetainedIr {
            run: run.id.clone(),
            id: receipt.semantic_ir.clone(),
        })
    }
    pub fn publish_ir(&mut self, run: &mut RunRecord, retained: RetainedIr) -> Result<()> {
        if run.id != retained.run
            || !matches!(run.effective_operation(), RunOperation::BuildIr { .. })
        {
            return Err(integrity("semantic IR retained for another run"));
        }
        let mut connection = open_connection(&self.project.root, true)?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db)?;
        let mut completed = run.clone();
        completed.state = RunState::Completed;
        completed.semantic_ir = Some(retained.id.clone());
        completed.assessment = Some(ResultAssessment::default());
        tx.execute(
            "INSERT OR IGNORE INTO semantic_ir(id,run) VALUES (?1,?2)",
            params![retained.id.as_str(), run.id.as_str()],
        )
        .map_err(db)?;
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

/// Validate retained identities, profile membership and original streams. Semantic
/// target resolution remains the application's responsibility, never reimplemented here.
pub fn validate_ir_records(
    project: &Project,
    manifest: &SemanticIrManifest,
    source: &dyn ByteSource,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    let _envelope = memory.reserve(1024 * 1024, c.position())?;
    let mut reader = project.analysis_reader(memory);
    let knowledge =
        project.knowledge_snapshot(manifest.request.scope.knowledge.as_ref(), memory, c)?;
    let mut ids = AdmittedVec::new(memory);
    let mut counts = vec![(0u64, 0u64, 0u64); manifest.profiles.len()];
    let (mut records, mut selected, mut provenance, mut unavailable) = (0u64, 0u64, 0u64, 0u64);
    visit_jsonl(source, c, |record: SemanticIrRecord, c| {
        records += 1;
        match record {
            SemanticIrRecord::Function {
                function,
                manifest: saved,
                profiles,
                roots,
                provenance_only,
                name,
            } => {
                let index_valid = |v: &[u8]| {
                    v.iter().all(|i| usize::from(*i) < counts.len())
                        && v.windows(2).all(|w| w[0] < w[1])
                };
                if !index_valid(&profiles)
                    || !index_valid(&roots)
                    || roots.iter().any(|i| !profiles.contains(i))
                    || provenance_only != profiles.is_empty()
                    || saved.recipe.project != manifest.project
                    || saved.recipe.revision != manifest.request.scope.revision
                    || function.location.source != saved.recipe.source
                    || function.location.selector != saved.recipe.selector
                {
                    return Err(integrity("invalid semantic IR function membership"));
                }
                for (i, profile) in manifest.request.profiles.iter().enumerate() {
                    c.checkpoint(1)?;
                    let is_root = !provenance_only
                        && match &profile.roots {
                            IrRoots::All => true,
                            IrRoots::Analyses { analyses } => analyses.contains(&function.analysis),
                            IrRoots::NamePrefix { prefix } => {
                                name.as_ref().is_some_and(|n| n.starts_with(prefix))
                            }
                        };
                    if roots.contains(&(i as u8)) != is_root
                        || !profile.include_reachable && profiles.contains(&(i as u8)) != is_root
                    {
                        return Err(integrity(
                            "IR root membership differs from configured selectors",
                        ));
                    }
                }
                let original = reader.analysis(&function.analysis, c)?;
                if original.manifest != *saved {
                    return Err(integrity("semantic IR changed original function manifest"));
                }
                let capacity = memory.reserve(function.analysis.allocated_bytes(), c.position())?;
                let mask = profiles.iter().fold(0u32, |m, i| m | (1 << i));
                ids.push((function.analysis, mask, capacity), c.position())?;
                selected += u64::from(!provenance_only);
                provenance += u64::from(provenance_only);
                for i in profiles {
                    counts[usize::from(i)].1 += 1;
                    counts[usize::from(i)].2 += u64::from(
                        !saved.coverage.complete() || saved.semantics.is_none_or(|s| !s.complete),
                    );
                }
                for i in roots {
                    counts[usize::from(i)].0 += 1;
                }
            }
            SemanticIrRecord::Call { record }
                if matches!(*record, NavigationRecord::Call { .. }) => {}
            SemanticIrRecord::Unavailable { record }
                if matches!(*record, NavigationRecord::Unavailable { .. }) =>
            {
                unavailable += 1;
            }
            SemanticIrRecord::Knowledge { entry } => {
                if knowledge.get(&entry.id, c)? != Some(&*entry) {
                    return Err(integrity(
                        "semantic IR changed or added unselected reviewed knowledge",
                    ));
                }
            }
            _ => return Err(integrity("invalid record in semantic IR index")),
        }
        Ok(())
    })?;
    c.checkpoint(ids.len() as u64 * (ids.len().max(1).ilog2() as u64 + 1))?;
    ids.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    if ids.windows(2).any(|w| w[0].0 == w[1].0)
        || records != manifest.record_count
        || selected != manifest.functions
        || provenance != manifest.provenance_functions
        || unavailable != manifest.unavailable_entries
        || counts
            .iter()
            .zip(&manifest.profiles)
            .any(|((roots, functions, partial), p)| {
                (*roots, *functions, *partial) != (p.roots, p.functions, p.partial_functions)
            })
    {
        return Err(integrity("semantic IR index and summary disagree"));
    }
    let member = |id: &FunctionAnalysisId, c: &mut dyn RunControl| -> Result<u32> {
        c.checkpoint(ids.len().max(1).ilog2() as u64 + 1)?;
        let i = ids
            .binary_search_by(|v| v.0.cmp(id))
            .map_err(|_| integrity("IR omitted a required analysis dependency"))?;
        Ok(ids[i].1)
    };
    let mut unresolved = vec![0u64; manifest.profiles.len()];
    let mut knowledge_ids = AdmittedVec::new(memory);
    visit_jsonl(source, c, |record: SemanticIrRecord, c| {
        match record {
            SemanticIrRecord::Function { function, .. } => {
                let original = reader.analysis(&function.analysis, c)?;
                visit_jsonl(&original.records, c, |record: FunctionRecord, c| {
                    match record {
                        FunctionRecord::CalleeEffect { analysis, .. }
                        | FunctionRecord::Expression {
                            origin: Some(analysis),
                            ..
                        }
                        | FunctionRecord::CallResolution {
                            analysis: Some(analysis),
                            ..
                        } => {
                            member(&analysis, c)?;
                        }
                        _ => (),
                    }
                    Ok(())
                })?;
            }
            SemanticIrRecord::Knowledge { entry } => {
                if entry.proposal.occurrence.revision != manifest.request.scope.revision {
                    return Err(integrity(
                        "IR contains knowledge from another source revision",
                    ));
                }
                let capacity = memory.reserve(entry.id.allocated_bytes(), c.position())?;
                knowledge_ids.push((entry.id, capacity), c.position())?;
                for evidence in &entry.proposal.evidence {
                    if let EvidenceRef::Analysis { analysis, .. } = evidence {
                        member(analysis, c)?;
                    }
                }
            }
            SemanticIrRecord::Call { record } => {
                let NavigationRecord::Call {
                    caller,
                    candidates,
                    issue,
                    ..
                } = *record
                else {
                    unreachable!()
                };
                let mask = member(&caller.analysis, c)?;
                if mask == 0 {
                    return Err(integrity("IR call caller is outside profiles"));
                }
                for (i, profile) in manifest.request.profiles.iter().enumerate() {
                    c.checkpoint(1)?;
                    if mask & (1 << i) == 0 {
                        continue;
                    }
                    if issue.is_some() || candidates.len() != 1 {
                        unresolved[i] += 1;
                    } else if profile.include_reachable
                        && member(&candidates[0].analysis, c)? & (1 << i) == 0
                    {
                        return Err(integrity(
                            "IR profile omitted a resolved reachable function",
                        ));
                    }
                }
            }
            _ => (),
        }
        Ok(())
    })?;
    c.checkpoint(knowledge_ids.len() as u64 * (knowledge_ids.len().max(1).ilog2() as u64 + 1))?;
    knowledge_ids.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    if knowledge_ids.windows(2).any(|w| w[0].0 == w[1].0)
        || knowledge_ids.len()
            != knowledge
                .entries()
                .filter(|e| e.proposal.occurrence.revision == manifest.request.scope.revision)
                .count()
        || unresolved
            .iter()
            .zip(&manifest.profiles)
            .any(|(n, p)| *n != p.unresolved_links)
    {
        return Err(integrity(
            "IR knowledge or link summary differs from saved records",
        ));
    }
    Ok(())
}
