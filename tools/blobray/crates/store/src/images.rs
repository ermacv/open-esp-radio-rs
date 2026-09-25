//! Immutable prepared-image publication; it never advances current_revision.
use super::*;
use std::io::Write;

pub struct ImageLease {
    pub id: PreparedImageId,
    pub manifest: ImageManifest,
    pub manifest_bytes: FileLease,
    pub elf: FileLease,
    pub map: FileLease,
    pub extraction: FileLease,
    pub provenance: FileLease,
    pub observations: FileLease,
}
pub struct RetainedImage {
    run: RunId,
    receipt: PreparedImageReceipt,
}
fn manifest(source: &dyn ByteSource, control: &mut dyn RunControl) -> Result<ImageManifest> {
    if source.len() > CONTROL_MESSAGE_BYTES as u64 {
        return Err(integrity("image manifest exceeds 64 KiB"));
    }
    let mut bytes = vec![0; source.len() as usize];
    source.read_at(0, &mut bytes, control)?;
    let manifest: ImageManifest = serde_json::from_slice(&bytes).map_err(jobs::json)?;
    if manifest.schema != 3 || !manifest.synthetic || !manifest.plan.ready() {
        return Err(integrity("invalid prepared image manifest"));
    }
    manifest.plan.recipe.validate_contract()?;
    if manifest.linker_diagnostics.exit_code != Some(0)
        || manifest.linker_diagnostics.signal.is_some()
        || manifest.linker_diagnostics.stderr_tail.len() > 8192
        || manifest.roots.is_empty()
        || manifest.roots.len() > 16
        || manifest.roots[0].address != manifest.entry
        || manifest.roots.len() != manifest.plan.recipe.roots.len() + 1
        || !manifest
            .roots
            .iter()
            .map(|root| &root.selection)
            .eq(std::iter::once(&manifest.plan.recipe.entry).chain(&manifest.plan.recipe.roots))
    {
        return Err(integrity("invalid image roots or tool observation"));
    }
    manifest.plan.recipe.layout.validate()?;
    let recipe = serde_json::to_vec(&manifest.plan.recipe).map_err(jobs::json)?;
    if ArtifactId::of_bytes_controlled(&recipe, control)?.as_str() != manifest.plan.id.as_str() {
        return Err(integrity("image recipe identity mismatch"));
    }
    Ok(manifest)
}
impl Project {
    /// Cheap admission identity; the worker validates the full image closure.
    pub fn image_revision(&self, id: &PreparedImageId) -> Result<RevisionId> {
        use rusqlite::OptionalExtension;
        let connection = open_connection(&self.root, false)?;
        let revision: Option<String> = connection
            .query_row(
                "SELECT revision FROM images WHERE id=?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(db)?;
        revision
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "prepared image not found"))?
            .parse()
    }
    pub fn images(
        &self,
        control: &mut dyn RunControl,
        sink: &mut dyn FnMut(&PreparedImageId, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        let connection = open_connection(&self.root, false)?;
        let schema: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db)?;
        if schema < 3 {
            return Ok(());
        }
        let mut query = connection
            .prepare("SELECT id FROM images ORDER BY sequence")
            .map_err(db)?;
        let mut rows = query.query([]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            control.checkpoint(1)?;
            let raw: String = row.get(0).map_err(db)?;
            sink(&raw.parse()?, control)?;
        }
        Ok(())
    }
    /// Verified image manifest whose publication and revision are retained.
    /// Opens no image artifact; readers verify each artifact they actually read.
    pub fn image_manifest(
        &self,
        id: &PreparedImageId,
        control: &mut dyn RunControl,
    ) -> Result<(ImageManifest, FileLease)> {
        let connection = open_connection(&self.root, false)?;
        let schema: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db)?;
        if schema < 3 {
            return Err(Error::new(
                ErrorCode::NotFound,
                "project has no prepared images",
            ));
        }
        use rusqlite::OptionalExtension;
        let expected: Option<(String, String)> = connection
            .query_row(
                "SELECT revision,plan FROM images WHERE id=?1",
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db)?;
        let (revision, plan) =
            expected.ok_or_else(|| Error::new(ErrorCode::NotFound, "prepared image not found"))?;
        let manifest_bytes = self.open_payload(&id.as_str().parse()?, control)?;
        let manifest = manifest(&manifest_bytes, control)?;
        if manifest.plan.recipe.project != self.id
            || manifest.plan.recipe.revision.as_str() != revision
            || manifest.plan.id.as_str() != plan
        {
            return Err(integrity("image publication and manifest disagree"));
        }
        self.require_revision(&revision.parse()?)?;
        Ok((manifest, manifest_bytes))
    }
    /// Verified manifest and executable ELF only, for execution.
    pub fn image_executable(
        &self,
        id: &PreparedImageId,
        control: &mut dyn RunControl,
    ) -> Result<(ImageManifest, FileLease)> {
        let (manifest, _) = self.image_manifest(id, control)?;
        let elf = self.open_payload(&manifest.elf, control)?;
        Ok((manifest, elf))
    }
    pub fn image(&self, id: &PreparedImageId, control: &mut dyn RunControl) -> Result<ImageLease> {
        let (manifest, manifest_bytes) = self.image_manifest(id, control)?;
        Ok(ImageLease {
            id: id.clone(),
            elf: self.open_payload(&manifest.elf, control)?,
            map: self.open_payload(&manifest.map, control)?,
            extraction: self.open_payload(&manifest.extraction, control)?,
            provenance: self.open_payload(&manifest.provenance, control)?,
            observations: self.open_payload(&manifest.observations, control)?,
            manifest,
            manifest_bytes,
        })
    }
}
impl Staging {
    /// Admit and capture one completed file produced inside this operation.
    pub fn retain_temporary(
        &self,
        mut file: TemporaryFile,
        control: &mut dyn RunControl,
    ) -> Result<ArtifactId> {
        file.flush().map_err(io)?;
        file.sync_all().map_err(io)?;
        let (id, _) = super::metered::hash_file(file.path(), control)?;
        self.persist(file, &id, control)?;
        Ok(id)
    }
    pub fn image_receipt(
        &self,
        manifest: TemporaryFile,
        control: &mut dyn RunControl,
    ) -> Result<PreparedImageReceipt> {
        let id = self.retain_temporary(manifest, control)?;
        let image = manifest_from_stage(self, &id, control)?;
        Ok(PreparedImageReceipt {
            schema: 1,
            project: image.plan.recipe.project,
            revision: image.plan.recipe.revision,
            plan: image.plan.id,
            image: id.as_str().parse()?,
        })
    }
}
fn manifest_from_stage(
    stage: &Staging,
    id: &ArtifactId,
    control: &mut dyn RunControl,
) -> Result<ImageManifest> {
    manifest(&stage.open_payload(id, control)?, control)
}
impl Writer {
    pub fn retain_image(
        &self,
        run: &RunRecord,
        receipt: &PreparedImageReceipt,
        control: &mut dyn RunControl,
    ) -> Result<RetainedImage> {
        if receipt.schema != 1
            || receipt.project != self.project.id
            || run.operation
                != (RunOperation::PrepareImage {
                    revision: receipt.revision.clone(),
                    plan: receipt.plan.clone(),
                })
        {
            return Err(integrity("image receipt belongs to another operation"));
        }
        let stage = Staging::open(&self.stage_path(&run.id))?;
        let id: ArtifactId = receipt.image.as_str().parse()?;
        let image = manifest_from_stage(&stage, &id, control)?;
        if image.plan.recipe.project != receipt.project
            || image.plan.recipe.revision != receipt.revision
            || image.plan.id != receipt.plan
        {
            return Err(integrity("image receipt and manifest disagree"));
        }
        self.project
            .open_payload(&receipt.revision.as_str().parse()?, control)?;
        for payload in [
            &image.elf,
            &image.map,
            &image.extraction,
            &image.provenance,
            &image.observations,
            &id,
        ] {
            self.promote(&stage, payload, None, control)?;
        }
        sync_dir(&self.project.root.join("objects"))?;
        Ok(RetainedImage {
            run: run.id.clone(),
            receipt: receipt.clone(),
        })
    }
    pub fn publish_image(&mut self, run: &mut RunRecord, retained: RetainedImage) -> Result<()> {
        if retained.run != run.id
            || run.operation
                != (RunOperation::PrepareImage {
                    revision: retained.receipt.revision.clone(),
                    plan: retained.receipt.plan.clone(),
                })
        {
            return Err(integrity("retained image belongs to another run"));
        }
        let mut connection = open_connection(&self.project.root, true)?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db)?;
        let receipt = retained.receipt;
        let mut completed = run.clone();
        completed.state = RunState::Completed;
        completed.image = Some(receipt.image.clone());
        completed.assessment = Some(ResultAssessment::default());
        tx.execute(
            "INSERT INTO images(id,revision,plan) VALUES(?1,?2,?3) ON CONFLICT(id) DO NOTHING",
            params![
                receipt.image.as_str(),
                receipt.revision.as_str(),
                receipt.plan.as_str()
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
