//! Verify the complete archive before exposing build inputs.

use super::*;

/// Materialize only verified regular files into a newly owned build directory.
pub(super) fn materialize(directory: &Path, destination: &Path) -> Result<(Snapshot, Manifest)> {
    let snapshot: Snapshot = serde_json::from_slice(&fs::read(directory.join("snapshot.json"))?)?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    if snapshot.schema != 1
        || manifest.schema != 1
        || digest(&serde_json::to_vec(&manifest)?) != snapshot.snapshot_id
        || crate::durable::sha256_file(&directory.join("sources.tar"))? != snapshot.archive_sha256
    {
        return Err("source snapshot identity or archive integrity mismatch".into());
    }
    let mut expected = BTreeMap::new();
    let mut roles = BTreeSet::new();
    for source in &manifest.sources {
        if !matches!(
            source.name.as_str(),
            "repository" | "esp-hal" | "embassy" | "xarxa"
        ) || !roles.insert(&source.name)
        {
            return Err("invalid or duplicate snapshot source role".into());
        }
        for file in &source.files {
            contained(&file.path)?;
            if !matches!(file.mode, 0o644 | 0o755)
                || expected
                    .insert(Path::new(&source.name).join(&file.path), file)
                    .is_some()
            {
                return Err("invalid or duplicate snapshot input".into());
            }
        }
    }
    if !roles.contains(&"repository".to_owned()) || expected.len() != snapshot.files {
        return Err("incomplete source snapshot manifest".into());
    }
    let mut archive = tar::Archive::new(std::io::BufReader::with_capacity(
        1 << 20,
        fs::File::open(directory.join("sources.tar"))?,
    ));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        contained(&path)?;
        let file = expected
            .remove(&path)
            .ok_or("unlisted or duplicate source archive member")?;
        if !entry.header().entry_type().is_file()
            || entry.size() != file.size_bytes
            || entry.header().mode()? != file.mode
        {
            return Err("source archive member disagrees with input manifest".into());
        }
        let output = destination.join(&path);
        fs::create_dir_all(output.parent().ok_or("source file has no parent")?)?;
        let mut target = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        // A checkout is transient: a failed build materializes it again, and
        // the digest below checks what was written, so no fsync is needed.
        std::io::copy(&mut entry, &mut target)?;
        drop(target);
        if crate::durable::sha256_file(&output)? != file.sha256 {
            return Err("source archive content digest mismatch".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&output, fs::Permissions::from_mode(file.mode))?;
        }
    }
    if !expected.is_empty() {
        return Err("source archive is missing declared inputs".into());
    }
    Ok((snapshot, manifest))
}
