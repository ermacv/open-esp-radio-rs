//! Bounded AP beacon storage and executor-time TSF publication.

#[cfg(test)]
use open_esp_radio_ieee80211::beacon::dtim;
use open_esp_radio_ieee80211::{
    beacon::{
        ApBeaconBuildError, TimPartialVirtualBitmap, WPA2_BEACON_CAPACITY, stamp,
        write_tim_partial_virtual_bitmap, write_wpa2_ht_beacon,
    },
    channel::WifiChannel,
    ssid::WifiSsid,
    tbtt::next_tbtt_delay,
};

pub struct Esp32s31ApBeacon<'storage> {
    storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
    len: usize,
    interval_micros: u32,
    /// Absolute wrapping TBTT following the most recently published beacon.
    ///
    /// This is a schedule cursor, not the actual publication timestamp.
    /// Complete vendor `wdev.o::wDev_Get_Next_TBTT` advances persistent
    /// `BcnSendTick` by `BcnInterval` and stores every catch-up step before
    /// returning the delay. Keeping the cursor independent of executor jitter
    /// prevents one late publication from moving every later TBTT.
    next_publication_tick: Option<u32>,
}

impl<'storage> Esp32s31ApBeacon<'storage> {
    pub fn new(
        storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
        access_point: [u8; 6],
        ssid: &WifiSsid,
        channel: WifiChannel,
        beacon_interval_tu: u16,
        dtim_period: u8,
        management_sequence: u16,
    ) -> Result<Self, ApBeaconBuildError> {
        let len = write_wpa2_ht_beacon(
            crate::profile::HT_CAPABILITIES,
            storage,
            access_point,
            ssid,
            channel,
            beacon_interval_tu,
            dtim_period,
            management_sequence,
        )?;
        Ok(Self {
            storage,
            len,
            interval_micros: u32::from(beacon_interval_tu) * 1_024,
            next_publication_tick: None,
        })
    }

    pub(crate) const fn from_initialized(
        storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
        len: usize,
        beacon_interval_tu: u16,
    ) -> Self {
        Self {
            storage,
            len,
            interval_micros: beacon_interval_tu as u32 * 1_024,
            next_publication_tick: None,
        }
    }

    /// Stamp one frame immediately before handing its lease to hardware.
    pub fn prepare(
        &mut self,
        executor_timestamp_micros: u64,
        management_sequence: u16,
        group_pending: bool,
        unicast_tim_bitmap: TimPartialVirtualBitmap<'_>,
    ) -> Option<&mut [u8]> {
        if management_sequence > 0x0fff {
            return None;
        }
        stamp(
            &mut self.storage[..self.len],
            executor_timestamp_micros,
            group_pending,
        )?;
        self.storage[22..24].copy_from_slice(&(management_sequence << 4).to_le_bytes());
        self.len = write_tim_partial_virtual_bitmap(self.storage, self.len, unicast_tim_bitmap)?;
        let now = executor_timestamp_micros as u32;
        let schedule_base = self.next_publication_tick.unwrap_or(now);
        let next = next_tbtt_delay(schedule_base, self.interval_micros, now)?.0;
        self.next_publication_tick = Some(next);
        Some(&mut self.storage[..self.len])
    }

    /// Return the next wrapping tick and a vendor-compatible millisecond wait.
    pub const fn next_delay(&self, now_micros: u32) -> Option<(u32, u32)> {
        if self.interval_micros == 0 {
            return None;
        }
        let next = match self.next_publication_tick {
            Some(next) => next,
            None => return None,
        };
        let remaining = next.wrapping_sub(now_micros);
        Some((next, remaining / 1_000 + 1))
    }

    /// Whether the current beacon interval has elapsed since publication.
    ///
    /// `next_tbtt_delay` deliberately skips an already missed TBTT, matching
    /// the recovered vendor timer calculation. An executor which was occupied
    /// by an uninterruptible TX exchange must test this edge first, otherwise
    /// repeated completions just after TBTT can postpone every beacon by one
    /// more interval.
    pub const fn publication_due(&self, now_micros: u32) -> bool {
        match self.next_publication_tick {
            Some(next) => now_micros.wrapping_sub(next) < 0x8000_0000,
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
    pub const fn publication_lateness(&self, now_micros: u32) -> (u32, u32) {
        let Some(next) = self.next_publication_tick else {
            return (0, 0);
        };
        if self.interval_micros == 0 || !self.publication_due(now_micros) {
            return (0, 0);
        }
        let lateness = now_micros.wrapping_sub(next);
        (lateness / self.interval_micros, lateness)
    }

    pub fn into_storage(self) -> &'storage mut [u8; WPA2_BEACON_CAPACITY] {
        self.storage
    }
}

#[cfg(test)]
mod tests;
