//! Retained function results. Interpretation belongs to the worker, never store.
use super::*;
use std::io::Write;
pub struct FunctionLease {
    pub manifest: FunctionManifest,
    pub manifest_bytes: FileLease,
    pub records: FileLease,
}
pub struct RetainedFunction {
    run: RunId,
    receipt: PreparedFunctionReceipt,
    assessment: ResultAssessment,
}
pub(crate) fn decode(
    source: &dyn ByteSource,
    control: &mut dyn RunControl,
) -> Result<FunctionManifest> {
    if source.len() > 65536 {
        return Err(integrity("function manifest exceeds 64 KiB"));
    }
    let mut bytes = vec![0; source.len() as usize];
    source.read_at(0, &mut bytes, control)?;
    #[derive(serde::Deserialize)]
    struct Version {
        schema: u32,
    }
    if serde_json::from_slice::<Version>(&bytes)
        .map_err(jobs::json)?
        .schema
        != 4
    {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported function analysis schema; captured source bytes are unchanged",
        ));
    }
    let manifest: FunctionManifest = serde_json::from_slice(&bytes).map_err(jobs::json)?;
    let supported = manifest.schema == 4
        && manifest.recipe.schema == 4
        && manifest.recipe.policy == 4
        && manifest.semantics.is_some()
        && manifest
            .recipe
            .semantics
            .as_ref()
            .is_some_and(|s| !s.is_empty());
    if !supported {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported function analysis schema",
        ));
    }
    if manifest.recipe.extent.length == 0
        || manifest
            .recipe
            .extent
            .start
            .checked_add(manifest.recipe.extent.length)
            .is_none()
    {
        return Err(integrity("invalid retained function extent"));
    }
    Ok(manifest)
}
impl Project {
    pub fn analyses(
        &self,
        control: &mut dyn RunControl,
        sink: &mut dyn FnMut(&FunctionAnalysisId, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        let connection = open_connection(&self.root, false)?;
        let schema: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db)?;
        if schema < 4 {
            return Ok(());
        }
        let mut query = connection
            .prepare("SELECT id FROM analyses ORDER BY sequence")
            .map_err(db)?;
        let mut rows = query.query([]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            control.checkpoint(1)?;
            let id: String = row.get(0).map_err(db)?;
            sink(&id.parse()?, control)?;
        }
        Ok(())
    }
    pub fn analysis(
        &self,
        id: &FunctionAnalysisId,
        control: &mut dyn RunControl,
    ) -> Result<FunctionLease> {
        let connection = open_connection(&self.root, false)?;
        let schema: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db)?;
        if schema < 4 {
            return Err(Error::new(
                ErrorCode::NotFound,
                "project has no function analyses",
            ));
        }
        use rusqlite::OptionalExtension;
        let revision: Option<String> = connection
            .query_row(
                "SELECT revision FROM analyses WHERE id=?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        let revision = revision
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "function analysis not found"))?;
        let manifest_bytes = self.open_payload(&id.as_str().parse()?, control)?;
        let manifest = decode(&manifest_bytes, control)?;
        if manifest.recipe.project != self.id || manifest.recipe.revision.as_str() != revision {
            return Err(integrity("analysis row and manifest disagree"));
        }
        self.open_payload(&revision.parse()?, control)?;
        if let Some(research) = &manifest.recipe.research {
            if research.companions.len() > 64 {
                return Err(integrity("too many research companions"));
            }
            for id in std::iter::once(&research.publication).chain(&research.companions) {
                // Check immediate retained roots without recursive traversal through
                // publications -> analyses -> publications. Doctor checks each row.
                let source = self.open_payload(&id.as_str().parse()?, control)?;
                let publication = investigations::decode(&source, control)?;
                if publication.plan.recipe.request.revision.as_ref()
                    != Some(&manifest.recipe.revision)
                {
                    return Err(integrity("research dependency revision differs"));
                }
                self.open_payload(&publication.members, control)?;
            }
            if let Some(id) = &research.knowledge {
                self.open_payload(&id.as_str().parse()?, control)?;
            }
        }
        // Ordinary member payload is inside the retained archive; the source revision
        // reader/doctor verifies its closure. The payload digest qualifies that range.
        Ok(FunctionLease {
            records: self.open_payload(&manifest.records, control)?,
            manifest_bytes,
            manifest,
        })
    }
}
impl Staging {
    pub fn function_receipt(
        &self,
        manifest: &FunctionManifest,
        control: &mut dyn RunControl,
    ) -> Result<PreparedFunctionReceipt> {
        let encoded = serde_json::to_vec(manifest).map_err(jobs::json)?;
        if encoded.len() > 65536 {
            return Err(integrity("function manifest exceeds 64 KiB"));
        }
        let mut file = self.disk.temporary(&self.root.join("staging"))?;
        file.write_all(&encoded).map_err(io)?;
        let id = self.retain_temporary(file, control)?;
        Ok(PreparedFunctionReceipt {
            schema: 1,
            project: manifest.recipe.project.clone(),
            revision: manifest.recipe.revision.clone(),
            analysis: id.as_str().parse()?,
        })
    }
}
impl Writer {
    pub fn retain_function(
        &self,
        run: &RunRecord,
        receipt: &PreparedFunctionReceipt,
        control: &mut dyn RunControl,
    ) -> Result<RetainedFunction> {
        let RunOperation::AnalyzeFunction { request } = run.effective_operation() else {
            return Err(integrity("function receipt belongs to another operation"));
        };
        if receipt.schema != 1
            || receipt.project != self.project.id
            || request.revision.as_ref() != Some(&receipt.revision)
        {
            return Err(integrity("function receipt identity differs"));
        }
        let stage = Staging::open(&self.stage_path(&run.id))?;
        let id: ArtifactId = receipt.analysis.as_str().parse()?;
        let manifest = decode(&stage.open_payload(&id, control)?, control)?;
        let recipe = &manifest.recipe;
        if recipe.project != receipt.project
            || recipe.revision != receipt.revision
            || recipe.symbol != request.symbol
            || recipe.research != request.research
            || recipe.source != request.source
            || recipe.user_extent != request.extent.is_some()
            || request.extent.is_some_and(|e| e != recipe.extent)
        {
            return Err(integrity("function recipe differs from admitted request"));
        }
        self.project
            .open_payload(&receipt.revision.as_str().parse()?, control)?;
        for payload in [&manifest.records, &id] {
            self.promote(&stage, payload, None, control)?;
        }
        sync_dir(&self.project.root.join("objects"))?;
        Ok(RetainedFunction {
            run: run.id.clone(),
            receipt: receipt.clone(),
            assessment: ResultAssessment::function(receipt.analysis.clone(), &manifest),
        })
    }
    pub fn publish_function(
        &mut self,
        run: &mut RunRecord,
        retained: RetainedFunction,
    ) -> Result<()> {
        if retained.run != run.id
            || !matches!(
                run.effective_operation(),
                RunOperation::AnalyzeFunction { .. }
            )
        {
            return Err(integrity("retained analysis belongs to another run"));
        }
        let mut connection = open_connection(&self.project.root, true)?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db)?;
        let mut completed = run.clone();
        completed.state = RunState::Completed;
        completed.analysis = Some(retained.receipt.analysis.clone());
        completed.assessment = Some(retained.assessment);
        tx.execute(
            "INSERT INTO analyses(id,revision) VALUES(?1,?2) ON CONFLICT(id) DO NOTHING",
            params![
                retained.receipt.analysis.as_str(),
                retained.receipt.revision.as_str()
            ],
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
