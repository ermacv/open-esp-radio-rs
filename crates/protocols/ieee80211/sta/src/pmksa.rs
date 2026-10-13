//! The station's PMKSA cache.
//!
//! An SAE authentication yields a PMK and its PMKID. The station keeps both
//! per access point, so a later association with the same access point and
//! SSID skips SAE: it authenticates by Open System and names the cached
//! PMKID in its RSN element. The cache holds at most ten entries; adding to a
//! full cache replaces the oldest, and an entry does not expire, as the
//! vendor supplicant's `dot11RSNAConfigPMKLifetime` is `INT32_MAX`. Only SAE
//! associations are cached: the vendor caches no PSK association
//! (`wpa_key_mgmt_supports_caching`).
//!
//! SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/wpa_supplicant/src/rsn_supp/pmksa_cache.c`
//! (`pmksa_cache_max_entries`, `dot11RSNAConfigPMKLifetime`,
//! `pmksa_cache_add`) and `src/rsn_supp/wpa.c` (`wpa_set_bss`, which uses an
//! entry only for the same BSSID, SSID and AKM and flushes it otherwise).

use oer_ieee80211_rsn::Pmk;

/// Entries the vendor's PMKSA cache holds.
pub const STA_PMKSA_CAPACITY: usize = 10;
/// The PMKID length.
pub const STA_PMKID_LEN: usize = 16;
const MAX_SSID_LEN: usize = 32;

/// One cached SAE security association.
pub struct StaPmksa {
    bssid: [u8; 6],
    ssid: [u8; MAX_SSID_LEN],
    ssid_len: u8,
    pmk: Pmk,
    pmkid: [u8; STA_PMKID_LEN],
}

impl StaPmksa {
    /// The association an SAE authentication with `bssid` of `ssid`
    /// derived; `None` when the SSID is longer than an SSID can be.
    pub fn new(bssid: [u8; 6], ssid: &[u8], pmk: Pmk, pmkid: [u8; STA_PMKID_LEN]) -> Option<Self> {
        if ssid.len() > MAX_SSID_LEN {
            return None;
        }
        let mut stored = [0; MAX_SSID_LEN];
        for (to, from) in stored.iter_mut().zip(ssid) {
            *to = *from;
        }
        Some(Self {
            bssid,
            ssid: stored,
            ssid_len: ssid.len() as u8,
            pmk,
            pmkid,
        })
    }

    pub const fn pmk(&self) -> &Pmk {
        &self.pmk
    }

    pub const fn pmkid(&self) -> [u8; STA_PMKID_LEN] {
        self.pmkid
    }

    fn has_ssid(&self, ssid: &[u8]) -> bool {
        self.ssid
            .iter()
            .take(usize::from(self.ssid_len))
            .eq(ssid.iter())
    }
}

/// The station's cached SAE security associations, oldest first.
///
/// The composition owns this cache by value and retains it across station
/// connections. Access requires a short exclusive borrow; no shared mutex
/// owner or implicit synchronization sits in the protocol crate.
pub struct StaPmksaCache {
    entries: [Option<StaPmksa>; STA_PMKSA_CAPACITY],
}

impl StaPmksaCache {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; STA_PMKSA_CAPACITY],
        }
    }

    /// The association to resume with `bssid` of `ssid`. An entry of the
    /// same access point under another SSID is flushed, as the vendor
    /// flushes a cached SAE association whose SSID changed.
    pub fn resume(&mut self, bssid: [u8; 6], ssid: &[u8]) -> Option<&StaPmksa> {
        let same_ssid = self
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.bssid == bssid)?
            .has_ssid(ssid);
        if !same_ssid {
            self.remove(bssid);
            return None;
        }
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.bssid == bssid)
    }

    /// Cache the association `entry` names, replacing an entry of the same
    /// access point; a full cache drops its oldest entry first.
    pub fn insert(&mut self, entry: StaPmksa) {
        self.remove(entry.bssid);
        match self.entries.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(entry),
            // Full: the oldest goes, the new entry takes the last place.
            None => {
                self.entries.rotate_left(1);
                let [_, _, _, _, _, _, _, _, _, last] = &mut self.entries;
                *last = Some(entry);
            }
        }
    }

    /// Forget the association with `bssid`, keeping the others in order.
    pub fn remove(&mut self, bssid: [u8; 6]) {
        let mut entries = self.entries.iter_mut();
        while let Some(slot) = entries.next() {
            if slot.as_ref().is_some_and(|entry| entry.bssid == bssid) {
                *slot = None;
                let mut empty = slot;
                for next in entries {
                    *empty = next.take();
                    empty = next;
                }
                break;
            }
        }
    }
}

impl Default for StaPmksaCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
