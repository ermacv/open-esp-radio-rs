//! Opt-in, two-checkout firmware reproducibility verification.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Artifacts, build_resolved, program_from_env};
use crate::{Result, image::ImageClass};

const SCHEMA: u16 = 1;
const BUILD_TYPE: &str = "open-esp-radio-hil-two-root-rebuild/v1";
const LOCAL_OVERRIDE_VARIABLES: [&str; 3] =
    ["ESP_HAL_ROOT", "EMBASSY_ROOT", "OPEN_RADIO_XARXA_ROOT"];
static VERIFICATION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Verdict {
    ByteIdentical,
    NotByteIdentical,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PathTreatment {
    Unmodified,
    CargoTrimPathsObject,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SubjectRole {
    Application,
    BootstrapElf,
    EffectiveEmbeddedLock,
    EffectiveBootstrapLock,
    RuntimeBin,
    RuntimeElf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FileIdentity {
    path: PathBuf,
    size_bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SectionLayoutComparison {
    left_path: PathBuf,
    right_path: PathBuf,
    all_sections_identical: bool,
    allocated_sections_identical: bool,
    left_all_sections_sha256: String,
    right_all_sections_sha256: String,
    left_allocated_sections_sha256: String,
    right_allocated_sections_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SubjectComparison {
    role: SubjectRole,
    left: FileIdentity,
    right: FileIdentity,
    byte_identical: bool,
    section_layout: Option<SectionLayoutComparison>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct VerificationConstraints {
    clean_commit: bool,
    local_path_overrides: bool,
    cargo_incremental: String,
    different_absolute_source_roots: bool,
    different_source_root_lengths: bool,
    path_treatment: PathTreatment,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RebuildVerification {
    schema: u16,
    verification_id: String,
    build_type: String,
    commit: String,
    image_class: ImageClass,
    runtime_profile: String,
    output_directory: PathBuf,
    constraints: VerificationConstraints,
    subjects: Vec<SubjectComparison>,
    verdict: Verdict,
}

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn create() -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = env::temp_dir().join(format!(
            "open-esp-radio-rebuild-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        // Only remove the empty directory created by this process. Worktree
        // cleanup owns its contents and a failed cleanup must remain visible.
        let _ = fs::remove_dir(&self.path);
    }
}

struct DetachedWorktree {
    repository: PathBuf,
    path: PathBuf,
}

impl DetachedWorktree {
    fn create(repository: &Path, path: PathBuf, commit: &str) -> Result<Self> {
        let mut command = Command::new("git");
        command
            .args(["-C"])
            .arg(repository)
            .args(["worktree", "add", "--detach"])
            .arg(&path)
            .arg(commit);
        require_success(command.output()?, "create detached rebuild worktree")?;
        Ok(Self {
            repository: repository.to_owned(),
            path,
        })
    }
}

impl Drop for DetachedWorktree {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .args(["-C"])
            .arg(&self.repository)
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

pub(crate) fn verify_rebuild(root: &Path, class: ImageClass, trim_paths: bool) -> Result<()> {
    reject_local_overrides()?;
    let commit = require_clean_commit(root)?;
    let path_treatment = if trim_paths {
        PathTreatment::CargoTrimPathsObject
    } else {
        PathTreatment::Unmodified
    };
    let verification_id = verification_id(&commit, class, path_treatment)?;
    let output = root
        .join("target/hil/esp32s31/reproducibility")
        .join(&verification_id);
    fs::create_dir_all(&output)?;

    let scratch = ScratchDirectory::create()?;
    // Intentionally use names with different lengths. A path-remapping bug
    // can otherwise be hidden by equal-length substitutions.
    let left_source = scratch.path.join("source-a");
    let right_source = scratch.path.join("source-directory-b");
    let left = DetachedWorktree::create(root, left_source, &commit)?;
    let right = DetachedWorktree::create(root, right_source, &commit)?;
    let different_absolute_source_roots =
        left.path != right.path && left.path.is_absolute() && right.path.is_absolute();
    let different_source_root_lengths = left.path.as_os_str().len() != right.path.as_os_str().len();
    if !different_absolute_source_roots || !different_source_root_lengths {
        return Err(
            "rebuild verifier failed to construct meaningfully different source roots".into(),
        );
    }

    eprintln!("==> rebuild A from clean detached worktree");
    let left_artifacts = build_resolved(
        &left.path,
        class,
        None,
        None,
        None,
        Some(&output.join("build-a")),
        trim_paths,
    )?;
    eprintln!("==> rebuild B from a different clean detached worktree");
    let right_artifacts = build_resolved(
        &right.path,
        class,
        None,
        None,
        None,
        Some(&output.join("build-directory-b")),
        trim_paths,
    )?;

    let subjects = compare_artifacts(root, &output, &left_artifacts, &right_artifacts)?;
    let verdict = if subjects.iter().all(|subject| subject.byte_identical) {
        Verdict::ByteIdentical
    } else {
        Verdict::NotByteIdentical
    };
    let report = RebuildVerification {
        schema: SCHEMA,
        verification_id,
        build_type: BUILD_TYPE.to_owned(),
        commit,
        image_class: class,
        runtime_profile: class.runtime_profile().to_owned(),
        output_directory: relative_to(root, &output)?,
        constraints: VerificationConstraints {
            clean_commit: true,
            local_path_overrides: false,
            cargo_incremental: String::from("0"),
            different_absolute_source_roots,
            different_source_root_lengths,
            path_treatment,
        },
        subjects,
        verdict,
    };
    let report_path = output.join("rebuild-verification.json");
    atomic_json(&report_path, &report)?;
    crate::emit_json(&report, true)?;
    if report.verdict == Verdict::ByteIdentical {
        Ok(())
    } else {
        Err(format!(
            "two-root rebuild is not byte-identical; inspect {}",
            report_path.display()
        )
        .into())
    }
}

fn reject_local_overrides() -> Result<()> {
    let active = LOCAL_OVERRIDE_VARIABLES
        .into_iter()
        .filter(|variable| env::var_os(variable).is_some())
        .collect::<Vec<_>>();
    if active.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "two-root rebuild verification requires immutable Git dependencies; unset local path override(s): {}",
            active.join(", ")
        )
        .into())
    }
}

fn require_clean_commit(root: &Path) -> Result<String> {
    let mut revision = Command::new("git");
    revision.args(["-C"]).arg(root).args(["rev-parse", "HEAD"]);
    let commit = require_stdout(revision.output()?, "resolve rebuild commit")?;
    let mut status = Command::new("git");
    status
        .args(["-C"])
        .arg(root)
        .args(["status", "--porcelain=v1", "--untracked-files=all"]);
    let status = require_stdout(status.output()?, "inspect rebuild source state")?;
    if !status.is_empty() {
        return Err(
            "two-root rebuild verification requires a clean source repository; commit or stash every tracked and untracked change"
                .into(),
        );
    }
    Ok(commit)
}

fn verification_id(
    commit: &str,
    class: ImageClass,
    path_treatment: PathTreatment,
) -> Result<String> {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let sequence = VERIFICATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let short_commit = commit.get(..12).unwrap_or(commit);
    let treatment = match path_treatment {
        PathTreatment::Unmodified => "native-paths",
        PathTreatment::CargoTrimPathsObject => "trim-paths",
    };
    Ok(format!(
        "{millis}-{sequence:04}-{short_commit}-{}-{treatment}",
        class.id(),
    ))
}

fn compare_artifacts(
    root: &Path,
    output: &Path,
    left: &Artifacts,
    right: &Artifacts,
) -> Result<Vec<SubjectComparison>> {
    [
        (
            SubjectRole::Application,
            &left.application_image,
            &right.application_image,
            false,
        ),
        (
            SubjectRole::BootstrapElf,
            &left.bootstrap_elf,
            &right.bootstrap_elf,
            true,
        ),
        (
            SubjectRole::EffectiveBootstrapLock,
            &left.effective_bootstrap_lock,
            &right.effective_bootstrap_lock,
            false,
        ),
        (
            SubjectRole::EffectiveEmbeddedLock,
            &left.effective_embedded_lock,
            &right.effective_embedded_lock,
            false,
        ),
        (
            SubjectRole::RuntimeBin,
            &left.runtime_bin,
            &right.runtime_bin,
            false,
        ),
        (
            SubjectRole::RuntimeElf,
            &left.runtime_elf,
            &right.runtime_elf,
            true,
        ),
    ]
    .into_iter()
    .map(|(role, left_path, right_path, elf)| {
        compare_subject(root, output, role, left_path, right_path, elf)
    })
    .collect()
}

fn compare_subject(
    root: &Path,
    output: &Path,
    role: SubjectRole,
    left_path: &Path,
    right_path: &Path,
    elf: bool,
) -> Result<SubjectComparison> {
    let left = file_identity(root, left_path)?;
    let right = file_identity(root, right_path)?;
    let byte_identical = left.size_bytes == right.size_bytes && left.sha256 == right.sha256;
    let section_layout = elf
        .then(|| compare_section_layout(root, output, role, left_path, right_path))
        .transpose()?;
    Ok(SubjectComparison {
        role,
        left,
        right,
        byte_identical,
        section_layout,
    })
}

fn file_identity(root: &Path, path: &Path) -> Result<FileIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(format!("rebuild subject is not a regular file: {}", path.display()).into());
    }
    Ok(FileIdentity {
        path: relative_to(root, path)?,
        size_bytes: metadata.len(),
        sha256: sha256_bytes(&fs::read(path)?),
    })
}

fn compare_section_layout(
    root: &Path,
    output: &Path,
    role: SubjectRole,
    left_elf: &Path,
    right_elf: &Path,
) -> Result<SectionLayoutComparison> {
    let left = elf_sections(left_elf)?;
    let right = elf_sections(right_elf)?;
    let stem = match role {
        SubjectRole::BootstrapElf => "bootstrap-elf",
        SubjectRole::RuntimeElf => "runtime-elf",
        _ => return Err("section comparison requested for a non-ELF subject".into()),
    };
    let left_path = output.join(format!("{stem}-sections-a.json"));
    let right_path = output.join(format!("{stem}-sections-b.json"));
    atomic_json(&left_path, &left.all)?;
    atomic_json(&right_path, &right.all)?;
    let left_all = serde_json::to_vec(&left.all)?;
    let right_all = serde_json::to_vec(&right.all)?;
    let left_allocated = serde_json::to_vec(&left.allocated)?;
    let right_allocated = serde_json::to_vec(&right.allocated)?;
    Ok(SectionLayoutComparison {
        left_path: relative_to(root, &left_path)?,
        right_path: relative_to(root, &right_path)?,
        all_sections_identical: left_all == right_all,
        allocated_sections_identical: left_allocated == right_allocated,
        left_all_sections_sha256: sha256_bytes(&left_all),
        right_all_sections_sha256: sha256_bytes(&right_all),
        left_allocated_sections_sha256: sha256_bytes(&left_allocated),
        right_allocated_sections_sha256: sha256_bytes(&right_allocated),
    })
}

struct ElfSections {
    all: Vec<serde_json::Value>,
    allocated: Vec<serde_json::Value>,
}

fn elf_sections(path: &Path) -> Result<ElfSections> {
    let mut command = Command::new(program_from_env("LLVM_READELF", "llvm-readelf"));
    command
        .args(["--elf-output-style=JSON", "--sections"])
        .arg(path);
    let stdout = require_output(command.output()?, "inspect ELF section layout")?;
    let document: serde_json::Value = serde_json::from_slice(&stdout)?;
    let sections = document
        .as_array()
        .and_then(|files| files.first())
        .and_then(|file| file.get("Sections"))
        .and_then(serde_json::Value::as_array)
        .ok_or("llvm-readelf returned an unexpected section document")?
        .clone();
    let allocated = sections
        .iter()
        .filter(|section| {
            section
                .get("Section")
                .and_then(|section| section.get("Flags"))
                .and_then(|flags| flags.get("Flags"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|flags| {
                    flags.iter().any(|flag| {
                        flag.get("Name").and_then(serde_json::Value::as_str) == Some("SHF_ALLOC")
                    })
                })
        })
        .cloned()
        .collect();
    Ok(ElfSections {
        all: sections,
        allocated,
    })
}

fn relative_to(root: &Path, path: &Path) -> Result<PathBuf> {
    path.strip_prefix(root).map(Path::to_owned).map_err(|_| {
        format!(
            "rebuild artifact escapes repository target: {}",
            path.display()
        )
        .into()
    })
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn require_stdout(output: Output, description: &str) -> Result<String> {
    let stdout = require_output(output, description)?;
    Ok(String::from_utf8(stdout)?.trim().to_owned())
}

fn require_output(output: Output, description: &str) -> Result<Vec<u8>> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "{description} failed with {}: {}",
            output.status,
            stderr.trim()
        )
        .into())
    }
}

fn require_success(output: Output, description: &str) -> Result<()> {
    require_output(output, description).map(|_| ())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests;
