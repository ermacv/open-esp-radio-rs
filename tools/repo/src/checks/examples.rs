use crate::{Context, Result, process};

use super::{TARGET, common};

mod dma;

pub fn run(ctx: &Context) -> Result<()> {
    let configurations = common::example_configurations(ctx, TARGET)?;
    for configuration in &configurations {
        let mut command = ctx.cargo();
        command.arg("check");
        configuration.apply(&mut command);
        process::run(&mut command)?;
    }
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let host = process::capture(ctx.command(rustc).arg("-vV"))?;
    let host = String::from_utf8(host.stdout)?;
    let host = host
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("rustc did not report its host target")?;
    let host_configurations = common::example_host_test_configurations(ctx, host)?;
    for configuration in &host_configurations {
        let mut command = ctx.cargo();
        command.arg("test");
        configuration.apply(&mut command);
        process::run(&mut command)?;
    }
    dma::check(ctx)?;
    eprintln!(
        "example compilation: {} target configurations, {} host library tests",
        configurations.len(),
        host_configurations.len()
    );
    Ok(())
}
