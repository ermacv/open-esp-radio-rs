use crate::{Context, Result, process};

use super::common::*;

const GENERATED: &str = "oer-esp32s31-pac-raw";
const AUDITED_UNSAFE: &[&str] = &[
    "oer-memory",
    "oer-esp32s31-bluetooth",
    "oer-esp32s31-hal",
    "oer-esp32s31-pac",
    "oer-esp32s31-soc",
    "oer-esp32s31-phy",
    "oer-esp32s31-ieee802154-dma",
    "oer-esp32s31-ieee802154-runtime",
    "oer-esp32s31-wifi-dma",
    "oer-esp32s31-radio-platform-esp-hal",
    "oer-esp32s31-embassy-runtime",
    "oer-esp32s31-bluetooth-integration",
    "oer-esp32s31-embassy-wifi",
];
const PAC_CONSUMERS: &[&str] = &[
    "oer-esp32s31-pac-raw",
    "oer-esp32s31-pac",
    "oer-esp32s31-soc",
    "oer-esp32s31-hal",
    "oer-esp32s31-bluetooth",
    "oer-esp32s31-ieee802154-irq",
    "oer-esp32s31-ieee802154-runtime",
    "oer-esp32s31-ieee802154-esp-hal",
];
const TEST_PACKAGES: &[&str] = &[
    "oer-memory",
    "oer-esp32s31-pac",
    "oer-esp32s31-hal",
    "oer-esp32s31-phy",
    "oer-esp32s31-bluetooth",
    "oer-esp32s31-ieee802154-dma",
    "oer-esp32s31-ieee802154-runtime",
    "oer-esp32s31-wifi-dma",
];

/// The crate-root attribute that states each production library's unsafe
/// policy. Rustc and Clippy enforce it in every build; this check keeps the
/// reviewed audited list and the source attributes in agreement.
fn required_attribute(name: &str) -> Option<&'static str> {
    if name == GENERATED {
        None
    } else if AUDITED_UNSAFE.contains(&name) {
        Some("#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]")
    } else {
        Some("#![forbid(unsafe_code)]")
    }
}

fn library_root(package: &cargo_metadata::Package) -> Option<&std::path::Path> {
    package
        .targets
        .iter()
        .find(|target| {
            target.kind.iter().any(|kind| {
                matches!(
                    kind,
                    cargo_metadata::TargetKind::Lib | cargo_metadata::TargetKind::RLib
                )
            })
        })
        .map(|target| target.src_path.as_std_path())
}

pub fn run(ctx: &Context) -> Result<()> {
    let packages = production_packages(ctx)?;
    for item in &packages {
        let name = item.package.name.as_str();
        let Some(root) = library_root(&item.package) else {
            return Err(format!("driver package has no library target: {name}").into());
        };
        if item
            .package
            .dependencies
            .iter()
            .any(|d| d.name == "oer-esp32s31-pac")
            && !PAC_CONSUMERS.contains(&name)
        {
            return Err(format!("package crosses closed-PAC ownership boundary: {name}").into());
        }
        if let Some(attribute) = required_attribute(name)
            && !std::fs::read_to_string(root)?
                .lines()
                .any(|line| line.trim() == attribute)
        {
            return Err(format!(
                "{name} must declare `{attribute}` at its crate root {}",
                root.display()
            )
            .into());
        }
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
