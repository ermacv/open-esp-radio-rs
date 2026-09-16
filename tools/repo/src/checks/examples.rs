use std::{collections::BTreeSet, time::Instant};

use crate::{Context, Result, process};

use super::{TARGET, common};

mod dma;

pub fn run(ctx: &Context) -> Result<()> {
    run_with_evidence(ctx).map(|_| ())
}

pub struct ExampleEvidence {
    pub(crate) target_configurations: BTreeSet<common::CargoConfiguration>,
}

pub fn run_with_evidence(ctx: &Context) -> Result<ExampleEvidence> {
    let total_start = Instant::now();
    let plan_start = Instant::now();
    let configurations = common::example_configurations(ctx, TARGET)?;
    eprintln!(
        "example timing: stage=plan-metadata total-us={}",
        plan_start.elapsed().as_micros()
    );
    for configuration in &configurations {
        let cargo_start = Instant::now();
        let mut command = ctx.cargo();
        command.arg("check");
        configuration.apply(&mut command);
        process::run(&mut command)?;
        eprintln!(
            "example timing: job=target-check id={} cargo-us={}",
            configuration.id(&ctx.root)?,
            cargo_start.elapsed().as_micros()
        );
    }
    let host_plan_start = Instant::now();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let host = process::capture(ctx.command(rustc).arg("-vV"))?;
    let host = String::from_utf8(host.stdout)?;
    let host = host
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("rustc did not report its host target")?;
    let host_configurations = common::example_host_test_configurations(ctx, host)?;
    eprintln!(
        "example timing: stage=host-plan-metadata total-us={}",
        host_plan_start.elapsed().as_micros()
    );
    for configuration in &host_configurations {
        let cargo_start = Instant::now();
        let mut command = ctx.cargo();
        command.arg("test");
        configuration.apply(&mut command);
        process::run(&mut command)?;
        eprintln!(
            "example timing: job=host-test id={} cargo-us={}",
            configuration.id(&ctx.root)?,
            cargo_start.elapsed().as_micros()
        );
    }
    let dma_start = Instant::now();
    dma::check(ctx)?;
    eprintln!(
        "example timing: stage=dma-check total-us={}",
        dma_start.elapsed().as_micros()
    );
    eprintln!(
        "example compilation: {} target configurations, {} host library tests",
        configurations.len(),
        host_configurations.len()
    );
    eprintln!(
        "example timing: stage=total total-us={}",
        total_start.elapsed().as_micros()
    );
    Ok(ExampleEvidence {
        target_configurations: configurations.into_iter().collect(),
    })
}
