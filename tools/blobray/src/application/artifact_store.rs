//! Session-owned published analysis inputs and one retained indexed IR reader.
//!
//! The first query selects one published epoch, including a retained failure
//! when none is available. Every profile then binds its files from that same
//! manifest. Reload creates a new owner; there is no generated-path fallback.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use super::PublishedAnalysisOutputs;
use crate::{Result, artifacts::LinkedIrReader};

pub(crate) struct PublishedIrEvidence {
    pub(crate) reader: Arc<LinkedIrReader>,
    pub(crate) manifest: Vec<u8>,
    pub(crate) members: BTreeMap<String, String>,
}

pub(crate) struct ProjectArtifactStore {
    manifest: PathBuf,
    declared_ir: BTreeSet<PathBuf>,
    state: Mutex<State>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{output_set::OutputSet, query_store::QueryStore};

    fn project(directory: &Path) -> (PathBuf, crate::ProjectSpec) {
        let manifest = directory.join("project.toml");
        let target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generic-project/target.toml");
        std::fs::write(&manifest, format!("schema = 4\nid = \"epoch-readers\"\ntarget-spec = {:?}\n[[analysis.ir]]\nid = \"first\"\nroots = \"all\"\noutput = \"generated/first.ir\"\n[[analysis.ir]]\nid = \"second\"\nroots = \"all\"\noutput = \"generated/second.ir\"\n", target.display().to_string())).unwrap();
        let project = crate::ProjectSpec::load(&manifest).unwrap();
        (manifest, project)
    }

    fn bundle(path: &Path, count: usize) {
        let mut document: serde_json::Value = serde_json::from_str(
            &crate::artifacts::render_linked_ir_fixture(Vec::new(), Vec::new()),
        )
        .unwrap();
        document["summary"]["functions"] = serde_json::json!(count);
        crate::artifacts::write_fixture_bundle(path, &document.to_string()).unwrap();
    }

    fn publish(manifest: &Path, paths: &[PathBuf]) {
        let outputs = OutputSet::new(paths, false).unwrap();
        for (index, path) in paths.iter().enumerate() {
            let bytes = std::fs::read(path).unwrap();
            outputs
                .file(index, "fixture")
                .unwrap()
                .bytes(&bytes)
                .unwrap();
        }
        QueryStore::open_analysis_epoch(manifest)
            .unwrap()
            .publish_analysis_outputs(&outputs.receipts().unwrap())
            .unwrap();
    }

    #[test]
    fn profile_eviction_and_auxiliary_reads_keep_the_original_published_epoch() {
        let directory = tempfile::tempdir().unwrap();
        let (manifest, project) = project(directory.path());
        let first = &project.ir_profiles[0].output;
        let second = &project.ir_profiles[1].output;
        let auxiliary = directory.path().join("generated/notes");
        let mut paths = project
            .ir_profiles
            .iter()
            .flat_map(|profile| crate::artifacts::bundle_files(&profile.output))
            .collect::<Vec<_>>();
        paths.push(auxiliary.clone());
        bundle(first, 11);
        bundle(second, 12);
        std::fs::write(&auxiliary, b"old evidence").unwrap();
        publish(&manifest, &paths);
        let store = ProjectArtifactStore::new(&manifest, &project);
        assert_eq!(store.linked_ir(first).unwrap().summary().functions, 11);
        assert_eq!(
            store.read_output(&auxiliary).unwrap().unwrap(),
            b"old evidence"
        );
        bundle(first, 21);
        bundle(second, 22);
        std::fs::write(&auxiliary, b"new evidence").unwrap();
        publish(&manifest, &paths);
        for path in &paths {
            std::fs::write(path, b"invalid exported content").unwrap();
        }
        assert_eq!(store.linked_ir(second).unwrap().summary().functions, 12);
        assert_eq!(store.linked_ir(first).unwrap().summary().functions, 11);
        assert_eq!(
            store.read_output(&auxiliary).unwrap().unwrap(),
            b"old evidence"
        );
        let fresh = ProjectArtifactStore::new(&manifest, &project);
        assert_eq!(fresh.linked_ir(first).unwrap().summary().functions, 21);
        assert_eq!(fresh.linked_ir(second).unwrap().summary().functions, 22);
        assert_eq!(
            fresh.read_output(&auxiliary).unwrap().unwrap(),
            b"new evidence"
        );
        assert!(
            fresh
                .linked_ir(&directory.path().join("undeclared.ir"))
                .err()
                .unwrap()
                .to_string()
                .contains("not a declared")
        );
    }

    #[test]
    fn exported_bundle_cannot_fill_a_missing_member_in_the_selected_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let (manifest, project) = project(directory.path());
        for profile in &project.ir_profiles {
            bundle(&profile.output, 1);
        }
        let first = &project.ir_profiles[0].output;
        publish(
            &manifest,
            &crate::artifacts::bundle_files(first).collect::<Vec<_>>(),
        );
        let store = ProjectArtifactStore::new(&manifest, &project);
        assert_eq!(store.linked_ir(first).unwrap().summary().functions, 1);
        let error = store
            .linked_ir(&project.ir_profiles[1].output)
            .err()
            .unwrap();
        assert!(error.to_string().contains("has no declared IR member"));
    }
}

#[derive(Default)]
struct State {
    published: OnceLock<std::result::Result<PublishedAnalysisOutputs, String>>,
    linked_ir: Option<(PathBuf, Arc<LinkedIrReader>)>,
}

impl State {
    fn published(&self, manifest: &Path) -> Result<&PublishedAnalysisOutputs> {
        self.published.get_or_init(|| {
            PublishedAnalysisOutputs::open(manifest).map_err(|error| error.to_string())?
                .ok_or_else(|| "project has no published analysis epoch; run project analyze, then reload the session".to_owned())
        }).as_ref().map_err(|reason| crate::Error::invalid(reason.clone()))
    }
}

impl ProjectArtifactStore {
    pub(super) fn selected_epoch(&self) -> Result<Option<String>> {
        let state = self
            .state
            .lock()
            .map_err(|_| crate::Error::invalid("project artifact store lock was poisoned"))?;
        Ok(state
            .published
            .get()
            .and_then(|result| result.as_ref().ok())
            .map(|published| published.manifest().epoch.clone()))
    }
    pub(super) fn new(manifest: &Path, project: &crate::ProjectSpec) -> Self {
        Self {
            manifest: manifest.to_owned(),
            declared_ir: project
                .ir_profiles
                .iter()
                .map(|profile| profile.output.clone())
                .collect(),
            state: Mutex::new(State::default()),
        }
    }

    pub(super) fn read_output(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        let state = self
            .state
            .lock()
            .map_err(|_| crate::Error::invalid("project artifact store lock was poisoned"))?;
        state
            .published(&self.manifest)?
            .read(path)
            .map_err(|error| crate::Error::invalid(error.to_string()))
    }

    pub(super) fn read_text(&self, path: &Path) -> Result<String> {
        let bytes = self.read_output(path)?.ok_or_else(|| {
            crate::Error::invalid(format!(
                "selected published analysis epoch has no output {}; run project analyze and reload",
                path.display()
            ))
        })?;
        String::from_utf8(bytes).map_err(|error| {
            crate::Error::invalid(format!(
                "published analysis output {} is not UTF-8: {error}",
                path.display()
            ))
        })
    }

    pub(super) fn ir_evidence(&self, path: &Path) -> Result<PublishedIrEvidence> {
        let reader = self.linked_ir(path)?;
        let state = self
            .state
            .lock()
            .map_err(|_| crate::Error::invalid("project artifact store lock was poisoned"))?;
        let published = state.published(&self.manifest)?;
        let mut members = BTreeMap::new();
        for input in crate::artifacts::bundle_files(path) {
            let output = published
                .manifest()
                .outputs
                .iter()
                .find(|output| output.path == input)
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "published IR bundle has no member {}",
                        input.display()
                    ))
                })?;
            members.insert(
                input
                    .file_name()
                    .expect("bundle member")
                    .to_string_lossy()
                    .into_owned(),
                output.sha256.clone(),
            );
        }
        let manifest = published
            .read(&path.join("manifest.json"))
            .map_err(|error| crate::Error::invalid(error.to_string()))?
            .ok_or_else(|| crate::Error::invalid("published IR bundle has no manifest"))?;
        Ok(PublishedIrEvidence {
            reader,
            manifest,
            members,
        })
    }

    pub(super) fn linked_ir(&self, path: &Path) -> Result<Arc<LinkedIrReader>> {
        if !self.declared_ir.contains(path) {
            return Err(crate::Error::invalid(format!(
                "IR path {} is not a declared project analysis output",
                path.display()
            )));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::Error::invalid("project artifact store lock was poisoned"))?;
        if let Some((cached_path, cached_reader)) = &state.linked_ir
            && cached_path == path
        {
            return Ok(Arc::clone(cached_reader));
        }
        // Release a different profile's indexes before parsing the next one.
        // Retain the epoch across profile eviction, so an MRU miss never selects
        // newer analysis results during an existing session.
        state.linked_ir = None;
        let published = state.published(&self.manifest)?;
        let files = crate::artifacts::BUNDLE_FILES.into_iter().map(|name| {
            let member = path.join(name);
            let file = published.output_view(&member)?.ok_or_else(|| {
                crate::Error::invalid(format!("published epoch {} has no declared IR member {}; rerun project analyze and reload", published.manifest().epoch, member.display()))
            })?;
            Ok((name, file))
        }).collect::<Result<BTreeMap<_, _>>>()?;
        let loaded = Arc::new(LinkedIrReader::from_files(path, files)?);
        state.linked_ir = Some((path.to_owned(), Arc::clone(&loaded)));
        Ok(loaded)
    }
}
