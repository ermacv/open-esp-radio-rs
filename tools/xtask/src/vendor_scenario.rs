//! Build the Blobray host and the typed vendor scenarios, then run one scenario.
//!
//! Private inputs stay explicit arguments of the scenario; this task only
//! selects the freshly built `blobray` executable.
use crate::{Context, Result, process};
use std::ffi::OsString;

pub fn run(ctx: &Context, chip: &str, args: &[OsString]) -> Result<std::process::ExitCode> {
    let (package, binary) = match chip {
        "esp32s31" => ("oer-esp32s31-vendor-scenarios", "esp32s31-vendor-scenarios"),
        other => return Err(format!("no vendor scenarios for chip {other}").into()),
    };
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
