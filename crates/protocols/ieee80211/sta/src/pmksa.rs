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

use core::cell::RefCell;

use oer_ieee80211_mac::scan::ScanRecord;
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
        stored[..ssid.len()].copy_from_slice(ssid);
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

    fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len)]
    }
}

/// The station's cached SAE security associations, oldest first.
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
        let index = self.position(bssid)?;
        if self.entries[index].as_ref()?.ssid() != ssid {
            self.remove(bssid);
            return None;
        }
        self.entries[index].as_ref()
    }

    /// Cache the association `entry` names, replacing an entry of the same
    /// access point; a full cache drops its oldest entry first.
    pub fn insert(&mut self, entry: StaPmksa) {
        self.remove(entry.bssid);
        if self.entries.iter().all(Option::is_some) {
            self.entries.rotate_left(1);
            self.entries[STA_PMKSA_CAPACITY - 1] = None;
        }
        let slot = self
            .entries
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("a slot is free after dropping the oldest entry");
        *slot = Some(entry);
    }

    /// Forget the association with `bssid`, keeping the others in order.
    pub fn remove(&mut self, bssid: [u8; 6]) {
        let Some(index) = self.position(bssid) else {
            return;
        };
        self.entries[index] = None;
        self.entries[index..].rotate_left(1);
    }

    fn position(&self, bssid: [u8; 6]) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|entry| entry.bssid == bssid))
    }
}

impl Default for StaPmksaCache {
    fn default() -> Self {
        Self::new()
    }
}

/// The station's PMKSA cache, shared by every station epoch of one Wi-Fi
/// owner as the vendor supplicant's `gWpaSm.pmksa` outlives its
/// associations: a stopped and restarted station still resumes a cached SAE
/// association.
pub struct StaSharedPmksa(critical_section::Mutex<RefCell<StaPmksaCache>>);

impl StaSharedPmksa {
    pub const fn new() -> Self {
        Self(critical_section::Mutex::new(RefCell::new(
            StaPmksaCache::new(),
        )))
    }

    /// The PMK and PMKID of the cached association with `access_point`.
    pub fn resume(&self, access_point: &ScanRecord) -> Option<(Pmk, [u8; STA_PMKID_LEN])> {
        critical_section::with(|cs| {
            self.0
                .borrow_ref_mut(cs)
                .resume(access_point.bssid, access_point.ssid_bytes())
                .map(|entry| (entry.pmk().duplicate(), entry.pmkid()))
        })
    }

    /// Cache the association an SAE authentication with `access_point`
    /// derived.
    pub fn insert(&self, access_point: &ScanRecord, pmk: &Pmk, pmkid: [u8; STA_PMKID_LEN]) {
        let Some(entry) = StaPmksa::new(
            access_point.bssid,
            access_point.ssid_bytes(),
            pmk.duplicate(),
            pmkid,
        ) else {
            return;
        };
        critical_section::with(|cs| self.0.borrow_ref_mut(cs).insert(entry));
    }

    /// Forget the association with `bssid`.
    pub fn remove(&self, bssid: [u8; 6]) {
        critical_section::with(|cs| self.0.borrow_ref_mut(cs).remove(bssid));
    }
}

impl Default for StaSharedPmksa {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
