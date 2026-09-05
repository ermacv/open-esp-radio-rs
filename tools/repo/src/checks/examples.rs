use crate::{Context, Result, process};

use super::TARGET;

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
            .arg(ctx.root.join("examples/esp32s31-station/Cargo.toml"))
            .args(["--no-default-features", "--features", "compat-network"]),
    )
}
