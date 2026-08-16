//! Bounded AP beacon storage and executor-time TSF publication.

use open_esp_radio_ieee80211::{
    beacon::{ApBeaconBuildError, WPA2_BEACON_CAPACITY, dtim, stamp, write_wpa2_ht_beacon},
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
            None => false,
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
mod tests {
    use super::*;

    #[test]
    fn static_storage_owns_beacon_dtim_and_next_deadline() {
        let mut storage = [0; WPA2_BEACON_CAPACITY];
        let ssid = WifiSsid::new(b"ap").unwrap();
        let mut beacon = Esp32s31ApBeacon::new(
            &mut storage,
            [2; 6],
            &ssid,
            WifiChannel::mhz20(6).unwrap(),
            100,
            2,
            3,
        )
        .unwrap();
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
        assert_eq!(beacon.publication_lateness(204_800), (0, 0));
        assert_eq!(beacon.publication_lateness(204_801), (0, 1));
        assert_eq!(beacon.publication_lateness(307_200), (1, 102_400));
        assert_eq!(beacon.publication_lateness(307_201), (1, 102_401));
    }

    #[test]
    fn late_publication_does_not_move_the_absolute_tbtt_schedule() {
        let mut storage = [0; WPA2_BEACON_CAPACITY];
        let ssid = WifiSsid::new(b"ap").unwrap();
        let mut beacon = Esp32s31ApBeacon::new(
            &mut storage,
            [2; 6],
            &ssid,
            WifiChannel::mhz20(6).unwrap(),
            100,
            2,
            3,
        )
        .unwrap();

        beacon.prepare(102_400, 4, false, 0).unwrap();
        assert_eq!(beacon.next_delay(102_400), Some((204_800, 103)));
        assert_eq!(beacon.publication_lateness(204_900), (0, 100));

        beacon.prepare(204_900, 5, false, 0).unwrap();
        assert_eq!(beacon.next_delay(204_900), Some((307_200, 103)));
    }
}
