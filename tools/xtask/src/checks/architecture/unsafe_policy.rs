//! Unsafe-code and PAC-access boundaries of production packages.
//!
//! The crate-root attributes are the lint policy that rustc and Clippy
//! enforce; this check keeps them in agreement with the reviewed lists.

use crate::{Result, checks::common::ProductionPackage};

/// Generated register bindings of each chip and of the register layouts
/// chips share; they state no crate-root policy.
const GENERATED: &[&str] = &[
    "oer-esp32s31-pac-raw",
    "oer-esp32c5-pac-raw",
    "oer-ieee80211-pac-raw",
];
const AUDITED_UNSAFE: &[&str] = &[
    "oer-memory",
    "oer-trace",
    "oer-esp32s31-bluetooth",
    "oer-esp32s31-hal",
    "oer-esp32s31-pac",
    "oer-esp32s31-soc-esp-hal",
    "oer-esp32s31-phy",
    "oer-ieee802154-engine",
    "oer-esp32s31-ieee80211-dma",
    "oer-esp32s31-radio-esp-hal",
    "oer-esp32s31-executor-embassy",
    "oer-esp32s31-bluetooth-system",
    "oer-esp32s31-ieee80211-system",
    "oer-esp32s31-ieee802154-system",
    "oer-esp32c5-pac",
];
/// The closed radio PAC of each chip and the closed PAC crates of the register
/// layouts chips share.
pub(super) const CLOSED_PACS: &[&str] = &[
    "oer-esp32s31-pac",
    "oer-esp32c5-pac",
    "oer-ieee80211-pac-raw",
    "oer-ieee80211-pac",
];
/// The HAL is the only production consumer of the closed PAC; drivers and
/// adapters reach hardware through HAL owners.
const PAC_CONSUMERS: &[&str] = &[
    "oer-esp32s31-pac-raw",
    "oer-esp32s31-pac",
    "oer-esp32s31-hal",
    "oer-esp32c5-pac-raw",
    "oer-esp32c5-pac",
    "oer-esp32c5-hal",
    "oer-ieee80211-pac",
];
/// The crate-root attribute that states each production library's unsafe
/// policy. Rustc and Clippy enforce it in every build; this check keeps the
/// reviewed audited list and the source attributes in agreement.
fn required_attribute(name: &str) -> Option<&'static str> {
    if GENERATED.contains(&name) {
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

pub(super) fn check(packages: &[ProductionPackage]) -> Result<()> {
    for item in packages {
        let name = item.package.name.as_str();
        let Some(root) = library_root(&item.package) else {
            return Err(format!("driver package has no library target: {name}").into());
        };
        if item
            .package
            .dependencies
            .iter()
            .any(|d| CLOSED_PACS.contains(&d.name.as_str()))
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audited_packages_deny_and_others_forbid_unsafe_code() {
        for generated in GENERATED {
            assert_eq!(required_attribute(generated), None);
        }
        assert_eq!(
            required_attribute("oer-esp32s31-hal"),
            Some("#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]")
        );
        assert_eq!(
            required_attribute("oer-ieee80211-sta"),
            Some("#![forbid(unsafe_code)]")
        );
    }

    #[test]
    fn the_unsafe_policy_document_lists_exactly_the_audited_packages() {
        let document = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/UNSAFE.md"),
        )
        .unwrap();
        let table = document
            .split("| Package suffix | Source path |")
            .nth(1)
            .expect("the audited package table");
        let documented = table
            .lines()
            .skip(2)
            .take_while(|line| line.starts_with('|'))
            .filter_map(|line| line.split('`').nth(1))
            .map(|suffix| format!("oer-{suffix}"))
            .collect::<std::collections::BTreeSet<_>>();
        let audited = AUDITED_UNSAFE
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(documented, audited);
    }
}
