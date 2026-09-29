//! Independent validation of captured source identities and every archived byte.
use super::*;
use serde_json::Value;

#[derive(Deserialize, PartialEq)]
pub(super) struct Source {
    pub(super) name: String,
    pub(super) commit: String,
    pub(super) dirty: bool,
    pub(super) workspace_sha256: String,
    pub(super) rebuild_status: String,
    pub(super) limitations: Vec<Value>,
    pub(super) untracked_files: Vec<Value>,
    pub(super) tracked_patch_path: Option<PathBuf>,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// The producer writes the manifest with the same types, so its fields and
// their order, which a source's identity digests, are one contract.
pub(super) use oer_hil_schema::snapshot::{MANIFEST_SCHEMA, Manifest};

#[derive(Deserialize)]
struct Snapshot {
    schema: u16,
    snapshot_id: String,
    archive_sha256: String,
    files: usize,
}

pub(super) fn verified(directory: &Path, sources: &[Source]) -> Result<Option<Manifest>> {
    let manifest: Manifest = read_json(&directory.join("manifest.json"))?;
    let snapshot: Snapshot = read_json(&directory.join("snapshot.json"))?;
    if manifest.schema != MANIFEST_SCHEMA
        || snapshot.schema != 1
        || digest(&serde_json::to_vec(&manifest)?) != snapshot.snapshot_id
        || sha256_file(&directory.join("sources.tar"))? != snapshot.archive_sha256
        || sources.len() != manifest.sources.len()
    {
        return Ok(None);
    }
    let mut expected = BTreeMap::new();
    for (captured, source) in manifest.sources.iter().zip(sources) {
        if captured.name != source.name
            || captured.commit != source.commit
            || captured.dirty != source.dirty
            || digest(&serde_json::to_vec(captured)?) != source.workspace_sha256
        {
            return Ok(None);
        }
        for file in &captured.files {
            if !safe_relative(&file.path)
                || !valid_sha256(&file.sha256)
                || !matches!(file.mode, 0o644 | 0o755)
                || expected
                    .insert(PathBuf::from(&captured.name).join(&file.path), file)
                    .is_some()
            {
                return Ok(None);
            }
        }
    }
    if snapshot.files != expected.len() {
        return Ok(None);
    }
    // Check all archived bytes without extracting anything into the filesystem.
    let mut archive = tar::Archive::new(fs::File::open(directory.join("sources.tar"))?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(file) = expected.remove(&path) else {
            return Ok(None);
        };
        if !entry.header().entry_type().is_file()
            || entry.size() != file.size_bytes
            || entry.header().mode()? != file.mode
        {
            return Ok(None);
        }
        let mut hash = Sha256::new();
        std::io::copy(&mut entry, &mut hash)?;
        if format!("{:x}", hash.finalize()) != file.sha256 {
            return Ok(None);
        }
    }
    Ok(expected.is_empty().then_some(manifest))
}

/// A direct observation binds the complete current source selection. Property
/// reviews deliberately bind a narrower, explicitly reviewed set elsewhere.
/// The closure of the checkout at `root`, computed once per evaluation;
/// `None` when Cargo cannot list it, and then every file is compared.
fn closure_of(root: &Path) -> Option<std::sync::Arc<super::closure::Closure>> {
    static CLOSURES: std::sync::Mutex<
        BTreeMap<PathBuf, Option<std::sync::Arc<super::closure::Closure>>>,
    > = std::sync::Mutex::new(BTreeMap::new());
    let root = root.canonicalize().ok()?;
    CLOSURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(root.clone())
        .or_insert_with(|| {
            super::closure::Closure::of(&root)
                .ok()
                .map(std::sync::Arc::new)
        })
        .clone()
}

/// The runner directories of the checkout at `root`, computed once per
/// evaluation; `None` when Cargo cannot list them.
fn runner_of(root: &Path) -> Option<std::sync::Arc<BTreeSet<PathBuf>>> {
    static RUNNERS: std::sync::Mutex<BTreeMap<PathBuf, Option<std::sync::Arc<BTreeSet<PathBuf>>>>> =
        std::sync::Mutex::new(BTreeMap::new());
    let root = root.canonicalize().ok()?;
    RUNNERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(root.clone())
        .or_insert_with(|| {
            super::closure::runner_directories(&root)
                .ok()
                .map(std::sync::Arc::new)
        })
        .clone()
}

/// The closure of the run in `run`: its own when every image recorded its
/// inputs, the checkout's otherwise.
fn run_closure(root: &Path, run: &Path) -> Option<std::sync::Arc<super::closure::Closure>> {
    runner_of(root)
        .and_then(|runner| {
            super::closure::Closure::of_run(root, run, &runner)
                .ok()
                .flatten()
        })
        .map(std::sync::Arc::new)
        .or_else(|| closure_of(root))
}

/// Whether every file of the run's closure in the checkout at `root` is as
/// it was at `commit`, the clean commit the run was built from: the run
/// then observed the checkout's current firmware, runner and scenarios even
/// though other files changed since. False when the commit is unknown here.
pub(super) fn unchanged_since(root: &Path, run: &Path, commit: &str) -> Result<bool> {
    match run_closure(root, run) {
        Some(closure) => unchanged_within(root, &closure, commit),
        None => Ok(false),
    }
}

/// Whether no file of `closure` differs in the checkout at `root` from
/// `commit`, counting untracked files as differences.
fn unchanged_within(root: &Path, closure: &super::closure::Closure, commit: &str) -> Result<bool> {
    let git = |arguments: &[&str]| -> Result<Option<Vec<PathBuf>>> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()?;
        Ok(output.status.success().then(|| {
            String::from_utf8_lossy(&output.stdout)
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .collect()
        }))
    };
    // The commit's tree against the working tree, staged or not.
    let Some(changed) = git(&["diff", "--name-only", "--no-renames", "-z", commit, "--"])? else {
        return Ok(false);
    };
    let Some(untracked) = git(&["ls-files", "--others", "--exclude-standard", "-z"])? else {
        return Ok(false);
    };
    Ok(!changed
        .iter()
        .chain(&untracked)
        .any(|path| closure.contains(path)))
}

/// Whether the run's snapshot is the checkout's current state in every file
/// that can change the observation (see [`super::closure`]).
pub(super) fn current(root: &Path, run: &Path, sources: &[Source]) -> Result<bool> {
    let directory = run.join("source/snapshot");
    // Reject known differences before scanning the archive a second time. The
    // outer seal is already checked; acceptance still validates every byte below.
    let manifest: Manifest = read_json(&directory.join("manifest.json"))?;
    let Some(repository) = manifest.sources.first() else {
        return Ok(false);
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    let closure = run_closure(root, run);
    let relevant = |path: &Path| {
        closure
            .as_ref()
            .is_none_or(|closure| closure.contains(path))
    };
    let tracked = std::str::from_utf8(&output.stdout)?
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .filter(|path| relevant(path))
        .collect::<BTreeSet<_>>();
    let captured = repository
        .files
        .iter()
        .filter(|file| relevant(&file.path))
        .collect::<Vec<_>>();
    if tracked != captured.iter().map(|f| f.path.clone()).collect() {
        return Ok(false);
    }
    for file in captured {
        let Some(identity) = super::subject::file(root, &file.path)? else {
            return Ok(false);
        };
        if identity.size_bytes != file.size_bytes || identity.sha256 != file.sha256 {
            return Ok(false);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if fs::metadata(root.join(&file.path))?.permissions().mode() & 0o111 == 0 {
                0o644
            } else {
                0o755
            };
            if mode != file.mode {
                return Ok(false);
            }
        }
    }
    Ok(verified(&directory, sources)?.is_some())
}

#[cfg(test)]
mod tests {
    use oer_hil_schema::snapshot::SourceInput;

    #[test]
    fn a_run_stays_current_across_commits_that_leave_its_inputs_alone() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let git = |arguments: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(arguments)
                .status()
                .unwrap();
            assert!(status.success());
        };
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        git(&["init", "-q"]);
        write("crates/radio/src/lib.rs", "a");
        write("crates/other/src/lib.rs", "b");
        write("hil/scenarios/system/boot-smoke.toml", "c");
        write("hil/scenarios/system/other.toml", "d");
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "run"]);
        let commit = String::from_utf8(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let commit = commit.trim();
        let run = tempfile::tempdir().unwrap();
        let image = run.path().join("firmware/boot-smoke");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::create_dir_all(run.path().join("scenarios/boot-smoke")).unwrap();
        std::fs::write(
            image.join("source-inputs.json"),
            r#"{"schema":1,"files":["crates/radio/src/lib.rs"]}"#,
        )
        .unwrap();
        let closure = super::super::closure::Closure::of_run(root, run.path(), &Default::default())
            .unwrap()
            .unwrap();

        // Another crate, another scenario and a new unrelated file.
        write("crates/other/src/lib.rs", "changed");
        write("hil/scenarios/system/other.toml", "changed");
        write("docs/new.md", "new");
        assert!(super::unchanged_within(root, &closure, commit).unwrap());

        write("hil/scenarios/system/boot-smoke.toml", "changed");
        assert!(!super::unchanged_within(root, &closure, commit).unwrap());
        write("hil/scenarios/system/boot-smoke.toml", "c");
        write("crates/radio/src/lib.rs", "changed");
        assert!(!super::unchanged_within(root, &closure, commit).unwrap());
        write("crates/radio/src/lib.rs", "a");
        assert!(super::unchanged_within(root, &closure, commit).unwrap());
        assert!(
            !super::unchanged_within(root, &closure, "0000000000000000000000000000000000000000")
                .unwrap()
        );
    }

    #[test]
    fn a_run_without_recorded_inputs_has_no_closure_of_its_own() {
        let run = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(run.path().join("firmware/boot-smoke")).unwrap();
        std::fs::create_dir_all(run.path().join("scenarios")).unwrap();
        assert!(
            super::super::closure::Closure::of_run(run.path(), run.path(), &Default::default())
                .unwrap()
                .is_none()
        );
    }

    /// The source identity is the digest of the reserialized manifest entry,
    /// so an entry must read back to the producer's exact bytes.
    #[test]
    fn a_source_entry_reserializes_to_the_producers_bytes() {
        let file = r#"{"path":"a.rs","size_bytes":1,"sha256":"00","mode":420}"#;
        for entry in [
            format!(r#"{{"name":"repository","commit":"c","dirty":true,"files":[{file}]}}"#),
            format!(
                r#"{{"name":"repository","commit":"c","dirty":true,"files":[{file}],"untracked":[{{"path":"a.rs","by":"source-include"}},{{"path":"b.rs","by":"image-package"}}]}}"#
            ),
        ] {
            let parsed: SourceInput = serde_json::from_str(&entry).unwrap();
            assert_eq!(serde_json::to_string(&parsed).unwrap(), entry);
        }
        let unknown = format!(
            r#"{{"name":"repository","commit":"c","dirty":true,"files":[{file}],"untracked":[{{"path":"a.rs","by":"guess"}}]}}"#
        );
        assert!(serde_json::from_str::<SourceInput>(&unknown).is_err());
    }
}
