//! Atomic report writes and canonical bundle file inventories.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{Attachment, IntegrityFile, IntegrityIndex, RUN_SCHEMA, UNIQUE_FILE_COUNTER};
use crate::Result;

pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub(crate) fn collect_attachments(
    output: &Path,
    artifact_directory: &Path,
) -> Result<Vec<Attachment>> {
    let mut attachments = Vec::new();
    collect_attachments_below(output, Path::new(""), artifact_directory, &mut attachments)?;
    Ok(attachments)
}

fn collect_attachments_below(
    directory: &Path,
    relative: &Path,
    artifact_directory: &Path,
    attachments: &mut Vec<Attachment>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        let child_relative = relative.join(entry.file_name());
        if file_type.is_dir() {
            collect_attachments_below(
                &entry.path(),
                &child_relative,
                artifact_directory,
                attachments,
            )?;
        } else if file_type.is_file() {
            let metadata = entry.metadata()?;
            attachments.push(Attachment {
                path: artifact_directory.join(&child_relative),
                media_type: attachment_media_type(&child_relative).to_owned(),
                size_bytes: metadata.len(),
                sha256: sha256_file(&entry.path())?,
            });
        } else {
            return Err(format!(
                "HIL artifact is neither a regular file nor a directory: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn attachment_media_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => "application/json",
        Some("jsonl") => "application/x-ndjson",
        Some("pcap") | Some("pcapng") => "application/vnd.tcpdump.pcap",
        Some("html") => "text/html",
        Some("md") => "text/markdown",
        Some("log") | Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

pub(in crate::reporting) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("report path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let counter = UNIQUE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("report path has no file name: {}", path.display()))?
        .to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(in crate::reporting) fn write_integrity_index(
    directory: &Path,
    run_id: &str,
) -> Result<PathBuf> {
    let path = directory.join("integrity.json");
    let index = IntegrityIndex {
        schema: RUN_SCHEMA,
        run_id: run_id.to_owned(),
        files: collect_integrity_files(directory)?,
    };
    atomic_json(&path, &index)?;
    Ok(path)
}

pub(in crate::reporting) fn collect_integrity_files(
    directory: &Path,
) -> Result<Vec<IntegrityFile>> {
    let mut files = Vec::new();
    collect_integrity_files_below(directory, Path::new(""), &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_integrity_files_below(
    directory: &Path,
    relative: &Path,
    files: &mut Vec<IntegrityFile>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        let child_relative = relative.join(entry.file_name());
        if file_type.is_dir() {
            collect_integrity_files_below(&entry.path(), &child_relative, files)?;
        } else if file_type.is_file() {
            if child_relative == Path::new("integrity.json") {
                continue;
            }
            files.push(IntegrityFile {
                path: child_relative,
                size_bytes: entry.metadata()?.len(),
                sha256: sha256_file(&entry.path())?,
            });
        } else {
            return Err(format!(
                "HIL run bundle contains neither a regular file nor a directory: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

pub(in crate::reporting) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
