//! Content-addressed cache for expensive project-analysis stages.
//!
//! This is derived local state, never project configuration. A cache hit is
//! accepted only when the owning stage revision, every declared input and
//! every generated output still have exactly the recorded content digest.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::Result;

pub(super) struct ProjectAnalysisCache {
    store: PersistentStore,
    compiled_knowledge_identity: String,
    digests: BTreeMap<PathBuf, DigestMemo>,
    observed_inputs: BTreeMap<String, ObservedInputs>,
    last_lookup_restored: bool,
    planning_snapshot: Option<PlanningSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ProjectAnalysisCachePlan {
    Current {
        signature: String,
    },
    Restorable {
        signature: String,
        changed_outputs: usize,
    },
    Missing {
        signature: String,
        cause: String,
    },
}

enum PersistentStore {
    Disabled,
    Deferred(PathBuf),
    Ready(Box<crate::application::query_store::QueryStore>),
    Failed(String),
}

enum PlanningSnapshot {
    Absent,
    Locked {
        guard: crate::application::query_store::PlanReadGuard,
    },
    Failed(String),
}

struct DigestMemo {
    version: FileVersion,
    value: String,
}

struct ObservedInputs {
    snapshot: InputSnapshot,
    mutation_guard: InputMutationGuard,
}

/// One fail-closed generation snapshot for every non-generated project input.
///
/// Stage-local guards protect cache publication. This guard spans the complete
/// coordinator run so a producer cannot publish facts from artifact A and a
/// later consumer combine them with an atomically rebound artifact B.
pub(super) struct PipelineInputObservation {
    roots: Vec<PathBuf>,
    filesystem: Vec<(PathBuf, InputPathVersion)>,
    mutation_guard: InputMutationGuard,
}

impl PipelineInputObservation {
    pub(super) fn capture(mut roots: Vec<PathBuf>) -> Result<Self> {
        roots.sort();
        roots.dedup();
        for _ in 0..2 {
            let filesystem = input_path_versions(&roots)?;
            let watched = filesystem
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>();
            let mut mutation_guard = InputMutationGuard::new(&watched)?;
            let verified = input_path_versions(&roots)?;
            if filesystem == verified && !mutation_guard.changed()? {
                return Ok(Self {
                    roots,
                    filesystem,
                    mutation_guard,
                });
            }
        }
        Err(crate::Error::invalid(
            "project inputs changed while the pipeline generation guard was being established",
        ))
    }

    pub(super) fn validate(&mut self) -> Result<()> {
        let current = input_path_versions(&self.roots)?;
        if current != self.filesystem || self.mutation_guard.changed()? {
            return Err(crate::Error::invalid(
                "project inputs changed during analysis; discard mixed-generation outputs and rerun the complete pipeline",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InputSnapshot {
    signature: String,
    filesystem: Vec<(PathBuf, InputPathVersion)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InputPathVersion {
    /// `lstat` identity of the directory entry itself. Unlike `metadata`, this
    /// preserves the generation of a symbolic link instead of only observing
    /// its current target.
    entry: Option<FileVersion>,
    entry_kind: InputEntryKind,
    /// Followed target identity. Regular entries repeat `entry`; symbolic
    /// links carry both identities so target and link changes are observable.
    target: Option<(InputTargetKind, FileVersion)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputEntryKind {
    Missing,
    File,
    Directory,
    SymbolicLink,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputTargetKind {
    File,
    Directory,
    Other,
}

#[cfg(target_os = "linux")]
struct InputMutationGuard {
    inotify: nix::sys::inotify::Inotify,
    watches: BTreeMap<
        nix::sys::inotify::WatchDescriptor,
        std::collections::BTreeSet<std::ffi::OsString>,
    >,
}

#[cfg(not(target_os = "linux"))]
struct InputMutationGuard {
    /// Portable fail-closed fallback. Platforms without a filename-filtered
    /// notification backend may invalidate on unrelated sibling mutations,
    /// but never accept an ABA-rebound input as current.
    parents: Vec<(PathBuf, Option<FileVersion>)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileVersion {
    len: u64,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

impl FileVersion {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed_seconds: metadata.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

#[cfg(target_os = "linux")]
impl InputMutationGuard {
    fn new(inputs: &[PathBuf]) -> Result<Self> {
        use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};

        let inotify =
            Inotify::init(InitFlags::IN_CLOEXEC | InitFlags::IN_NONBLOCK).map_err(|error| {
                crate::Error::invalid(format!(
                    "cannot start the analysis-input mutation guard: {error}"
                ))
            })?;
        let mut guard = Self {
            inotify,
            watches: BTreeMap::new(),
        };
        let mask = AddWatchFlags::IN_ATTRIB
            | AddWatchFlags::IN_CLOSE_WRITE
            | AddWatchFlags::IN_CREATE
            | AddWatchFlags::IN_DELETE
            | AddWatchFlags::IN_DELETE_SELF
            | AddWatchFlags::IN_MODIFY
            | AddWatchFlags::IN_MOVED_FROM
            | AddWatchFlags::IN_MOVED_TO
            | AddWatchFlags::IN_MOVE_SELF
            | AddWatchFlags::IN_UNMOUNT;
        for binding in input_binding_paths(inputs)? {
            let Some((parent, name)) = closest_watchable_binding(&binding) else {
                continue;
            };
            let descriptor = guard.inotify.add_watch(&parent, mask).map_err(|error| {
                crate::Error::invalid(format!(
                    "cannot watch analysis input binding {} in {}: {error}",
                    binding.display(),
                    parent.display()
                ))
            })?;
            guard.watches.entry(descriptor).or_default().insert(name);
        }
        Ok(guard)
    }

    fn changed(&mut self) -> Result<bool> {
        use nix::{errno::Errno, sys::inotify::AddWatchFlags};

        loop {
            let events = match self.inotify.read_events() {
                Ok(events) => events,
                Err(Errno::EAGAIN) => return Ok(false),
                Err(error) => {
                    return Err(crate::Error::invalid(format!(
                        "cannot read the analysis-input mutation guard: {error}"
                    )));
                }
            };
            for event in events {
                if event.mask.intersects(
                    AddWatchFlags::IN_Q_OVERFLOW
                        | AddWatchFlags::IN_UNMOUNT
                        | AddWatchFlags::IN_IGNORED,
                ) {
                    return Ok(true);
                }
                let Some(names) = self.watches.get(&event.wd) else {
                    return Ok(true);
                };
                match event.name {
                    Some(name) if !names.contains(&name) => {}
                    _ => return Ok(true),
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for InputMutationGuard {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;

        let _ = nix::unistd::close(self.inotify.as_raw_fd());
    }
}

#[cfg(not(target_os = "linux"))]
impl InputMutationGuard {
    fn new(inputs: &[PathBuf]) -> Result<Self> {
        let parents = input_binding_paths(inputs)?
            .into_iter()
            .filter_map(|path| path.parent().map(Path::to_owned))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|path| {
                let version = fs::metadata(&path)
                    .ok()
                    .map(|metadata| FileVersion::from_metadata(&metadata));
                (path, version)
            })
            .collect();
        Ok(Self { parents })
    }

    fn changed(&mut self) -> Result<bool> {
        for (path, expected) in &self.parents {
            let current = fs::metadata(path)
                .ok()
                .map(|metadata| FileVersion::from_metadata(&metadata));
            if &current != expected {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn input_binding_paths(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut bindings = std::collections::BTreeSet::new();
    for input in inputs {
        bindings.insert(input.clone());
        let mut ancestor = input.parent();
        while let Some(path) = ancestor {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    bindings.insert(path.to_owned());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            ancestor = path.parent();
        }
        if let Ok(canonical) = fs::canonicalize(input) {
            bindings.insert(canonical);
        }
    }
    Ok(bindings.into_iter().collect())
}

#[cfg(target_os = "linux")]
fn closest_watchable_binding(path: &Path) -> Option<(PathBuf, std::ffi::OsString)> {
    let mut binding = path;
    loop {
        let parent = binding.parent()?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if fs::metadata(parent).is_ok_and(|metadata| metadata.is_dir()) {
            return binding
                .file_name()
                .map(|name| (parent.to_owned(), name.to_owned()));
        }
        binding = parent;
    }
}

impl ProjectAnalysisCache {
    /// Construct a cache boundary that cannot touch persistent state.
    ///
    /// Check mode never asks this value for a lookup or record. Keeping the
    /// disabled value explicit makes an accidental cache access fail closed
    /// instead of silently opening SQLite or creating a cache directory.
    pub(super) fn disabled() -> Self {
        Self {
            store: PersistentStore::Disabled,
            compiled_knowledge_identity: String::new(),
            digests: BTreeMap::new(),
            observed_inputs: BTreeMap::new(),
            last_lookup_restored: false,
            planning_snapshot: None,
        }
    }

    /// Construct a write-mode cache without touching persistent state.
    ///
    /// Project orchestration performs dependency preflight before invoking a
    /// stage. Deferring the SQLite open until the first actual lookup keeps a
    /// fully blocked analysis byte-for-byte read-only while preserving cache
    /// reuse for every stage that is allowed to run.
    pub(super) fn deferred(project_manifest: &Path) -> Self {
        Self {
            store: PersistentStore::Deferred(project_manifest.to_owned()),
            compiled_knowledge_identity: String::new(),
            digests: BTreeMap::new(),
            observed_inputs: BTreeMap::new(),
            last_lookup_restored: false,
            planning_snapshot: None,
        }
    }

    /// Freeze the persistent cache generation observed by a complete plan.
    /// Missing cache state stays missing even if another process creates it
    /// later; an existing database is protected by a shared lifetime lock.
    pub(super) fn planning(project_manifest: &Path) -> Self {
        let planning_snapshot =
            match crate::application::query_store::QueryStore::plan_read_guard(project_manifest) {
                Ok(Some(guard)) => PlanningSnapshot::Locked { guard },
                Ok(None) => PlanningSnapshot::Absent,
                Err(error) => PlanningSnapshot::Failed(error.to_string()),
            };
        Self {
            store: PersistentStore::Disabled,
            compiled_knowledge_identity: String::new(),
            digests: BTreeMap::new(),
            observed_inputs: BTreeMap::new(),
            last_lookup_restored: false,
            planning_snapshot: Some(planning_snapshot),
        }
    }

    /// Bind the compiled provider/contracts identity used by semantic stages.
    /// Planner and executor must receive the same value so a plan never
    /// predicts reuse that execution would reject.
    pub(super) fn with_compiled_knowledge_identity(mut self, identity: String) -> Self {
        self.compiled_knowledge_identity = identity;
        self
    }

    pub(super) fn ensure_planning_snapshot(&self) -> Result<()> {
        match self.planning_snapshot.as_ref() {
            Some(PlanningSnapshot::Absent | PlanningSnapshot::Locked { .. }) => Ok(()),
            Some(PlanningSnapshot::Failed(error)) => Err(crate::Error::invalid(format!(
                "persistent query cache snapshot is unavailable: {error}"
            ))),
            None => Err(crate::Error::invalid(
                "persistent query cache was not opened for planning",
            )),
        }
    }

    /// Inspect one exact stage signature without allowing CAS restoration or
    /// any SQLite mutation.
    pub(super) fn plan(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
        outputs: &[PathBuf],
    ) -> Result<ProjectAnalysisCachePlan> {
        self.ensure_planning_snapshot()?;
        let signature = self.signature(stage, configuration, inputs)?;
        if matches!(self.planning_snapshot, Some(PlanningSnapshot::Absent)) {
            return Ok(ProjectAnalysisCachePlan::Missing {
                signature,
                cause: "persistent cache was absent when planning started".to_owned(),
            });
        }
        let cached = {
            let PlanningSnapshot::Locked { guard } = self
                .planning_snapshot
                .as_ref()
                .expect("planning snapshot was validated")
            else {
                unreachable!("absent planning snapshot returned before cache lookup")
            };
            crate::application::query_store::QueryStore::stage_output_digests_read_only(
                guard, &signature, false,
            )?
        };
        let Some(cached) = cached else {
            return Ok(ProjectAnalysisCachePlan::Missing {
                signature,
                cause: "no cached result matches the current stage signature".to_owned(),
            });
        };
        if cached.len() != outputs.len() {
            return Ok(ProjectAnalysisCachePlan::Missing {
                signature,
                cause: format!(
                    "cached result has {} output(s), but the stage now declares {}",
                    cached.len(),
                    outputs.len()
                ),
            });
        }
        let mut changed_outputs = 0;
        for (path, expected) in outputs.iter().zip(&cached) {
            let current = path
                .is_file()
                .then(|| self.digest(path))
                .transpose()?
                .is_some_and(|actual| &actual == expected);
            if !current {
                changed_outputs += 1;
            }
        }
        if changed_outputs == 0 {
            Ok(ProjectAnalysisCachePlan::Current { signature })
        } else {
            let verified = {
                let PlanningSnapshot::Locked { guard } = self
                    .planning_snapshot
                    .as_ref()
                    .expect("planning snapshot was validated")
                else {
                    unreachable!("absent planning snapshot returned before payload validation")
                };
                crate::application::query_store::QueryStore::stage_output_digests_read_only(
                    guard, &signature, true,
                )?
            };
            if verified.as_ref() != Some(&cached) {
                return Err(crate::Error::invalid(
                    "query cache stage binding changed during planning; retry after the cache writer exits",
                ));
            }
            Ok(ProjectAnalysisCachePlan::Restorable {
                signature,
                changed_outputs,
            })
        }
    }

    pub(super) fn is_current(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
        outputs: &[PathBuf],
    ) -> Result<bool> {
        self.is_current_with_post_bind_hook(stage, configuration, inputs, outputs, || {})
    }

    fn is_current_with_post_bind_hook(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
        outputs: &[PathBuf],
        after_bind: impl FnOnce(),
    ) -> Result<bool> {
        self.last_lookup_restored = false;
        self.observed_inputs.remove(stage);
        // Initialize the persistent cache before taking the stage snapshot.
        // First use may create `generated/.blobray-cache`; that local cache
        // setup must not look like a concurrent rebind of a project input in
        // the containing directory.
        self.store_mut()?;
        let mut observed = self.begin_observing_inputs(stage, configuration, inputs)?;
        let signature = observed.snapshot.signature.clone();
        let Some(cached) = self.store_mut()?.stage_output_digests(&signature)? else {
            self.observed_inputs.insert(stage.to_owned(), observed);
            return Ok(false);
        };
        if cached.len() != outputs.len() {
            self.observed_inputs.insert(stage.to_owned(), observed);
            return Ok(false);
        }
        let mut restored = false;
        for (path, expected) in outputs.iter().zip(&cached) {
            let current = path
                .is_file()
                .then(|| self.digest(path))
                .transpose()?
                .is_some_and(|actual| &actual == expected);
            if !current {
                self.store_mut()?.restore_output(expected, path)?;
                self.digests.remove(path);
                if self.digest(path)? != *expected {
                    return Err(crate::Error::invalid(format!(
                        "query cache restored {} with the wrong content digest",
                        path.display()
                    )));
                }
                restored = true;
            }
        }
        if restored {
            tracing::info!(cache_stage = stage, "restored generated outputs from CAS");
        }
        if self.observed_inputs_changed(stage, configuration, inputs, &mut observed)? {
            self.resnapshot_inputs(stage, configuration, inputs)?;
            tracing::warn!(
                cache_stage = stage,
                "analysis inputs changed during cache lookup; recomputing the stage"
            );
            return Ok(false);
        }
        let paths = outputs
            .iter()
            .map(|path| path_key(path))
            .collect::<Vec<_>>();
        self.store_mut()?
            .bind_restored_stage(stage, &signature, &paths, &cached)?;
        after_bind();
        match self.observed_inputs_changed(stage, configuration, inputs, &mut observed) {
            Ok(false) => {}
            Ok(true) => {
                self.store_mut()?.retire_stage_binding(stage, &signature)?;
                self.resnapshot_inputs(stage, configuration, inputs)?;
                tracing::warn!(
                    cache_stage = stage,
                    "analysis inputs changed while the cache hit was being published; recomputing the stage"
                );
                return Ok(false);
            }
            Err(validation_error) => {
                self.digests.clear();
                self.store_mut()?.retire_stage_binding(stage, &signature)?;
                return Err(crate::Error::invalid(format!(
                    "analysis inputs could not be validated after cached stage {stage:?} was rebound; the cached binding was retired: {validation_error}"
                )));
            }
        }
        self.last_lookup_restored = restored;
        Ok(true)
    }

    pub(super) const fn last_lookup_restored(&self) -> bool {
        self.last_lookup_restored
    }

    pub(super) fn record(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
        outputs: &[PathBuf],
    ) -> Result<()> {
        self.record_with_post_publish_hook(stage, configuration, inputs, outputs, || {})
    }

    fn record_with_post_publish_hook(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
        outputs: &[PathBuf],
        after_publish: impl FnOnce(),
    ) -> Result<()> {
        let mut expected = self.observed_inputs.remove(stage).ok_or_else(|| {
            crate::Error::invalid(format!(
                "analysis cache stage {stage:?} was recorded without a preceding input snapshot"
            ))
        })?;
        if self.observed_inputs_changed(stage, configuration, inputs, &mut expected)? {
            return Err(crate::Error::invalid(format!(
                "analysis input changed while stage {stage:?} was running; generated output was not cached, rerun the analysis"
            )));
        }
        let signature = expected.snapshot.signature.clone();
        let mut cached_outputs = Vec::with_capacity(outputs.len());
        for path in outputs {
            self.digests.remove(path);
            cached_outputs.push((path_key(path), self.digest(path)?, path.clone()));
        }
        self.store_mut()?
            .record_stage(stage, &signature, &cached_outputs)?;
        after_publish();
        match self.observed_inputs_changed(stage, configuration, inputs, &mut expected) {
            Ok(false) => Ok(()),
            Ok(true) => {
                self.digests.clear();
                self.store_mut()?.retire_stage_binding(stage, &signature)?;
                Err(crate::Error::invalid(format!(
                    "analysis input changed while stage {stage:?} output was entering the persistent cache; the cached binding was retired, rerun the analysis"
                )))
            }
            Err(validation_error) => {
                self.digests.clear();
                self.store_mut()?.retire_stage_binding(stage, &signature)?;
                Err(crate::Error::invalid(format!(
                    "analysis inputs could not be validated after stage {stage:?} output entered the persistent cache; the cached binding was retired: {validation_error}"
                )))
            }
        }
    }

    /// Borrow the coordinator-owned query store for nested analysis queries.
    ///
    /// Project analysis keeps one exclusive store for its complete lifetime.
    /// Expensive stages that also use the function-fact cache must borrow this
    /// value instead of opening a second writer and conflicting with the
    /// coordinator's lifetime lock.
    pub(super) fn query_store_mut(
        &mut self,
    ) -> Result<&mut crate::application::query_store::QueryStore> {
        self.store_mut()
    }

    /// Publish the run-owned epoch only after the complete coordinator has
    /// succeeded. If no cacheable stage opened the store, success is a no-op
    /// and preserves the lazy/disposable cache boundary.
    pub(super) fn complete_analysis_epoch(&mut self) -> Result<()> {
        match &mut self.store {
            PersistentStore::Ready(store) => store.complete_analysis_epoch(),
            PersistentStore::Deferred(_) | PersistentStore::Disabled => Ok(()),
            PersistentStore::Failed(error) => Err(crate::Error::invalid(format!(
                "persistent query store is unavailable: {error}"
            ))),
        }
    }

    fn store_mut(&mut self) -> Result<&mut crate::application::query_store::QueryStore> {
        let manifest = match &self.store {
            PersistentStore::Deferred(manifest) => Some(manifest.clone()),
            _ => None,
        };
        if let Some(manifest) = manifest {
            self.store =
                match crate::application::query_store::QueryStore::open_analysis_epoch(&manifest) {
                    Ok(store) => PersistentStore::Ready(Box::new(store)),
                    Err(error) => PersistentStore::Failed(error.to_string()),
                };
        }
        match &mut self.store {
            PersistentStore::Ready(store) => Ok(store),
            PersistentStore::Disabled => Err(crate::Error::invalid(
                "persistent query store is unavailable: disabled for read-only analysis",
            )),
            PersistentStore::Failed(error) => Err(crate::Error::invalid(format!(
                "persistent query store is unavailable: {error}"
            ))),
            PersistentStore::Deferred(_) => unreachable!("deferred query store was not opened"),
        }
    }

    fn observed_inputs_changed(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
        observed: &mut ObservedInputs,
    ) -> Result<bool> {
        let verified = self.observe_inputs(stage, configuration, inputs)?;
        Ok(verified != observed.snapshot || observed.mutation_guard.changed()?)
    }

    fn resnapshot_inputs(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
    ) -> Result<()> {
        self.digests.clear();
        let current = self.begin_observing_inputs(stage, configuration, inputs)?;
        self.observed_inputs.insert(stage.to_owned(), current);
        Ok(())
    }

    fn begin_observing_inputs(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
    ) -> Result<ObservedInputs> {
        for _ in 0..2 {
            let mut mutation_guard = InputMutationGuard::new(inputs)?;
            let snapshot = self.observe_inputs(stage, configuration, inputs)?;
            if !mutation_guard.changed()? {
                return Ok(ObservedInputs {
                    snapshot,
                    mutation_guard,
                });
            }
            self.digests.clear();
        }
        Err(crate::Error::invalid(format!(
            "analysis inputs for stage {stage:?} changed while their mutation guard was being established"
        )))
    }

    fn observe_inputs(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
    ) -> Result<InputSnapshot> {
        for _ in 0..2 {
            let observed = InputSnapshot {
                signature: self.signature(stage, configuration, inputs)?,
                filesystem: input_path_versions(inputs)?,
            };
            let verified = InputSnapshot {
                signature: self.signature(stage, configuration, inputs)?,
                filesystem: input_path_versions(inputs)?,
            };
            if observed == verified {
                return Ok(observed);
            }
            self.digests.clear();
        }
        Err(crate::Error::invalid(format!(
            "analysis inputs for stage {stage:?} changed while their cache snapshot was being captured"
        )))
    }

    fn signature(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
    ) -> Result<String> {
        let mut inputs = inputs.to_vec();
        inputs.sort();
        inputs.dedup();
        let mut digest = Sha256::new();
        digest.update(b"blobray-project-stage-v3\0");
        // A profile name is a project-local binding, not an analysis input.
        // Equivalent linked-IR profiles with different IDs/output paths must
        // address the same immutable query result.
        let query_kind = if stage.starts_with("linked-ir:") {
            "linked-ir"
        } else {
            stage
        };
        digest.update(query_kind.as_bytes());
        digest.update([0]);
        digest.update(env!("CARGO_PKG_VERSION").as_bytes());
        digest.update([0]);
        digest.update(stage_revision(stage)?.to_le_bytes());
        if let Some(schema) = stage_artifact_schema(stage) {
            digest.update([0]);
            digest.update(b"output-schema");
            digest.update([0]);
            digest.update(schema.version.to_le_bytes());
            digest.update([0]);
            digest.update(schema.command.as_bytes());
        }
        if stage_uses_compiled_knowledge(stage) {
            digest.update([0]);
            digest.update(self.compiled_knowledge_identity.as_bytes());
        }
        digest.update([0]);
        digest.update(configuration.as_bytes());
        for path in inputs {
            digest.update([0]);
            digest.update(path_key(&path).as_bytes());
            digest.update([0]);
            let content = if path.is_file() {
                self.digest(&path)?
            } else if path.is_dir() {
                self.directory_digest(&path)?
            } else {
                "<missing>".to_owned()
            };
            digest.update(content.as_bytes());
        }
        Ok(format!("{:x}", digest.finalize()))
    }

    fn directory_digest(&mut self, root: &Path) -> Result<String> {
        fn collect_files(
            directory: &Path,
            ancestors: &mut Vec<PathBuf>,
            output: &mut Vec<PathBuf>,
        ) -> Result<()> {
            let canonical = fs::canonicalize(directory)?;
            if ancestors.contains(&canonical) {
                return Err(crate::Error::invalid(format!(
                    "cache input directory {} contains a symbolic-link cycle",
                    directory.display()
                )));
            }
            ancestors.push(canonical);
            for entry in fs::read_dir(directory)? {
                let path = entry?.path();
                if path.is_dir() {
                    collect_files(&path, ancestors, output)?;
                } else if path.is_file() {
                    output.push(path);
                }
            }
            ancestors.pop();
            Ok(())
        }

        let mut files = Vec::new();
        collect_files(root, &mut Vec::new(), &mut files)?;
        files.sort();
        let mut digest = Sha256::new();
        digest.update(b"blobray-directory-v1\0");
        for path in files {
            let relative = path.strip_prefix(root).map_err(|error| {
                crate::Error::invalid(format!(
                    "cache input {} is not below directory {}: {error}",
                    path.display(),
                    root.display()
                ))
            })?;
            digest.update(relative.to_string_lossy().as_bytes());
            digest.update([0]);
            digest.update(self.digest(&path)?.as_bytes());
            digest.update([0]);
        }
        Ok(format!("{:x}", digest.finalize()))
    }

    fn digest(&mut self, path: &Path) -> Result<String> {
        let metadata = fs::metadata(path)?;
        let version = FileVersion::from_metadata(&metadata);
        if let Some(existing) = self.digests.get(path)
            && existing.version == version
        {
            return Ok(existing.value.clone());
        }
        for _ in 0..2 {
            let before = fs::metadata(path)?;
            let value = crate::artifact_sha256(path)?;
            let after = fs::metadata(path)?;
            if same_file_version(&before, &after) {
                self.digests.insert(
                    path.to_owned(),
                    DigestMemo {
                        version: FileVersion::from_metadata(&after),
                        value: value.clone(),
                    },
                );
                return Ok(value);
            }
        }
        Err(crate::Error::invalid(format!(
            "analysis input {} changed while its cache identity was being computed",
            path.display()
        )))
    }
}

fn input_path_versions(inputs: &[PathBuf]) -> Result<Vec<(PathBuf, InputPathVersion)>> {
    fn target_version(metadata: &fs::Metadata) -> (InputTargetKind, FileVersion) {
        let kind = if metadata.is_file() {
            InputTargetKind::File
        } else if metadata.is_dir() {
            InputTargetKind::Directory
        } else {
            InputTargetKind::Other
        };
        (kind, FileVersion::from_metadata(metadata))
    }

    fn snapshot(path: &Path) -> Result<InputPathVersion> {
        let entry = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(InputPathVersion {
                    entry: None,
                    entry_kind: InputEntryKind::Missing,
                    target: None,
                });
            }
            Err(error) => return Err(error.into()),
        };
        let entry_kind = if entry.file_type().is_symlink() {
            InputEntryKind::SymbolicLink
        } else if entry.is_file() {
            InputEntryKind::File
        } else if entry.is_dir() {
            InputEntryKind::Directory
        } else {
            InputEntryKind::Other
        };
        let target = match fs::metadata(path) {
            Ok(metadata) => Some(target_version(&metadata)),
            Err(error)
                if entry.file_type().is_symlink()
                    && error.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => return Err(error.into()),
        };
        Ok(InputPathVersion {
            entry: Some(FileVersion::from_metadata(&entry)),
            entry_kind,
            target,
        })
    }

    fn collect_symlink_ancestors(
        path: &Path,
        output: &mut BTreeMap<PathBuf, InputPathVersion>,
    ) -> Result<()> {
        let mut ancestor = path.parent();
        while let Some(path) = ancestor {
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(error.into()),
            };
            if metadata.file_type().is_symlink() {
                output.insert(path.to_owned(), snapshot(path)?);
            }
            ancestor = path.parent();
        }
        Ok(())
    }

    fn collect(
        path: &Path,
        directory_ancestors: &mut Vec<PathBuf>,
        output: &mut BTreeMap<PathBuf, InputPathVersion>,
    ) -> Result<()> {
        collect_symlink_ancestors(path, output)?;
        let version = snapshot(path)?;
        let directory = matches!(version.target, Some((InputTargetKind::Directory, _)));
        output.insert(path.to_owned(), version);
        if !directory {
            return Ok(());
        }

        let canonical = fs::canonicalize(path)?;
        if directory_ancestors.contains(&canonical) {
            return Err(crate::Error::invalid(format!(
                "cache input directory {} contains a symbolic-link cycle",
                path.display()
            )));
        }
        directory_ancestors.push(canonical);
        let mut children = fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        children.sort();
        for child in children {
            collect(&child, directory_ancestors, output)?;
        }
        directory_ancestors.pop();
        Ok(())
    }

    let mut roots = inputs.to_vec();
    roots.sort();
    roots.dedup();
    let mut output = BTreeMap::new();
    for path in roots {
        collect(&path, &mut Vec::new(), &mut output)?;
    }
    Ok(output.into_iter().collect())
}

fn same_file_version(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.file_type() == after.file_type()
        && FileVersion::from_metadata(before) == FileVersion::from_metadata(after)
}

fn stage_uses_compiled_knowledge(stage: &str) -> bool {
    stage == "linked-ir" || stage.starts_with("linked-ir:") || stage == "event-replays"
}

fn stage_artifact_schema(stage: &str) -> Option<crate::artifacts::ArtifactSchema> {
    let owner = stage.split_once(':').map_or(stage, |(owner, _)| owner);
    match owner {
        "symbol-inventory" => Some(crate::artifacts::SYMBOL_INVENTORY),
        "mmio-discovery" => Some(crate::artifacts::MMIO_FACTS),
        "interface-discovery" => Some(crate::artifacts::INTERFACE_FACTS),
        "linked-ir" => Some(crate::artifacts::LINKED_IR),
        "event-replays" => Some(crate::artifacts::REPLAY_EVIDENCE),
        _ => None,
    }
}

/// Explicit semantic revision of each cached generator.
///
/// A digest of the whole executable made presentation-only changes invalidate
/// every expensive artifact-wide stage.  Bump only the owner below when its
/// generated document or analysis semantics change.  Input/output content
/// hashes continue to protect project and caller-owned state.
fn stage_revision(stage: &str) -> Result<u32> {
    if stage.starts_with("linked-ir:") {
        return Ok(42);
    }
    match stage {
        "symbol-inventory" => Ok(2),
        "mmio-discovery" => Ok(4),
        "interface-discovery" => Ok(7),
        "linked-ir" => Ok(42),
        "event-replays" => Ok(1),
        "review-scopes" => Ok(5),
        "navigation-index" => Ok(2),
        "code-boundary-review" => Ok(1),
        "register-review" => Ok(1),
        "function-review" => Ok(1),
        "code-boundary-validation:deny-unreviewed=false"
        | "code-boundary-validation:deny-unreviewed=true"
        | "register-validation:deny-unreviewed=false"
        | "register-validation:deny-unreviewed=true"
        | "function-validation:deny-unreviewed=false"
        | "function-validation:deny-unreviewed=true"
        | "interface-validation:deny-unreviewed=false"
        | "interface-validation:deny-unreviewed=true" => Ok(1),
        _ => Err(crate::Error::invalid(format!(
            "analysis cache has no semantic revision for stage {stage:?}"
        ))),
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_cache_construction_does_not_touch_persistent_state() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-deferred-analysis-cache-{}",
            std::process::id()
        ));
        if directory.is_dir() {
            fs::remove_dir_all(&directory).unwrap();
        }
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("vendor-project.toml");
        fs::write(&manifest, "schema = 3\n").unwrap();

        let mut cache = ProjectAnalysisCache::deferred(&manifest);
        assert!(!directory.join("generated/.blobray-cache").exists());
        cache.complete_analysis_epoch().unwrap();
        assert!(!directory.join("generated/.blobray-cache").exists());

        drop(cache);
        assert!(!directory.join("generated").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_restores_outputs_but_never_reuses_changed_inputs() {
        let directory =
            std::env::temp_dir().join(format!("blobray-analysis-cache-{}", std::process::id()));
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("input.bin");
        let output = directory.join("generated/output.json");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&manifest, "schema = 1\n").unwrap();
        fs::write(&input, "input-a").unwrap();
        fs::write(&output, "output-a").unwrap();

        let mut cache = ProjectAnalysisCache::deferred(&manifest);
        assert!(
            !cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output)
                )
                .unwrap()
        );
        cache
            .record(
                "linked-ir",
                "profile=a",
                std::slice::from_ref(&input),
                std::slice::from_ref(&output),
            )
            .unwrap();
        assert!(
            cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output)
                )
                .unwrap()
        );

        fs::write(&output, "output-b").unwrap();
        assert!(
            cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output)
                )
                .unwrap()
        );
        assert_eq!(fs::read_to_string(&output).unwrap(), "output-a");

        fs::remove_file(&output).unwrap();
        assert!(
            cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output)
                )
                .unwrap()
        );
        assert_eq!(fs::read_to_string(&output).unwrap(), "output-a");

        fs::write(&input, "input-a").unwrap();
        cache.digests.clear();
        assert!(
            !cache
                .is_current(
                    "linked-ir",
                    "profile=b",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output)
                )
                .unwrap()
        );
        fs::write(&output, "output-a").unwrap();
        fs::write(&input, "input-b").unwrap();
        cache.digests.clear();
        assert!(
            !cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output)
                )
                .unwrap()
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_refuses_to_record_when_an_input_changes_during_a_stage() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-analysis-cache-input-race-{}",
            std::process::id()
        ));
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("vendor.bin");
        let output = directory.join("generated/output.json");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&manifest, "schema = 1\n").unwrap();
        fs::write(&input, "artifact-a").unwrap();
        fs::write(&output, "output").unwrap();

        let mut cache = ProjectAnalysisCache::deferred(&manifest);
        assert!(
            !cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output),
                )
                .unwrap()
        );
        // Keep the byte length identical and restore the original contents:
        // content signatures alone are ABA-blind, while the captured file
        // generation must still prove that a rebuild happened mid-stage.
        fs::write(&input, "artifact-b").unwrap();
        fs::write(&input, "artifact-a").unwrap();
        let error = cache
            .record(
                "linked-ir",
                "profile=a",
                std::slice::from_ref(&input),
                std::slice::from_ref(&output),
            )
            .unwrap_err();

        assert!(error.to_string().contains("changed while stage"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_retires_a_stage_binding_when_inputs_change_after_publication() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-analysis-cache-post-publish-input-race-{}",
            std::process::id()
        ));
        if directory.is_dir() {
            fs::remove_dir_all(&directory).unwrap();
        }
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("vendor.bin");
        let saved = directory.join("vendor.saved.bin");
        let replacement = directory.join("vendor.replacement.bin");
        let output = directory.join("generated/output.json");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&manifest, "schema = 1\n").unwrap();
        fs::write(&input, "artifact-a").unwrap();
        fs::write(&replacement, "artifact-b").unwrap();
        fs::write(&output, vec![0x5a; 128 * 1024]).unwrap();

        let mut cache = ProjectAnalysisCache::deferred(&manifest);
        assert!(
            !cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output),
                )
                .unwrap()
        );
        let original_snapshot = cache
            .observed_inputs
            .get("linked-ir")
            .unwrap()
            .snapshot
            .clone();
        let signature = original_snapshot.signature.clone();

        let error = cache
            .record_with_post_publish_hook(
                "linked-ir",
                "profile=a",
                std::slice::from_ref(&input),
                std::slice::from_ref(&output),
                || {
                    fs::rename(&input, &saved).unwrap();
                    fs::rename(&replacement, &input).unwrap();
                    fs::rename(&input, &replacement).unwrap();
                    fs::rename(&saved, &input).unwrap();
                },
            )
            .unwrap_err();

        assert!(error.to_string().contains("entering the persistent cache"));
        assert!(
            cache
                .store_mut()
                .unwrap()
                .stage_output_digests(&signature)
                .unwrap()
                .is_none()
        );
        assert!(!cache.observed_inputs.contains_key("linked-ir"));
        assert!(
            !cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output),
                )
                .unwrap()
        );
        assert_eq!(fs::read(&input).unwrap(), b"artifact-a");
        assert_eq!(
            cache
                .observed_inputs
                .get("linked-ir")
                .unwrap()
                .snapshot
                .signature,
            original_snapshot.signature
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_retires_a_restored_binding_when_inputs_change_after_rebind() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-analysis-cache-post-bind-input-race-{}",
            std::process::id()
        ));
        if directory.is_dir() {
            fs::remove_dir_all(&directory).unwrap();
        }
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("vendor.bin");
        let saved = directory.join("vendor.saved.bin");
        let replacement = directory.join("vendor.replacement.bin");
        let output = directory.join("generated/output.json");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&manifest, "schema = 1\n").unwrap();
        fs::write(&input, "artifact-a").unwrap();
        fs::write(&replacement, "artifact-b").unwrap();
        let output_bytes = vec![0x31; 128 * 1024];
        fs::write(&output, &output_bytes).unwrap();

        let mut cache = ProjectAnalysisCache::deferred(&manifest);
        assert!(
            !cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output),
                )
                .unwrap()
        );
        let original_snapshot = cache
            .observed_inputs
            .get("linked-ir")
            .unwrap()
            .snapshot
            .clone();
        let signature = original_snapshot.signature.clone();
        cache
            .record(
                "linked-ir",
                "profile=a",
                std::slice::from_ref(&input),
                std::slice::from_ref(&output),
            )
            .unwrap();
        fs::remove_file(&output).unwrap();

        assert!(
            !cache
                .is_current_with_post_bind_hook(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output),
                    || {
                        fs::rename(&input, &saved).unwrap();
                        fs::rename(&replacement, &input).unwrap();
                        fs::rename(&input, &replacement).unwrap();
                        fs::rename(&saved, &input).unwrap();
                    },
                )
                .unwrap()
        );

        assert_eq!(fs::read(&output).unwrap(), output_bytes);
        assert!(!cache.last_lookup_restored());
        assert!(
            cache
                .store_mut()
                .unwrap()
                .stage_output_digests(&signature)
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read(&input).unwrap(), b"artifact-a");
        assert_eq!(
            cache
                .observed_inputs
                .get("linked-ir")
                .unwrap()
                .snapshot
                .signature,
            original_snapshot.signature
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_refuses_an_atomic_input_rebind_even_when_the_original_inode_returns() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-analysis-cache-input-rebind-{}",
            std::process::id()
        ));
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("vendor.bin");
        let saved = directory.join("vendor.saved.bin");
        let replacement = directory.join("vendor.replacement.bin");
        let output = directory.join("generated/output.json");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&manifest, "schema = 1\n").unwrap();
        fs::write(&input, "artifact-a").unwrap();
        fs::write(&replacement, "artifact-b").unwrap();
        fs::write(&output, "output").unwrap();

        let mut cache = ProjectAnalysisCache::deferred(&manifest);
        assert!(
            !cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output),
                )
                .unwrap()
        );
        fs::rename(&input, &saved).unwrap();
        fs::rename(&replacement, &input).unwrap();
        fs::rename(&input, &replacement).unwrap();
        fs::rename(&saved, &input).unwrap();

        let error = cache
            .record(
                "linked-ir",
                "profile=a",
                std::slice::from_ref(&input),
                std::slice::from_ref(&output),
            )
            .unwrap_err();

        assert!(error.to_string().contains("changed while stage"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pipeline_guard_detects_an_aba_rebind_between_stages() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-pipeline-input-rebind-{}",
            std::process::id()
        ));
        if directory.is_dir() {
            fs::remove_dir_all(&directory).unwrap();
        }
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("vendor.bin");
        let saved = directory.join("vendor.saved.bin");
        let replacement = directory.join("vendor.replacement.bin");
        fs::write(&input, "artifact-a").unwrap();
        fs::write(&replacement, "artifact-b").unwrap();

        let mut observation = PipelineInputObservation::capture(vec![input.clone()]).unwrap();
        fs::rename(&input, &saved).unwrap();
        fs::rename(&replacement, &input).unwrap();
        fs::rename(&input, &replacement).unwrap();
        fs::rename(&saved, &input).unwrap();

        let error = observation.validate().unwrap_err();
        assert!(error.to_string().contains("changed during analysis"));
        assert_eq!(fs::read(&input).unwrap(), b"artifact-a");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cache_refuses_a_symlink_input_rebind_even_when_the_target_returns() {
        use std::os::unix::fs::symlink;

        let directory = std::env::temp_dir().join(format!(
            "blobray-analysis-cache-symlink-rebind-{}",
            std::process::id()
        ));
        let manifest = directory.join("vendor-project.toml");
        let first = directory.join("vendor-a.bin");
        let second = directory.join("vendor-b.bin");
        let input = directory.join("vendor.bin");
        let output = directory.join("generated/output.json");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&manifest, "schema = 1\n").unwrap();
        fs::write(&first, "artifact-a").unwrap();
        fs::write(&second, "artifact-b").unwrap();
        symlink(&first, &input).unwrap();
        fs::write(&output, "output").unwrap();

        let mut cache = ProjectAnalysisCache::deferred(&manifest);
        assert!(
            !cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output),
                )
                .unwrap()
        );
        fs::remove_file(&input).unwrap();
        symlink(&second, &input).unwrap();
        fs::remove_file(&input).unwrap();
        symlink(&first, &input).unwrap();

        let error = cache
            .record(
                "linked-ir",
                "profile=a",
                std::slice::from_ref(&input),
                std::slice::from_ref(&output),
            )
            .unwrap_err();

        assert!(error.to_string().contains("changed while stage"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_allows_a_stage_to_publish_a_sibling_of_its_input() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-analysis-cache-sibling-output-{}",
            std::process::id()
        ));
        if directory.is_dir() {
            fs::remove_dir_all(&directory).unwrap();
        }
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("symbols.json");
        let output = directory.join("mmio.json");
        fs::write(&manifest, "schema = 1\n").unwrap();
        fs::write(&input, "symbols").unwrap();

        let mut cache = ProjectAnalysisCache::deferred(&manifest);
        assert!(
            !cache
                .is_current(
                    "mmio-discovery",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output),
                )
                .unwrap()
        );
        fs::write(&output, "mmio").unwrap();

        cache
            .record(
                "mmio-discovery",
                "profile=a",
                std::slice::from_ref(&input),
                std::slice::from_ref(&output),
            )
            .unwrap();

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_hashes_files_inside_declared_input_directories() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-analysis-directory-cache-{}",
            std::process::id()
        ));
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("analysis.ir");
        let nested_input = input.join("functions.jsonl");
        let output = directory.join("generated/review.md");
        fs::create_dir_all(&input).unwrap();
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&manifest, "schema = 1\n").unwrap();
        fs::write(&nested_input, "facts-a").unwrap();
        fs::write(&output, "review-a").unwrap();

        let mut cache = ProjectAnalysisCache::deferred(&manifest);
        assert!(
            !cache
                .is_current(
                    "function-review",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output),
                )
                .unwrap()
        );
        cache
            .record(
                "function-review",
                "profile=a",
                std::slice::from_ref(&input),
                std::slice::from_ref(&output),
            )
            .unwrap();
        assert!(
            cache
                .is_current(
                    "function-review",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output)
                )
                .unwrap()
        );

        fs::write(&nested_input, "facts-b-expanded").unwrap();
        assert!(
            !cache
                .is_current(
                    "function-review",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output)
                )
                .unwrap()
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_signatures_distinguish_optional_absence_from_later_presence() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-analysis-missing-cache-input-{}",
            std::process::id()
        ));
        if directory.is_dir() {
            fs::remove_dir_all(&directory).unwrap();
        }
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("vendor-project.toml");
        let missing = directory.join("missing-vendor.a");
        fs::write(&manifest, "schema = 3\n").unwrap();
        let mut cache = ProjectAnalysisCache::deferred(&manifest);

        let absent = cache
            .signature(
                "symbol-inventory",
                "symbols",
                std::slice::from_ref(&missing),
            )
            .unwrap();
        fs::write(&missing, "now available").unwrap();
        let present = cache
            .signature(
                "symbol-inventory",
                "symbols",
                std::slice::from_ref(&missing),
            )
            .unwrap();

        assert_ne!(absent, present);
        assert!(!directory.join("generated").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn equivalent_profile_bindings_restore_one_shared_result() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-shared-profile-cache-{}",
            std::process::id()
        ));
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("vendor.a");
        let focused = directory.join("generated/focused/functions.jsonl");
        let renamed = directory.join("generated/renamed/functions.jsonl");
        fs::create_dir_all(focused.parent().unwrap()).unwrap();
        fs::write(&manifest, "schema = 3\n").unwrap();
        fs::write(&input, "artifact-bytes").unwrap();
        fs::write(&focused, "recovered-function").unwrap();

        let mut cache = ProjectAnalysisCache::deferred(&manifest);
        assert!(
            !cache
                .is_current(
                    "linked-ir:focused",
                    "sources=[vendor];roots=all",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&focused),
                )
                .unwrap()
        );
        cache
            .record(
                "linked-ir:focused",
                "sources=[vendor];roots=all",
                std::slice::from_ref(&input),
                std::slice::from_ref(&focused),
            )
            .unwrap();
        assert!(
            cache
                .is_current(
                    "linked-ir:renamed",
                    "sources=[vendor];roots=all",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&renamed),
                )
                .unwrap()
        );
        assert_eq!(fs::read_to_string(renamed).unwrap(), "recovered-function");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn every_cached_generator_has_an_explicit_semantic_revision() {
        for stage in [
            "symbol-inventory",
            "mmio-discovery",
            "interface-discovery",
            "linked-ir",
            "event-replays",
            "review-scopes",
            "navigation-index",
            "code-boundary-review",
            "register-review",
            "function-review",
            "code-boundary-validation:deny-unreviewed=false",
            "code-boundary-validation:deny-unreviewed=true",
            "register-validation:deny-unreviewed=false",
            "register-validation:deny-unreviewed=true",
            "function-validation:deny-unreviewed=false",
            "function-validation:deny-unreviewed=true",
            "interface-validation:deny-unreviewed=false",
            "interface-validation:deny-unreviewed=true",
        ] {
            assert!(stage_revision(stage).unwrap() > 0);
        }
        assert!(stage_revision("new-unversioned-stage").is_err());
        assert_eq!(stage_revision("linked-ir").unwrap(), 42);
        assert_eq!(stage_revision("linked-ir:any-profile").unwrap(), 42);
    }

    #[test]
    fn persistent_artifact_stages_include_their_strict_output_schema() {
        assert_eq!(
            stage_artifact_schema("symbol-inventory"),
            Some(crate::artifacts::SYMBOL_INVENTORY)
        );
        assert_eq!(
            stage_artifact_schema("mmio-discovery"),
            Some(crate::artifacts::MMIO_FACTS)
        );
        assert_eq!(
            stage_artifact_schema("interface-discovery"),
            Some(crate::artifacts::INTERFACE_FACTS)
        );
        assert_eq!(
            stage_artifact_schema("linked-ir:focused"),
            Some(crate::artifacts::LINKED_IR)
        );
        assert_eq!(
            stage_artifact_schema("event-replays"),
            Some(crate::artifacts::REPLAY_EVIDENCE)
        );
        assert_eq!(stage_artifact_schema("register-review"), None);
    }

    #[test]
    fn linked_ir_profile_ids_are_bindings_not_query_inputs() {
        let directory =
            std::env::temp_dir().join(format!("blobray-profile-query-key-{}", std::process::id()));
        if directory.is_dir() {
            fs::remove_dir_all(&directory).unwrap();
        }
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("artifact.elf");
        fs::write(&input, "same-artifact").unwrap();
        let mut cache = ProjectAnalysisCache::deferred(&manifest);

        let focused = cache
            .signature(
                "linked-ir:focused-name",
                "roots=all;include-reachable=true",
                std::slice::from_ref(&input),
            )
            .unwrap();
        let full = cache
            .signature(
                "linked-ir:different-name",
                "roots=all;include-reachable=true",
                std::slice::from_ref(&input),
            )
            .unwrap();

        assert_eq!(focused, full);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn linked_ir_harness_domain_changes_the_outer_stage_query_key() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-linked-ir-domain-query-key-{}",
            std::process::id()
        ));
        if directory.is_dir() {
            fs::remove_dir_all(&directory).unwrap();
        }
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("artifact.elf");
        fs::write(&input, "same-artifact").unwrap();
        let mut cache = ProjectAnalysisCache::deferred(&manifest);

        let first = cache
            .signature(
                "linked-ir:rom-all",
                "roots=all;riscv-semantic-cache-domain=provider/riscv/v1",
                std::slice::from_ref(&input),
            )
            .unwrap();
        let second = cache
            .signature(
                "linked-ir:rom-all",
                "roots=all;riscv-semantic-cache-domain=provider/riscv/v2",
                std::slice::from_ref(&input),
            )
            .unwrap();

        assert_ne!(first, second);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn compiled_knowledge_revision_invalidates_only_semantic_stages() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-provider-revision-cache-{}",
            std::process::id()
        ));
        if directory.is_dir() {
            fs::remove_dir_all(&directory).unwrap();
        }
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("artifact.elf");
        fs::write(&input, "same-artifact").unwrap();

        let mut first = ProjectAnalysisCache::deferred(&manifest)
            .with_compiled_knowledge_identity("provider@1".to_owned());
        let mut second = ProjectAnalysisCache::deferred(&manifest)
            .with_compiled_knowledge_identity("provider@2".to_owned());
        let inputs = std::slice::from_ref(&input);

        assert_ne!(
            first
                .signature("linked-ir:test", "roots=all", inputs)
                .unwrap(),
            second
                .signature("linked-ir:test", "roots=all", inputs)
                .unwrap()
        );
        assert_ne!(
            first.signature("event-replays", "cases=a", inputs).unwrap(),
            second
                .signature("event-replays", "cases=a", inputs)
                .unwrap()
        );
        assert_eq!(
            first
                .signature("symbol-inventory", "sources=a", inputs)
                .unwrap(),
            second
                .signature("symbol-inventory", "sources=a", inputs)
                .unwrap()
        );

        fs::remove_dir_all(directory).unwrap();
    }
}
