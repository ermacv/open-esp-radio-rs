//! Observational disk accounting, without writer acquisition, recovery or GC.
use super::*;

fn add(target: &mut u64, n: u64) -> Result<()> {
    *target = target
        .checked_add(n)
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "storage usage overflow"))?;
    Ok(())
}
fn file(usage: &mut FileUsage, metadata: &fs::Metadata) -> Result<()> {
    if metadata.is_file() {
        add(&mut usage.files, 1)?;
        add(&mut usage.logical_bytes, metadata.len())
    } else {
        add(&mut usage.non_regular_entries, 1)
    }
}
fn tree(
    path: &Path,
    usage: &mut FileUsage,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    // One traversal frame per directory depth, never a list of all descendants.
    let mut frames = AdmittedVec::new(memory);
    frames.push(
        (
            fs::read_dir(path).map_err(io)?,
            memory.reserve(4096, c.position())?,
        ),
        c.position(),
    )?;
    while !frames.is_empty() {
        c.checkpoint(1)?;
        let entry = frames.last_mut().unwrap().0.next();
        let Some(entry) = entry else {
            frames.pop();
            continue;
        };
        let entry = entry.map_err(io)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(io)?;
        if metadata.is_dir() {
            let charge = memory.reserve(4096, c.position())?;
            frames.push(
                (fs::read_dir(entry.path()).map_err(io)?, charge),
                c.position(),
            )?;
        } else {
            file(usage, &metadata)?;
        }
    }
    Ok(())
}
impl Project {
    /// Logical file sizes, including unreachable CAS objects. Does not follow
    /// symlinks or classify any object as reclaimable. Concurrent disappearance
    /// fails explicitly; concurrent size changes remain interval observations.
    pub fn storage_usage(
        &self,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<StorageUsage> {
        let _workspace = memory.reserve(65536, c.position())?;
        let mut usage = StorageUsage::default();
        let connection = open_connection(&self.root, false)?;
        connection.execute_batch("BEGIN").map_err(db)?;
        for (table, count) in [
            ("revisions", &mut usage.revisions),
            ("analyses", &mut usage.analyses),
            ("publications", &mut usage.publications),
            ("images", &mut usage.images),
            ("knowledge_revisions", &mut usage.knowledge_revisions),
            ("runs", &mut usage.runs),
        ] {
            let mut statement = connection
                .prepare(&format!("SELECT 1 FROM {table}"))
                .map_err(db)?;
            let mut rows = statement.query([]).map_err(db)?;
            while rows.next().map_err(db)?.is_some() {
                c.checkpoint(1)?;
                add(count, 1)?;
            }
        }
        for entry in fs::read_dir(&self.root).map_err(io)? {
            c.checkpoint(1)?;
            let entry = entry.map_err(io)?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(io)?;
            let usage = if entry.file_name() == "objects" {
                &mut usage.cas
            } else if entry.file_name() == "staging" {
                &mut usage.staging
            } else {
                &mut usage.metadata
            };
            if metadata.is_dir() {
                tree(&entry.path(), usage, memory, c)?;
            } else {
                file(usage, &metadata)?;
            }
        }
        Ok(usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usage_is_read_only_includes_unreachable_files_and_honors_control() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::create(dir.path()).unwrap();
        fs::write(project.root.join("objects/unreachable"), [0; 123]).unwrap();
        fs::create_dir(project.root.join("staging/attempt")).unwrap();
        fs::write(project.root.join("staging/attempt/partial"), [0; 45]).unwrap();
        let before = fs::read(project.root.join("project.sqlite3")).unwrap();
        let _writer = project.writer().unwrap();
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let usage = project.storage_usage(&memory, &mut || Ok(())).unwrap();
        assert_eq!(usage.cas.logical_bytes, 123);
        assert_eq!(usage.staging.logical_bytes, 45);
        assert!(!usage.reachability_assessed);
        assert!(!usage.atomic_filesystem_snapshot);
        assert_eq!(memory.used(), 0);
        assert_eq!(
            before,
            fs::read(project.root.join("project.sqlite3")).unwrap()
        );
        assert_eq!(
            project
                .storage_usage(&memory, &mut || Err(Error::new(
                    ErrorCode::Cancelled,
                    "cancelled"
                )))
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        assert_eq!(
            fs::read(project.root.join("objects/unreachable"))
                .unwrap()
                .len(),
            123
        );
        assert_eq!(memory.used(), 0);
    }
}
