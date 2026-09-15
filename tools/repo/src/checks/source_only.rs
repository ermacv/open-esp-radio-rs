//! Compose source gates; domain validators remain with their existing owners.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Cursor,
    path::{Path, PathBuf},
    process::Stdio,
};

use cargo_metadata::Message;
use sha2::{Digest, Sha256};

use super::{
    TARGET, architecture, artifacts, bluetooth, common, docs, examples, metadata, network, safety,
};
use crate::{
    Context, Result, cargo,
    process::{self, owned},
};

const PHY: &str = "crates/hardware/esp32s31/phy/Cargo.toml";
const INVESTIGATION: &str = "verification/vendor/projects/esp32s31/vendor-project.toml";
const PUBLICATION: &str = "registers/esp32s31/publication/vendor-project.toml";
const PHY_PACKAGES: &[&str] = &[
    "critical-section",
    "oer-memory",
    "oer-esp32s31-coex",
    "oer-esp32s31-hal",
    "oer-esp32s31-ieee802154-irq",
    "oer-esp32s31-pac",
    "oer-esp32s31-pac-raw",
    "oer-esp32s31-phy",
    // Safe structural pin projection for the observed child future; this is
    // a Rust macro library, with no allocator, native build or radio ABI.
    "pin-project-lite",
    "vcell",
];

fn production_lints(ctx: &Context) -> Result<()> {
    let packages = common::production_packages(ctx)?;
    let mut members = Vec::new();
    let mut isolated = Vec::new();
    for package in &packages {
        if package.workspace_member && common::declared_profiles(&package.package)?.is_empty() {
            members.push(package);
        } else {
            isolated.push(package);
        }
    }
    if !members.is_empty() {
        let mut command = ctx.cargo();
        command.args(["clippy", "--quiet", "--locked", "--offline"]);
        for package in members {
            command.args(["--package", package.package.name.as_str()]);
        }
        command.args([
            "--target",
            TARGET,
            "--lib",
            "--all-features",
            "--no-deps",
            "--",
            "-D",
            "clippy::disallowed-methods",
        ]);
        process::run(&mut command)?;
    }
    for package in isolated {
        for profile in common::maximal_profiles(&package.package)? {
            process::run(
                ctx.cargo()
                    .args([
                        "clippy",
                        "--quiet",
                        "--locked",
                        "--offline",
                        "--manifest-path",
                    ])
                    .arg(&package.manifest)
                    .args([
                        "--package",
                        package.package.name.as_str(),
                        "--target",
                        TARGET,
                        "--lib",
                    ])
                    .args(profile)
                    .args(["--no-deps", "--", "-D", "clippy::disallowed-methods"]),
            )?;
        }
    }
    Ok(())
}

fn publication(ctx: &Context) -> Result<()> {
    process::run(ctx.cargo().args([
        "blobray",
        "project",
        "configure",
        "--project",
        INVESTIGATION,
        "--check",
    ]))?;
    process::run(
        ctx.cargo()
            .args(["blobray", "registers", "validate", "--project", PUBLICATION]),
    )?;
    for generator in [
        "export-svd",
        "generate-pac-raw",
        "generate-pac-api",
        "generate-bindings",
    ] {
        process::run(ctx.cargo().args([
            "blobray",
            "registers",
            generator,
            "--project",
            PUBLICATION,
            "--check",
        ]))?;
    }
    if ctx
        .root
        .join("verification/vendor/projects/esp32s31/generated/findings/review-scopes.json")
        .is_file()
    {
        process::run(ctx.cargo().args([
            "blobray",
            "project",
            "publish",
            "--project",
            INVESTIGATION,
            "--check",
        ]))?;
    } else {
        println!(
            "source-only: optional review-scope report absent; artifact-scoped publication not selected"
        );
    }
    Ok(())
}

fn phy_artifact(messages: &[u8]) -> Result<PathBuf> {
    let mut artifacts = BTreeSet::new();
    for message in Message::parse_stream(Cursor::new(messages)) {
        if let Message::CompilerArtifact(artifact) = message?
            && artifact.target.name == "oer_esp32s31_phy"
            && !artifact.profile.test
            && artifact
                .target
                .kind
                .contains(&cargo_metadata::TargetKind::Lib)
        {
            artifacts.extend(
                artifact
                    .filenames
                    .into_iter()
                    .filter(|p| p.extension() == Some("rlib")),
            );
        }
    }
    if artifacts.len() != 1 {
        return Err("build must emit exactly one PHY rlib".into());
    }
    Ok(artifacts
        .into_iter()
        .next()
        .expect("one artifact")
        .into_std_path_buf())
}

fn phy(ctx: &Context) -> Result<PathBuf> {
    let output = process::capture(ctx.cargo().args([
        "build",
        "--locked",
        "--offline",
        "-p",
        "oer-esp32s31-phy",
        "--lib",
        "--release",
        "--target",
        TARGET,
        "--message-format=json-render-diagnostics",
    ]))?;
    let artifact = phy_artifact(&output.stdout)?;
    artifacts::audit_phy(ctx, &artifact)?;
    let manifest = ctx.root.join(PHY);
    let graph = cargo::metadata(ctx, &manifest, &[], Some(TARGET), true)?;
    for package in common::closure(&graph, &graph.root(&manifest)?)? {
        if !PHY_PACKAGES.contains(&package.name.as_str()) {
            return Err(format!(
                "unexpected package in source-only PHY graph: {}",
                package.name
            )
            .into());
        }
    }
    Ok(artifact)
}

fn final_image_audit(ctx: &Context, runtime: &Path) -> Result<()> {
    // Select the exact binaries just built, regardless of caller target/binary overrides.
    for (package, binary) in [("blobray-esp32s31", "blobray"), ("blobray", "blobray-run")] {
        process::run(
            ctx.cargo()
                .env("CARGO_TARGET_DIR", ctx.root.join("target"))
                .args([
                    "build",
                    "--locked",
                    "--offline",
                    "--profile",
                    "blobray",
                    "-p",
                    package,
                    "--bin",
                    binary,
                ]),
        )?;
    }
    // The launcher owns a separate session or systemd service and needs its
    // full shutdown grace before this outer owner may force termination.
    process::run_with_shutdown_grace(
        ctx.command(ctx.root.join("target/blobray/blobray-run"))
            .env("BLOBRAY_BINARY", ctx.root.join("target/blobray/blobray"))
            .args([
                "advanced",
                "image",
                "audit-targets",
                "--target-spec",
                "verification/vendor/projects/esp32s31/target.toml",
                "--artifact",
            ])
            .arg(runtime)
            .args([
                "--forbid",
                "esp32s31-eco0-radio-api=0x2f800bf0..0x2f8016bc",
                "--forbid",
                "esp32s31-eco0-radio-body=0x2f823c12..0x2f83e6d0",
            ]),
        std::time::Duration::from_secs(20),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreImageStage {
    RepositoryTests,
    BlobrayLibraryTests,
    BlobrayLauncherTests,
    Metadata,
    NetworkDependencies,
    Examples,
    Docs,
}

const PRE_IMAGE_STAGES: &[PreImageStage] = &[
    PreImageStage::RepositoryTests,
    PreImageStage::BlobrayLibraryTests,
    PreImageStage::BlobrayLauncherTests,
    PreImageStage::Metadata,
    PreImageStage::NetworkDependencies,
    PreImageStage::Examples,
    PreImageStage::Docs,
];

fn run_pre_image_stages(mut execute: impl FnMut(PreImageStage) -> Result<()>) -> Result<()> {
    for stage in PRE_IMAGE_STAGES {
        execute(*stage)?;
    }
    Ok(())
}

fn execute_pre_image_stage(ctx: &Context, stage: PreImageStage) -> Result<()> {
    match stage {
        PreImageStage::RepositoryTests => process::run(ctx.cargo().args([
            "test",
            "--locked",
            "--offline",
            "-p",
            "oer-xtask",
            "-p",
            "oer-process",
            "-p",
            "oer-firmware",
        ])),
        PreImageStage::BlobrayLibraryTests => process::run(ctx.cargo().args([
            "test",
            "--locked",
            "--offline",
            "-p",
            "blobray",
            "--lib",
            "launcher::",
        ])),
        PreImageStage::BlobrayLauncherTests => process::run(ctx.cargo().args([
            "test",
            "--locked",
            "--offline",
            "-p",
            "blobray",
            "--test",
            "launcher",
        ])),
        PreImageStage::Metadata => metadata::run(ctx).map(|_| ()),
        PreImageStage::NetworkDependencies => network::run(ctx, true),
        PreImageStage::Examples => examples::run(ctx),
        PreImageStage::Docs => docs::run(ctx, false),
    }
}

pub fn run(ctx: &Context) -> Result<()> {
    run_pre_image_stages(|stage| execute_pre_image_stage(ctx, stage))?;

    process::run(
        ctx.cargo()
            .env("CARGO_TARGET_DIR", ctx.root.join("target"))
            .args([
                "build",
                "--quiet",
                "--locked",
                "--offline",
                "-p",
                "open-esp-radio-hil-runner",
            ]),
    )?;
    let temporary = tempfile::tempdir()?;
    let mut performance =
        FinalImageBuild::spawn(ctx, temporary.path(), FinalImageClass::Performance)?;
    println!("source-only: final performance HIL image build running concurrently");

    process::run(ctx.cargo().args([
        "clippy",
        "--locked",
        "--offline",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
        "-A",
        "clippy::disallowed-methods",
    ]))?;
    production_lints(ctx)?;
    safety::run(ctx)?;
    architecture::run(ctx)?;
    bluetooth::run(ctx)?;
    publication(ctx)?;
    let artifact = phy(ctx)?;

    audit_final_images(
        |class| match class {
            FinalImageClass::Performance => performance.finish(&ctx.root),
            FinalImageClass::Correctness => {
                FinalImageBuild::spawn(ctx, temporary.path(), class)?.finish(&ctx.root)
            }
        },
        |artifact| final_image_audit(ctx, &artifact.runtime_elf),
    )?;
    println!(
        "source-only radio audit passed: rlib={} performance+correctness",
        artifact.display(),
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FinalImageClass {
    Performance,
    Correctness,
}

impl FinalImageClass {
    const ALL: [Self; 2] = [Self::Performance, Self::Correctness];

    const fn id(self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Correctness => "correctness",
        }
    }
}

const FINAL_IMAGE_PROFILE: &str = "psram-code-psram-data-psram-stack";
const FINAL_IMAGE_NETWORK: &str = "upstream-xarxa";

struct FinalImageBuild {
    class: FinalImageClass,
    child: owned::Child,
    log: PathBuf,
    output: PathBuf,
    start: PathBuf,
}

impl FinalImageBuild {
    fn spawn(ctx: &Context, temporary: &Path, class: FinalImageClass) -> Result<Self> {
        let log = temporary.join(format!("{}-image-build.log", class.id()));
        let output = temporary.join(format!("{}-image-build.json", class.id()));
        let start = temporary.join(format!("{}-image-build.start", class.id()));
        fs::write(&start, b"image-build-start")?;
        let child = owned::Child::spawn_with_shutdown_grace(
            ctx.command(ctx.root.join("target/debug/open-esp-radio-hil-runner"))
                .env_remove("ESP_HAL_ROOT")
                .env_remove("EMBASSY_ROOT")
                .env_remove("OPEN_RADIO_XARXA_ROOT")
                .args([
                    "image",
                    "build",
                    class.id(),
                    "--network",
                    FINAL_IMAGE_NETWORK,
                ])
                .stdout(Stdio::from(File::create(&output)?))
                .stderr(Stdio::from(File::create(&log)?)),
            std::time::Duration::from_secs(40),
        )?;
        Ok(Self {
            class,
            child,
            log,
            output,
            start,
        })
    }

    fn finish(&mut self, root: &Path) -> Result<FinalImageArtifact> {
        let status = self.child.wait()?;
        eprint!("{}", fs::read_to_string(&self.log)?);
        if !status.success() {
            return Err(
                format!("final {} HIL image build failed: {status}", self.class.id()).into(),
            );
        }
        validate_final_image_report(&fs::read(&self.output)?, self.class, root, &self.start)
    }
}

#[derive(Debug)]
struct FinalImageArtifact {
    report: serde_json::Value,
    runtime_elf: PathBuf,
}

fn audit_final_images(
    mut build: impl FnMut(FinalImageClass) -> Result<FinalImageArtifact>,
    mut audit: impl FnMut(&FinalImageArtifact) -> Result<()>,
) -> Result<()> {
    let mut runtimes = BTreeSet::new();
    for class in FinalImageClass::ALL {
        let mut artifact = build(class)?;
        if !runtimes.insert(artifact.runtime_elf.clone()) {
            return Err("final image classes reported the same runtime ELF".into());
        }
        audit(&artifact)?;
        artifact.report["final_radio_target_audit"] = serde_json::Value::from("PASS");
        println!("{}", serde_json::to_string(&artifact.report)?);
    }
    Ok(())
}

fn validate_final_image_report(
    report: &[u8],
    class: FinalImageClass,
    root: &Path,
    start: &Path,
) -> Result<FinalImageArtifact> {
    let report: serde_json::Value = serde_json::from_slice(report)?;
    for (field, expected) in [
        ("image_class", class.id()),
        ("target", TARGET),
        ("profile", FINAL_IMAGE_PROFILE),
        ("network", FINAL_IMAGE_NETWORK),
    ] {
        if report.get(field).and_then(serde_json::Value::as_str) != Some(expected) {
            return Err(format!("HIL report {field} must identify {expected}").into());
        }
    }
    if report.get("schema").and_then(serde_json::Value::as_u64) != Some(2)
        || report.get("flashed").and_then(serde_json::Value::as_bool) != Some(false)
    {
        return Err("HIL build report has invalid schema or flashed state".into());
    }
    for field in [
        "stack_frame_audit",
        "move_size_audit",
        "placement_audit",
        "application_audit",
        "autonomous_source_graph",
    ] {
        if report.get(field).and_then(serde_json::Value::as_str) != Some("PASS") {
            return Err(format!("HIL report missing successful {field}").into());
        }
    }
    let base = root.join("target/hil/esp32s31").join(format!(
        "{FINAL_IMAGE_PROFILE}-{}-{FINAL_IMAGE_NETWORK}",
        class.id()
    ));
    let start_time = fs::metadata(start)?.modified()?;
    let required = |field: &str, expected: Option<PathBuf>, fresh: bool| -> Result<PathBuf> {
        let value = report
            .get(field)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("HIL report missing {field}"))?;
        let path = PathBuf::from(value);
        if !path.is_absolute()
            || !path.is_file()
            || !path.starts_with(&base)
            || expected.as_ref().is_some_and(|expected| &path != expected)
        {
            return Err(format!(
                "HIL report {field} must identify its existing class-owned absolute file"
            )
            .into());
        }
        if fresh && fs::metadata(&path)?.modified()? <= start_time {
            return Err(format!("HIL report {field} identifies a stale build artifact").into());
        }
        Ok(path)
    };
    let runtime_elf = required("runtime_elf", None, false)?;
    let bootstrap_elf = required("bootstrap_elf", None, false)?;
    for (field, path, stage) in [
        ("runtime_elf", &runtime_elf, "runtime"),
        ("bootstrap_elf", &bootstrap_elf, "bootstrap"),
    ] {
        if path.parent()
            != Some(
                base.join("cargo")
                    .join(stage)
                    .join(TARGET)
                    .join("release")
                    .as_path(),
            )
        {
            return Err(format!("HIL report {field} has the wrong target output path").into());
        }
    }
    required("runtime_bin", Some(base.join("runtime.bin")), true)?;
    required(
        "runtime_stack_report",
        Some(base.join("runtime-stack.txt")),
        true,
    )?;
    required("placement_report", Some(base.join("placement.txt")), true)?;
    required(
        "bootstrap_stack_report",
        Some(base.join("bootstrap-stack.txt")),
        true,
    )?;
    required(
        "effective_embedded_lock",
        Some(base.join("effective-Cargo.lock")),
        true,
    )?;
    required(
        "effective_bootstrap_lock",
        Some(base.join("bootstrap-Cargo.lock")),
        true,
    )?;
    let application = required(
        "application_image",
        Some(base.join("application.bin")),
        true,
    )?;
    let expected_sha = report
        .get("application_sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or("HIL report missing application_sha256")?;
    let actual_sha = format!("{:x}", Sha256::digest(fs::read(application)?));
    if expected_sha != actual_sha {
        return Err("HIL report application_sha256 does not match the final image".into());
    }
    Ok(FinalImageArtifact {
        report,
        runtime_elf,
    })
}

#[cfg(test)]
#[path = "source_only/tests.rs"]
mod tests;
