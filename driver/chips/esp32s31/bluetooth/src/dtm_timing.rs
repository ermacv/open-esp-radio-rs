//! Exact Direct Test Mode transmitter timing in the software microsecond domain.
//!
//! Current `r_sym_ble_bWydXXPAXzjyon1EdAMg` and named same-chip
//! `r_ble_lll_dtm_calculate_itvl` use the complete PHY duration helper and
//! byte-identical four-entry header-time table modeled below. Current
//! `r_sym_ble_E4auD6oVVomYiG2Pm144` initializes the environment conversion
//! unit to one, while both current conversion leaves are identities. The
//! resulting scheduler microsecond image equals the selected interval and its
//! conversion remainder is always zero on ESP32-S31.

#![forbid(unsafe_code)]

use crate::{BluetoothDtmPayloadLength, BluetoothDtmPhy};

const TEST_INTERVAL_QUANTUM_MICROS: u32 = 625;
const TEST_PACKET_SPACING_MICROS: u32 = 249;

/// Exact packet duration and selected DTM interval in the reviewed usec domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxTimingMicros {
    packet_duration: u32,
    packet_window_duration: u32,
    interval: u32,
}

/// One DTM interval after the complete ESP32-S31 LLL unit conversion.
///
/// This is a positional software-scheduler image. It neither samples a live
/// clock nor establishes an absolute deadline or hardware-publication right.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxSchedulerTiming {
    interval_micros: u32,
    packet_window_micros: u32,
    remainder_micros: u8,
}

impl BluetoothDtmTxTimingMicros {
    /// Derive the packet duration and interval selected before raw-tick projection.
    ///
    /// The vendor helper rounds only its calculated minimum to the 625-usec
    /// DTM quantum, then takes the larger of that value and the caller's
    /// extended requested interval. A zero request selects the normal minimum.
    pub const fn new(
        length: BluetoothDtmPayloadLength,
        phy: BluetoothDtmPhy,
        requested_interval_micros: u16,
    ) -> Self {
        let (micros_per_octet, header_micros) = match phy {
            BluetoothDtmPhy::Le1M => (8, 80),
            BluetoothDtmPhy::Le2M => (4, 44),
            BluetoothDtmPhy::LeCoded => (64, 720),
            BluetoothDtmPhy::LeCodedS2 => (16, 462),
        };
        let packet_duration =
            calculate_packet_duration(length.hci_image(), micros_per_octet, header_micros);
        let packet_window_duration =
            calculate_packet_duration(u8::MAX, micros_per_octet, header_micros);
        let calculated_interval =
            round_up_to_test_interval(packet_duration + TEST_PACKET_SPACING_MICROS);
        let requested_interval = requested_interval_micros as u32;
        let interval = if requested_interval > calculated_interval {
            requested_interval
        } else {
            calculated_interval
        };

        Self {
            packet_duration,
            packet_window_duration,
            interval,
        }
    }

    /// Return the complete packet duration before test-event spacing.
    pub const fn packet_duration(self) -> u32 {
        self.packet_duration
    }

    /// Return the selected microsecond interval before raw-tick projection.
    pub const fn interval(self) -> u32 {
        self.interval
    }

    /// Convert through the complete reviewed S31 LLL unit/remainder tail.
    ///
    /// The source-owned LLL conversion unit is one microsecond.
    /// Both conversion directions are identity functions, so the vendor's
    /// one-byte remainder is zero and cannot equal the unit. Consequently its
    /// conditional unit increment is unreachable for every representable DTM
    /// interval.
    pub const fn scheduler_timing(self) -> BluetoothDtmTxSchedulerTiming {
        BluetoothDtmTxSchedulerTiming {
            interval_micros: self.interval,
            packet_window_micros: self.packet_window_duration,
            remainder_micros: 0,
        }
    }
}

impl BluetoothDtmTxSchedulerTiming {
    /// Return the complete positional interval image stored by the DTM body.
    pub const fn interval_micros(self) -> u32 {
        self.interval_micros
    }

    /// Return the scheduler window reserved for one TX event.
    ///
    /// The complete DTM scheduler body deliberately uses the maximum
    /// eight-bit packet length here, independently of the requested payload.
    pub const fn packet_window_micros(self) -> u32 {
        self.packet_window_micros
    }

    /// Return the sub-unit microsecond remainder stored next to the interval image.
    pub const fn remainder_micros(self) -> u8 {
        self.remainder_micros
    }
}

const fn calculate_packet_duration(length: u8, micros_per_octet: u32, header_micros: u32) -> u32 {
    (length as u32 + 2) * micros_per_octet + header_micros
}

const fn round_up_to_test_interval(micros: u32) -> u32 {
    micros.div_ceil(TEST_INTERVAL_QUANTUM_MICROS) * TEST_INTERVAL_QUANTUM_MICROS
}

#[cfg(test)]
mod tests {
    use super::BluetoothDtmTxTimingMicros;
    use crate::{BluetoothDtmPayloadLength, BluetoothDtmPhy};

    fn timing(length: u8, phy: BluetoothDtmPhy) -> BluetoothDtmTxTimingMicros {
        BluetoothDtmTxTimingMicros::new(BluetoothDtmPayloadLength::from_hci_image(length), phy, 0)
    }

    #[test]
    fn zero_length_packets_match_all_four_complete_phy_branches() {
        assert_eq!(timing(0, BluetoothDtmPhy::Le1M).packet_duration(), 96);
        assert_eq!(timing(0, BluetoothDtmPhy::Le1M).interval(), 625);
        assert_eq!(timing(0, BluetoothDtmPhy::Le2M).packet_duration(), 52);
        assert_eq!(timing(0, BluetoothDtmPhy::Le2M).interval(), 625);
        assert_eq!(timing(0, BluetoothDtmPhy::LeCoded).packet_duration(), 848);
        assert_eq!(timing(0, BluetoothDtmPhy::LeCoded).interval(), 1_250);
        assert_eq!(timing(0, BluetoothDtmPhy::LeCodedS2).packet_duration(), 494);
        assert_eq!(timing(0, BluetoothDtmPhy::LeCodedS2).interval(), 1_250);
    }

    #[test]
    fn maximum_length_packets_retain_exact_duration_and_rounding() {
        assert_eq!(timing(255, BluetoothDtmPhy::Le1M).packet_duration(), 2_136);
        assert_eq!(timing(255, BluetoothDtmPhy::Le1M).interval(), 2_500);
        assert_eq!(timing(255, BluetoothDtmPhy::Le2M).packet_duration(), 1_072);
        assert_eq!(timing(255, BluetoothDtmPhy::Le2M).interval(), 1_875);
        assert_eq!(
            timing(255, BluetoothDtmPhy::LeCoded).packet_duration(),
            17_168
        );
        assert_eq!(timing(255, BluetoothDtmPhy::LeCoded).interval(), 17_500);
        assert_eq!(
            timing(255, BluetoothDtmPhy::LeCodedS2).packet_duration(),
            4_574
        );
        assert_eq!(timing(255, BluetoothDtmPhy::LeCodedS2).interval(), 5_000);
    }

    #[test]
    fn every_length_and_phy_matches_the_complete_vendor_integer_formula() {
        let phys = [
            BluetoothDtmPhy::Le1M,
            BluetoothDtmPhy::Le2M,
            BluetoothDtmPhy::LeCoded,
            BluetoothDtmPhy::LeCodedS2,
        ];

        for phy in phys {
            for length in 0..=u8::MAX {
                let timing = timing(length, phy);
                let vendor_interval = ((timing.packet_duration() + 0x369) / 0x271) * 0x271;
                assert_eq!(timing.interval(), vendor_interval);
            }
        }
    }

    #[test]
    fn extended_requested_interval_is_a_maximum_without_vendor_rerounding() {
        let below = BluetoothDtmTxTimingMicros::new(
            BluetoothDtmPayloadLength::from_hci_image(255),
            BluetoothDtmPhy::LeCoded,
            17_499,
        );
        assert_eq!(below.interval(), 17_500);

        let above = BluetoothDtmTxTimingMicros::new(
            BluetoothDtmPayloadLength::from_hci_image(255),
            BluetoothDtmPhy::LeCoded,
            17_501,
        );
        assert_eq!(above.interval(), 17_501);

        let maximum = BluetoothDtmTxTimingMicros::new(
            BluetoothDtmPayloadLength::from_hci_image(0),
            BluetoothDtmPhy::Le1M,
            u16::MAX,
        );
        assert_eq!(maximum.interval(), u32::from(u16::MAX));
    }

    #[test]
    fn complete_s31_tick_tail_is_identity_with_zero_remainder() {
        let phys = [
            BluetoothDtmPhy::Le1M,
            BluetoothDtmPhy::Le2M,
            BluetoothDtmPhy::LeCoded,
            BluetoothDtmPhy::LeCodedS2,
        ];
        let requests = [0, 1, 624, 625, 626, 17_501, u16::MAX];

        for phy in phys {
            for length in 0..=u8::MAX {
                for requested_interval in requests {
                    let micros = BluetoothDtmTxTimingMicros::new(
                        BluetoothDtmPayloadLength::from_hci_image(length),
                        phy,
                        requested_interval,
                    );
                    let scheduler = micros.scheduler_timing();

                    assert_eq!(scheduler.interval_micros(), micros.interval());
                    assert_eq!(scheduler.remainder_micros(), 0);
                    assert_ne!(scheduler.remainder_micros(), 1);
                }
            }
        }
    }

    #[test]
    fn scheduler_window_uses_maximum_packet_capacity_for_every_payload_length() {
        let expected = [
            (BluetoothDtmPhy::Le1M, 2_136),
            (BluetoothDtmPhy::Le2M, 1_072),
            (BluetoothDtmPhy::LeCoded, 17_168),
            (BluetoothDtmPhy::LeCodedS2, 4_574),
        ];

        for (phy, expected_window) in expected {
            for length in 0..=u8::MAX {
                let timing = timing(length, phy).scheduler_timing();
                assert_eq!(timing.packet_window_micros(), expected_window);
            }
        }
    }
}
