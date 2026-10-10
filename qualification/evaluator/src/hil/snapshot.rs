//! Independent validation of captured source identities and every captured byte.
use super::*;

/// A source a build record cites: the run bundle's own record.
pub(super) use oer_hil_run_bundle_format::build::SourceMaterial as Source;

// The producer writes the manifest with the same types, so its fields and
// their order, which a source's identity digests, are one contract.
use oer_hil_schema::snapshot::is_agent_guidance;
pub(super) use oer_hil_schema::snapshot::{MANIFEST_SCHEMA, Manifest};
use oer_hil_schema::snapshot::{SNAPSHOT_SCHEMA, object};

#[derive(Deserialize)]
struct Snapshot {
    schema: u16,
    snapshot_id: String,
    files: usize,
}

/// The manifest of the snapshot in `directory` when it binds `sources` and
/// every file it names is an object in `objects` holding exactly its bytes.
pub(super) fn verified(
    directory: &Path,
    objects: &Path,
    sources: &[Source],
) -> Result<Option<Manifest>> {
    let manifest: Manifest = read_json(&directory.join("manifest.json"))?;
    let snapshot: Snapshot = read_json(&directory.join("snapshot.json"))?;
    if manifest.schema != MANIFEST_SCHEMA
        || snapshot.schema != SNAPSHOT_SCHEMA
        || oer_durable::sha256_bytes(&serde_json::to_vec(&manifest)?) != snapshot.snapshot_id
        || sources.len() != manifest.sources.len()
    {
        return Ok(None);
    }
    let mut expected = BTreeMap::new();
    for (captured, source) in manifest.sources.iter().zip(sources) {
        if captured.name != source.name
            || captured.commit != source.commit
            || captured.dirty != source.dirty
            || oer_durable::sha256_bytes(&serde_json::to_vec(captured)?) != source.workspace_sha256
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
    // Every file is the object its digest names; a missing or different one
    // leaves the snapshot unverified.
    for file in expected.values() {
        let stored = object(objects, &file.sha256);
        if !fs::metadata(&stored).is_ok_and(|metadata| metadata.len() == file.size_bytes)
            || crate::digests()
                .sha256_file(&stored)
                .map_err(|error| error.to_string())?
                != file.sha256
        {
            return Ok(None);
        }
    }
    Ok(Some(manifest))
}

/// A direct observation binds the complete current source selection. Property
/// reviews deliberately bind a narrower, explicitly reviewed set elsewhere.
/// The closure of the checkout at `root`, computed once per evaluation;
/// `None` when the repository model cannot be read, and then every file is
/// compared.
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

/// The runner and stand operation packages of the checkout at `root`,
/// computed once per evaluation; `None` when the model cannot be read.
fn runner_of(root: &Path) -> Option<std::sync::Arc<super::closure::Host>> {
    static RUNNERS: std::sync::Mutex<
        BTreeMap<PathBuf, Option<std::sync::Arc<super::closure::Host>>>,
    > = std::sync::Mutex::new(BTreeMap::new());
    let root = root.canonicalize().ok()?;
    RUNNERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(root.clone())
        .or_insert_with(|| {
            super::closure::Host::of(&root)
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
        Ok(oer_process::git::output(root, arguments)
            .ok()
            .map(|stdout| {
                String::from_utf8_lossy(&stdout)
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
        .any(|path| !is_agent_guidance(path) && closure.contains(path)))
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
    let Ok(stdout) = oer_process::git::output(
        root,
        [
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    ) else {
        return Ok(false);
    };
    let closure = run_closure(root, run);
    let relevant = |path: &Path| {
        !is_agent_guidance(path)
            && closure
                .as_ref()
                .is_none_or(|closure| closure.contains(path))
    };
    let tracked = std::str::from_utf8(&stdout)?
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
    Ok(verified(
        &directory,
        &oer_hil_schema::snapshot::objects_of_run(run)?,
        sources,
    )?
    .is_some())
}

#[cfg(test)]
mod tests {
    use oer_hil_schema::snapshot::SourceInput;

    #[cfg(unix)]
    #[test]
    fn agent_guidance_does_not_stale_a_snapshot_without_a_source_closure() {
        use super::*;
        use oer_hil_run_bundle_format::build::SourceRebuildStatus;
        use oer_hil_schema::snapshot::FileInput;

        let root = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let run = store.path().join("runs/run-1");
        let git = |arguments: &[&str]| {
            let configured = [
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ];
            oer_process::git::output(root.path(), configured.iter().chain(arguments)).unwrap();
        };
        git(&["init", "-q"]);
        let inputs = [("input.rs", "source\n"), ("CLAUDE.md", "instructions\n")];
        for (path, bytes) in inputs {
            fs::write(root.path().join(path), bytes).unwrap();
        }
        std::os::unix::fs::symlink("CLAUDE.md", root.path().join("AGENTS.md")).unwrap();
        fs::create_dir(root.path().join(".agents")).unwrap();
        fs::write(root.path().join(".agents/local.md"), "local instructions\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "source and agent guidance"]);

        let snapshot = run.join("source/snapshot");
        fs::create_dir_all(&snapshot).unwrap();
        let source = SourceInput {
            name: "repository".into(),
            commit: oer_process::git::text(root.path(), ["rev-parse", "HEAD"]).unwrap(),
            dirty: false,
            files: inputs
                .iter()
                .map(|(path, bytes)| FileInput {
                    path: PathBuf::from(path),
                    size_bytes: bytes.len() as u64,
                    sha256: oer_durable::sha256_bytes(bytes.as_bytes()),
                    mode: 0o644,
                })
                .collect(),
            untracked: Vec::new(),
        };
        let sources = vec![Source {
            name: source.name.clone(),
            checkout_path: root.path().to_owned(),
            remote: None,
            commit: source.commit.clone(),
            dirty: source.dirty,
            workspace_sha256: oer_durable::sha256_bytes(&serde_json::to_vec(&source).unwrap()),
            rebuild_status: SourceRebuildStatus::SourceSnapshot,
            tracked_patch_path: None,
            tracked_patch_size_bytes: None,
            tracked_patch_sha256: None,
            untracked_files: Vec::new(),
            limitations: Vec::new(),
        }];
        let manifest = Manifest {
            schema: MANIFEST_SCHEMA,
            sources: vec![source],
        };
        for (_, bytes) in inputs {
            let stored = oer_hil_schema::snapshot::object(
                &store.path().join(oer_hil_schema::snapshot::OBJECTS),
                &oer_durable::sha256_bytes(bytes.as_bytes()),
            );
            fs::create_dir_all(stored.parent().unwrap()).unwrap();
            fs::write(stored, bytes).unwrap();
        }
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        fs::write(snapshot.join("manifest.json"), &manifest_bytes).unwrap();
        fs::write(
            snapshot.join("snapshot.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": SNAPSHOT_SCHEMA,
                "snapshot_id": oer_durable::sha256_bytes(&manifest_bytes),
                "files": inputs.len(),
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(run_closure(root.path(), &run).is_none());
        assert!(current(root.path(), &run, &sources).unwrap());

        fs::remove_file(root.path().join("AGENTS.md")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.path().join("AGENTS.md")).unwrap();
        fs::create_dir(root.path().join("tools")).unwrap();
        fs::write(root.path().join("tools/AGENTS.md"), "new instructions\n").unwrap();
        fs::write(
            root.path().join(".agents/local.md"),
            "changed instructions\n",
        )
        .unwrap();
        assert!(current(root.path(), &run, &sources).unwrap());

        fs::write(root.path().join("input.rs"), "changed source\n").unwrap();
        assert!(!current(root.path(), &run, &sources).unwrap());
    }

    #[test]
    fn a_run_stays_current_across_commits_that_leave_its_inputs_alone() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let git = |arguments: &[&str]| {
            let configured = [
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ];
            oer_process::git::output(root, configured.iter().chain(arguments)).unwrap();
        };
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        git(&["init", "-q"]);
        write("crates/radio/src/lib.rs", "a");
        write("crates/radio/AGENTS.md", "instructions");
        write("crates/radio/.agents/settings.toml", "instructions");
        write("crates/other/src/lib.rs", "b");
        write("hil/scenarios/system/boot-smoke.toml", "c");
        write("hil/scenarios/system/other.toml", "d");
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "run"]);
        let commit = oer_process::git::text(root, ["rev-parse", "HEAD"]).unwrap();
        let commit = commit.as_str();
        let run = tempfile::tempdir().unwrap();
        let image = run.path().join("firmware/boot-smoke");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::create_dir_all(run.path().join("scenarios/boot-smoke")).unwrap();
        std::fs::write(
            image.join("source-inputs.json"),
            r#"{"schema":2,"files":["crates/radio/src/lib.rs"]}"#,
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

        // A broader source directory still excludes tracked and untracked guidance.
        let directory_closure =
            super::super::closure::Closure::from_directories(&["crates/radio"], &[]);
        write("crates/radio/AGENTS.md", "changed instructions");
        write("crates/radio/.agents/settings.toml", "changed instructions");
        write("crates/radio/.agents/new.rs", "new instructions");
        assert!(super::unchanged_within(root, &directory_closure, commit).unwrap());

        write("hil/scenarios/system/boot-smoke.toml", "changed");
        assert!(!super::unchanged_within(root, &closure, commit).unwrap());
        write("hil/scenarios/system/boot-smoke.toml", "c");
        write("crates/radio/src/lib.rs", "changed");
        assert!(!super::unchanged_within(root, &closure, commit).unwrap());
        assert!(!super::unchanged_within(root, &directory_closure, commit).unwrap());
        write("crates/radio/src/lib.rs", "a");
        assert!(super::unchanged_within(root, &closure, commit).unwrap());
        assert!(
            !super::unchanged_within(root, &closure, "0000000000000000000000000000000000000000")
                .unwrap()
        );
    }

    #[test]
    fn a_source_inputs_record_of_another_schema_is_an_error() {
        let run = tempfile::tempdir().unwrap();
        let image = run.path().join("firmware/boot-smoke");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::create_dir_all(run.path().join("scenarios")).unwrap();
        std::fs::write(
            image.join("source-inputs.json"),
            r#"{"schema":1,"files":["crates/radio/src/lib.rs"]}"#,
        )
        .unwrap();
        let error =
            super::super::closure::Closure::of_run(run.path(), run.path(), &Default::default())
                .err()
                .expect("schema 1 is not read");
        assert!(error.to_string().contains("only schema 2"), "{error}");
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
