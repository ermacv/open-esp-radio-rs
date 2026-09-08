//! Compile Bluetooth configurations without workspace feature unification.

use std::path::Path;

use crate::{Context, Result, cargo, graph::Graph, process};

use super::TARGET;

/// Reject production Wi-Fi dependencies in a resolved Bluetooth consumer.
pub fn audit(graph: &Graph, manifest: &Path) -> Result<()> {
    let root = graph.root(manifest)?;
    for id in graph.reachable(&root).keys() {
        let name = graph.package(id).name.as_str();
        if name.starts_with("oer-wifi")
            || name.starts_with("oer-ieee80211")
            || name.starts_with("oer-esp32s31-wifi")
            || name == "oer-esp32s31-embassy-wifi"
            || name.starts_with("embassy-net")
        {
            return Err(format!("Bluetooth consumer depends on Wi-Fi package {name}").into());
        }
    }
    Ok(())
}

pub fn run(ctx: &Context) -> Result<()> {
    let manifest = ctx.root.join("crates/oer/Cargo.toml");
    for profile in [
        "bluetooth",
        "esp32s31-bluetooth",
        "embassy-esp32s31-bluetooth",
    ] {
        let flags = [
            "--no-default-features".to_owned(),
            "--features".to_owned(),
            profile.to_owned(),
        ];
        for target in [None, Some(TARGET)] {
            let graph = cargo::isolated_graph(ctx, &manifest, &flags, target)?;
            audit(&graph, &manifest)?;
            let mut command = ctx.cargo();
            command.args(["check", "--locked", "--offline", "-p", "open-esp-radio"]);
            command.args(&flags);
            if let Some(target) = target {
                command.args(["--target", target]);
            }
            process::run(&mut command)?;
        }
    }
    // `test` alone compiles the worker under cfg(test), concealing invalid
    // validation-only imports in the ordinary host library.
    for target in [None, Some(TARGET)] {
        let mut command = ctx.cargo();
        command.args([
            "check",
            "--locked",
            "--offline",
            "-p",
            "oer-esp32s31-bluetooth",
            "--features",
            "validation-probes",
        ]);
        if let Some(target) = target {
            command.args(["--target", target]);
        }
        process::run(&mut command)?;
    }
    Ok(())
}
