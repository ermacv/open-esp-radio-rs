//! Bind one complete source snapshot before any experiment can be sealed.

use super::*;
use crate::image::Artifacts;

impl RunSession {
    pub fn bind_source_snapshot(&mut self, directory: &Path) -> Result<()> {
        if self.frozen_sources.is_some()
            || !self.manifest.firmware.is_empty()
            || self.directory.join("attempts").exists()
        {
            return Err("source snapshot must be bound once, before firmware or attempts".into());
        }
        let (frozen, sources, files) = crate::evidence::build_record::archive_snapshot(
            directory,
            &self.directory,
            &self.target_directory,
        )?;
        let primary = sources
            .first()
            .filter(|s| s.name == "repository")
            .ok_or("snapshot must start with the primary repository")?;
        self.manifest.repository = RepositoryProvenance {
            commit: primary.commit.clone(),
            dirty: primary.dirty,
            workspace_sha256: primary.workspace_sha256.clone(),
        };
        self.source_materials = sources;
        self.snapshot_materials = files;
        self.frozen_sources = Some(frozen);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)?;
        self.record_event(RunEventKind::SourceSnapshotBound, None, None, None)
    }

    pub fn build_frozen_image(
        &self,
        class: ImageClass,
        build: crate::image::CurrentBuild,
    ) -> Result<Artifacts> {
        self.frozen_sources
            .as_ref()
            .ok_or("HIL build requires a bound source snapshot")?
            .build_for_chip(
                &self.repository_root,
                &self.manifest.target,
                class,
                build.network,
                build.layout_seed,
                &build.features,
            )
    }
}
