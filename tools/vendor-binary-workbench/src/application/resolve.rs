//! Project resolution for non-CLI frontends.

use std::path::{Path, PathBuf};

use crate::{MemoryMap, MmioMap, ProjectSpec, Result, TargetSpec, run_spec::RunSpec};

pub(super) struct ResolvedProject {
    pub(super) manifest: PathBuf,
    pub(super) project: ProjectSpec,
    pub(super) target_path: PathBuf,
    pub(super) target: TargetSpec,
    pub(super) run_spec_path: Option<PathBuf>,
    pub(super) run_spec: Option<RunSpec>,
    pub(super) memory_map: Option<MemoryMap>,
    pub(super) svd_paths: Vec<PathBuf>,
    pub(super) mmio: MmioMap,
}

impl ResolvedProject {
    pub(super) fn open(manifest: &Path) -> Result<Self> {
        let manifest = manifest.to_owned();
        let project = ProjectSpec::load(&manifest)?;
        let target_path = project.target_spec.clone();
        let mut target = TargetSpec::load(&target_path)?;
        if let Some(pack) = project.platform_pack.as_ref() {
            pack.apply_to_target(&mut target)?;
        }

        let run_spec_path = project.run_spec.clone().or_else(|| {
            manifest
                .parent()
                .map(|parent| parent.join("local.run"))
                .filter(|path| path.is_file())
        });
        let run_spec = run_spec_path.as_deref().map(RunSpec::load).transpose()?;
        let memory_map_path = project
            .memory_map
            .as_deref()
            .or(target.memory_map.as_deref());
        let memory_map = memory_map_path.map(MemoryMap::load).transpose()?;
        let svd_paths = if project.svd_configured {
            project.svd_paths.clone()
        } else {
            target.svd_paths.clone()
        };
        let mut mmio = crate::register_catalog::load(&svd_paths, Some(&project))?;
        if let Some(memory_map) = &memory_map {
            mmio.regions.extend(memory_map.resolved_mmio_regions()?);
            mmio.regions
                .sort_by_key(|region| (region.start, region.end, region.name.clone()));
            mmio.regions.dedup();
        }

        Ok(Self {
            manifest,
            project,
            target_path,
            target,
            run_spec_path,
            run_spec,
            memory_map,
            svd_paths,
            mmio,
        })
    }

    pub(super) fn context(&self) -> crate::cli::commands::ProjectContext<'_> {
        crate::cli::commands::ProjectContext {
            project_path: &self.manifest,
            project: &self.project,
            target_path: &self.target_path,
            target: &self.target,
            run_spec_path: self.run_spec_path.as_deref(),
            run_spec: self.run_spec.as_ref(),
            memory_map: self.memory_map.as_ref(),
            svd_paths: &self.svd_paths,
            svd: &self.mmio,
        }
    }
}
