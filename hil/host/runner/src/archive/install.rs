//! Publish verified evidence into the local store, preserving existing run identities.
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    TARGET,
    package::{self, Verified},
};
use crate::Result;

pub(super) fn install(root: &Path, archive: &Verified) -> Result<PathBuf> {
    let target = root.join("target/hil").join(TARGET);
    fs::create_dir_all(&target)?;
    let _guard = crate::evidence::run::IndexGuard::acquire(&target)?;
    let runs = target.join("runs");
    let archives = target.join("archives");
    fs::create_dir_all(&runs)?;
    fs::create_dir_all(&archives)?;
    let destination = archives.join(&archive.manifest.id);
    // Preflight every collision before exposing any new run.
    same_or_absent(&destination, archive.directory.path())?;
    for id in &archive.manifest.runs {
        same_or_absent(
            &runs.join(id),
            &archive.directory.path().join("runs").join(id),
        )?;
    }
    if !destination.try_exists()? {
        publish_directory(archive.directory.path(), &destination, &target)?;
    }
    for id in &archive.manifest.runs {
        let run = runs.join(id);
        if !run.try_exists()? {
            publish_directory(&destination.join("runs").join(id), &run, &target)?;
        }
    }
    // If a disk error interrupted publication, retry resumes only matching identities.
    Ok(destination)
}

fn same_or_absent(destination: &Path, source: &Path) -> Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            if package::inventory(destination, Path::new(""))?
                != package::inventory(source, Path::new(""))?
            {
                return Err(format!(
                    "existing evidence differs; refusing to overwrite {}",
                    destination.display()
                )
                .into());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn publish_directory(source: &Path, destination: &Path, staging_parent: &Path) -> Result<()> {
    let staging = tempfile::tempdir_in(staging_parent)?;
    copy_tree(source, staging.path())?;
    fs::rename(staging.path(), destination)?;
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    for member in package::inventory(source, Path::new(""))? {
        oer_process::check_cancelled()?;
        let from = source.join(&member.path);
        let to = destination.join(&member.path);
        fs::create_dir_all(to.parent().ok_or("missing parent")?)?;
        // Run bundles are immutable, as in the existing firmware object store.
        if fs::hard_link(&from, &to).is_err() {
            fs::copy(from, to)?;
        }
    }
    Ok(())
}
