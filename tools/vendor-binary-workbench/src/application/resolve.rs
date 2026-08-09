//! Project resolution for non-CLI frontends.

use std::path::{Path, PathBuf};

use crate::{MemoryMap, MmioMap, ProjectSpec, Result, TargetSpec, run_spec::RunSpec};

pub(crate) struct ProjectContext<'a> {
    pub(crate) project_path: &'a Path,
    pub(crate) project: &'a ProjectSpec,
    pub(crate) target_path: &'a Path,
    pub(crate) target: &'a TargetSpec,
    pub(crate) run_spec_path: Option<&'a Path>,
    pub(crate) run_spec: Option<&'a RunSpec>,
    pub(crate) memory_map: Option<&'a MemoryMap>,
    pub(crate) svd_paths: &'a [PathBuf],
    pub(crate) svd: &'a MmioMap,
}

pub(crate) struct ProjectSessionOptions {
    pub(crate) target_spec: Option<PathBuf>,
    pub(crate) run_spec: Option<PathBuf>,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) load_run_spec: bool,
    pub(crate) load_memory_map: bool,
    pub(crate) load_register_catalog: bool,
}

impl Default for ProjectSessionOptions {
    fn default() -> Self {
        Self {
            target_spec: None,
            run_spec: None,
            svd_paths: Vec::new(),
            load_run_spec: true,
            load_memory_map: true,
            load_register_catalog: true,
        }
    }
}

pub(crate) struct ProjectSession {
    pub(crate) manifest: PathBuf,
    pub(crate) project: ProjectSpec,
    pub(crate) target_path: PathBuf,
    pub(crate) target: TargetSpec,
    pub(crate) run_spec_path: Option<PathBuf>,
    pub(crate) run_spec: Option<RunSpec>,
    pub(crate) memory_map: Option<MemoryMap>,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) mmio: MmioMap,
}

impl ProjectSession {
    pub(crate) fn open(manifest: &Path) -> Result<Self> {
        Self::open_with(manifest, ProjectSessionOptions::default())
    }

    pub(crate) fn open_with(manifest: &Path, options: ProjectSessionOptions) -> Result<Self> {
        let manifest = manifest.to_owned();
        let project = ProjectSpec::load(&manifest)?;
        let target_path = options
            .target_spec
            .unwrap_or_else(|| project.target_spec.clone());
        let mut target = TargetSpec::load(&target_path)?;
        if let Some(pack) = project.platform_pack.as_ref() {
            pack.apply_to_target(&mut target)?;
        }

        let run_spec_path = options
            .load_run_spec
            .then(|| {
                options
                    .run_spec
                    .or_else(|| project.run_spec.clone())
                    .or_else(|| {
                        manifest
                            .parent()
                            .map(|parent| parent.join("local.toml"))
                            .filter(|path| path.is_file())
                    })
            })
            .flatten();
        let run_spec = run_spec_path.as_deref().map(RunSpec::load).transpose()?;
        let memory_map_path = project
            .memory_map
            .as_deref()
            .or(target.memory_map.as_deref());
        let memory_map = if options.load_memory_map {
            memory_map_path.map(MemoryMap::load).transpose()?
        } else {
            None
        };
        let svd_paths = if !options.svd_paths.is_empty() {
            options.svd_paths
        } else if project.svd_configured {
            project.svd_paths.clone()
        } else {
            target.svd_paths.clone()
        };
        let mut mmio = if options.load_register_catalog {
            crate::register_catalog::load(&svd_paths, Some(&project))?
        } else {
            MmioMap::load_all(&[])?
        };
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

    pub(crate) fn context(&self) -> ProjectContext<'_> {
        ProjectContext {
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
