//! Compose source gates; domain validators remain with their existing owners.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Cursor,
    path::{Path, PathBuf},
    process::Stdio,
};

use cargo_metadata::Message;

use super::{TARGET, architecture, artifacts, common, examples, metadata, network, safety};
use crate::{
    Context, Result, cargo,
    process::{self, owned},
};

const PHY: &str = "driver/chips/esp32s31/phy/Cargo.toml";
const INVESTIGATION: &str = "verification/vendor/projects/esp32s31/vendor-project.toml";
const PUBLICATION: &str = "registers/esp32s31/publication/vendor-project.toml";
const PHY_PACKAGES: &[&str] = &[
    "critical-section",
    "open-esp-radio-dma",
    "open-esp-radio-esp32s31-coex",
    "open-esp-radio-esp32s31-hal",
    "open-esp-radio-esp32s31-ieee802154-irq",
    "open-esp-radio-esp32s31-pac",
    "open-esp-radio-esp32s31-pac-raw",
    "open-esp-radio-esp32s31-phy",
    "vcell",
];

fn production_lints(ctx: &Context) -> Result<()> {
    let packages = common::driver_packages(ctx)?;
    let members: Vec<_> = packages.iter().filter(|p| p.workspace_member).collect();
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
    for package in packages.iter().filter(|p| !p.workspace_member) {
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
            && artifact.target.name == "open_esp_radio_esp32s31_phy"
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
        "open-esp-radio-esp32s31-phy",
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

pub fn run(ctx: &Context) -> Result<()> {
    process::run(
        ctx.cargo()
            .args(["test", "--locked", "--offline", "-p", "oer-xtask"]),
    )?;
    process::run(ctx.cargo().args([
        "test",
        "--locked",
        "--offline",
        "-p",
        "blobray",
        "--lib",
        "launcher::",
    ]))?;
    process::run(ctx.cargo().args([
        "test",
        "--locked",
        "--offline",
        "-p",
        "blobray",
        "--test",
        "launcher",
    ]))?;
    metadata::run(ctx)?;
    network::run(ctx, true)?;
    examples::run(ctx)?;

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
    let log = temporary.path().join("image-build.log");
    let output = temporary.path().join("image-build.json");
    let mut image = owned::Child::spawn(
        ctx.command(ctx.root.join("target/debug/open-esp-radio-hil-runner"))
            .env_remove("ESP_HAL_ROOT")
            .args(["image", "build", "performance"])
            .stdout(Stdio::from(File::create(&output)?))
            .stderr(Stdio::from(File::create(&log)?)),
    )?;
    println!("source-only: final HIL image build running concurrently");

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
    publication(ctx)?;
    let artifact = phy(ctx)?;

    let status = image.wait()?;
    eprint!("{}", fs::read_to_string(&log)?);
    if !status.success() {
        return Err(format!("final HIL image build failed: {status}").into());
    }
    let runtime = runtime_artifact(&fs::read(&output)?)?;
    final_image_audit(ctx, &runtime)?;
    println!(
        "source-only radio audit passed: rlib={} runtime={}",
        artifact.display(),
        runtime.display()
    );
    Ok(())
}

fn runtime_artifact(report: &[u8]) -> Result<PathBuf> {
    let report: serde_json::Value = serde_json::from_slice(report)?;
    if report
        .get("image_class")
        .and_then(serde_json::Value::as_str)
        != Some("performance")
    {
        return Err("HIL report must identify the performance image".into());
    }
    let path = report
        .get("runtime_elf")
        .and_then(serde_json::Value::as_str)
        .ok_or("HIL report missing runtime_elf")?;
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.is_file() {
        return Err("HIL report runtime_elf must identify an existing absolute file".into());
    }
    Ok(path)
}

#[cfg(test)]
#[path = "source_only/tests.rs"]
mod tests;
