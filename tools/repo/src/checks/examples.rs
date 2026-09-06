use crate::{Context, Result, process};

use super::TARGET;

mod dma;

pub fn run(ctx: &Context) -> Result<()> {
    for name in [
        "esp32s31-station",
        "esp32s31-access-point",
        "esp32s31-monitor",
        "esp32s31-bluetooth-controller",
    ] {
        process::run(
            ctx.cargo()
                .args([
                    "check",
                    "--locked",
                    "--offline",
                    "--release",
                    "--target",
                    TARGET,
                    "--manifest-path",
                ])
                .arg(ctx.root.join("examples").join(name).join("Cargo.toml")),
        )?;
    }
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let host = process::capture(ctx.command(rustc).arg("-vV"))?;
    let host = String::from_utf8(host.stdout)?;
    let host = host
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("rustc did not report its host target")?;
    process::run(
        ctx.cargo()
            .args([
                "test",
                "--locked",
                "--offline",
                "--lib",
                "--target",
                host,
                "--manifest-path",
            ])
            .arg(ctx.root.join("examples/esp32s31-access-point/Cargo.toml")),
    )?;
    dma::check(ctx)?;
    for (example, feature) in [
        ("station", "compat-network"),
        ("station", "upstream-network"),
        ("access-point", "upstream-network"),
    ] {
        process::run(
            ctx.cargo()
                .args([
                    "check",
                    "--locked",
                    "--offline",
                    "--release",
                    "--target",
                    TARGET,
                    "--manifest-path",
                ])
                .arg(
                    ctx.root
                        .join(format!("examples/esp32s31-{example}/Cargo.toml")),
                )
                .args(["--no-default-features", "--features", feature]),
        )?;
    }
    Ok(())
}
