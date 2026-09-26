//! Build both final HIL application images and audit them: every class must
//! report fresh class-owned artifacts, and the Blobray target audit must find
//! no call into the vendor radio ROM.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::{Path, PathBuf},
    process::Stdio,
    time::Instant,
};

use sha2::{Digest, Sha256};

use super::TARGET;
use crate::{
    Context, Result,
    process::{self, owned},
};

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
    started_at: Instant,
}

impl FinalImageBuild {
    fn spawn(ctx: &Context, temporary: &Path, class: FinalImageClass) -> Result<Self> {
        let started_at = Instant::now();
        let log = temporary.join(format!("{}-image-build.log", class.id()));
        let output = temporary.join(format!("{}-image-build.json", class.id()));
        let start = temporary.join(format!("{}-image-build.start", class.id()));
        fs::write(&start, b"image-build-start")?;
        let child = owned::Child::spawn_with_shutdown_grace(
            ctx.command(ctx.root.join("target/debug/oer-hil-runner"))
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
            started_at,
        })
    }

    fn finish(&mut self, root: &Path) -> Result<FinalImageArtifact> {
        let status = self.child.wait()?;
        eprint!("{}", fs::read_to_string(&self.log)?);
        eprintln!(
            "source-only timing: stage=image-build-{} total-us={} status={}",
            self.class.id(),
            self.started_at.elapsed().as_micros(),
            if status.success() { "PASS" } else { "FAIL" },
        );
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

/// Audit one final image with the host built by the Blobray lane.
fn final_image_audit(ctx: &Context, runtime: &Path) -> Result<()> {
    process::run_with_shutdown_grace(
        ctx.command(crate::blobray::binary(ctx, "blobray"))
            .args(["audit-targets", "--artifact"])
            .arg(runtime)
            .args([
                "--limit-mode",
                "watchdog",
                "--forbid",
                "esp32s31-eco0-radio-api=0x2f800bf0..0x2f8016bc",
                "--forbid",
                "esp32s31-eco0-radio-body=0x2f823c12..0x2f83e6d0",
            ]),
        std::time::Duration::from_secs(20),
    )
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
    // Builds copy both ELFs out of the shared compile cache into the
    // class-owned output, so each report names a fresh class-owned copy.
    let runtime_elf = required("runtime_elf", Some(base.join("runtime.elf")), true)?;
    required("bootstrap_elf", Some(base.join("bootstrap.elf")), true)?;
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

/// Build the HIL runner and the Blobray audit host, build both image classes
/// concurrently (each owns its output and lockfile copy), then audit them.
pub fn run(ctx: &Context) -> Result<()> {
    process::run(
        ctx.cargo()
            .env("CARGO_TARGET_DIR", ctx.root.join("target"))
            .args([
                "build",
                "--quiet",
                "--locked",
                "--offline",
                "-p",
                "oer-hil-runner",
            ]),
    )?;
    process::run(crate::blobray::cargo(ctx, "build").args([
        "--locked",
        "--offline",
        "--profile",
        "blobray",
        "-p",
        "blobray-next",
        "--bin",
        "blobray",
    ]))?;
    let temporary = tempfile::tempdir()?;
    let mut builds = FinalImageClass::ALL
        .into_iter()
        .map(|class| FinalImageBuild::spawn(ctx, temporary.path(), class))
        .collect::<Result<Vec<_>>>()?;
    audit_final_images(
        |class| {
            let build = builds
                .iter_mut()
                .find(|build| build.class == class)
                .ok_or("final image class was not started")?;
            build.finish(&ctx.root)
        },
        |artifact| final_image_audit(ctx, &artifact.runtime_elf),
    )?;
    println!("final image audit passed: performance+correctness");
    Ok(())
}

#[cfg(test)]
#[path = "images/tests.rs"]
mod tests;
