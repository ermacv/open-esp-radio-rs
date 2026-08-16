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

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

const CACHE_SCHEMA: u32 = 3;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CacheDocument {
    schema: u32,
    stages: BTreeMap<String, CacheStage>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CacheStage {
    signature: String,
    outputs: BTreeMap<String, String>,
}

pub(super) struct ProjectAnalysisCache {
    path: PathBuf,
    document: CacheDocument,
    digests: BTreeMap<PathBuf, DigestMemo>,
}

struct DigestMemo {
    len: u64,
    modified: Option<std::time::SystemTime>,
    value: String,
}

impl ProjectAnalysisCache {
    pub(super) fn load(project_manifest: &Path) -> Self {
        let root = project_manifest.parent().unwrap_or_else(|| Path::new("."));
        let path = root.join("generated/.project-analyze-cache.json");
        let document = fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str::<CacheDocument>(&contents).ok())
            .filter(|document| document.schema == CACHE_SCHEMA)
            .unwrap_or_else(|| CacheDocument {
                schema: CACHE_SCHEMA,
                stages: BTreeMap::new(),
            });
        Self {
            path,
            document,
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
        if outputs.iter().any(|path| !path.is_file()) {
            return Ok(false);
        }
        let signature = self.signature(stage, configuration, inputs)?;
        let Some(cached) = self.document.stages.get(stage) else {
            return Ok(false);
        };
        if cached.signature != signature || cached.outputs.len() != outputs.len() {
            return Ok(false);
        }
        let expected_outputs = cached.outputs.clone();
        for path in outputs {
            let key = path_key(path);
            let Some(expected) = expected_outputs.get(&key) else {
                return Ok(false);
            };
            if &self.digest(path)? != expected {
                return Ok(false);
            }
        }
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
        let mut output_digests = BTreeMap::new();
        for path in outputs {
            self.digests.remove(path);
            output_digests.insert(path_key(path), self.digest(path)?);
        }
        self.document.stages.insert(
            stage.to_owned(),
            CacheStage {
                signature,
                outputs: output_digests,
            },
        );
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &self.path,
            serde_json::to_string_pretty(&self.document)? + "\n",
        )?;
        Ok(())
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
        digest.update(b"vendor-binary-workbench-project-stage-v3\0");
        digest.update(stage.as_bytes());
        digest.update([0]);
        digest.update(env!("CARGO_PKG_VERSION").as_bytes());
        digest.update([0]);
        digest.update(stage_revision(stage)?.to_le_bytes());
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
        digest.update(b"vendor-binary-workbench-directory-v1\0");
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

/// Explicit semantic revision of each cached generator.
///
/// A digest of the whole executable made presentation-only changes invalidate
/// every expensive artifact-wide stage.  Bump only the owner below when its
/// generated document or analysis semantics change.  Input/output content
/// hashes continue to protect project and caller-owned state.
fn stage_revision(stage: &str) -> Result<u32> {
    match stage {
        "symbol-inventory" => Ok(2),
        "mmio-discovery" => Ok(4),
        "interface-discovery" => Ok(3),
        "linked-ir" => Ok(20),
        "event-replays" => Ok(1),
        "review-scopes" => Ok(3),
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
    fn cache_requires_unchanged_inputs_and_outputs() {
        let directory = std::env::temp_dir().join(format!(
            "vendor-workbench-analysis-cache-{}",
            std::process::id()
        ));
        let manifest = directory.join("vendor-project.toml");
        let input = directory.join("input.bin");
        let output = directory.join("generated/output.json");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&manifest, "schema = 1\n").unwrap();
        fs::write(&input, "input-a").unwrap();
        fs::write(&output, "output-a").unwrap();

        let mut cache = ProjectAnalysisCache::load(&manifest);
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
            !cache
                .is_current(
                    "linked-ir",
                    "profile=a",
                    std::slice::from_ref(&input),
                    std::slice::from_ref(&output)
                )
                .unwrap()
        );

        fs::write(&output, "output-a").unwrap();
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
            "vendor-workbench-analysis-directory-cache-{}",
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

        let mut cache = ProjectAnalysisCache::load(&manifest);
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
}
