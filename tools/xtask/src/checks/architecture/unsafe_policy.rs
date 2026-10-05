//! Unsafe-code and PAC-access boundaries of production packages.
//!
//! The crate-root attributes are the lint policy that rustc and Clippy
//! enforce; this check keeps them in agreement with the reviewed lists.

use crate::{Result, checks::common::Classified};

/// The shared packages of each policy list; every chip adds its own from
/// its profile's `[packages]`.
///
/// Generated register bindings of the register layouts chips share; they
/// state no crate-root policy.
const GENERATED: &[&str] = &["oer-ieee80211-pac-raw"];
const AUDITED_UNSAFE: &[&str] = &[
    "oer-memory",
    "oer-trace",
    "oer-embassy-net-owned",
    "oer-espressif-ieee802154-engine",
    "oer-espressif-executor-embassy",
    "oer-espressif-interrupt-table-esp-hal",
    "oer-interrupt-table",
];
/// The closed PAC crates of the register layouts chips share.
const CLOSED_PACS: &[&str] = &[
    "oer-ieee80211-pac-raw",
    "oer-ieee80211-pac",
    "oer-ieee802154-pac",
];
/// The HAL is the only production consumer of the closed PAC; drivers and
/// adapters reach hardware through HAL owners.
const PAC_CONSUMERS: &[&str] = &["oer-ieee80211-pac"];

/// The policy lists: the shared packages and every chip's.
#[derive(Debug, Default)]
pub(super) struct Policy {
    pub(super) generated: Vec<String>,
    pub(super) audited_unsafe: Vec<String>,
    pub(super) closed_pacs: Vec<String>,
    pub(super) pac_consumers: Vec<String>,
}

impl Policy {
    /// The shared lists with the chips' of `root`.
    pub(super) fn load(root: &std::path::Path) -> Result<Self> {
        let chips = oer_repo::chips::Chips::at(root)?;
        let mut policy = Self::shared();
        for profile in chips.profiles() {
            let packages = &profile.packages;
            policy.generated.extend(packages.generated.iter().cloned());
            policy
                .audited_unsafe
                .extend(packages.audited_unsafe.iter().cloned());
            policy
                .closed_pacs
                .extend(packages.closed_pac.iter().cloned());
            policy
                .pac_consumers
                .extend(packages.pac_consumers.iter().cloned());
        }
        Ok(policy)
    }

    fn shared() -> Self {
        let owned = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        Self {
            generated: owned(GENERATED),
            audited_unsafe: owned(AUDITED_UNSAFE),
            closed_pacs: owned(CLOSED_PACS),
            pac_consumers: owned(PAC_CONSUMERS),
        }
    }

    /// The crate-root attribute that states each production library's
    /// unsafe policy. Rustc and Clippy enforce it in every build; this check
    /// keeps the reviewed audited list and the source attributes in
    /// agreement.
    fn required_attribute(&self, name: &str) -> Option<&'static str> {
        let listed = |list: &[String]| list.iter().any(|listed| listed == name);
        if listed(&self.generated) {
            None
        } else if listed(&self.audited_unsafe) {
            Some("#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]")
        } else {
            Some("#![forbid(unsafe_code)]")
        }
    }

    /// Whether `name` is a closed PAC.
    pub(super) fn closed_pac(&self, name: &str) -> bool {
        self.closed_pacs.iter().any(|pac| pac == name)
    }
}

pub(super) fn check(
    root: &std::path::Path,
    policy: &Policy,
    packages: &[Classified],
) -> Result<()> {
    for item in packages {
        let name = item.package.name.as_str();
        let Some(library) = &item.package.library_root else {
            return Err(format!("driver package has no library target: {name}").into());
        };
        if item
            .package
            .dependencies
            .iter()
            .any(|d| policy.closed_pac(d.package.as_str()))
            && !policy.pac_consumers.iter().any(|consumer| consumer == name)
        {
            return Err(format!("package crosses closed-PAC ownership boundary: {name}").into());
        }
        if let Some(attribute) = policy.required_attribute(name)
            && !std::fs::read_to_string(root.join(library))?
                .lines()
                .any(|line| line.trim() == attribute)
        {
            return Err(
                format!("{name} must declare `{attribute}` at its crate root {library}").into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> &'static std::path::Path {
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }

    #[test]
    fn audited_packages_deny_and_others_forbid_unsafe_code() {
        let policy = Policy::load(repository()).unwrap();
        for generated in &policy.generated {
            assert_eq!(policy.required_attribute(generated), None);
        }
        for audited in &policy.audited_unsafe {
            assert_eq!(
                policy.required_attribute(audited),
                Some("#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]")
            );
        }
        assert_eq!(
            policy.required_attribute("oer-ieee80211-sta"),
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
        let audited = Policy::load(repository())
            .unwrap()
            .audited_unsafe
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(documented, audited);
    }
}
