use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use oer_hil_run_bundle_format::run::{INTEGRITY, write_integrity_index};
use oer_hil_schema::snapshot;

use crate::{
    Manifest, Pin, Refused, Repository, RunId, RunRef, Sha256, Version,
    copy::{Copy, unpack},
    manifest::{ARCHIVE, MANIFEST},
    pack::pack,
    pin::unpinned,
    release::{Published, Releases, State, fetch, publish},
};

fn refused(error: Box<dyn std::error::Error + Send + Sync>) -> String {
    assert!(error.is::<Refused>(), "not a refusal: {error}");
    error.to_string()
}

/// A sealed run `id` in the store at `store`, naming an observer build and
/// one source object; returns its reference.
fn sealed_run(store: &Path, id: &str, body: &str) -> RunRef {
    let observer = format!("{{\"observer\":\"{id}\"}}");
    let observer_digest = oer_durable::sha256_bytes(observer.as_bytes());
    let observers = store.join("observers");
    fs::create_dir_all(&observers).unwrap();
    fs::write(observers.join(format!("{observer_digest}.json")), &observer).unwrap();
    let source = format!("source of {id}\n");
    let source_digest = oer_durable::sha256_bytes(source.as_bytes());
    let object = snapshot::object(&store.join(snapshot::OBJECTS), &source_digest);
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    fs::write(&object, &source).unwrap();

    let run = store.join("runs").join(id);
    fs::create_dir_all(run.join("source/snapshot")).unwrap();
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "run_id": id,
            "runner": { "observer": { "schema": 2, "build_sha256": observer_digest } },
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(run.join("suite.json"), body).unwrap();
    let manifest = snapshot::Manifest {
        schema: snapshot::MANIFEST_SCHEMA,
        sources: vec![snapshot::SourceInput {
            name: "repository".into(),
            commit: "0".repeat(40),
            dirty: false,
            files: vec![snapshot::FileInput {
                path: "src/lib.rs".into(),
                size_bytes: source.len() as u64,
                sha256: source_digest,
                mode: 0o644,
            }],
            untracked: Vec::new(),
        }],
    };
    fs::write(
        run.join("source/snapshot/manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    write_integrity_index(&run, id).unwrap();
    RunRef {
        run: RunId::parse(id).unwrap(),
        sha256: Sha256::parse(&oer_durable::sha256_file(&run.join(INTEGRITY)).unwrap()).unwrap(),
    }
}

fn store() -> (tempfile::TempDir, Vec<RunRef>) {
    let store = tempfile::tempdir().unwrap();
    let refs = vec![
        sealed_run(store.path(), "1700000000001-ab", "first"),
        sealed_run(store.path(), "1700000000000-cd", "second"),
    ];
    // A run nobody cites stays out of the version.
    sealed_run(store.path(), "1700000000002-ef", "uncited");
    (store, refs)
}

#[test]
fn the_same_runs_pack_to_the_same_bytes_and_version() {
    let (store, refs) = store();
    let out = tempfile::tempdir().unwrap();
    let first = pack(store.path(), &refs, None, &out.path().join("a")).unwrap();
    let mut reversed = refs.clone();
    reversed.reverse();
    let second = pack(store.path(), &reversed, None, &out.path().join("b")).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        fs::read(out.path().join("a").join(ARCHIVE)).unwrap(),
        fs::read(out.path().join("b").join(ARCHIVE)).unwrap()
    );
    assert_eq!(
        first.version().to_string(),
        format!("evidence-{}", first.digest())
    );
    assert_eq!(Manifest::parse(&first.bytes()).unwrap(), first);
    // Only the cited runs, in run ID order.
    assert_eq!(
        first
            .runs
            .iter()
            .map(|r| r.run.to_string())
            .collect::<Vec<_>>(),
        ["1700000000000-cd", "1700000000001-ab"]
    );
}

#[test]
fn packing_refuses_a_wrong_digest_a_changed_run_and_an_existing_output() {
    let (store, refs) = store();
    let out = tempfile::tempdir().unwrap();
    let mut wrong = refs.clone();
    wrong[0].sha256 = Sha256::of(b"another seal");
    assert!(
        refused(pack(store.path(), &wrong, None, &out.path().join("a")).unwrap_err())
            .contains("not to the cited")
    );
    assert!(!out.path().join("a").exists());

    fs::write(
        store.path().join("runs/1700000000001-ab/suite.json"),
        "changed",
    )
    .unwrap();
    assert!(pack(store.path(), &refs, None, &out.path().join("b")).is_err());

    fs::create_dir(out.path().join("c")).unwrap();
    let (store, refs) = self::store();
    assert!(
        refused(pack(store.path(), &refs, None, &out.path().join("c")).unwrap_err())
            .contains("already exists")
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_run_is_the_environment_not_a_refusal() {
    use std::os::unix::fs::PermissionsExt;
    let (store, refs) = store();
    let run = store.path().join("runs/1700000000001-ab");
    let snapshot = run.join("source/snapshot");
    fs::set_permissions(&snapshot, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&snapshot).is_ok();
    let result = crate::run_reference(&run, &refs[0].run);
    fs::set_permissions(&snapshot, fs::Permissions::from_mode(0o755)).unwrap();
    if !readable {
        let error = result.unwrap_err();
        assert!(!error.is::<Refused>(), "{error}");
    }
    // Nor is an observer build the run store cannot read.
    let observers = store.path().join("observers");
    fs::set_permissions(&observers, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&observers).is_ok();
    let packed = tempfile::tempdir().unwrap();
    let result = pack(store.path(), &refs, None, &packed.path().join("version"));
    fs::set_permissions(&observers, fs::Permissions::from_mode(0o755)).unwrap();
    if !readable {
        let error = result.unwrap_err();
        assert!(!error.is::<Refused>(), "{error}");
    }
    // A socket where an observer build belongs is no regular file: refused,
    // never opened.
    let build = observers.join(format!(
        "{}.json",
        oer_durable::sha256_bytes(br#"{"observer":"1700000000001-ab"}"#)
    ));
    let bytes = fs::read(&build).unwrap();
    fs::remove_file(&build).unwrap();
    let socket = std::os::unix::net::UnixListener::bind(&build).unwrap();
    let result = pack(store.path(), &refs, None, &packed.path().join("socket"));
    drop(socket);
    fs::remove_file(&build).unwrap();
    fs::write(&build, bytes).unwrap();
    assert!(refused(result.unwrap_err()).contains("is not a regular file"));
    // Nor is a seal that cannot be read; a missing one is a refusal.
    let seal = run.join("integrity.json");
    fs::set_permissions(&seal, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read(&seal).is_ok();
    let result = crate::run_reference(&run, &refs[0].run);
    fs::set_permissions(&seal, fs::Permissions::from_mode(0o644)).unwrap();
    if !readable {
        let error = result.unwrap_err();
        assert!(!error.is::<Refused>(), "{error}");
    }
    let missing = store.path().join("runs/1700000000009-aa");
    refused(
        crate::run_reference(&missing, &RunId::parse("1700000000009-aa").unwrap()).unwrap_err(),
    );
    // A sealed run gives its reference; a changed one is refused.
    assert_eq!(crate::run_reference(&run, &refs[0].run).unwrap(), refs[0]);
    fs::write(run.join("suite.json"), "changed").unwrap();
    refused(crate::run_reference(&run, &refs[0].run).unwrap_err());
}

#[test]
fn packing_refuses_a_missing_named_object() {
    let (store, refs) = store();
    fs::remove_dir_all(store.path().join(snapshot::OBJECTS)).unwrap();
    let out = tempfile::tempdir().unwrap();
    assert!(
        refused(pack(store.path(), &refs, None, &out.path().join("a")).unwrap_err())
            .contains("does not exist")
    );
}

#[test]
fn a_manifest_is_read_only_in_its_canonical_form() {
    let (store, refs) = store();
    let out = tempfile::tempdir().unwrap();
    let manifest = pack(store.path(), &refs, None, &out.path().join("a")).unwrap();
    let pretty = serde_json::to_vec_pretty(&manifest).unwrap();
    assert!(refused(Manifest::parse(&pretty).unwrap_err()).contains("canonical"));
    let mut unsorted = manifest.clone();
    unsorted.runs.reverse();
    assert!(unsorted.validate().is_err());
    let mut twice = manifest.clone();
    twice.runs.push(twice.runs[1].clone());
    assert!(twice.validate().is_err());
    assert!(Manifest::new(manifest.archive, Vec::new()).is_err());
}

/// The packed version in a fresh directory, with its store kept alive.
fn packed() -> (tempfile::TempDir, PathBuf, Manifest) {
    let (store, refs) = store();
    let out = tempfile::tempdir().unwrap();
    let directory = out.path().join("version");
    let manifest = pack(store.path(), &refs, None, &directory).unwrap();
    (out, directory, manifest)
}

#[test]
fn an_unpacked_copy_holds_the_runs_and_what_they_name() {
    let (out, directory, manifest) = packed();
    let copy = unpack(
        &directory.join(ARCHIVE),
        &manifest,
        &out.path().join("copy"),
    )
    .unwrap();
    assert!(copy.runs().join("1700000000000-cd/suite.json").is_file());
    assert!(!copy.runs().join("1700000000002-ef").exists());
    assert_eq!(
        fs::read_dir(copy.root().join("observers")).unwrap().count(),
        2
    );
    assert!(Copy::open(copy.root(), &manifest).is_ok());
}

#[test]
fn unpacking_refuses_other_bytes_and_an_existing_output() {
    let (out, directory, manifest) = packed();
    let archive = directory.join(ARCHIVE);
    let mut bytes = fs::read(&archive).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(out.path().join("changed.tar.gz"), &bytes).unwrap();
    assert!(
        refused(
            unpack(
                &out.path().join("changed.tar.gz"),
                &manifest,
                &out.path().join("a")
            )
            .unwrap_err()
        )
        .contains("is not the payload")
    );
    fs::create_dir(out.path().join("b")).unwrap();
    assert!(
        refused(unpack(&archive, &manifest, &out.path().join("b")).unwrap_err())
            .contains("already exists")
    );
}

/// A payload holding `members` (path, bytes), and a manifest naming it.
fn crafted(
    directory: &Path,
    members: &[(&str, &[u8])],
    manifest: &Manifest,
) -> (PathBuf, Manifest) {
    let path = directory.join("crafted.tar.gz");
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        fs::File::create(&path).unwrap(),
        flate2::Compression::default(),
    ));
    for (name, bytes) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, *bytes).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
    let (sha256, bytes) =
        crate::copy::copy_digest(&mut fs::File::open(&path).unwrap(), &mut std::io::sink())
            .unwrap();
    let mut manifest = manifest.clone();
    manifest.archive = crate::Payload { sha256, bytes };
    (path, manifest)
}

#[test]
fn unpacking_refuses_members_outside_the_held_runs_and_duplicates() {
    let (out, _, manifest) = packed();
    for (index, members) in [
        vec![("runs/1700000000002-ef/suite.json", &b"uncited"[..])],
        vec![("notes.txt", &b"x"[..])],
        vec![("runs/1700000000000-cd", &b"x"[..])],
        vec![
            ("observers/a.json", &b"x"[..]),
            ("observers/a.json", &b"x"[..]),
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let (archive, manifest) = crafted(out.path(), &members, &manifest);
        let error = unpack(
            &archive,
            &manifest,
            &out.path().join(format!("copy-{index}")),
        )
        .unwrap_err();
        refused(error);
        assert!(!out.path().join(format!("copy-{index}")).exists());
    }
}

/// What a raw tar member is.
enum Raw {
    File,
    Symlink,
    HardLink,
}

/// A payload of raw members (their names written as given, past the tar
/// builder's own path checks), followed by `trailing` bytes after the gzip
/// stream, and a manifest naming it.
fn raw_payload(
    directory: &Path,
    members: &[(&[u8], Raw, &[u8])],
    trailing: &[u8],
    manifest: &Manifest,
) -> (PathBuf, Manifest) {
    use std::io::Write as _;
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        for (name, kind, data) in members {
            let mut header = tar::Header::new_old();
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_mode(0o644);
            header.set_size(data.len() as u64);
            match kind {
                Raw::File => header.set_entry_type(tar::EntryType::Regular),
                Raw::Symlink => {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_link_name("/etc/passwd").unwrap();
                }
                Raw::HardLink => {
                    header.set_entry_type(tar::EntryType::Link);
                    header
                        .set_link_name("runs/1700000000000-cd/suite.json")
                        .unwrap();
                }
            }
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.finish().unwrap();
    }
    let path = directory.join("raw.tar.gz");
    let mut encoder = flate2::write::GzEncoder::new(
        fs::File::create(&path).unwrap(),
        flate2::Compression::default(),
    );
    encoder.write_all(&tar_bytes).unwrap();
    let mut file = encoder.finish().unwrap();
    file.write_all(trailing).unwrap();
    drop(file);
    let (sha256, bytes) =
        crate::copy::copy_digest(&mut fs::File::open(&path).unwrap(), &mut std::io::sink())
            .unwrap();
    let mut manifest = manifest.clone();
    manifest.archive = crate::Payload { sha256, bytes };
    (path, manifest)
}

/// Unpack `members` within `limits` and return the refusal, checking that
/// nothing was extracted.
fn refused_extraction(
    members: &[(&[u8], Raw, &[u8])],
    trailing: &[u8],
    limits: crate::copy::Limits,
) -> String {
    let (out, _, manifest) = packed();
    let (archive, manifest) = raw_payload(out.path(), members, trailing, &manifest);
    let copy = out.path().join("copy");
    let error = crate::copy::unpack_within(&archive, &manifest, &copy, limits).unwrap_err();
    assert!(!copy.exists());
    refused(error)
}

const LIMITS: crate::copy::Limits = crate::copy::Limits::DEFAULT;

#[test]
fn extraction_refuses_a_parent_directory_component() {
    let error = refused_extraction(
        &[(b"runs/1700000000000-cd/../../escape", Raw::File, b"x")],
        b"",
        LIMITS,
    );
    assert!(error.contains("outside a held run"), "{error}");
}

#[test]
fn extraction_refuses_an_absolute_path() {
    let error = refused_extraction(&[(b"/tmp/escape", Raw::File, b"x")], b"", LIMITS);
    assert!(error.contains("outside a held run"), "{error}");
}

#[test]
fn extraction_refuses_a_symbolic_link() {
    let error = refused_extraction(
        &[(b"runs/1700000000000-cd/link", Raw::Symlink, b"")],
        b"",
        LIMITS,
    );
    assert!(error.contains("only regular files"), "{error}");
}

#[test]
fn extraction_refuses_a_hard_link() {
    let error = refused_extraction(
        &[(b"runs/1700000000000-cd/link", Raw::HardLink, b"")],
        b"",
        LIMITS,
    );
    assert!(error.contains("only regular files"), "{error}");
}

#[test]
fn extraction_refuses_a_member_over_the_size_limit() {
    let limits = crate::copy::Limits {
        file_bytes: 1,
        ..LIMITS
    };
    let error = refused_extraction(&[(b"observers/a.json", Raw::File, b"xy")], b"", limits);
    assert!(error.contains("extraction limits"), "{error}");
    let limits = crate::copy::Limits {
        total_bytes: 3,
        ..LIMITS
    };
    let error = refused_extraction(
        &[
            (b"observers/a.json", Raw::File, b"xy"),
            (b"observers/b.json", Raw::File, b"xy"),
        ],
        b"",
        limits,
    );
    assert!(error.contains("extraction limits"), "{error}");
}

#[test]
fn extraction_refuses_more_files_than_the_limit() {
    let limits = crate::copy::Limits { files: 1, ..LIMITS };
    let error = refused_extraction(
        &[
            (b"observers/a.json", Raw::File, b"x"),
            (b"observers/b.json", Raw::File, b"x"),
        ],
        b"",
        limits,
    );
    assert!(error.contains("extraction limits"), "{error}");
}

#[test]
fn extraction_refuses_data_after_the_tar_end() {
    use std::io::Write as _;
    let (out, _, manifest) = packed();
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "observers/a.json", &b"x"[..])
            .unwrap();
        builder.finish().unwrap();
    }
    tar_bytes.extend_from_slice(b"junk inside the gzip stream");
    let path = out.path().join("junk.tar.gz");
    let mut encoder = flate2::write::GzEncoder::new(
        fs::File::create(&path).unwrap(),
        flate2::Compression::default(),
    );
    encoder.write_all(&tar_bytes).unwrap();
    encoder.finish().unwrap();
    let (sha256, bytes) =
        crate::copy::copy_digest(&mut fs::File::open(&path).unwrap(), &mut std::io::sink())
            .unwrap();
    let mut manifest = manifest.clone();
    manifest.archive = crate::Payload { sha256, bytes };
    let copy = out.path().join("copy");
    let error = refused(unpack(&path, &manifest, &copy).unwrap_err());
    assert!(error.contains("after the tar end"), "{error}");
    assert!(!copy.exists());
}

#[test]
fn extraction_refuses_data_after_the_gzip_stream() {
    let error = refused_extraction(
        &[(b"observers/a.json", Raw::File, b"x")],
        b"trailing bytes",
        LIMITS,
    );
    assert!(error.contains("trailing compressed data"), "{error}");
}

#[test]
fn extraction_refuses_a_name_not_in_its_canonical_form() {
    for name in [
        &b"runs/1700000000000-cd//suite.json"[..],
        b"runs/1700000000000-cd/./suite.json",
        b"observers/a.json/",
    ] {
        let error = refused_extraction(&[(name, Raw::File, b"x")], b"", LIMITS);
        assert!(error.contains("not in its canonical form"), "{error}");
    }
}

#[test]
fn extraction_refuses_a_file_that_is_also_a_directory() {
    for members in [
        [
            (&b"observers/a"[..], Raw::File, &b"x"[..]),
            (b"observers/a/b", Raw::File, b"x"),
        ],
        [
            (b"observers/a/b", Raw::File, b"x"),
            (b"observers/a", Raw::File, b"x"),
        ],
    ] {
        let error = refused_extraction(&members, b"", LIMITS);
        assert!(
            error.contains("both as a file and as a directory"),
            "{error}"
        );
    }
}

/// A target that appeared after its check, even an empty directory, is
/// never replaced.
#[test]
fn a_rename_never_replaces_its_target() {
    let directory = tempfile::tempdir().unwrap();
    let (from, to) = (
        directory.path().join("staging"),
        directory.path().join("out"),
    );
    fs::create_dir(&from).unwrap();
    fs::create_dir(&to).unwrap();
    let error = refused(crate::copy::rename_new(&from, &to).unwrap_err());
    assert!(error.contains("already exists"), "{error}");
    assert!(from.is_dir());
    fs::remove_dir(&to).unwrap();
    crate::copy::rename_new(&from, &to).unwrap();
    assert!(to.is_dir() && !from.exists());
}

/// A copy missing an entry, or holding one of the wrong kind, breaks a rule
/// (exit 1), not the environment.
#[test]
fn a_copy_missing_an_entry_or_of_the_wrong_kind_is_refused() {
    let (out, directory, manifest) = packed();
    let root = out.path().join("copy");
    unpack(&directory.join(ARCHIVE), &manifest, &root).unwrap();
    fs::remove_dir_all(root.join("observers")).unwrap();
    fs::write(root.join("observers"), "").unwrap();
    assert!(refused(Copy::open(&root, &manifest).unwrap_err()).contains("is not a directory"));
    fs::remove_file(root.join(MANIFEST)).unwrap();
    refused(Copy::open(&root, &manifest).unwrap_err());
    refused(Copy::open(&out.path().join("absent"), &manifest).unwrap_err());
}

#[test]
fn a_copy_with_an_extra_or_changed_file_is_refused() {
    let (out, directory, manifest) = packed();
    let copy = out.path().join("copy");
    unpack(&directory.join(ARCHIVE), &manifest, &copy).unwrap();
    let extra = copy
        .join("observers")
        .join(format!("{}.json", "0".repeat(64)));
    fs::write(&extra, "{}").unwrap();
    assert!(refused(Copy::open(&copy, &manifest).unwrap_err()).contains("other named files"));
    fs::remove_file(&extra).unwrap();
    fs::write(copy.join("runs/1700000000000-cd/suite.json"), "edited").unwrap();
    assert!(Copy::open(&copy, &manifest).is_err());
}

fn repository() -> Repository {
    Repository::parse("ermacv/open-esp-radio-evidence").unwrap()
}

#[test]
fn a_pin_round_trips_and_refuses_a_manifest_or_version_that_disagree() {
    let (_out, _, manifest) = packed();
    let pin = Pin::new(repository(), manifest.clone());
    let text = pin.to_toml();
    assert_eq!(Pin::parse(&text).unwrap(), pin);
    let other = manifest.runs[0].sha256.to_string();
    let changed = text.replacen(&other, &Sha256::of(b"x").to_string(), 1);
    assert!(refused(Pin::parse(&changed).unwrap_err()).contains("hashes to"));
    let renamed = text.replace(
        &manifest.version().to_string(),
        &Version(Sha256::of(b"y")).to_string(),
    );
    assert!(refused(Pin::parse(&renamed).unwrap_err()).contains("names version"));
    assert!(Repository::parse("no-slash").is_err());
    assert!(Repository::parse("a/../b").is_err());
}

#[test]
fn rule_two_lists_every_citation_the_pin_does_not_hold() {
    let (_out, _, manifest) = packed();
    let pin = Pin::new(repository(), manifest.clone());
    let held = manifest.runs[0].clone();
    let wrong = RunRef {
        sha256: Sha256::of(b"z"),
        ..manifest.runs[1].clone()
    };
    let absent = RunRef {
        run: RunId::parse("1700000000009-aa").unwrap(),
        sha256: Sha256::of(b"z"),
    };
    let citations = [
        ("a".to_owned(), &held),
        ("b".to_owned(), &wrong),
        ("c".to_owned(), &absent),
    ];
    let found = unpinned(Some(&pin), citations.clone());
    assert_eq!(
        found
            .iter()
            .map(|u| u.citation.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
    assert_eq!(found[0].pinned, Some(manifest.runs[1].sha256));
    assert_eq!(unpinned(None, citations).len(), 3);
}

/// Releases in memory; `corrupt` flips a byte of every downloaded payload.
#[derive(Default)]
struct Fake {
    releases: RefCell<BTreeMap<Version, (State, tempfile::TempDir)>>,
    corrupt: bool,
}

impl Releases for Fake {
    fn state(&self, version: &Version) -> crate::Result<Option<State>> {
        Ok(self.releases.borrow().get(version).map(|(state, _)| *state))
    }

    fn location(&self) -> String {
        String::from("owner/evidence")
    }

    fn assets(&self, version: &Version) -> crate::Result<Vec<String>> {
        let releases = self.releases.borrow();
        let (_, assets) = releases.get(version).ok_or("no release")?;
        Ok(fs::read_dir(assets.path())?
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect())
    }

    fn create_draft(&self, version: &Version, assets: &[PathBuf]) -> crate::Result<()> {
        let directory = tempfile::tempdir()?;
        for asset in assets {
            fs::copy(asset, directory.path().join(asset.file_name().unwrap()))?;
        }
        assert!(
            self.releases
                .borrow_mut()
                .insert(*version, (State::Draft, directory))
                .is_none()
        );
        Ok(())
    }

    fn download(&self, version: &Version, name: &str, directory: &Path) -> crate::Result<()> {
        let releases = self.releases.borrow();
        let (_, assets) = releases.get(version).ok_or("no release")?;
        let mut bytes = fs::read(assets.path().join(name))?;
        if self.corrupt && name == ARCHIVE {
            bytes[0] ^= 1;
        }
        fs::write(directory.join(name), bytes)?;
        Ok(())
    }

    fn publish(&self, version: &Version) -> crate::Result<()> {
        self.releases
            .borrow_mut()
            .get_mut(version)
            .ok_or("no release")?
            .0 = State::Published;
        Ok(())
    }
}

#[test]
fn publishing_creates_once_then_accepts_only_the_same_bytes() {
    let (out, directory, manifest) = packed();
    let fake = Fake::default();
    assert_eq!(
        publish(&fake, &directory).unwrap(),
        (manifest.clone(), Published::Created)
    );
    assert_eq!(
        fake.state(&manifest.version()).unwrap(),
        Some(State::Published)
    );
    assert_eq!(publish(&fake, &directory).unwrap().1, Published::Existing);

    let copy = fetch(
        &fake,
        &Pin::new(repository(), manifest.clone()),
        &out.path().join("copy"),
    )
    .unwrap();
    assert!(copy.runs().join("1700000000001-ab").is_dir());
}

#[test]
fn a_read_back_mismatch_leaves_the_version_a_draft() {
    let (_out, directory, manifest) = packed();
    let fake = Fake {
        corrupt: true,
        ..Fake::default()
    };
    assert!(publish(&fake, &directory).is_err());
    assert_eq!(fake.state(&manifest.version()).unwrap(), Some(State::Draft));
}

#[test]
fn an_interrupted_draft_of_the_same_bytes_is_completed_and_another_refused() {
    let (out, directory, manifest) = packed();
    let fake = Fake::default();
    fake.create_draft(
        &manifest.version(),
        &[directory.join(MANIFEST), directory.join(ARCHIVE)],
    )
    .unwrap();
    assert!(
        refused(
            fetch(
                &fake,
                &Pin::new(repository(), manifest.clone()),
                &out.path().join("a")
            )
            .unwrap_err()
        )
        .contains("draft")
    );
    assert_eq!(publish(&fake, &directory).unwrap().1, Published::Completed);

    // A draft under the same name whose manifest differs is never published.
    let (_other_out, other_directory, _) = packed();
    let fake = Fake::default();
    let decoy = tempfile::tempdir().unwrap();
    fs::write(decoy.path().join(MANIFEST), b"{}").unwrap();
    fs::copy(other_directory.join(ARCHIVE), decoy.path().join(ARCHIVE)).unwrap();
    fake.create_draft(
        &manifest.version(),
        &[decoy.path().join(MANIFEST), decoy.path().join(ARCHIVE)],
    )
    .unwrap();
    assert!(refused(publish(&fake, &directory).unwrap_err()).contains("another manifest"));
    assert_eq!(fake.state(&manifest.version()).unwrap(), Some(State::Draft));
}

#[test]
fn a_draft_whose_upload_was_interrupted_is_refused_with_its_recovery() {
    let (_out, directory, manifest) = packed();
    let fake = Fake::default();
    fake.create_draft(&manifest.version(), &[directory.join(MANIFEST)])
        .unwrap();
    let error = refused(publish(&fake, &directory).unwrap_err());
    assert!(error.contains("delete it by hand"), "{error}");
    assert!(
        error.contains(&format!(
            "gh release delete {} --repo owner/evidence",
            manifest.version()
        )),
        "{error}"
    );
    assert_eq!(fake.state(&manifest.version()).unwrap(), Some(State::Draft));
    assert!(error.contains("its upload was interrupted"), "{error}");

    // A draft with an extra asset names that, not an interrupted upload.
    let fake = Fake::default();
    let notes = tempfile::tempdir().unwrap();
    fs::write(notes.path().join("notes.txt"), "x").unwrap();
    fake.create_draft(
        &manifest.version(),
        &[
            directory.join(MANIFEST),
            directory.join(ARCHIVE),
            notes.path().join("notes.txt"),
        ],
    )
    .unwrap();
    let error = refused(publish(&fake, &directory).unwrap_err());
    assert!(error.contains("an asset no version has"), "{error}");
}

#[test]
fn a_cancellation_stays_one_behind_context() {
    let cancelled = crate::within(Box::new(oer_process::Cancelled), "reading the payload");
    assert!(oer_process::is_cancelled(&*cancelled));
    let refusal = crate::within(Box::new(Refused(String::from("digest"))), "context");
    assert!(refusal.is::<Refused>());
}

#[test]
fn fetching_refuses_an_existing_output_before_any_download() {
    let (out, _, manifest) = packed();
    let taken = out.path().join("taken");
    fs::create_dir(&taken).unwrap();
    // No release exists: an attempt to reach it would refuse otherwise.
    let error =
        refused(fetch(&Fake::default(), &Pin::new(repository(), manifest), &taken).unwrap_err());
    assert!(error.contains("already exists"), "{error}");
}

#[test]
fn fetching_refuses_a_missing_version() {
    let (out, _, manifest) = packed();
    let error = fetch(
        &Fake::default(),
        &Pin::new(repository(), manifest),
        &out.path().join("a"),
    )
    .unwrap_err();
    assert!(refused(error).contains("has no version"));
}

/// A new version keeps the pinned version's runs after the run store pruned
/// them: they come from its verified copy (owner decision 6231504542).
#[test]
fn a_new_version_takes_the_pinned_runs_from_its_copy() {
    let (store, refs) = store();
    let out = tempfile::tempdir().unwrap();
    let first = pack(store.path(), &refs, None, &out.path().join("v1")).unwrap();
    let base = unpack(
        &out.path().join("v1").join(ARCHIVE),
        &first,
        &out.path().join("base"),
    )
    .unwrap();
    // The run store pruned the first version's runs and what they name.
    for reference in &refs {
        fs::remove_dir_all(store.path().join("runs").join(reference.run.to_string())).unwrap();
    }
    fs::remove_dir_all(store.path().join("observers")).unwrap();
    fs::remove_dir_all(store.path().join(snapshot::OBJECTS)).unwrap();
    let third = sealed_run(store.path(), "1700000000003-aa", "third");
    let mut cited = refs.clone();
    cited.push(third);
    assert!(pack(store.path(), &cited, None, &out.path().join("without")).is_err());
    let second = pack(store.path(), &cited, Some(&base), &out.path().join("v2")).unwrap();
    assert_eq!(second.runs.len(), 3);
    let copy = unpack(
        &out.path().join("v2").join(ARCHIVE),
        &second,
        &out.path().join("copy"),
    )
    .unwrap();
    for run in ["1700000000000-cd", "1700000000001-ab", "1700000000003-aa"] {
        assert!(copy.runs().join(run).is_dir(), "{run}");
    }
}

/// A base copy changed after it was verified gives no run: its seal fails.
#[test]
fn a_tampered_base_copy_is_refused() {
    let (store, refs) = store();
    let out = tempfile::tempdir().unwrap();
    let first = pack(store.path(), &refs, None, &out.path().join("v1")).unwrap();
    let base = unpack(
        &out.path().join("v1").join(ARCHIVE),
        &first,
        &out.path().join("base"),
    )
    .unwrap();
    fs::write(base.runs().join("1700000000001-ab/suite.json"), "tampered").unwrap();
    let error = pack(store.path(), &refs, Some(&base), &out.path().join("v2")).unwrap_err();
    refused(error);
    assert!(!out.path().join("v2").exists());
}

/// A sealed run whose observer build digest is no SHA-256 breaks an
/// archive rule (exit 1), not the environment (exit 3).
#[test]
fn a_malformed_observer_digest_is_a_refusal() {
    let (store, refs) = store();
    let run = store.path().join("runs/1700000000001-ab");
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "run_id": "1700000000001-ab",
            "runner": { "observer": { "schema": 2, "build_sha256": "not-a-digest" } },
        }))
        .unwrap(),
    )
    .unwrap();
    write_integrity_index(&run, "1700000000001-ab").unwrap();
    let resealed = crate::run_reference(&run, &refs[0].run).unwrap();
    let out = tempfile::tempdir().unwrap();
    let error = pack(store.path(), &[resealed], None, &out.path().join("v")).unwrap_err();
    assert!(refused(error).contains("is not a SHA-256"));
}

/// A dangling link where the output belongs counts as taken: `pack`,
/// `unpack` and `fetch` refuse it and leave nothing beside it.
#[cfg(unix)]
#[test]
fn a_dangling_link_as_the_output_is_refused() {
    let (out, directory, manifest) = packed();
    let link = out.path().join("dangling");
    std::os::unix::fs::symlink(out.path().join("nowhere"), &link).unwrap();
    let beside = || fs::read_dir(out.path()).unwrap().count();
    let entries = beside();
    let (store, refs) = store();
    assert!(
        refused(pack(store.path(), &refs, None, &link).unwrap_err()).contains("already exists")
    );
    assert!(
        refused(unpack(&directory.join(ARCHIVE), &manifest, &link).unwrap_err())
            .contains("already exists")
    );
    let error = fetch(&Fake::default(), &Pin::new(repository(), manifest), &link).unwrap_err();
    assert!(refused(error).contains("already exists"));
    assert_eq!(beside(), entries);
}

/// A named file comes from the verified base when it holds one; one taken
/// from the run store must hash to its name, checked before anything is
/// written.
#[test]
fn a_named_file_comes_from_the_base_else_must_hash_to_its_name() {
    let (store, refs) = store();
    let out = tempfile::tempdir().unwrap();
    let first = pack(store.path(), &refs, None, &out.path().join("v1")).unwrap();
    let base = unpack(
        &out.path().join("v1").join(ARCHIVE),
        &first,
        &out.path().join("base"),
    )
    .unwrap();
    let observers = store.path().join("observers");
    for entry in fs::read_dir(&observers).unwrap() {
        fs::write(entry.unwrap().path(), "corrupted").unwrap();
    }
    // The base holds the cited runs' builds: the store's are not read.
    pack(store.path(), &refs, Some(&base), &out.path().join("v2")).unwrap();
    // Without it the store's corrupted build is refused, and nothing appears.
    let error = pack(store.path(), &refs, None, &out.path().join("v3")).unwrap_err();
    assert!(refused(error).contains("does not hash to its name"));
    assert!(!out.path().join("v3").exists());
}

/// Runs over the extraction's limits are refused before anything is
/// written: no output and no staging beside it.
#[test]
fn packing_refuses_runs_over_the_limits_before_writing() {
    let (store, refs) = store();
    let out = tempfile::tempdir().unwrap();
    for (limits, message) in [
        (
            crate::copy::Limits {
                total_bytes: 64,
                ..LIMITS
            },
            "size limit",
        ),
        (
            crate::copy::Limits {
                file_bytes: 4,
                ..LIMITS
            },
            "file limit",
        ),
    ] {
        let error =
            crate::pack::pack_within(store.path(), &refs, None, &out.path().join("v"), limits)
                .unwrap_err();
        assert!(refused(error).contains(message), "{message}");
        assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
    }
}

/// A download larger than the manifest says is copied only up to one byte
/// past its size, and a size no download can have is refused, not an
/// overflow.
#[test]
fn a_download_is_copied_only_up_to_its_declared_size() {
    let (out, directory, manifest) = packed();
    let archive = directory.join(ARCHIVE);
    let mut bytes = fs::read(&archive).unwrap();
    bytes.extend(std::iter::repeat_n(0, 4096));
    let larger = out.path().join("larger.tar.gz");
    fs::write(&larger, &bytes).unwrap();
    let error = unpack(&larger, &manifest, &out.path().join("a")).unwrap_err();
    let copied = manifest.archive.bytes + 1;
    assert!(refused(error).contains(&format!(": {copied} bytes")));
    let mut unbounded = manifest.clone();
    unbounded.archive.bytes = u64::MAX;
    refused(unpack(&archive, &unbounded, &out.path().join("b")).unwrap_err());
}
