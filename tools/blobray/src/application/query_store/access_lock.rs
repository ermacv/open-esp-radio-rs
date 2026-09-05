//! Explicit release of the database lock shared by one logical cache access.

use std::fs::File;

/// Wrap only a file whose shared or exclusive lock has already been acquired.
///
/// Owners share this guard through `Arc`; duplicated descriptors alone do not
/// extend the logical access. In particular, a concurrent fork can inherit a
/// descriptor before close-on-exec runs. Closing the parent's copy would leave
/// that lock held until the child closes its copy, so the last logical owner
/// explicitly unlocks before closing its descriptor.
#[derive(Debug)]
pub(super) struct AccessLock {
    file: File,
}

impl AccessLock {
    pub(super) fn new(file: File) -> Self {
        Self { file }
    }

    /// Borrow the descriptor for identity checks or SQLite cleanup; cloning it
    /// does not create another logical owner of the access lock.
    pub(super) fn file(&self) -> &File {
        &self.file
    }
}

impl Drop for AccessLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
