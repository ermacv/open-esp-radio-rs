//! Who owns a lease: one of the agents that use the stand.
//!
//! Every lease is charged to an owner, so an owner must name one agent the
//! same way every time. The owners are a closed vocabulary, and each checkout
//! registers its owner once (`cargo hil owner set NAME`) in `owners.json` of
//! the arbiter directory; nothing is derived from directory names.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::Arbiter;

/// An agent that uses the stand.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Owner {
    Stand,
    Wifi,
    Phy,
    Bluetooth,
    BluetoothHil,
    Blobray,
    Infra,
    #[serde(rename = "802154")]
    Ieee802154,
    Esp32c5,
    Network,
}

impl Owner {
    pub const ALL: [Self; 10] = [
        Self::Stand,
        Self::Wifi,
        Self::Phy,
        Self::Bluetooth,
        Self::BluetoothHil,
        Self::Blobray,
        Self::Infra,
        Self::Ieee802154,
        Self::Esp32c5,
        Self::Network,
    ];

    /// The owner's name in leases, balances and history.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Stand => "stand",
            Self::Wifi => "wifi",
            Self::Phy => "phy",
            Self::Bluetooth => "bluetooth",
            Self::BluetoothHil => "bluetooth-hil",
            Self::Blobray => "blobray",
            Self::Infra => "infra",
            Self::Ieee802154 => "802154",
            Self::Esp32c5 => "esp32c5",
            Self::Network => "network",
        }
    }

    pub fn parse(name: &str) -> Result<Self, NotAnOwner> {
        Self::ALL
            .into_iter()
            .find(|owner| owner.id() == name)
            .ok_or_else(|| NotAnOwner(name.to_owned()))
    }
}

impl std::fmt::Display for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// A lease named an owner outside [`Owner`].
#[derive(Debug)]
pub struct NotAnOwner(pub String);

impl std::fmt::Display for NotAnOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is not a stand owner; register this checkout's owner once with \
             `cargo hil owner set <{}>`, or pass --owner",
            self.0,
            Owner::ALL.map(Owner::id).join("|")
        )
    }
}

impl std::error::Error for NotAnOwner {}

/// No owner is registered for the checkout a lease was requested from.
#[derive(Debug)]
pub struct NoOwner(pub PathBuf);

impl std::fmt::Display for NoOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no stand owner is registered for {}; register it once with \
             `cargo hil owner set <{}>`",
            self.0.display(),
            Owner::ALL.map(Owner::id).join("|")
        )
    }
}

impl std::error::Error for NoOwner {}

#[derive(Default, Deserialize, Serialize)]
struct Registry {
    schema: u32,
    /// Checkout root → its owner.
    checkouts: BTreeMap<PathBuf, Owner>,
}

impl Arbiter {
    fn owners_path(&self) -> PathBuf {
        self.directory().join("owners.json")
    }

    fn registry(&self) -> crate::Result<Registry> {
        match fs::read(self.owners_path()) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Register `owner` as the owner of the checkout at `checkout`.
    pub fn set_checkout_owner(&self, checkout: &Path, owner: Owner) -> crate::Result<()> {
        let checkout = checkout.canonicalize()?;
        self.locked(|| {
            let mut registry = self.registry()?;
            registry.schema = 1;
            registry.checkouts.insert(checkout.clone(), owner);
            oer_durable::atomic_write(&self.owners_path(), &serde_json::to_vec_pretty(&registry)?)
        })
    }

    /// The registered checkouts that still exist.
    pub fn registered_checkouts(&self) -> crate::Result<Vec<PathBuf>> {
        Ok(self
            .registry()?
            .checkouts
            .into_keys()
            .filter(|checkout| checkout.is_dir())
            .collect())
    }

    /// The owner registered for the checkout containing `directory`, the
    /// innermost one when checkouts nest.
    pub fn checkout_owner(&self, directory: &Path) -> crate::Result<Option<Owner>> {
        let directory = directory.canonicalize()?;
        Ok(self
            .registry()?
            .checkouts
            .into_iter()
            .filter(|(checkout, _)| directory.starts_with(checkout))
            .max_by_key(|(checkout, _)| checkout.components().count())
            .map(|(_, owner)| owner))
    }

    /// Charge `old`'s balance to `new` and name `new` in `old`'s history,
    /// for an owner that was known under another name.
    pub fn merge_owner(&self, old: &str, new: Owner) -> crate::Result<()> {
        self.transaction(|state| {
            if let Some(balance) = state.balances.remove(old) {
                let merged = state.balances.entry(new.id().to_owned()).or_default();
                merged.balance_ms += balance.balance_ms;
                merged.last_active_unix_ms =
                    merged.last_active_unix_ms.max(balance.last_active_unix_ms);
            }
            crate::history::rename_owner(&self.history_path(), old, new.id())
        })
    }

    /// Drop `name`'s balance, for an owner that never was an agent.
    pub fn forget_owner(&self, name: &str) -> crate::Result<bool> {
        self.transaction(|state| Ok(state.balances.remove(name).is_some()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_are_a_closed_vocabulary_of_agents() {
        for owner in Owner::ALL {
            assert_eq!(Owner::parse(owner.id()).unwrap(), owner);
            assert_eq!(
                serde_json::to_value(owner).unwrap(),
                serde_json::Value::from(owner.id())
            );
        }
        let error = Owner::parse("open-esp-radio-rs-wifi")
            .unwrap_err()
            .to_string();
        assert!(error.contains("cargo hil owner set"), "{error}");
    }

    #[test]
    fn a_checkout_names_its_owner_for_every_directory_inside_it() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path().join("arbiter")).unwrap();
        let checkout = directory.path().join("checkout");
        fs::create_dir_all(checkout.join("hil/host")).unwrap();
        assert_eq!(arbiter.checkout_owner(&checkout).unwrap(), None);
        arbiter.set_checkout_owner(&checkout, Owner::Wifi).unwrap();
        assert_eq!(
            arbiter.checkout_owner(&checkout.join("hil/host")).unwrap(),
            Some(Owner::Wifi)
        );
        assert_eq!(arbiter.checkout_owner(directory.path()).unwrap(), None);
    }
}
