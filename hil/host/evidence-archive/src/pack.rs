//! A version's payload, written from a run store and the pinned version.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use flate2::{Compression, GzBuilder};
use oer_hil_model::RunRef;

use crate::{
    Manifest, Payload, Result,
    copy::{self, Named, RUNS},
    manifest::{ARCHIVE, MANIFEST},
    refused,
};

/// Pack the runs `refs` cites into `out`, which must not exist:
/// `evidence.tar.gz` and `manifest.json`. A run `base`, the verified copy of
/// the pinned version, already holds with the cited digest comes from it, so
/// a new version keeps the runs of the last one after the run store pruned
/// them (owner decision 6231504542); every other run comes from the run
/// store at `store`. Each run's seal must hash to its reference (rule 1: the
/// version holds exactly the cited runs). The payload holds the runs and the
/// observer builds and source objects they name, as regular files in path
/// order with fixed metadata, so the same runs give the same bytes with the
/// same build: another gzip backend may compress them differently and so
/// name another version. It is extracted and verified before `out` appears.
pub fn pack(
    store: &Path,
    refs: &[RunRef],
    base: Option<&crate::Copy>,
    out: &Path,
) -> Result<Manifest> {
    pack_within(store, refs, base, out, copy::Limits::DEFAULT)
}

/// [`pack`] within `limits`, the extraction's, checked before writing.
pub(crate) fn pack_within(
    store: &Path,
    refs: &[RunRef],
    base: Option<&crate::Copy>,
    out: &Path,
    limits: copy::Limits,
) -> Result<Manifest> {
    if copy::taken(out) {
        return refused(format!("{} already exists", out.display()));
    }
    // Each member, by its path in the payload, and the root it is read from.
    let mut members = BTreeMap::<PathBuf, PathBuf>::new();
    let mut named = Named::default();
    for reference in refs {
        oer_process::check_cancelled()?;
        let root = match base {
            Some(base) if base.manifest().find(&reference.run) == Some(reference) => base.root(),
            _ => store,
        };
        let relative = Path::new(RUNS).join(reference.run.to_string());
        let run = root.join(&relative);
        copy::seal(&run, reference)?;
        named.add(&run)?;
        let mut files = BTreeSet::new();
        copy::files(root, &relative, &mut files)?;
        members.extend(files.into_iter().map(|path| (path, root.to_owned())));
    }
    // A named file is content-addressed: the verified base's copy when it
    // holds one, else the run store's, which must hash to its name.
    for path in named.paths()? {
        let root = match base {
            Some(base) if base.root().join(&path).is_file() => base.root(),
            _ => {
                let file = store.join(&path);
                copy::present(&file, false).map_err(|error| {
                    crate::within(error, "a packed run names a file the run store must hold")
                })?;
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.trim_end_matches(".json"))
                    .unwrap_or_default();
                if oer_durable::sha256_file(&file)? != name {
                    return refused(format!(
                        "{} in the run store does not hash to its name",
                        path.display()
                    ));
                }
                store
            }
        };
        members.insert(path, root.to_owned());
    }
    if members.len() > limits.files {
        return refused("the runs exceed the payload's file limit");
    }
    // The extraction's limits, before anything is written.
    let mut total = 0_u64;
    for (path, root) in &members {
        let size = fs::metadata(root.join(path))?.len();
        if size > limits.file_bytes {
            return refused(format!(
                "{} exceeds the payload's file limit",
                path.display()
            ));
        }
        total = total.saturating_add(size);
    }
    if total > limits.total_bytes {
        return refused("the runs exceed the payload's size limit");
    }
    let parent = out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = tempfile::tempdir_in(parent)?;
    let archive = staging.path().join(ARCHIVE);
    write(&members, &archive)?;
    let (sha256, bytes) = copy::copy_digest(&mut fs::File::open(&archive)?, &mut std::io::sink())?;
    let manifest = Manifest::new(Payload { sha256, bytes }, refs.to_vec())?;
    fs::write(staging.path().join(MANIFEST), manifest.bytes())?;
    // Verify the bytes actually written, every seal included.
    let check = tempfile::tempdir_in(parent)?;
    copy::unpack(&archive, &manifest, &check.path().join("copy"))?;
    // The staging is cleaned up unless the rename succeeds; it never
    // replaces an entry that appeared at `out` meanwhile.
    copy::rename_new(staging.path(), out)?;
    drop(staging);
    Ok(manifest)
}

/// Write `members` (path in the payload, root it is read from) as a
/// deterministic gzip tar.
fn write(members: &BTreeMap<PathBuf, PathBuf>, archive: &Path) -> Result<()> {
    let file = fs::File::create_new(archive)?;
    let encoder = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (path, root) in members {
        oer_process::check_cancelled()?;
        let source = root.join(path);
        let size = fs::metadata(&source)?.len();
        let mut header = tar::Header::new_gnu();
        header.set_size(size);
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append_data(&mut header, path, fs::File::open(&source)?)?;
    }
    let mut file = builder.into_inner()?.finish()?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}
