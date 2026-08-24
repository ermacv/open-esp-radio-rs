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
    store: Option<crate::application::query_store::QueryStore>,
    store_error: Option<String>,
    compiled_knowledge_identity: String,
    digests: BTreeMap<PathBuf, DigestMemo>,
}

struct DigestMemo {
    len: u64,
    modified: Option<std::time::SystemTime>,
    value: String,
}

impl ProjectAnalysisCache {
    pub(super) fn load(project_manifest: &Path, compiled_knowledge_identity: String) -> Self {
        let (store, store_error) =
            match crate::application::query_store::QueryStore::open(project_manifest) {
                Ok(store) => (Some(store), None),
                Err(error) => (None, Some(error.to_string())),
            };
        Self {
            store,
            store_error,
            compiled_knowledge_identity,
            digests: BTreeMap::new(),
        }
    }

    pub(super) fn is_current(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
        outputs: &[PathBuf],
    ) -> Result<bool> {
        let signature = self.signature(stage, configuration, inputs)?;
        let Some(cached) = self.store()?.stage_output_digests(&signature)? else {
            return Ok(false);
        };
        if cached.len() != outputs.len() {
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
                self.store()?.restore_output(expected, path)?;
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
        let paths = outputs
            .iter()
            .map(|path| path_key(path))
            .collect::<Vec<_>>();
        self.store_mut()?
            .bind_restored_stage(stage, &signature, &paths, &cached)?;
        Ok(true)
    }

    pub(super) fn record(
        &mut self,
        stage: &str,
        configuration: &str,
        inputs: &[PathBuf],
        outputs: &[PathBuf],
    ) -> Result<()> {
        let signature = self.signature(stage, configuration, inputs)?;
        let mut cached_outputs = Vec::with_capacity(outputs.len());
        for path in outputs {
            self.digests.remove(path);
            cached_outputs.push((path_key(path), self.digest(path)?, path.clone()));
        }
        self.store_mut()?
            .record_stage(stage, &signature, &cached_outputs)?;
        Ok(())
    }

    fn store(&self) -> Result<&crate::application::query_store::QueryStore> {
        self.store.as_ref().ok_or_else(|| {
            crate::Error::invalid(format!(
                "persistent query store is unavailable: {}",
                self.store_error.as_deref().unwrap_or("unknown error")
            ))
        })
    }

    fn store_mut(&mut self) -> Result<&mut crate::application::query_store::QueryStore> {
        self.store.as_mut().ok_or_else(|| {
            crate::Error::invalid(format!(
                "persistent query store is unavailable: {}",
                self.store_error.as_deref().unwrap_or("unknown error")
            ))
        })
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
            // Persistent document formats are owned independently from the
            // analysis algorithm revision. Including the exact output schema
            // here prevents a content-addressed cache hit from restoring an
            // older document that the current strict reader must reject.
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
        fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
            for entry in fs::read_dir(directory)? {
                let path = entry?.path();
                if path.is_dir() {
                    collect_files(&path, output)?;
                } else if path.is_file() {
                    output.push(path);
                }
            }
            Ok(())
        }

        let mut files = Vec::new();
        collect_files(root, &mut files)?;
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
        let modified = metadata.modified().ok();
        if let Some(existing) = self.digests.get(path)
            && existing.len == metadata.len()
            && existing.modified == modified
        {
            return Ok(existing.value.clone());
        }
        let value = crate::artifact_sha256(path)?;
        self.digests.insert(
            path.to_owned(),
            DigestMemo {
                len: metadata.len(),
                modified,
                value: value.clone(),
            },
        );
        Ok(value)
    }
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

    fn load_cache(manifest: &Path) -> ProjectAnalysisCache {
        ProjectAnalysisCache::load(manifest, "test-knowledge@1".to_owned())
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

        let mut cache = load_cache(&manifest);
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

        let mut cache = load_cache(&manifest);
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

        let mut cache = load_cache(&manifest);
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
        let mut cache = load_cache(&manifest);

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

        let mut first = ProjectAnalysisCache::load(&manifest, "provider@1".to_owned());
        let mut second = ProjectAnalysisCache::load(&manifest, "provider@2".to_owned());
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
