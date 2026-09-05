use crate::{Context, Result, process};

use super::{TARGET, common::*};

const GENERATED: &str = "open-esp-radio-esp32s31-pac-raw";
const AUDITED_UNSAFE: &[&str] = &[
    "open-esp-radio-dma",
    "open-esp-radio-esp32s31-bluetooth",
    "open-esp-radio-esp32s31-hal",
    "open-esp-radio-esp32s31-pac",
    "open-esp-radio-esp32s31-platform-pac",
    "open-esp-radio-esp32s31-phy",
    "open-esp-radio-esp32s31-ieee802154-dma",
    "open-esp-radio-esp32s31-ieee802154-runtime",
    "open-esp-radio-esp32s31-wifi-dma",
    "open-esp-radio-esp32s31-radio-platform-esp-hal",
    "open-esp-radio-esp32s31-embassy-runtime",
    "open-esp-radio-esp32s31-bluetooth-integration",
    "open-esp-radio-esp32s31-embassy-wifi",
];
const PAC_CONSUMERS: &[&str] = &[
    "open-esp-radio-esp32s31-pac-raw",
    "open-esp-radio-esp32s31-pac",
    "open-esp-radio-esp32s31-platform-pac",
    "open-esp-radio-esp32s31-hal",
    "open-esp-radio-esp32s31-bluetooth",
    "open-esp-radio-esp32s31-ieee802154-irq",
    "open-esp-radio-esp32s31-ieee802154-runtime",
    "open-esp-radio-esp32s31-ieee802154-esp-hal",
];
const TEST_PACKAGES: &[&str] = &[
    "open-esp-radio-dma",
    "open-esp-radio-esp32s31-pac",
    "open-esp-radio-esp32s31-hal",
    "open-esp-radio-esp32s31-phy",
    "open-esp-radio-esp32s31-bluetooth",
    "open-esp-radio-esp32s31-ieee802154-dma",
    "open-esp-radio-esp32s31-ieee802154-runtime",
    "open-esp-radio-esp32s31-wifi-dma",
];

#[derive(Clone, Copy)]
enum Policy {
    Generated,
    Audited,
    Safe,
}

impl Policy {
    fn for_package(name: &str) -> Self {
        if name == GENERATED {
            Self::Generated
        } else if AUDITED_UNSAFE.contains(&name) {
            Self::Audited
        } else {
            Self::Safe
        }
    }

    fn command(self, ctx: &Context) -> std::process::Command {
        let mut command = ctx.cargo();
        command.args([
            if matches!(self, Self::Generated) {
                "check"
            } else {
                "clippy"
            },
            "--quiet",
            "--locked",
            "--offline",
            "--target",
            TARGET,
            "--lib",
        ]);
        command
    }

    fn finish(self, command: &mut std::process::Command) -> Result<()> {
        match self {
            Self::Generated => (),
            Self::Audited => {
                command.args([
                    "--no-deps",
                    "--",
                    "-D",
                    "unsafe-code",
                    "-D",
                    "unsafe-op-in-unsafe-fn",
                ]);
            }
            Self::Safe => {
                command.args(["--no-deps", "--", "-F", "unsafe-code"]);
            }
        }
        process::run(command)
    }
}

pub fn run(ctx: &Context) -> Result<()> {
    let packages = driver_packages(ctx)?;
    let mut groups: [Vec<String>; 3] = Default::default();
    for item in &packages {
        let name = item.package.name.as_str();
        if !item.package.targets.iter().any(|target| {
            target.kind.iter().any(|kind| {
                matches!(
                    kind,
                    cargo_metadata::TargetKind::Lib | cargo_metadata::TargetKind::RLib
                )
            })
        }) {
            return Err(format!("driver package has no library target: {name}").into());
        }
        if item
            .package
            .dependencies
            .iter()
            .any(|d| d.name == "open-esp-radio-esp32s31-pac")
            && !PAC_CONSUMERS.contains(&name)
        {
            return Err(format!("package crosses closed-PAC ownership boundary: {name}").into());
        }
        let policy = Policy::for_package(name);
        if item.workspace_member {
            groups[match policy {
                Policy::Generated => 0,
                Policy::Audited => 1,
                Policy::Safe => 2,
            }]
            .push(name.to_owned());
        } else {
            for profile in maximal_profiles(&item.package)? {
                let mut command = policy.command(ctx);
                command
                    .arg("--manifest-path")
                    .arg(&item.manifest)
                    .args(["--package", name])
                    .args(profile);
                policy.finish(&mut command)?;
            }
        }
    }
    for (policy, names) in [Policy::Generated, Policy::Audited, Policy::Safe]
        .into_iter()
        .zip(groups)
    {
        // An empty selection must never fall back to linting an implicit workspace.
        if names.is_empty() {
            continue;
        }
        let mut command = policy.command(ctx);
        command.arg("--all-features");
        for name in names {
            command.args(["--package", &name]);
        }
        policy.finish(&mut command)?;
    }
    let mut tests = ctx.cargo();
    tests.args(["test", "--quiet", "--locked", "--offline"]);
    for name in TEST_PACKAGES {
        tests.args(["--package", name]);
    }
    process::run(&mut tests)?;
    eprintln!(
        "driver safety audit passed ({} production packages)",
        packages.len()
    );
    Ok(())
}
