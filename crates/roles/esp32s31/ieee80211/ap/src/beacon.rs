//! Bounded AP beacon storage and executor-time TSF publication.

#[cfg(test)]
use oer_ieee80211_mac::beacon::dtim;
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_ieee80211_mac::{
    beacon::{
        AP_BEACON_CAPACITY, ApBeaconBuildError, ApBeaconProtectionError, TimPartialVirtualBitmap,
        stamp, update_bss_protection, write_ht_beacon, write_tim_partial_virtual_bitmap,
    },
    channel::WifiChannel,
    protection::ApBssProtection,
    security::ApSecurityPolicy,
    ssid::WifiSsid,
    tbtt::next_tbtt,
};
use oer_time::{Duration, Instant};

pub struct ApBeacon<'storage> {
    storage: &'storage mut [u8; AP_BEACON_CAPACITY],
    len: usize,
    interval: Duration,
    /// Absolute TBTT following the most recently published beacon.
    ///
    /// This is a schedule cursor, not the actual publication timestamp.
    /// Complete vendor `wdev.o::wDev_Get_Next_TBTT` advances persistent
    /// `BcnSendTick` by `BcnInterval` and stores every catch-up step before
    /// returning the delay. Keeping the cursor independent of executor jitter
    /// prevents one late publication from moving every later TBTT.
    next_publication: Option<Instant>,
}

impl<'storage> ApBeacon<'storage> {
    pub(crate) fn advertisement(&self) -> &[u8] {
        &self.storage[..self.len]
    }

    /// Replace the template's ERP and HT Operation protection fields.
    pub(crate) fn set_bss_protection(
        &mut self,
        protection: ApBssProtection,
    ) -> Result<(), ApBeaconProtectionError> {
        update_bss_protection(&mut self.storage[..self.len], protection)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "each beacon field is an independent 802.11 input the caller owns"
    )]
    pub fn new(
        storage: &'storage mut [u8; AP_BEACON_CAPACITY],
        access_point: [u8; 6],
        ssid: &WifiSsid,
        channel: WifiChannel,
        beacon_interval_tu: u16,
        dtim_period: u8,
        management_sequence: SequenceNumber,
        security: ApSecurityPolicy,
    ) -> Result<Self, ApBeaconBuildError> {
        let len = write_ht_beacon(
            &crate::profile::ADVERTISEMENT,
            storage,
            access_point,
            ssid,
            channel,
            beacon_interval_tu,
            dtim_period,
            management_sequence,
            security,
            ApBssProtection::default(),
        )?;
        Ok(Self {
            storage,
            len,
            interval: Duration::from_micros(u64::from(beacon_interval_tu) * 1_024),
            next_publication: None,
        })
    }

    pub(crate) const fn from_initialized(
        storage: &'storage mut [u8; AP_BEACON_CAPACITY],
        len: usize,
        beacon_interval_tu: u16,
    ) -> Self {
        Self {
            storage,
            len,
            interval: Duration::from_micros(beacon_interval_tu as u64 * 1_024),
            next_publication: None,
        }
    }

    /// Stamp one frame immediately before handing its lease to hardware.
    pub fn prepare(
        &mut self,
        now: Instant,
        management_sequence: SequenceNumber,
        group_pending: bool,
        unicast_tim_bitmap: TimPartialVirtualBitmap<'_>,
    ) -> Option<&mut [u8]> {
        stamp(
            &mut self.storage[..self.len],
            now.as_micros(),
            group_pending,
        )?;
        self.storage[22..24].copy_from_slice(&management_sequence.sequence_control().to_le_bytes());
        self.len = write_tim_partial_virtual_bitmap(self.storage, self.len, unicast_tim_bitmap)?;
        let schedule_base = self.next_publication.unwrap_or(now);
        self.next_publication = Some(next_tbtt(schedule_base, self.interval, now)?);
        Some(&mut self.storage[..self.len])
    }

    /// The next TBTT, once the first beacon has established the schedule.
    pub const fn next_publication(&self) -> Option<Instant> {
        if self.interval.as_micros() == 0 {
            return None;
        }
        self.next_publication
    }

    /// Whether the current beacon interval has elapsed since publication.
    ///
    /// `next_tbtt` deliberately skips an already missed TBTT, matching
    /// the recovered vendor timer calculation. An executor which was occupied
    /// by an uninterruptible TX exchange must test this edge first, otherwise
    /// repeated completions just after TBTT can postpone every beacon by one
    /// more interval.
    pub fn publication_due(&self, now: Instant) -> bool {
        match self.next_publication {
            Some(next) => now >= next,
            // A newly started AP has no TBTT cursor until its first beacon is
            // handed to hardware.  Treat that uninitialized epoch as due so
            // every composition (standalone or paired VIF) establishes the
            // same absolute schedule before it attempts to wait on it.
            None => true,
        }
    }

    /// Return complete beacon intervals skipped after the next expected
    /// publication and the exact lateness beyond that publication. The first
    /// publication establishes the epoch and therefore has no preceding
    /// deadline to miss.
    ///
    /// These are deliberately separate facts: a publication can be almost a
    /// full interval late without crossing the following interval boundary.
    /// Qualification must bound `lateness` independently rather than
    /// relabeling every nonzero scheduler delay as a skipped interval.
    pub fn publication_lateness(&self, now: Instant) -> (u64, Duration) {
        let Some(next) = self.next_publication else {
            return (0, Duration::ZERO);
        };
        let interval = self.interval.as_micros();
        if interval == 0 {
            return (0, Duration::ZERO);
        }
        let lateness = now.saturating_duration_since(next);
        (lateness.as_micros() / interval, lateness)
    }

    pub fn into_storage(self) -> &'storage mut [u8; AP_BEACON_CAPACITY] {
        self.storage
    }
}

#[cfg(test)]
mod tests;
