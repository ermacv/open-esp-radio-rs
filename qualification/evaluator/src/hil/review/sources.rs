//! Independently bind reviewed files to sealed build inputs, never live labels.
use super::*;
use crate::hil::{build_record::BuildEvidence, snapshot::Source};
use serde_json::Value;
use std::process::Command;

#[derive(Deserialize, PartialEq)]
struct Build {
    schema: u16,
    build_id: String,
    build_type: String,
    source_reconstructable: bool,
    parameters: serde_json::Value,
    sources: Vec<Source>,
    subjects: Vec<Subject>,
    #[serde(default)]
    files: Vec<BuildFile>,
    #[serde(default)]
    environment: Value,
}
#[derive(Deserialize, PartialEq)]
struct Subject {
    role: String,
    size_bytes: u64,
    sha256: String,
}

fn build(observation: &BuildEvidence, image: &str) -> Result<Option<Build>> {
    let run = &observation.directory;
    let subject = &observation.subject;
    let Some(firmware) = subject
        .firmware
        .iter()
        .find(|f| f.image.as_deref() == Some(image))
    else {
        return Ok(None);
    };
    let (Some(provenance), Some(application)) = (&firmware.build_provenance, &firmware.application)
    else {
        return Ok(None);
    };
    if subject.firmware.iter().any(|f| f.replayed) {
        return Ok(None);
    }
    let build: Build = read_json(&run.join(&provenance.path))?;
    if build.schema != 1
        || build.build_type != "open-esp-radio-hil-firmware/v1"
        || Some(&build.build_id) != firmware.build_id.as_ref()
        || !build.source_reconstructable
        || build.parameters.get("image").and_then(Value::as_str) != Some(image)
    {
        return Ok(None);
    }
    let applications = build
        .subjects
        .iter()
        .filter(|s| s.role == "application")
        .collect::<Vec<_>>();
    if applications.len() != 1
        || applications[0].sha256 != application.sha256
        || applications[0].size_bytes != application.size_bytes
    {
        return Ok(None);
    }
    let Some(repository) = build.sources.first() else {
        return Ok(None);
    };
    if repository.name != "repository"
        || repository.commit != subject.repository.commit
        || repository.dirty != subject.repository.dirty
        || repository.workspace_sha256 != subject.repository.workspace_sha256
    {
        return Ok(None);
    }
    let mut names = BTreeSet::new();
    if build.sources.iter().any(|s| {
        !names.insert(&s.name)
            || !valid_id(&s.name)
            || !s.limitations.is_empty()
            || !s.untracked_files.is_empty()
            || s.tracked_patch_path.is_some()
            || !valid_sha256(&s.workspace_sha256)
            || !matches!(
                s.rebuild_status.as_str(),
                "clean-commit" | "source-snapshot"
            )
            || (s.rebuild_status == "clean-commit" && (s.dirty || !commit_id(&s.commit)))
    }) {
        return Ok(None);
    }
    Ok(Some(build))
}

#[derive(Deserialize, PartialEq)]
struct BuildFile {
    name: String,
    path: PathBuf,
    archive_path: Option<PathBuf>,
    size_bytes: Option<u64>,
    sha256: String,
}

pub(super) fn same_dependencies(
    a: &BuildEvidence,
    ai: &str,
    b: &BuildEvidence,
    bi: &str,
    roots: &[String],
    current_root: &Path,
    owners: &[PathBuf],
) -> Result<bool> {
    let (Some(left), Some(right)) = (build(a, ai)?, build(b, bi)?) else {
        return Ok(false);
    };
    // Reusing the very same recorded build does not infer any missing build
    // selection. Older records remain usable for that exact application.
    if left == right && roots.is_empty() {
        return Ok(true);
    }
    let (Some(a), Some(b)) = (
        composition(&left, a, roots)?,
        composition(&right, b, roots)?,
    ) else {
        return Ok(false);
    };
    Ok(left.parameters == right.parameters
        && left.sources[1..] == right.sources[1..]
        && a == b
        && (roots.is_empty() || dependencies::current(current_root, roots, owners, &a)?))
}

fn composition(
    build: &Build,
    observation: &BuildEvidence,
    roots: &[String],
) -> Result<Option<Value>> {
    if !matches!(
        build.parameters.get("network").and_then(Value::as_str),
        Some("upstream-xarxa" | "patched-xarxa" | "upstream-smoltcp" | "owned-xarxa")
    ) {
        return Ok(None);
    }
    for name in ["image", "runtime_profile", "target", "runtime_features"] {
        if !build
            .parameters
            .get(name)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
        {
            return Ok(None);
        }
    }
    let run = &observation.directory;
    let mut locks = BTreeMap::new();
    for name in ["embedded-lock", "bootstrap-lock"] {
        let files = build
            .files
            .iter()
            .filter(|file| file.name == name)
            .collect::<Vec<_>>();
        let [file] = files.as_slice() else {
            return Ok(None);
        };
        let Some(path) = &file.archive_path else {
            return Ok(None);
        };
        let Some(actual) = crate::hil::subject::file(run, path)? else {
            return Ok(None);
        };
        if !valid_sha256(&file.sha256)
            || actual.sha256 != file.sha256
            || Some(actual.size_bytes) != file.size_bytes
        {
            return Ok(None);
        }
        let identity = if name == "embedded-lock" && !roots.is_empty() {
            dependencies::archived(
                &observation.directory,
                &build.sources,
                roots,
                &fs::read(run.join(path))?,
            )?
        } else {
            serde_json::json!({"path":file.path,"sha256":file.sha256})
        };
        locks.insert(name, identity);
    }
    let Some(tools) = build.environment.get("tools").and_then(Value::as_array) else {
        return Ok(None);
    };
    let mut versions = BTreeMap::new();
    // Tool installation paths do not define compiled behavior. Include espflash
    // because it also encodes the application image, not only flashing.
    for name in ["rustc", "cargo", "llvm-objcopy", "llvm-nm", "espflash"] {
        let tools = tools
            .iter()
            .filter(|tool| tool["name"] == name)
            .collect::<Vec<_>>();
        let [tool] = tools.as_slice() else {
            return Ok(None);
        };
        let Some(version) = tool["version"].as_str().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        versions.insert(name, version);
    }
    let mut environment = BTreeMap::new();
    for name in [
        "inherited_rustflags",
        "inherited_encoded_rustflags",
        "cargo_incremental",
        "source_date_epoch",
    ] {
        let Some(value) = build.environment.get(name) else {
            return Ok(None);
        };
        if (name == "cargo_incremental" && value.as_str() != Some("0"))
            || (name != "cargo_incremental" && !value.is_null() && !value.is_string())
        {
            return Ok(None);
        }
        environment.insert(name, value);
    }
    Ok(Some(
        serde_json::json!({"locks":locks,"tools":versions,"environment":environment}),
    ))
}

pub(super) fn matches(
    root: &Path,
    observation: &BuildEvidence,
    image: &str,
    inputs: &[InputBinding],
) -> Result<bool> {
    let Some(build) = build(observation, image)? else {
        return Ok(false);
    };
    let primary = &build.sources[0];
    if primary.rebuild_status == "source-snapshot" {
        return snapshot(observation, &build.sources, inputs);
    }
    for input in inputs {
        if input.kind == InputKind::Evidence {
            continue;
        }
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["cat-file", "blob"])
            .arg(format!("{}:{}", primary.commit, input.path.display()))
            .output()?;
        if !output.status.success() || input_hash(&output.stdout, &input.kind)? != input.sha256 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn commit_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn snapshot(
    observation: &BuildEvidence,
    sources: &[Source],
    inputs: &[InputBinding],
) -> Result<bool> {
    let run = &observation.directory;
    let subject = &observation.subject;
    let Some(file) = &subject.source_snapshot_manifest else {
        return Ok(false);
    };
    let directory = run
        .join(&file.path)
        .parent()
        .ok_or("snapshot manifest has no parent")?
        .to_owned();
    let Some(manifest) = crate::hil::snapshot::verified(&directory, sources)? else {
        return Ok(false);
    };
    let Some(repository) = manifest.sources.first() else {
        return Ok(false);
    };
    for input in inputs {
        if input.kind == InputKind::Evidence {
            continue;
        }
        let Some(file) = repository.files.iter().find(|file| file.path == input.path) else {
            return Ok(false);
        };
        if input.kind == InputKind::Bytes {
            if file.sha256 != input.sha256 {
                return Ok(false);
            }
        } else {
            let bytes = dependencies::archive_file(
                &directory.join("sources.tar"),
                &Path::new("repository").join(&input.path),
            )?;
            if input_hash(&bytes, &input.kind)? != input.sha256 {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
