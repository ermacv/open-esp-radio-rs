//! Bind one complete source snapshot before any experiment can be sealed.

use super::*;
use crate::image::{Artifacts, Integration, snapshot::FrozenSources};
use build::{BuildFileMaterial, SourceRebuildStatus};

impl RunSession {
    pub(crate) fn bind_source_snapshot(&mut self, directory: &Path) -> Result<()> {
        if self.frozen_sources.is_some()
            || !self.manifest.firmware.is_empty()
            || self.directory.join("attempts").exists()
        {
            return Err("source snapshot must be bound once, before firmware or attempts".into());
        }
        let destination = self.directory.join("source/snapshot");
        let mut files: Vec<BuildFileMaterial> = Vec::new();
        for (name, filename) in [
            ("source-snapshot-metadata", "snapshot.json"),
            ("source-snapshot-manifest", "manifest.json"),
            ("source-snapshot-archive", "sources.tar"),
        ] {
            let relative = PathBuf::from("source/snapshot").join(filename);
            let archived = build::archive_content_addressed(
                &directory.join(filename),
                &self.directory.join(&relative),
                &self.target_directory,
            )?;
            files.push(build::archived_file_material(
                name,
                Path::new(filename),
                relative,
                &archived,
            ));
        }
        let frozen = FrozenSources::open(&destination)?;
        let sources = frozen
            .sources()
            .iter()
            .map(|source| {
                Ok(SourceMaterial {
                    name: source.name.clone(),
                    checkout_path: PathBuf::from(&source.name),
                    remote: None,
                    commit: source.commit.clone(),
                    dirty: source.dirty,
                    workspace_sha256: source.identity()?,
                    rebuild_status: SourceRebuildStatus::SourceSnapshot,
                    tracked_patch_path: None,
                    tracked_patch_size_bytes: None,
                    tracked_patch_sha256: None,
                    untracked_files: Vec::new(),
                    limitations: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
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
        self.record_event("source-snapshot-bound", None, None, None)
    }

    pub(crate) fn build_frozen_image(
        &self,
        class: ImageClass,
        network: Integration,
    ) -> Result<Artifacts> {
        self.frozen_sources
            .as_ref()
            .ok_or("HIL build requires a bound source snapshot")?
            .build(&self.repository_root, class, network)
    }
}
