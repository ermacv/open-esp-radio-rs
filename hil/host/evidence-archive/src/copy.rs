//! A verified local copy of one version, laid out as a run store:
//! `runs/<run>/` for each run, the observer builds in `observers/` and the
//! source objects in `sources/` that those runs name, and the version's
//! `manifest.json`. Every file outside a run, except `manifest.json` (byte-equal
//! to the pinned manifest), is named by its own SHA-256, and every file inside
//! one is covered by its seal, whose digest the manifest holds: the manifest's
//! digest therefore pins every byte of the copy.

use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, BufReader, Read, Seek, Write},
    path::{Component, Path, PathBuf},
};

use flate2::bufread::GzDecoder;
use oer_hil_model::{RunRef, Sha256};
use oer_hil_run_bundle_format::{observer::store as observers, run::integrity};
use oer_hil_schema::snapshot;

use crate::{Manifest, Result, manifest::MANIFEST, refused};

/// The runs of a store, below its root.
pub const RUNS: &str = "runs";

/// The most files a payload may hold.
pub(crate) const MAX_FILES: usize = 200_000;
/// The largest file a payload may hold.
pub(crate) const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The most bytes a payload may extract to.
pub(crate) const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// A local copy whose every byte matched its manifest when opened.
#[derive(Debug)]
pub struct Copy {
    root: PathBuf,
    manifest: Manifest,
}

impl Copy {
    /// The copy at `root`, verified against `expected`: its manifest's bytes,
    /// each run's seal and the digest of that seal, and exactly the observer
    /// builds and source objects the runs name, each hashing to its name.
    pub fn open(root: &Path, expected: &Manifest) -> Result<Self> {
        // What is missing or of the wrong kind breaks a rule (exit 1).
        present(root, true)?;
        present(&root.join(MANIFEST), false)?;
        let bytes = fs::read(root.join(MANIFEST))?;
        if bytes != expected.bytes() {
            return refused(format!(
                "{} is not the manifest of version {}",
                root.join(MANIFEST).display(),
                expected.version()
            ));
        }
        present(&root.join(RUNS), true)?;
        // A version whose runs name no build or object holds none.
        for directory in [observers::DIRECTORY, snapshot::OBJECTS] {
            let path = root.join(directory);
            if fs::symlink_metadata(&path).is_ok() {
                present(&path, true)?;
            }
        }
        let mut entries = names(root)?;
        entries.remove(MANIFEST);
        for directory in [RUNS, observers::DIRECTORY, snapshot::OBJECTS] {
            entries.remove(directory);
        }
        if let Some(extra) = entries.first() {
            return refused(format!(
                "the evidence copy {} holds `{extra}`, which no version holds",
                root.display()
            ));
        }
        let held = names(&root.join(RUNS))?;
        let listed = expected
            .runs
            .iter()
            .map(|reference| reference.run.to_string())
            .collect::<BTreeSet<_>>();
        if held != listed {
            return refused(format!(
                "the evidence copy {} holds the runs {held:?}, its manifest {listed:?}",
                root.display()
            ));
        }
        let mut named = Named::default();
        for reference in &expected.runs {
            oer_process::check_cancelled()?;
            let run = root.join(RUNS).join(reference.run.to_string());
            seal(&run, reference)?;
            named.add(&run)?;
        }
        named.verify(root)?;
        Ok(Self {
            root: root.to_owned(),
            manifest: expected.clone(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory of the runs, as a run store's `runs/`.
    pub fn runs(&self) -> PathBuf {
        self.root.join(RUNS)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

/// The names in `directory`; none when it does not exist.
fn names(directory: &Path) -> Result<BTreeSet<String>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error.into()),
    };
    entries
        .map(|entry| {
            entry?.file_name().into_string().map_err(|name| {
                format!("non-UTF-8 name {name:?} in {}", directory.display()).into()
            })
        })
        .collect()
}

/// Whether `path` is a directory (`directory`) or a regular file. What is
/// missing or of the wrong kind breaks a rule; any other failure to reach it
/// is the environment's.
pub(crate) fn present(path: &Path, directory: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if if directory {
                metadata.file_type().is_dir()
            } else {
                metadata.file_type().is_file()
            } =>
        {
            Ok(())
        }
        Ok(_) => refused(format!(
            "{} is not a {}",
            path.display(),
            if directory {
                "directory"
            } else {
                "regular file"
            }
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            refused(format!("{} does not exist", path.display()))
        }
        Err(error) => Err(error.into()),
    }
}

/// The reference of the run `id` whose bundle is at `run`: its seal
/// verified, and the SHA-256 of `integrity.json`. A run that is no directory
/// or whose seal does not verify is [`Refused`](crate::Refused); an I/O
/// failure while reading it is the environment's.
pub fn run_reference(run: &Path, id: &oer_hil_model::RunId) -> Result<RunRef> {
    // The seal is read before it is verified, so a failure to read it is not
    // mistaken for a broken seal, and again after: the reference is the
    // digest of the bytes verified, unchanged meanwhile.
    let seal = run.join(integrity::INTEGRITY);
    present(run, true)?;
    present(&seal, false)?;
    let bytes = fs::read(&seal)?;
    integrity::verify(run, &id.to_string()).map_err(
        |error| -> Box<dyn std::error::Error + Send + Sync> {
            if error.is::<std::io::Error>() {
                error
            } else {
                Box::new(crate::Refused(error.to_string()))
            }
        },
    )?;
    if fs::read(&seal)? != bytes {
        return refused(format!("the seal of run {id} changed while it was read"));
    }
    Ok(RunRef {
        run: id.clone(),
        sha256: Sha256::of(&bytes),
    })
}

/// Whether the run at `run` is sealed and its seal hashes to the reference's
/// digest.
pub(crate) fn seal(run: &Path, reference: &RunRef) -> Result<()> {
    let digest = run_reference(run, &reference.run)?.sha256;
    if digest != reference.sha256 {
        return refused(format!(
            "run {}'s seal hashes to {digest}, not to the cited {}",
            reference.run, reference.sha256
        ));
    }
    Ok(())
}

/// The content-addressed files outside the runs that the runs name.
#[derive(Debug, Default)]
pub(crate) struct Named {
    /// Observer build digests.
    pub(crate) observers: BTreeSet<String>,
    /// Source object digests.
    pub(crate) sources: BTreeSet<String>,
}

impl Named {
    /// Add what the run at `run` names: its observer build and the objects of
    /// its source snapshot.
    pub(crate) fn add(&mut self, run: &Path) -> Result<()> {
        let malformed = |what: &str, error: serde_json::Error| {
            crate::Refused(format!("run {}: invalid {what}: {error}", run.display()))
        };
        let manifest: serde_json::Value = serde_json::from_slice(&fs::read(
            run.join(oer_hil_run_bundle_format::read::MANIFEST),
        )?)
        .map_err(|error| malformed("manifest", error))?;
        if let Some(record) = manifest
            .pointer("/runner/observer")
            .filter(|record| !record.is_null())
        {
            let digest = observers::build_digest(record).ok_or_else(|| {
                crate::Refused(format!(
                    "run {} names its observer build by no digest",
                    run.display()
                ))
            })?;
            self.observers.insert(digest.to_owned());
        }
        let snapshot = run.join("source/snapshot/manifest.json");
        if snapshot.is_file() {
            let manifest: snapshot::Manifest = serde_json::from_slice(&fs::read(&snapshot)?)
                .map_err(|error| malformed("source snapshot", error))?;
            for source in manifest.sources {
                self.sources
                    .extend(source.files.into_iter().map(|file| file.sha256));
            }
        }
        Ok(())
    }

    /// The files, relative to a store's root, each named file lives at.
    pub(crate) fn paths(&self) -> Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        for digest in &self.observers {
            if !observers::is_digest(digest) {
                return refused(format!("observer build `{digest}` is not a SHA-256"));
            }
            paths.push(observers::path(Path::new(""), digest)?);
        }
        for digest in &self.sources {
            if !observers::is_digest(digest) {
                return refused(format!("source object `{digest}` is not a SHA-256"));
            }
            paths.push(snapshot::object(Path::new(snapshot::OBJECTS), digest));
        }
        Ok(paths)
    }

    /// Whether the store at `root` holds exactly these files outside its runs,
    /// each hashing to its name.
    fn verify(&self, root: &Path) -> Result<()> {
        let expected = self.paths()?.into_iter().collect::<BTreeSet<_>>();
        let mut held = BTreeSet::new();
        for directory in [observers::DIRECTORY, snapshot::OBJECTS] {
            files(root, Path::new(directory), &mut held)?;
        }
        if held != expected {
            let extra = held.difference(&expected).next();
            let missing = expected.difference(&held).next();
            return refused(format!(
                "the evidence copy {} holds other named files than its runs name \
                 (first extra {extra:?}, first missing {missing:?})",
                root.display()
            ));
        }
        for path in &expected {
            oer_process::check_cancelled()?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.trim_end_matches(".json"))
                .unwrap_or_default();
            if oer_durable::sha256_file(&root.join(path))? != name {
                return refused(format!("{} does not hash to its name", path.display()));
            }
        }
        Ok(())
    }
}

/// The regular files below `root/relative`, relative to `root`; refuses
/// links and special files.
pub(crate) fn files(root: &Path, relative: &Path, out: &mut BTreeSet<PathBuf>) -> Result<()> {
    let directory = root.join(relative);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            files(root, &path, out)?;
        } else if kind.is_file() {
            out.insert(path);
        } else {
            return refused(format!(
                "{} is a link or a special file",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

/// Extract the payload at `archive` into `out`, which must not exist, and
/// open it as the copy of `manifest`. The payload's bytes are hashed as they
/// are read into a private snapshot, so a file changed meanwhile cannot pass;
/// only regular files at canonical paths below `runs/<a held run>/`,
/// `observers/` and `sources/` are extracted, each once, within the limits.
pub fn unpack(archive: &Path, manifest: &Manifest, out: &Path) -> Result<Copy> {
    unpack_within(archive, manifest, out, Limits::DEFAULT)
}

/// How much one payload may extract to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) files: usize,
    pub(crate) file_bytes: u64,
    pub(crate) total_bytes: u64,
}

impl Limits {
    pub(crate) const DEFAULT: Self = Self {
        files: MAX_FILES,
        file_bytes: MAX_FILE_BYTES,
        total_bytes: MAX_TOTAL_BYTES,
    };
}

/// [`unpack`] within `limits`.
pub(crate) fn unpack_within(
    archive: &Path,
    manifest: &Manifest,
    out: &Path,
    limits: Limits,
) -> Result<Copy> {
    if taken(out) {
        return refused(format!("{} already exists", out.display()));
    }
    let parent = beside(out)?;
    // The payload's snapshot and the extraction stay next to `out`, never in
    // a temporary file system: a version is gigabytes.
    let mut snapshot = tempfile::tempfile_in(&parent)?;
    // Never more than the manifest's size plus one byte is copied: a larger
    // payload is refused without being written whole.
    let (sha256, bytes) = copy_digest(
        &mut fs::File::open(archive)?.take(manifest.archive.bytes.saturating_add(1)),
        &mut snapshot,
    )?;
    if sha256 != manifest.archive.sha256 || bytes != manifest.archive.bytes {
        return refused(format!(
            "{} is not the payload of version {}: {bytes} bytes hashing to {sha256}",
            archive.display(),
            manifest.version()
        ));
    }
    snapshot.rewind()?;
    let staging = tempfile::tempdir_in(&parent)?;
    let runs = manifest
        .runs
        .iter()
        .map(|reference| reference.run.to_string())
        .collect::<BTreeSet<_>>();
    let mut reader = tar::Archive::new(GzDecoder::new(BufReader::new(snapshot)));
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    for entry in reader.entries()? {
        oer_process::check_cancelled()?;
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return refused("an evidence payload holds only regular files");
        }
        let path = entry.path()?.into_owned();
        // The name as written must be its own canonical form: no `.`, no
        // empty component, no trailing `/`.
        let written = std::str::from_utf8(&entry.path_bytes())
            .unwrap_or_default()
            .to_owned();
        if !permitted(&path, &runs) {
            return refused(format!(
                "the evidence payload holds `{}`, outside a held run, `observers/` and `sources/`",
                path.display()
            ));
        }
        if written != canonical(&path) {
            return refused(format!(
                "the evidence payload holds `{written}`, a name not in its canonical form"
            ));
        }
        let size = entry.size();
        total = total.saturating_add(size);
        if size > limits.file_bytes || total > limits.total_bytes || seen.len() >= limits.files {
            return refused("the evidence payload exceeds the extraction limits");
        }
        if !seen.insert(path.clone()) {
            return refused(format!(
                "the evidence payload holds `{}` twice",
                path.display()
            ));
        }
        // A file is never also a directory of another member.
        if path
            .ancestors()
            .skip(1)
            .any(|ancestor| seen.contains(ancestor))
            || seen
                .range(path.clone()..)
                .nth(1)
                .is_some_and(|next| next.starts_with(&path))
        {
            return refused(format!(
                "the evidence payload holds `{}` both as a file and as a directory",
                path.display()
            ));
        }
        let output = staging.path().join(&path);
        fs::create_dir_all(output.parent().expect("a member has a parent"))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        if std::io::copy(&mut entry, &mut file)? != size {
            return refused(format!("`{}` is truncated", path.display()));
        }
    }
    // After the tar end only its zero padding may follow; reading it to the
    // end also checks the gzip trailer.
    let mut decoder = reader.into_inner();
    let mut rest = [0_u8; 65536];
    loop {
        let read = decoder.read(&mut rest)?;
        if read == 0 {
            break;
        }
        if rest[..read].iter().any(|byte| *byte != 0) {
            return refused("the evidence payload holds data after the tar end");
        }
    }
    if !decoder.into_inner().fill_buf()?.is_empty() {
        return refused("the evidence payload has trailing compressed data");
    }
    fs::write(staging.path().join(MANIFEST), manifest.bytes())?;
    Copy::open(staging.path(), manifest)?;
    // The staging is cleaned up unless the rename succeeds; it never
    // replaces an entry that appeared at `out` meanwhile.
    rename_new(staging.path(), out)?;
    drop(staging);
    Copy::open(out, manifest).map_err(|error| {
        crate::within(
            error,
            format_args!("{} changed after its verification", out.display()),
        )
    })
}

/// Whether `path` names any entry, a dangling link included.
pub(crate) fn taken(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// The directory `path` lies in, created: where its staging goes, on the
/// same file system and never in a temporary one.
pub(crate) fn beside(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    Ok(parent.to_owned())
}

/// The canonical text of `path`: its normal components joined by `/`.
fn canonical(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Rename `from` to `to`, which must not exist: a `to` that appeared since
/// it was checked, even an empty directory, is refused, never replaced.
pub(crate) fn rename_new(from: &Path, to: &Path) -> Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::EXIST) => refused(format!("{} already exists", to.display())),
        Err(error) => Err(format!("rename {} to {}: {error}", from.display(), to.display()).into()),
    }
}

/// Whether a payload may hold `path`.
fn permitted(path: &Path, runs: &BTreeSet<String>) -> bool {
    let parts = path
        .components()
        .map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>();
    match parts.as_deref() {
        Some([first, run, _, ..]) if *first == RUNS => runs.contains(*run),
        Some([first, _, ..]) => *first == observers::DIRECTORY || *first == snapshot::OBJECTS,
        _ => false,
    }
}

/// Copy `input` to `output`, returning the SHA-256 and size of the bytes.
pub(crate) fn copy_digest(input: &mut impl Read, output: &mut impl Write) -> Result<(Sha256, u64)> {
    use sha2::Digest as _;
    let mut hash = sha2::Sha256::new();
    let mut buffer = [0; 65536];
    let mut bytes = 0_u64;
    loop {
        oer_process::check_cancelled()?;
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        output.write_all(&buffer[..n])?;
        hash.update(&buffer[..n]);
        bytes += n as u64;
    }
    Ok((Sha256(hash.finalize().into()), bytes))
}
