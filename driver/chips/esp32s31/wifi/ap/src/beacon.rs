//! Bounded AP beacon storage and executor-time TSF publication.

use open_esp_radio_ieee80211::{
    beacon::{ApBeaconBuildError, WPA2_BEACON_CAPACITY, dtim, stamp, write_wpa2_erp_beacon},
    ssid::WifiSsid,
    tbtt::next_tbtt_delay,
};

pub struct Esp32s31ApBeacon<'storage> {
    storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
    len: usize,
    interval_micros: u32,
    last_publication_tick: u32,
}

impl<'storage> Esp32s31ApBeacon<'storage> {
    pub fn new(
        storage: &'storage mut [u8; WPA2_BEACON_CAPACITY],
        access_point: [u8; 6],
        ssid: &WifiSsid,
        primary_channel: u8,
        beacon_interval_tu: u16,
        dtim_period: u8,
        management_sequence: u16,
    ) -> Result<Self, ApBeaconBuildError> {
        let len = write_wpa2_erp_beacon(
            storage,
            access_point,
            ssid,
            primary_channel,
            beacon_interval_tu,
            dtim_period,
            management_sequence,
        )?;
        Ok(Self {
            storage,
            len,
            interval_micros: u32::from(beacon_interval_tu) * 1_024,
            last_publication_tick: 0,
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
            last_publication_tick: 0,
        }
    }

    /// Stamp one frame immediately before handing its lease to hardware.
    pub fn prepare(
        &mut self,
        executor_timestamp_micros: u64,
        management_sequence: u16,
        group_pending: bool,
        unicast_tim_bitmap: u8,
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
        let (tim_offset, _, _) = dtim(&self.storage[..self.len])?;
        self.storage[tim_offset + 5] = unicast_tim_bitmap;
        self.last_publication_tick = executor_timestamp_micros as u32;
        Some(&mut self.storage[..self.len])
    }

    /// Return the next wrapping tick and a vendor-compatible millisecond wait.
    pub const fn next_delay(&self, now_micros: u32) -> Option<(u32, u32)> {
        next_tbtt_delay(self.last_publication_tick, self.interval_micros, now_micros)
    }

    /// Whether the current beacon interval has elapsed since publication.
    ///
    /// `next_tbtt_delay` deliberately skips an already missed TBTT, matching
    /// the recovered vendor timer calculation. An executor which was occupied
    /// by an uninterruptible TX exchange must test this edge first, otherwise
    /// repeated completions just after TBTT can postpone every beacon by one
    /// more interval.
    pub const fn publication_due(&self, now_micros: u32) -> bool {
        now_micros.wrapping_sub(self.last_publication_tick) >= self.interval_micros
    }

    /// Return whole beacon intervals skipped and lateness beyond the next
    /// expected publication. The first publication establishes the epoch and
    /// therefore has no preceding deadline to miss.
    pub const fn publication_lateness(&self, now_micros: u32) -> (u32, u32) {
        if self.last_publication_tick == 0 || self.interval_micros == 0 {
            return (0, 0);
        }
        let elapsed = now_micros.wrapping_sub(self.last_publication_tick);
        if elapsed <= self.interval_micros {
            return (0, 0);
        }
        (
            elapsed / self.interval_micros - 1,
            elapsed - self.interval_micros,
        )
    }

    pub fn into_storage(self) -> &'storage mut [u8; WPA2_BEACON_CAPACITY] {
        self.storage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_storage_owns_beacon_dtim_and_next_deadline() {
        let mut storage = [0; WPA2_BEACON_CAPACITY];
        let ssid = WifiSsid::new(b"ap").unwrap();
        let mut beacon = Esp32s31ApBeacon::new(&mut storage, [2; 6], &ssid, 6, 100, 2, 3).unwrap();
        let frame = beacon.prepare(102_400, 4, true, 0x02).unwrap();
        let (offset, count, period) = dtim(frame).unwrap();
        assert_eq!((count, period), (0, 2));
        assert_eq!(frame[offset + 4] & 1, 1);
        assert_eq!(frame[offset + 5], 0x02);
        assert_eq!(&frame[22..24], &0x0040_u16.to_le_bytes());
        assert_eq!(beacon.next_delay(102_400), Some((204_800, 103)));
        assert!(!beacon.publication_due(204_799));
        assert!(beacon.publication_due(204_800));
        assert!(beacon.publication_due(204_801));
        assert_eq!(beacon.publication_lateness(204_801), (0, 1));
        assert_eq!(beacon.publication_lateness(307_201), (1, 102_401));
    }
}
