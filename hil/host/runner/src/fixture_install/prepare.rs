use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Command,
};

use fs2::FileExt as _;
use sha2::{Digest as _, Sha256};

use super::{
    Artifact, ArtifactRole, ArtifactState, BUNDLE_SCHEMA, Bundle, InstallPlan, Provider,
    SourceIdentity,
    model::{PlannedArtifact, validate_adapters},
};
use crate::Result;

pub fn build_plan(root: &Path, provider: Provider, adapters: &[String]) -> Result<InstallPlan> {
    let adapters = validate_adapters(provider, adapters)?;
    let artifacts = provider
        .artifact_specs()
        .iter()
        .map(|artifact| {
            let source = root.join(artifact.source);
            PlannedArtifact {
                role: artifact.role,
                state: if source.is_file() {
                    ArtifactState::PresentUnverified
                } else {
                    ArtifactState::Missing
                },
                source,
                target: artifact.target.into(),
            }
        })
        .collect();
    let build_steps = match provider {
        Provider::LinuxNet => vec![
            "cargo xtask build hostapd".to_owned(),
            "cargo build --locked -p open-esp-radio-hil-runner --bin open-radio-probe --bin open-radio-fixture-install --target-dir target/hil/fixture-build".to_owned(),
        ],
        Provider::LinuxBluetooth => vec![
            "cargo build --locked -p open-esp-radio-hil-runner --bin open-radio-bluetooth --bin open-radio-fixture-install --target-dir target/hil/fixture-build".to_owned(),
        ],
    };
    Ok(InstallPlan {
        schema: 1,
        provider,
        artifacts,
        build_steps,
        authorization_boundary: "foreground sudo exec after unprivileged preparation".to_owned(),
        activation_boundary: "root-owned generation switch after candidate policy validation"
            .to_owned(),
        readonly_verification: vec![
            "imported artifact bytes and modes".to_owned(),
            "effective sudoers policy".to_owned(),
            "finite helper capabilities".to_owned(),
        ],
        automatic_hardware_checks: false,
        allowed_bluetooth_adapters: adapters,
    })
}

pub fn prepare(
    root: &Path,
    provider: Provider,
    operator: &str,
    adapters: &[String],
) -> Result<PathBuf> {
    validate_operator(operator)?;
    let adapters = validate_adapters(provider, adapters)?;
    let output = root.join("target/hil/fixture-install");
    fs::create_dir_all(&output)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(output.join("prepare.lock"))?;
    lock.lock_exclusive()?;

    let source = source_identity(root)?;
    let temporary = tempfile::Builder::new()
        .prefix(&format!("{}-", provider.as_str()))
        .tempdir_in(&output)?;
    let artifact_directory = temporary.path().join("artifacts");
    fs::create_dir(&artifact_directory)?;
    let mut artifacts = Vec::new();
    for spec in provider.artifact_specs() {
        let source_path = root.join(spec.source);
        let metadata = fs::symlink_metadata(&source_path).map_err(|error| {
            format!(
                "prepared artifact {} is unavailable: {error}",
                source_path.display()
            )
        })?;
        if !metadata.file_type().is_file() {
            return Err(format!(
                "prepared artifact is not a regular file: {}",
                source_path.display()
            )
            .into());
        }
        let destination = artifact_directory.join(spec.name);
        copy_and_hash(&source_path, &destination)?;
        fs::set_permissions(&destination, fs::Permissions::from_mode(spec.mode))?;
        let bytes = fs::read(&destination)?;
        artifacts.push(Artifact {
            role: spec.role,
            file_name: spec.name.to_owned(),
            target: spec.target.into(),
            size_bytes: bytes.len() as u64,
            sha256: sha256(&bytes),
            mode: spec.mode,
        });
    }
    validate_prepared_contract(provider, &artifact_directory, &artifacts)?;

    let mut bundle = Bundle {
        schema: BUNDLE_SCHEMA,
        provider,
        generation: String::new(),
        operator: operator.to_owned(),
        source,
        runtime_contract: provider.runtime_contract().to_owned(),
        allowed_bluetooth_adapters: adapters,
        artifacts,
    };
    bundle.generation = bundle_identity(&bundle)?;
    let final_directory = output.join(provider.as_str()).join(&bundle.generation);
    if final_directory.exists() {
        validate_existing_bundle(&final_directory, &bundle)?;
        return Ok(final_directory);
    }
    fs::create_dir_all(final_directory.parent().expect("generation parent"))?;
    write_json(&temporary.path().join("bundle.json"), &bundle)?;
    fs::rename(temporary.keep(), &final_directory)?;
    Ok(final_directory)
}

fn validate_prepared_contract(
    provider: Provider,
    directory: &Path,
    artifacts: &[Artifact],
) -> Result<()> {
    match provider {
        Provider::LinuxNet => {
            let hostapd = artifacts
                .iter()
                .find(|artifact| artifact.role == ArtifactRole::Hostapd)
                .ok_or("hostapd artifact missing")?;
            let provenance: serde_json::Value =
                serde_json::from_slice(&fs::read(directory.join("open-radio-hostapd.json"))?)?;
            if provenance
                .get("binary_sha256")
                .and_then(serde_json::Value::as_str)
                != Some(hostapd.sha256.as_str())
            {
                return Err("hostapd provenance does not identify the prepared binary".into());
            }
            require_capabilities(
                &directory.join("open-radio-net"),
                provider.runtime_contract(),
            )
        }
        Provider::LinuxBluetooth => require_capabilities(
            &directory.join("open-radio-bluetooth"),
            provider.runtime_contract(),
        ),
    }
}

fn require_capabilities(helper: &Path, expected: &str) -> Result<()> {
    let output = Command::new(helper).arg("capabilities").output()?;
    if !output.status.success() || String::from_utf8(output.stdout)?.trim() != expected {
        return Err(format!(
            "prepared helper has an incompatible runtime contract: {}",
            helper.display()
        )
        .into());
    }
    Ok(())
}

fn validate_existing_bundle(directory: &Path, expected: &Bundle) -> Result<()> {
    let actual: Bundle = serde_json::from_slice(&fs::read(directory.join("bundle.json"))?)?;
    if &actual != expected {
        return Err(format!("existing bundle identity mismatch: {}", directory.display()).into());
    }
    for artifact in &actual.artifacts {
        let path = directory.join("artifacts").join(&artifact.file_name);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file()
            || metadata.len() != artifact.size_bytes
            || metadata.permissions().mode() & 0o777 != artifact.mode
            || sha256_file(&path)? != artifact.sha256
        {
            return Err(format!("existing bundle artifact mismatch: {}", path.display()).into());
        }
    }
    validate_prepared_contract(
        actual.provider,
        &directory.join("artifacts"),
        &actual.artifacts,
    )?;
    Ok(())
}

fn source_identity(root: &Path) -> Result<SourceIdentity> {
    let commit = command_output(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"]),
    )?;
    let state = Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()?;
    if !state.status.success() {
        return Err("cannot capture repository state for fixture bundle".into());
    }
    let diff = Command::new("git")
        .current_dir(root)
        .args(["diff", "--binary", "--no-ext-diff", "HEAD", "--"])
        .output()?;
    if !diff.status.success() {
        return Err("cannot capture repository content identity for fixture bundle".into());
    }
    let mut workspace = Sha256::new();
    workspace.update(&state.stdout);
    workspace.update(&diff.stdout);
    Ok(SourceIdentity {
        commit: commit.trim().to_owned(),
        dirty: !state.stdout.is_empty(),
        workspace_state_sha256: format!("{:x}", workspace.finalize()),
    })
}

fn command_output(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!("command failed with {}", output.status).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn bundle_identity(bundle: &Bundle) -> Result<String> {
    let mut identity = bundle.clone();
    identity.generation.clear();
    Ok(sha256(&serde_json::to_vec(&identity)?))
}

fn copy_and_hash(source: &Path, destination: &Path) -> Result<String> {
    let mut source = File::open(source)?;
    let mut destination = File::create(destination)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        destination.write_all(&buffer[..count])?;
        digest.update(&buffer[..count]);
    }
    destination.sync_all()?;
    Ok(format!("{:x}", digest.finalize()))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let mut file = File::create(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn validate_operator(operator: &str) -> Result<()> {
    if operator.is_empty()
        || operator == "root"
        || !operator
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("operator must be a non-root Linux account name".into());
    }
    Ok(())
}
