//! Root/R1 caches and authorized inter-AP key distribution contracts.
use super::*;

/// Source-side grant. Lifetimes cross APs as durations, never as another
/// device's monotonic timestamps. The authenticated envelope covers this
/// lifetime and the complete key scope, recipient and operation identity.
pub struct KeyGrant {
    pub request: OperationId,
    pub target: MacAddress,
    pub remaining_lifetime_us: u64,
    pub key: PmkR1,
}

pub struct KeyDelivery {
    pub request: OperationId,
    pub target: MacAddress,
    pub expires_at_us: u64,
    pub key: PmkR1,
}

impl KeyDelivery {
    /// Call after authenticating the source and authorizing this delivery.
    /// `age_upper_bound_us` includes every queue/transport delay since granting;
    /// the transport must establish that bound rather than assume zero age.
    pub fn from_authenticated_grant(
        grant: KeyGrant,
        age_upper_bound_us: u64,
        now_us: u64,
    ) -> Result<Self, Error> {
        let remaining = grant
            .remaining_lifetime_us
            .checked_sub(age_upper_bound_us)
            .filter(|value| *value != 0)
            .ok_or(Error::KeyExpired)?;
        let expires_at_us = now_us
            .checked_add(remaining)
            .ok_or(Error::DeadlineOverflow)?;
        Ok(Self {
            request: grant.request,
            target: grant.target,
            key: grant.key,
            expires_at_us,
        })
    }
}

/// Admission from the caller's authenticated backhaul and configured R1KH
/// policy. Neither an untrusted FT frame nor an R1KH ID establishes authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthorizedKeyRequest {
    lookup: KeyLookup,
}
impl AuthorizedKeyRequest {
    pub fn from_authenticated_peer(
        lookup: KeyLookup,
        authorized_target: MacAddress,
        authorized_r1kh: R1khId,
    ) -> Result<Self, Error> {
        if lookup.target != authorized_target || lookup.r1kh != authorized_r1kh {
            return Err(Error::WrongPeer);
        }
        Ok(Self { lookup })
    }
    pub const fn lookup(self) -> KeyLookup {
        self.lookup
    }
}

struct RootEntry {
    key: PmkR0,
    expires: u64,
}
/// Bounded R0KH cache; no live entry is silently evicted to admit another.
pub struct RootKeyHolder<const N: usize> {
    entries: [Option<RootEntry>; N],
    now: u64,
}
impl<const N: usize> RootKeyHolder<N> {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; N],
            now: 0,
        }
    }
    pub fn expire(&mut self, now_us: u64) -> Result<(), Error> {
        if now_us < self.now {
            return Err(Error::TimeWentBackwards);
        }
        self.now = now_us;
        for slot in &mut self.entries {
            if slot.as_ref().is_some_and(|entry| now_us >= entry.expires) {
                *slot = None;
            }
        }
        Ok(())
    }
    pub fn insert(&mut self, key: PmkR0, expires_at_us: u64, now_us: u64) -> Result<(), Error> {
        self.expire(now_us)?;
        if now_us >= expires_at_us {
            return Err(Error::KeyExpired);
        }
        let index = self
            .entries
            .iter()
            .position(|slot| {
                slot.as_ref().is_some_and(|entry| {
                    entry.key.context() == key.context() && entry.key.akm() == key.akm()
                })
            })
            .or_else(|| self.entries.iter().position(Option::is_none))
            .ok_or(Error::CapacityExceeded)?;
        self.entries[index] = Some(RootEntry {
            key,
            expires: expires_at_us,
        });
        Ok(())
    }
    pub fn deliver(
        &mut self,
        request: AuthorizedKeyRequest,
        now_us: u64,
    ) -> Result<KeyGrant, Error> {
        self.expire(now_us)?;
        let lookup = request.lookup;
        // This deadline belongs to the requesting AP's epoch. That AP checks
        // it on delivery; an R0KH cannot compare it with its own local clock.
        let entry = self
            .entries
            .iter()
            .flatten()
            .find(|entry| {
                entry.key.context() == lookup.root
                    && entry.key.akm() == lookup.akm
                    && entry.key.name() == lookup.root_name
            })
            .ok_or(Error::WrongKeyContext)?;
        Ok(KeyGrant {
            request: lookup.id,
            target: lookup.target,
            remaining_lifetime_us: entry.expires - now_us,
            key: entry.key.derive_r1(lookup.r1kh),
        })
    }
    pub fn remove_station(&mut self, station: MacAddress) {
        for slot in &mut self.entries {
            if slot
                .as_ref()
                .is_some_and(|entry| entry.key.context().station == station)
            {
                *slot = None;
            }
        }
    }
}
impl<const N: usize> Default for RootKeyHolder<N> {
    fn default() -> Self {
        Self::new()
    }
}

struct R1Entry {
    key: PmkR1,
    expires: u64,
}
/// R1KH local cache. Imported keys must already be authenticated/authorized.
pub struct R1KeyHolder<const N: usize> {
    entries: [Option<R1Entry>; N],
    now: u64,
}
impl<const N: usize> R1KeyHolder<N> {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; N],
            now: 0,
        }
    }
    pub fn expire(&mut self, now_us: u64) -> Result<(), Error> {
        if now_us < self.now {
            return Err(Error::TimeWentBackwards);
        }
        self.now = now_us;
        for slot in &mut self.entries {
            if slot.as_ref().is_some_and(|entry| now_us >= entry.expires) {
                *slot = None;
            }
        }
        Ok(())
    }
    pub fn insert_authenticated(
        &mut self,
        key: PmkR1,
        expires_at_us: u64,
        now_us: u64,
    ) -> Result<(), Error> {
        self.expire(now_us)?;
        if now_us >= expires_at_us {
            return Err(Error::KeyExpired);
        }
        let index = self
            .entries
            .iter()
            .position(|slot| {
                slot.as_ref().is_some_and(|entry| {
                    entry.key.root_context() == key.root_context()
                        && entry.key.r1kh() == key.r1kh()
                        && entry.key.akm() == key.akm()
                })
            })
            .or_else(|| self.entries.iter().position(Option::is_none))
            .ok_or(Error::CapacityExceeded)?;
        self.entries[index] = Some(R1Entry {
            key,
            expires: expires_at_us,
        });
        Ok(())
    }
    pub fn lookup(&mut self, request: KeyLookup, now_us: u64) -> Result<KeyDelivery, Error> {
        self.expire(now_us)?;
        if now_us >= request.deadline_us {
            return Err(Error::ExpiredOperation);
        }
        let entry = self
            .entries
            .iter()
            .flatten()
            .find(|entry| {
                entry.key.root_context() == request.root
                    && entry.key.root_name() == request.root_name
                    && entry.key.r1kh() == request.r1kh
                    && entry.key.akm() == request.akm
            })
            .ok_or(Error::WrongKeyContext)?;
        Ok(KeyDelivery {
            request: request.id,
            target: request.target,
            expires_at_us: entry.expires,
            key: entry.key.duplicate(),
        })
    }
    pub fn remove_station(&mut self, station: MacAddress) {
        for slot in &mut self.entries {
            if slot
                .as_ref()
                .is_some_and(|entry| entry.key.root_context().station == station)
            {
                *slot = None;
            }
        }
    }
}
impl<const N: usize> Default for R1KeyHolder<N> {
    fn default() -> Self {
        Self::new()
    }
}
