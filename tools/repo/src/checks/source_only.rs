//! Compose source gates; domain validators remain with their existing owners.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Cursor,
    path::{Path, PathBuf},
    process::Stdio,
    time::Instant,
};

use cargo_metadata::Message;
use sha2::{Digest, Sha256};

use super::{TARGET, architecture, artifacts, common, docs, examples, metadata, network, safety};
use crate::{
    Context, Result, cargo,
    process::{self, owned},
};

const PHY: &str = "crates/hardware/esp32s31/phy/Cargo.toml";
const PUBLICATION: &str = "registers/esp32s31/publication/registers.toml";
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

fn timed_stage<T>(label: &str, work: impl FnOnce() -> Result<T>) -> Result<T> {
    let start = Instant::now();
    let result = work();
    eprintln!(
        "source-only timing: stage={label} total-us={} status={}",
        start.elapsed().as_micros(),
        if result.is_ok() { "PASS" } else { "FAIL" },
    );
    result
}

fn publication(ctx: &Context) -> Result<()> {
    process::run(
        ctx.cargo()
            .args(["registers", "validate", "--manifest", PUBLICATION]),
    )?;
    process::run(ctx.cargo().args([
        "registers",
        "generate",
        "--manifest",
        PUBLICATION,
        "--check",
    ]))
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

/// Independent work of the source checkpoint. Each lane owns one Cargo target
/// directory, so lanes never wait on each other's build lock; stages inside a
/// lane share it and run in order, cheapest first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lane {
    /// Repository gates and lints over the root `target/`.
    Root,
    /// Blobray core tests and the audit host in `tools/blobray/target`.
    Blobray,
    /// Example workspaces with their own target directories.
    Examples,
}

impl Lane {
    const ALL: [Self; 3] = [Self::Root, Self::Blobray, Self::Examples];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Blobray => "blobray",
            Self::Examples => "examples",
        }
    }

    pub fn parse(id: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|lane| lane.id() == id)
            .ok_or_else(|| format!("unknown source-only lane {id}").into())
    }

    const fn stages(self) -> &'static [Stage] {
        match self {
            Self::Root => &[
                Stage::RepositoryTests,
                Stage::RegisterPublicationTests,
                Stage::Metadata,
                Stage::NetworkDependencies,
                Stage::Docs,
                Stage::WorkspaceClippy,
                Stage::Safety,
                Stage::Architecture,
                Stage::Publication,
                Stage::Phy,
            ],
            Self::Blobray => &[Stage::BlobrayCoreTests, Stage::BlobrayAuditHost],
            Self::Examples => &[Stage::Examples],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    RepositoryTests,
    RegisterPublicationTests,
    Metadata,
    NetworkDependencies,
    Docs,
    WorkspaceClippy,
    Safety,
    Architecture,
    Publication,
    Phy,
    BlobrayCoreTests,
    BlobrayAuditHost,
    Examples,
}

impl Stage {
    const fn label(self) -> &'static str {
        match self {
            Self::RepositoryTests => "repository-tests",
            Self::RegisterPublicationTests => "register-publication-tests",
            Self::Metadata => "metadata",
            Self::NetworkDependencies => "network-dependencies",
            Self::Docs => "docs",
            Self::WorkspaceClippy => "workspace-clippy",
            Self::Safety => "safety",
            Self::Architecture => "architecture",
            Self::Publication => "publication",
            Self::Phy => "phy",
            Self::BlobrayCoreTests => "blobray-core-tests",
            Self::BlobrayAuditHost => "blobray-audit-host",
            Self::Examples => "examples",
        }
    }
}

fn run_stages(stages: &[Stage], mut execute: impl FnMut(Stage) -> Result<()>) -> Result<()> {
    for stage in stages {
        execute(*stage)?;
    }
    Ok(())
}

fn execute_stage(ctx: &Context, stage: Stage) -> Result<()> {
    match stage {
        Stage::RepositoryTests => process::run(ctx.cargo().args([
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
        Stage::RegisterPublicationTests => process::run(ctx.cargo().args([
            "test",
            "--locked",
            "--offline",
            "-p",
            "oer-register-tool",
            "-p",
            "open-esp-radio-register-model",
            "-p",
            "open-radio-vendor-review",
            "-p",
            "oer-reviewed-contracts",
        ])),
        Stage::Metadata => metadata::run(ctx).map(|_| ()),
        Stage::NetworkDependencies => network::run(ctx, true),
        Stage::Docs => docs::run_selected(ctx, checkpoint_docs_scope(), false, false, 1),
        Stage::WorkspaceClippy => process::run(ctx.cargo().args([
            "clippy",
            "--locked",
            "--offline",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])),
        Stage::Safety => safety::run(ctx),
        Stage::Architecture => architecture::run(ctx),
        Stage::Publication => publication(ctx),
        Stage::Phy => {
            let artifact = phy(ctx)?;
            println!("source-only PHY rlib audit passed: {}", artifact.display());
            Ok(())
        }
        Stage::BlobrayCoreTests => process::run(crate::blobray::cargo(ctx, "test").args([
            "--locked",
            "--offline",
            "-p",
            "blobray-next",
            "-p",
            "blobray-application",
            "-p",
            "blobray-analysis",
            "-p",
            "blobray-artifacts",
            "-p",
            "blobray-store",
            "-p",
            "blobray-domain",
            "-p",
            "blobray-backend-riscv",
            "-p",
            "blobray-knowledge",
            "-p",
            "blobray-verification",
        ])),
        Stage::BlobrayAuditHost => process::run(crate::blobray::cargo(ctx, "build").args([
            "--locked",
            "--offline",
            "--profile",
            "blobray",
            "-p",
            "blobray-next",
            "--bin",
            "blobray",
        ])),
        Stage::Examples => examples::run(ctx),
    }
}

fn checkpoint_docs_scope() -> docs::Scope {
    docs::Scope::Static
}

/// Run one lane in this process; `run` executes each lane as a child.
pub fn run_lane(ctx: &Context, lane: Lane) -> Result<()> {
    run_stages(lane.stages(), |stage| {
        timed_stage(stage.label(), || execute_stage(ctx, stage))
    })
}

/// Result of polling one concurrently running job.
enum Poll {
    Running,
    Done,
}

/// A concurrently running checkpoint job. Dropping it cancels its process group.
trait Job {
    fn label(&self) -> String;
    fn poll(&mut self) -> Result<Poll>;
    /// Final images produced by a completed job.
    fn take_images(&mut self) -> Vec<FinalImageArtifact> {
        Vec::new()
    }
}

/// Poll every job until all pass. The first failure is returned at once and
/// the remaining jobs are dropped with it, which cancels their processes.
fn supervise(
    mut jobs: Vec<Box<dyn Job + '_>>,
    mut passed: impl FnMut(&str),
) -> Result<Vec<FinalImageArtifact>> {
    let mut images = Vec::new();
    while !jobs.is_empty() {
        process::check_cancelled()?;
        let mut index = 0;
        while index < jobs.len() {
            match jobs[index].poll() {
                Ok(Poll::Running) => index += 1,
                Ok(Poll::Done) => {
                    let mut job = jobs.remove(index);
                    images.extend(job.take_images());
                    passed(&job.label());
                }
                Err(error) => {
                    return Err(format!("{} failed: {error}", jobs[index].label()).into());
                }
            }
        }
        if !jobs.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    Ok(images)
}

struct LaneProcess {
    lane: Lane,
    child: owned::Child,
    log: PathBuf,
    started_at: Instant,
}

impl LaneProcess {
    fn spawn(ctx: &Context, temporary: &Path, lane: Lane) -> Result<Self> {
        let log = temporary.join(format!("{}-lane.log", lane.id()));
        let file = File::create(&log)?;
        let child = owned::Child::spawn_with_shutdown_grace(
            ctx.command(std::env::current_exe()?)
                .arg("--root")
                .arg(&ctx.root)
                .args(["check", "source-only", "--lane", lane.id()])
                .stdin(Stdio::null())
                .stdout(Stdio::from(file.try_clone()?))
                .stderr(Stdio::from(file)),
            std::time::Duration::from_secs(40),
        )?;
        Ok(Self {
            lane,
            child,
            log,
            started_at: Instant::now(),
        })
    }
}

impl Job for LaneProcess {
    fn label(&self) -> String {
        format!("lane {}", self.lane.id())
    }

    fn poll(&mut self) -> Result<Poll> {
        let Some(status) = self.child.try_wait()? else {
            return Ok(Poll::Running);
        };
        eprintln!("==> source-only lane {}", self.lane.id());
        eprint!("{}", fs::read_to_string(&self.log)?);
        eprintln!(
            "source-only timing: stage=lane-{} total-us={} status={}",
            self.lane.id(),
            self.started_at.elapsed().as_micros(),
            if status.success() { "PASS" } else { "FAIL" },
        );
        if !status.success() {
            return Err(format!("exited with {status}").into());
        }
        Ok(Poll::Done)
    }
}

/// One final image class. Each class owns its output and target directories
/// and resolves through a private lockfile copy, so both classes build at once.
struct ImageJob<'a> {
    root: &'a Path,
    build: FinalImageBuild,
    built: Option<FinalImageArtifact>,
}

impl Job for ImageJob<'_> {
    fn label(&self) -> String {
        format!("{} image build", self.build.class.id())
    }

    fn poll(&mut self) -> Result<Poll> {
        if self.build.child.try_wait()?.is_none() {
            return Ok(Poll::Running);
        }
        self.built = Some(self.build.finish(self.root)?);
        Ok(Poll::Done)
    }

    fn take_images(&mut self) -> Vec<FinalImageArtifact> {
        self.built.take().into_iter().collect()
    }
}

pub fn run(ctx: &Context) -> Result<()> {
    let total_start = Instant::now();
    // The image builder must exist before the images start; afterwards the
    // root lane is the only user of `target/`.
    timed_stage("build-hil-runner", || {
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
        )
    })?;
    let temporary = tempfile::tempdir()?;
    let mut jobs: Vec<Box<dyn Job + '_>> = Vec::new();
    for class in FinalImageClass::ALL {
        jobs.push(Box::new(ImageJob {
            root: &ctx.root,
            build: FinalImageBuild::spawn(ctx, temporary.path(), class)?,
            built: None,
        }));
    }
    for lane in Lane::ALL {
        jobs.push(Box::new(LaneProcess::spawn(ctx, temporary.path(), lane)?));
    }
    println!("source-only: final images and lanes root, blobray, examples run concurrently");
    // Both images and the Blobray audit host exist only after every job passed.
    let mut built = supervise(jobs, |label| eprintln!("source-only: {label} passed"))?;
    timed_stage("final-image-audits", || {
        audit_final_images(
            |class| {
                let index = built
                    .iter()
                    .position(|artifact| artifact.report["image_class"] == class.id())
                    .ok_or_else(|| format!("final {} image was not built", class.id()))?;
                Ok(built.swap_remove(index))
            },
            |artifact| {
                let class = artifact.report["image_class"].as_str().unwrap_or("unknown");
                timed_stage(&format!("target-audit-{class}"), || {
                    final_image_audit(ctx, &artifact.runtime_elf)
                })
            },
        )
    })?;
    println!("source-only radio audit passed: performance+correctness");
    eprintln!(
        "source-only timing: stage=total total-us={} status=PASS",
        total_start.elapsed().as_micros()
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
