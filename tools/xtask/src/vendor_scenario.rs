//! Build the Blobray host and the typed vendor scenarios, then run one scenario.
//!
//! Private inputs stay explicit arguments of the scenario; this task only
//! selects the freshly built `blobray` executable.
use crate::{Context, Result, process};
use std::ffi::OsString;

pub fn run(ctx: &Context, chip: &str, args: &[OsString]) -> Result<std::process::ExitCode> {
    let chip = crate::chips::Chip::new(&ctx.root, chip)?;
    let (package, binary) = chip.scenarios();
    if !ctx
        .root
        .join(chip.verification("scenarios/Cargo.toml"))
        .is_file()
    {
        return Err(format!("no vendor scenarios for chip {}", chip.name()).into());
    }
    let (package, binary) = (package.as_str(), binary.as_str());
    let Some((scenario, rest)) = args.split_first() else {
        return Err("select a scenario, for example `gain`".into());
    };
    process::run(crate::blobray::cargo(ctx, "build").args([
        "--profile",
        "blobray",
        "-p",
        "blobray-next",
        "--bin",
        "blobray",
    ]))?;
    process::run(crate::blobray::cargo(ctx, "build").args([
        "--profile",
        "blobray",
        "-p",
        package,
        "--bin",
        binary,
    ]))?;
    let mut command = ctx.command(crate::blobray::binary(ctx, binary));
    command
        .arg(scenario)
        .arg("--binary")
        .arg(crate::blobray::binary(ctx, "blobray"))
        .args(rest);
    let mut child = oer_process::owned::Child::spawn(&mut command)?;
    Ok(crate::hil::exit_code(child.wait_forwarding_cancellation()?))
}
