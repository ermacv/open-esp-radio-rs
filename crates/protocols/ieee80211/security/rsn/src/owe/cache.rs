//! Bounded OWE PMKSA lifetime. Secret ownership is independent of peer tables.
use super::*;
use oer_ieee80211_mac::ssid::WifiSsid;

/// One OWE/CCMP security domain. A transition BSS with a different SSID or
/// BSSID cannot resume this key, nor can an association with different PMF.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheScope {
    pub addresses: Addresses,
    pub ssid: WifiSsid,
    pub management_protection: bool,
}

/// A cached secret with its scope and an explicit monotonic expiry.
/// No implicit lifetime or cache eviction policy is supplied by the protocol.
pub struct CachedPmk {
    scope: CacheScope,
    key: OwePmk,
    created_at_us: u64,
    expires_at_us: u64,
}
impl CachedPmk {
    pub fn new(
        scope: CacheScope,
        key: OwePmk,
        now_us: u64,
        expires_at_us: u64,
    ) -> Result<Self, Error> {
        scope.addresses.validate()?;
        if key.addresses() != scope.addresses {
            return Err(Error::WrongContext);
        }
        if expires_at_us <= now_us {
            return Err(Error::KeyExpired);
        }
        Ok(Self {
            scope,
            key,
            created_at_us: now_us,
            expires_at_us,
        })
    }
    pub const fn scope(&self) -> CacheScope {
        self.scope
    }
    pub const fn group(&self) -> Group {
        self.key.group()
    }
    pub const fn pmkid(&self) -> [u8; RSN_PMKID_LEN] {
        self.key.pmkid()
    }
    pub const fn expires_at_us(&self) -> u64 {
        self.expires_at_us
    }

    /// Copying a secret is explicit. The association owner must recheck the
    /// lease when accepting the AP's PMKID, rather than keeping a stale offer.
    pub fn duplicate(&self, now_us: u64) -> Result<Self, Error> {
        self.validate_at(now_us)?;
        Ok(Self {
            scope: self.scope,
            key: self.key.duplicate(),
            created_at_us: self.created_at_us,
            expires_at_us: self.expires_at_us,
        })
    }
    pub(super) fn validate_at(&self, now_us: u64) -> Result<(), Error> {
        if now_us < self.created_at_us {
            Err(Error::TimeWentBackwards)
        } else if now_us >= self.expires_at_us {
            Err(Error::KeyExpired)
        } else {
            Ok(())
        }
    }
    pub(super) fn into_key(self) -> OwePmk {
        self.key
    }
}

/// Fixed-capacity cache with no hidden replacement of live entries. The caller
/// can remove a chosen security domain before retrying an insertion. Expired
/// entries and replaced keys are dropped through their zeroizing key owner.
pub struct PmkCache<const N: usize> {
    entries: [Option<CachedPmk>; N],
    now_us: u64,
}
impl<const N: usize> PmkCache<N> {
    pub fn new(now_us: u64) -> Self {
        Self {
            entries: core::array::from_fn(|_| None),
            now_us,
        }
    }
    fn observe(&mut self, now_us: u64) -> Result<(), Error> {
        if now_us < self.now_us {
            return Err(Error::TimeWentBackwards);
        }
        self.now_us = now_us;
        for entry in &mut self.entries {
            if entry
                .as_ref()
                .is_some_and(|value| value.validate_at(now_us).is_err())
            {
                *entry = None;
            }
        }
        Ok(())
    }
    /// On failure the supplied key is dropped; existing live entries remain.
    pub fn insert(&mut self, entry: CachedPmk, now_us: u64) -> Result<(), Error> {
        self.observe(now_us)?;
        entry.validate_at(now_us)?;
        let index = self
            .entries
            .iter()
            .position(|value| {
                value.as_ref().is_some_and(|value| {
                    value.scope == entry.scope && value.group() == entry.group()
                })
            })
            .or_else(|| self.entries.iter().position(Option::is_none))
            .ok_or(Error::CapacityExceeded)?;
        self.entries[index] = Some(entry);
        Ok(())
    }
    pub fn lookup(
        &mut self,
        scope: CacheScope,
        group: Group,
        now_us: u64,
    ) -> Result<Option<&CachedPmk>, Error> {
        self.observe(now_us)?;
        Ok(self
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.scope == scope && entry.group() == group))
    }
    pub fn remove(&mut self, scope: CacheScope) {
        for entry in &mut self.entries {
            if entry.as_ref().is_some_and(|entry| entry.scope == scope) {
                *entry = None;
            }
        }
    }
    pub fn clear(&mut self) {
        for entry in &mut self.entries {
            *entry = None;
        }
    }
}
