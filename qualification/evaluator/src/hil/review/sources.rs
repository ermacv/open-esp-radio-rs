//! Independently bind reviewed files to sealed build inputs, never live labels.
use super::*;
use serde_json::Value;
use std::process::Command;

#[derive(Deserialize)]
struct Build {
    schema: u16,
    build_id: String,
    build_type: String,
    source_reconstructable: bool,
    parameters: serde_json::Value,
    sources: Vec<Source>,
    subjects: Vec<Subject>,
}
#[derive(Deserialize, PartialEq)]
struct Source {
    name: String,
    commit: String,
    dirty: bool,
    workspace_sha256: String,
    rebuild_status: String,
    limitations: Vec<Value>,
    untracked_files: Vec<Value>,
    tracked_patch_path: Option<PathBuf>,
}
#[derive(Deserialize)]
struct Subject {
    role: String,
    size_bytes: u64,
    sha256: String,
}

fn build(observation: &ScenarioEvidence, image: &str) -> Result<Option<Build>> {
    let Some(run) = &observation.run_directory else {
        return Ok(None);
    };
    let Some(subject) = &observation.subject else {
        return Ok(None);
    };
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

pub(super) fn same_dependencies(
    a: &ScenarioEvidence,
    ai: &str,
    b: &ScenarioEvidence,
    bi: &str,
) -> Result<bool> {
    let (Some(a), Some(b)) = (build(a, ai)?, build(b, bi)?) else {
        return Ok(false);
    };
    // Build selection/features must not change implicitly with the image class.
    Ok(a.parameters == b.parameters && a.sources[1..] == b.sources[1..])
}

pub(super) fn matches(
    root: &Path,
    observation: &ScenarioEvidence,
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
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["cat-file", "blob"])
            .arg(format!("{}:{}", primary.commit, input.path.display()))
            .output()?;
        if !output.status.success() || digest(&output.stdout) != input.sha256 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn commit_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// Serialization order is the producer's v1 snapshot identity contract.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileInput {
    path: PathBuf,
    size_bytes: u64,
    sha256: String,
    mode: u32,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceInput {
    name: String,
    commit: String,
    dirty: bool,
    files: Vec<FileInput>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u16,
    sources: Vec<SourceInput>,
}
#[derive(Deserialize)]
struct Snapshot {
    schema: u16,
    snapshot_id: String,
    archive_sha256: String,
    files: usize,
}

fn snapshot(
    observation: &ScenarioEvidence,
    sources: &[Source],
    inputs: &[InputBinding],
) -> Result<bool> {
    let Some(run) = &observation.run_directory else {
        return Ok(false);
    };
    let Some(subject) = &observation.subject else {
        return Ok(false);
    };
    let Some(manifest_file) = &subject.source_snapshot_manifest else {
        return Ok(false);
    };
    let directory = run
        .join(&manifest_file.path)
        .parent()
        .ok_or("snapshot manifest has no parent")?
        .to_owned();
    let manifest: Manifest = read_json(&directory.join("manifest.json"))?;
    let snapshot: Snapshot = read_json(&directory.join("snapshot.json"))?;
    if manifest.schema != 1
        || snapshot.schema != 1
        || digest(&serde_json::to_vec(&manifest)?) != snapshot.snapshot_id
        || sha256_file(&directory.join("sources.tar"))? != snapshot.archive_sha256
        || sources.len() != manifest.sources.len()
    {
        return Ok(false);
    }
    let mut expected = BTreeMap::new();
    for (captured, source) in manifest.sources.iter().zip(sources) {
        if captured.name != source.name
            || captured.commit != source.commit
            || captured.dirty != source.dirty
            || digest(&serde_json::to_vec(captured)?) != source.workspace_sha256
        {
            return Ok(false);
        }
        for file in &captured.files {
            if !safe_relative(&file.path)
                || !valid_sha256(&file.sha256)
                || !matches!(file.mode, 0o644 | 0o755)
                || expected
                    .insert(PathBuf::from(&captured.name).join(&file.path), file)
                    .is_some()
            {
                return Ok(false);
            }
        }
    }
    if snapshot.files != expected.len()
        || !inputs.iter().all(|i| {
            expected
                .get(&Path::new("repository").join(&i.path))
                .is_some_and(|f| f.sha256 == i.sha256)
        })
    {
        return Ok(false);
    }
    // Check all archived bytes without extracting anything into the filesystem.
    let mut archive = tar::Archive::new(fs::File::open(directory.join("sources.tar"))?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(file) = expected.remove(&path) else {
            return Ok(false);
        };
        if !entry.header().entry_type().is_file()
            || entry.size() != file.size_bytes
            || entry.header().mode()? != file.mode
        {
            return Ok(false);
        }
        let mut hash = Sha256::new();
        std::io::copy(&mut entry, &mut hash)?;
        if format!("{:x}", hash.finalize()) != file.sha256 {
            return Ok(false);
        }
    }
    Ok(expected.is_empty())
}
