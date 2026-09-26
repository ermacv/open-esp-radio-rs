//! Setup results memoized by content.
//!
//! Capture, inventory, the probe catalog, image linking and data exports are
//! determined by the Blobray executable, the authenticated input bytes and the
//! operation's request document. Their results are kept below the scenario's
//! ignored output and reused by later runs, so a warm run creates no Blobray
//! project. A changed executable, input or request selects another entry.
use crate::harness::{Result, sha256};
use serde::{Serialize, de::DeserializeOwned};
use std::fs;
use std::path::{Path, PathBuf};

/// File holding an entry's serialized value.
const VALUE: &str = "value.json";

pub struct SetupCache {
    root: PathBuf,
    /// Digest of the Blobray executable and every input with its role.
    salt: String,
}

impl SetupCache {
    pub fn new(
        output: &Path,
        binary: &Path,
        roles: &[&str],
        identities: &[String],
    ) -> Result<Self> {
        let salt = serde_json::json!({
            "blobray": sha256(&fs::read(binary)?),
            "roles": roles,
            "inputs": identities,
        });
        Ok(Self {
            root: output.join("setup-cache"),
            salt: sha256(salt.to_string().as_bytes()),
        })
    }

    /// The entry directory of `operation` over `document`.
    pub fn entry(&self, operation: &str, document: &impl Serialize) -> Result<Entry> {
        let key = format!(
            "{}\0{operation}\0{}",
            self.salt,
            serde_json::to_string(document)?
        );
        Ok(Entry {
            path: self
                .root
                .join(format!("{operation}-{}", sha256(key.as_bytes()))),
        })
    }
}

pub struct Entry {
    path: PathBuf,
}

impl Entry {
    /// The cached value, when a complete entry exists.
    pub fn load<T: DeserializeOwned>(&self) -> Result<Option<T>> {
        match fs::read(self.path.join(VALUE)) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// A file or directory stored with the entry.
    pub fn file(&self, name: &str) -> PathBuf {
        self.path.join("files").join(name)
    }

    /// Copy the stored file or directory `name` to `destination`.
    pub fn restore(&self, name: &str, destination: &Path) -> Result<()> {
        copy_tree(&self.file(name), destination)
    }

    /// Store `value` with `files` (name, source file or directory). The entry
    /// becomes visible only once complete, so an interrupted store is a miss.
    pub fn store<T: Serialize>(&self, value: &T, files: &[(&str, &Path)]) -> Result<()> {
        let parent = self
            .path
            .parent()
            .expect("entries live below the cache root");
        fs::create_dir_all(parent)?;
        let staging = tempfile::Builder::new()
            .prefix(".entry-")
            .tempdir_in(parent)?;
        fs::create_dir_all(staging.path().join("files"))?;
        for (name, source) in files {
            copy_tree(source, &staging.path().join("files").join(name))?;
        }
        fs::write(staging.path().join(VALUE), serde_json::to_vec(value)?)?;
        let staged = staging.keep();
        match fs::rename(&staged, &self.path) {
            Ok(()) => Ok(()),
            // Another run completed the same entry first.
            Err(_) if self.path.join(VALUE).exists() => {
                fs::remove_dir_all(staged)?;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
}

/// Copy a file, or a directory with everything below it.
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    if source.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        fs::copy(source, destination)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(dir: &Path, identity: &str) -> SetupCache {
        let binary = dir.join("blobray");
        fs::write(&binary, b"engine").unwrap();
        SetupCache::new(dir, &binary, &["rom"], &[identity.to_owned()]).unwrap()
    }

    #[test]
    fn entries_round_trip_and_key_on_inputs_and_documents() {
        let dir = tempfile::tempdir().unwrap();
        let first = cache(dir.path(), "a");
        let entry = first.entry("link", &"request").unwrap();
        assert!(entry.load::<u32>().unwrap().is_none());
        let file = dir.path().join("image.elf");
        fs::write(&file, b"image").unwrap();
        entry.store(&7u32, &[("image.elf", &file)]).unwrap();
        assert_eq!(entry.load::<u32>().unwrap(), Some(7));
        assert_eq!(fs::read(entry.file("image.elf")).unwrap(), b"image");
        // Directories round-trip with their contents.
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("nested")).unwrap();
        fs::write(tree.join("nested/leaf"), b"leaf").unwrap();
        let directory = first.entry("tree", &"request").unwrap();
        directory.store(&(), &[("tree", &tree)]).unwrap();
        directory.restore("tree", &dir.path().join("copy")).unwrap();
        assert_eq!(
            fs::read(dir.path().join("copy/nested/leaf")).unwrap(),
            b"leaf"
        );
        // Another document, input or engine selects another entry.
        assert!(
            first
                .entry("link", &"other")
                .unwrap()
                .load::<u32>()
                .unwrap()
                .is_none()
        );
        let changed = cache(dir.path(), "b");
        assert!(
            changed
                .entry("link", &"request")
                .unwrap()
                .load::<u32>()
                .unwrap()
                .is_none()
        );
        fs::write(dir.path().join("blobray"), b"new engine").unwrap();
        let rebuilt = SetupCache::new(
            dir.path(),
            &dir.path().join("blobray"),
            &["rom"],
            &["a".into()],
        )
        .unwrap();
        assert!(
            rebuilt
                .entry("link", &"request")
                .unwrap()
                .load::<u32>()
                .unwrap()
                .is_none()
        );
    }
}
