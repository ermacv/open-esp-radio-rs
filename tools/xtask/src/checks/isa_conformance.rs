//! Conformance of Blobray's RISC-V executor with the official architectural
//! tests, against the Sail formal model.
//!
//! The pinned inputs are `tools/blobray/cli/tests/riscv_conformance/inputs.toml`.
//! Both are fetched once into `target/isa-conformance/` and verified on every
//! run: the riscv-arch-test checkout must resolve to the pinned commit and the
//! Sail archive must have the pinned SHA-256. The check then runs the ignored
//! `riscv_conformance` test of `blobray-cli` with them and the caller's clang.
use crate::Result;
use oer_process as process;
use oer_process::Checkout;
use std::fs;
use std::path::{Path, PathBuf};

/// The pinned inputs, relative to the repository root.
const INPUTS: &str = "tools/blobray/cli/tests/riscv_conformance/inputs.toml";

#[derive(serde::Deserialize)]
struct Inputs {
    schema: u32,
    #[serde(rename = "arch-test")]
    arch_test: ArchTest,
    sail: Sail,
}

#[derive(serde::Deserialize)]
struct ArchTest {
    repository: String,
    tag: String,
    revision: String,
    suite: Vec<Suite>,
}

#[derive(serde::Deserialize)]
struct Suite {
    name: String,
}

#[derive(serde::Deserialize)]
struct Sail {
    url: String,
    sha256: String,
    binary: String,
}

/// Fetch and verify the pinned inputs, then run the conformance test with
/// the clang `cc`, which needs the riscv32 target and lld.
pub fn run(ctx: &Checkout, cc: &Path) -> Result<()> {
    let inputs: Inputs = toml::from_str(&fs::read_to_string(ctx.root.join(INPUTS))?)?;
    if inputs.schema != 1 {
        return Err(format!("{INPUTS}: unsupported schema {}", inputs.schema).into());
    }
    let directory = ctx.root.join("target/isa-conformance");
    fs::create_dir_all(&directory)?;
    let suite = arch_test(&directory, &inputs.arch_test)?;
    let sail = sail(ctx, &directory, &inputs.sail)?;
    process::run(
        oer_toolchain::workspace::BLOBRAY
            .cargo(&ctx.root, "test")
            .args([
                "--locked",
                "-p",
                "blobray-cli",
                "--test",
                "riscv_conformance",
            ])
            .args(["--", "--ignored", "--nocapture"])
            .env("BLOBRAY_RISCV_ARCH_TEST", &suite)
            .env("BLOBRAY_RISCV_CC", cc)
            .env("BLOBRAY_SAIL", &sail),
    )?;
    Ok(())
}

/// The riscv-arch-test checkout at the pinned commit, fetched without the
/// history and with only the test environment and the pinned suites.
fn arch_test(directory: &Path, pin: &ArchTest) -> Result<PathBuf> {
    let suites: Vec<&str> = pin.suite.iter().map(|s| s.name.as_str()).collect();
    let checkout = directory.join(format!(
        "riscv-arch-test-{}-{}",
        pin.revision,
        suites.join("-")
    ));
    if !checkout.join(".git").is_dir() {
        let partial = checkout.with_extension("partial");
        if partial.exists() {
            fs::remove_dir_all(&partial)?;
        }
        fs::create_dir_all(&partial)?;
        let git = |args: &[&str]| oer_process::git::run(&partial, args);
        git(&["init", "--quiet"])?;
        git(&["remote", "add", "origin", &pin.repository])?;
        let mut sparse = vec!["sparse-checkout".to_owned(), "set".to_owned()];
        sparse.push("riscv-test-suite/env".to_owned());
        sparse.extend(
            pin.suite
                .iter()
                .map(|s| format!("riscv-test-suite/rv32i_m/{}", s.name)),
        );
        git(&sparse.iter().map(String::as_str).collect::<Vec<_>>())?;
        git(&[
            "fetch",
            "--quiet",
            "--depth",
            "1",
            "--filter=blob:none",
            "origin",
            &pin.revision,
        ])?;
        git(&["checkout", "--quiet", "FETCH_HEAD"])?;
        fs::rename(&partial, &checkout)?;
    }
    let head = process::capture(oer_process::git::command(&checkout).args(["rev-parse", "HEAD"]))?;
    let head = String::from_utf8(head.stdout)?;
    if head.trim() != pin.revision {
        return Err(format!(
            "{} is at {}, not riscv-arch-test {} ({})",
            checkout.display(),
            head.trim(),
            pin.tag,
            pin.revision
        )
        .into());
    }
    Ok(checkout)
}

/// The Sail simulator from the pinned release archive.
fn sail(ctx: &Checkout, directory: &Path, pin: &Sail) -> Result<PathBuf> {
    let archive = directory.join(format!("sail-{}.tar.gz", pin.sha256));
    if !archive.is_file() || oer_durable::sha256_file(&archive)? != pin.sha256 {
        let partial = archive.with_extension("partial");
        process::run(
            ctx.command("curl")
                .args([
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--output",
                ])
                .arg(&partial)
                .arg(&pin.url),
        )?;
        let actual = oer_durable::sha256_file(&partial)?;
        if actual != pin.sha256 {
            fs::remove_file(&partial)?;
            return Err(format!(
                "{} has SHA-256 {actual}, not the pinned {}",
                pin.url, pin.sha256
            )
            .into());
        }
        fs::rename(&partial, &archive)?;
    }
    let unpacked = directory.join(format!("sail-{}", pin.sha256));
    let binary = unpacked.join(&pin.binary);
    if !binary.is_file() {
        fs::create_dir_all(&unpacked)?;
        process::run(
            ctx.command("tar")
                .arg("-xzf")
                .arg(&archive)
                .arg("-C")
                .arg(&unpacked),
        )?;
    }
    if !binary.is_file() {
        return Err(format!("the Sail archive has no {}", pin.binary).into());
    }
    Ok(binary)
}
