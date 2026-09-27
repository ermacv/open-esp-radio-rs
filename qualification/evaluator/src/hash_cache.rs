//! SHA-256 of sealed evidence files, remembered across evaluations.
//!
//! Sealed HIL bundles are immutable, yet every evaluation hashed all of their
//! files again. A digest is remembered for the file's device, inode, size,
//! modification time and status-change time. Any write to the file changes its
//! status-change time, which user code cannot set, so a changed file is always
//! hashed again. Hard links of one file share one entry, which suits the shared
//! run store. The cache lives in the user's cache directory; set
//! `OER_QUALIFICATION_HASH_CACHE=0` to hash every file.

use std::{
    collections::HashMap,
    fs,
    io::Read as _,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use sha2::{Digest, Sha256};

type Key = (u64, u64, u64, i64, i64, i64, i64);

/// Entries unseen this long are dropped when the cache is saved.
const MAX_AGE_SECONDS: u64 = 30 * 24 * 3600;

struct Cache {
    path: Option<PathBuf>,
    /// Digest and the last time an evaluation used it.
    entries: HashMap<String, (String, u64)>,
    added: bool,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Hash every file for the rest of this process, for commands whose results
/// enter tracked files: the unauthenticated cache must not vouch for them.
pub(crate) fn disable() {
    if let Ok(mut cache) = cache().lock() {
        cache.path = None;
    }
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let path = location();
        let entries = path
            .as_ref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Mutex::new(Cache {
            path,
            entries,
            added: false,
        })
    })
}

fn location() -> Option<PathBuf> {
    if std::env::var("OER_QUALIFICATION_HASH_CACHE").is_ok_and(|value| value == "0") || cfg!(test) {
        return None;
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("open-esp-radio/qualification/sha256.json"))
}

fn key(metadata: &fs::Metadata) -> String {
    let key: Key = (
        metadata.dev(),
        metadata.ino(),
        metadata.size(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    );
    format!("{key:?}")
}

/// SHA-256 of the regular file at `path`, from the cache when its identity
/// and times are unchanged.
pub(crate) fn sha256_file(path: &Path) -> crate::Result<String> {
    let before = fs::metadata(path)?;
    let key = key(&before);
    let use_cache = {
        let mut known = cache().lock().map_err(|_| "hash cache poisoned")?;
        let enabled = known.path.is_some();
        if enabled && let Some((digest, seen)) = known.entries.get_mut(&key) {
            *seen = now();
            let digest = digest.clone();
            known.added = true;
            return Ok(digest);
        }
        enabled
    };
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let digest = format!("{:x}", digest.finalize());
    // A file written while it was hashed is not remembered.
    if use_cache && self::key(&fs::metadata(path)?) == key {
        let mut known = cache().lock().map_err(|_| "hash cache poisoned")?;
        known.entries.insert(key, (digest.clone(), now()));
        known.added = true;
    }
    Ok(digest)
}

/// Persist newly computed digests. Failure only loses the cache.
pub(crate) fn save() {
    let Ok(mut cache) = cache().lock() else {
        return;
    };
    let Some(path) = cache.path.clone().filter(|_| cache.added) else {
        return;
    };
    let oldest = now().saturating_sub(MAX_AGE_SECONDS);
    cache.entries.retain(|_, (_, seen)| *seen >= oldest);
    let write = || -> crate::Result<()> {
        fs::create_dir_all(path.parent().ok_or("cache path has no parent")?)?;
        let temporary = path.with_extension(format!("json.{}", std::process::id()));
        fs::write(&temporary, serde_json::to_vec(&cache.entries)?)?;
        fs::rename(&temporary, &path)?;
        Ok(())
    };
    if write().is_ok() {
        cache.added = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_file_gets_a_new_key() {
        let directory = tempfile_directory();
        let path = directory.join("file");
        fs::write(&path, b"one").unwrap();
        let first = key(&fs::metadata(&path).unwrap());
        assert_eq!(first, key(&fs::metadata(&path).unwrap()));
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&path, b"two").unwrap();
        assert_ne!(first, key(&fs::metadata(&path).unwrap()));
        assert_eq!(
            sha256_file(&path).unwrap(),
            format!("{:x}", Sha256::digest(b"two"))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    fn tempfile_directory() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "oer-hash-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }
}
