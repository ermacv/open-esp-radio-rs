//! Deterministic tar/gzip encoding with a complete inventory and bounded extraction.
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, Write},
    path::{Component, Path},
};

use flate2::{Compression, bufread::GzDecoder, write::GzEncoder};
use sha2::{Digest, Sha256};

use super::{Manifest, Member, TARGET, validate_id};
use crate::Result;

const MAX_FILES: usize = 100_000;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024 * 1024;

pub(super) struct Verified {
    pub(super) directory: tempfile::TempDir,
    pub(super) manifest: Manifest,
    pub(super) sha256: String,
}

pub(super) fn digest(path: &Path) -> Result<String> {
    copy_digest(&mut File::open(path)?, &mut std::io::sink())
}

fn copy_digest(input: &mut impl Read, output: &mut impl Write) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        oer_process::check_cancelled()?;
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        output.write_all(&buffer[..n])?;
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn validate_path(path: &Path) -> Result<String> {
    let text = path.to_str().ok_or("archive path is not UTF-8")?;
    if text.is_empty()
        || text.contains(['\\', ':'])
        || text
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("non-canonical archive path: {text}").into());
    }
    Ok(text.to_owned())
}

pub(super) fn inventory(directory: &Path, prefix: &Path) -> Result<Vec<Member>> {
    let mut files = Vec::new();
    collect(directory, prefix, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect(directory: &Path, prefix: &Path, files: &mut Vec<Member>) -> Result<()> {
    if !fs::symlink_metadata(directory)?.is_dir() {
        return Err("archive input must be a real directory".into());
    }
    for entry in fs::read_dir(directory)? {
        oer_process::check_cancelled()?;
        let entry = entry?;
        let relative = prefix.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(&entry.path(), &relative, files)?;
        } else if kind.is_file() {
            files.push(Member {
                path: validate_path(&relative)?,
                size_bytes: entry.metadata()?.len(),
                sha256: digest(&entry.path())?,
            });
        } else {
            return Err(format!(
                "archive input contains a link or special file: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.schema != 1 || manifest.target != TARGET || manifest.runs.is_empty() {
        return Err("unsupported or empty HIL archive".into());
    }
    validate_id(&manifest.id)?;
    let mut runs = BTreeSet::new();
    for id in &manifest.runs {
        validate_id(id)?;
        if !runs.insert(id) {
            return Err("duplicate run ID".into());
        }
    }
    if manifest.files.len() > MAX_FILES {
        return Err("archive file count exceeds limit".into());
    }
    let mut paths = BTreeSet::new();
    let mut total = 0_u64;
    for file in &manifest.files {
        validate_path(Path::new(&file.path))?;
        let permitted = file.path.starts_with("supplement/")
            || runs
                .iter()
                .any(|id| file.path.starts_with(&format!("runs/{id}/")));
        if !permitted
            || !paths.insert(&file.path)
            || file.size_bytes > MAX_FILE_BYTES
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(format!("invalid archive member: {}", file.path).into());
        }
        total = total
            .checked_add(file.size_bytes)
            .ok_or("archive size overflow")?;
    }
    if total > MAX_TOTAL_BYTES {
        return Err("archive exceeds extraction limit".into());
    }
    Ok(())
}

pub(super) fn export(
    root: &Path,
    id: &str,
    mut runs: Vec<String>,
    supplement: Option<&Path>,
    output: &Path,
) -> Result<String> {
    validate_id(id)?;
    runs.sort();
    if runs.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("duplicate run selection".into());
    }
    let target = root.join("target/hil").join(TARGET);
    let mut files = Vec::new();
    for run in &runs {
        validate_id(run)?;
        eprintln!("archive: verify source run {run}");
        crate::evidence::verify::verify_at(&target, TARGET, Some(run))?;
        files.extend(inventory(
            &target.join("runs").join(run),
            &Path::new("runs").join(run),
        )?);
    }
    if let Some(supplement) = supplement {
        files.extend(inventory(supplement, Path::new("supplement"))?);
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest = Manifest {
        schema: 1,
        id: id.to_owned(),
        target: TARGET.to_owned(),
        runs,
        files,
    };
    validate_manifest(&manifest)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    eprintln!(
        "archive: encode {} files from {} runs",
        manifest.files.len(),
        manifest.runs.len()
    );
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let encoder = GzEncoder::new(temporary.as_file_mut(), Compression::default());
        let mut writer = tar::Builder::new(encoder);
        let bytes = serde_json::to_vec_pretty(&manifest)?;
        append(&mut writer, "archive.json", bytes.len() as u64, &bytes[..])?;
        for member in &manifest.files {
            oer_process::check_cancelled()?;
            let source = match member.path.strip_prefix("supplement/") {
                Some(relative) => supplement.ok_or("missing supplement")?.join(relative),
                None => target.join(&member.path),
            };
            append(
                &mut writer,
                &member.path,
                member.size_bytes,
                File::open(source)?,
            )?;
        }
        writer.into_inner()?.finish()?;
    }
    temporary.as_file().sync_all()?;
    // Verify the bytes actually encoded, including native run seals, before publishing.
    eprintln!("archive: verify encoded package");
    let verified = open(temporary.path(), None)?;
    temporary.persist_noclobber(output)?;
    Ok(verified.sha256)
}

fn append<W: Write, R: Read>(
    writer: &mut tar::Builder<W>,
    path: &str,
    size: u64,
    input: R,
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(size);
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    writer.append_data(&mut header, path, input)?;
    Ok(())
}

pub(super) fn open(archive: &Path, expected: Option<&str>) -> Result<Verified> {
    eprintln!("archive: check digest and extract {}", archive.display());
    // Bind the digest to exactly the bytes decoded, even if the caller replaces
    // or modifies the source file while verification is in progress.
    let mut snapshot = tempfile::tempfile()?;
    let sha256 = copy_digest(&mut File::open(archive)?, &mut snapshot)?;
    snapshot.rewind()?;
    if expected.is_some_and(|value| value != sha256) {
        return Err("archive SHA-256 does not match the pinned digest".into());
    }
    let directory = tempfile::tempdir()?;
    let mut reader = tar::Archive::new(GzDecoder::new(BufReader::new(snapshot)));
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    for entry in reader.entries()? {
        oer_process::check_cancelled()?;
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err("archive may contain only regular files".into());
        }
        let path = validate_path(&entry.path()?)?;
        let size = entry.size();
        total = total.checked_add(size).ok_or("archive size overflow")?;
        if size > MAX_FILE_BYTES
            || total > MAX_TOTAL_BYTES
            || !seen.insert(path.clone())
            || seen.len() > MAX_FILES + 1
        {
            return Err("archive has duplicate entries or exceeds extraction limits".into());
        }
        if path == "archive.json" && size > 32 * 1024 * 1024 {
            return Err("archive manifest is too large".into());
        }
        let output = directory.path().join(&path);
        fs::create_dir_all(output.parent().ok_or("archive member has no parent")?)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        if std::io::copy(&mut entry, &mut file)? != size {
            return Err("truncated archive entry".into());
        }
    }
    // Drain the gzip trailer too, so compressed-stream checksum failures are observed.
    let mut decoder = reader.into_inner();
    std::io::copy(&mut decoder, &mut std::io::sink())?;
    if !decoder.into_inner().fill_buf()?.is_empty() {
        return Err("archive has trailing compressed data".into());
    }
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(directory.path().join("archive.json"))?)?;
    validate_manifest(&manifest)?;
    let actual = inventory(directory.path(), Path::new(""))?;
    let payload: Vec<_> = actual
        .into_iter()
        .filter(|file| file.path != "archive.json")
        .collect();
    if payload != manifest.files {
        return Err("archive contents do not match the complete inventory".into());
    }
    eprintln!(
        "archive: verify {} files and {} native run seals",
        manifest.files.len(),
        manifest.runs.len()
    );
    for run in &manifest.runs {
        crate::evidence::verify::verify_at(directory.path(), TARGET, Some(run))?;
    }
    Ok(Verified {
        directory,
        manifest,
        sha256,
    })
}
