//! Protocol-derived HT A-MPDU PPDU duration.
//!
//! This module deliberately does not call its result measured airtime.  The
//! ESP32-S31 ordinary TX completion currently reports no hardware airtime
//! counter, and submitting a descriptor does not prove that contention let
//! the PPDU reach the medium.  The value below is only the mixed-format data
//! PPDU duration implied by the exact published A-MPDU length and PHY vector.
//! It excludes contention/backoff, protection, SIFS and BlockAck airtime.

use crate::tx::{HtChannelWidth, HtGuardInterval, HtRate};

use super::HtAmpduLength;

const HUNDRED_NANOSECONDS_PER_MICROSECOND: u32 = 10;
const HT_MIXED_PREAMBLE_HUNDRED_NANOSECONDS: u32 = 400;
const SERVICE_BITS: u32 = 16;
const BCC_TAIL_BITS_PER_ENCODER: u32 = 6;

/// Why a protocol-derived HT PPDU duration could not be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduPpduDurationError {
    EmptyAggregate,
}

/// Duration of one mixed-format HT A-MPDU PPDU derived from its published
/// vector, represented exactly in 100 ns units.
///
/// The 100 ns unit represents both the 4 us long-GI and 3.6 us short-GI HT
/// symbols without rounding.  This is a model input for later shadow airtime
/// accounting, not evidence that hardware actually occupied the medium for
/// this duration.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ModeledHtAmpduPpduDuration {
    hundred_nanoseconds: u32,
}

impl ModeledHtAmpduPpduDuration {
    /// Derive the mixed-format HT data PPDU duration for one exact A-MPDU.
    ///
    /// The ESP32-S31 is one spatial stream and the supported MCS0..7 set uses
    /// one BCC encoder.  The calculation is therefore:
    ///
    /// `40 us + ceil((16 + 8 * bytes + 6) / NDBPS) * symbol_duration`
    ///
    /// where NDBPS is selected by MCS and channel width and symbol duration is
    /// 4 us for long GI or 3.6 us for short GI.
    pub const fn from_published_ampdu(
        rate: HtRate,
        aggregate: HtAmpduLength,
    ) -> Result<Self, HtAmpduPpduDurationError> {
        if aggregate.bytes == 0 || aggregate.subframes == 0 {
            return Err(HtAmpduPpduDurationError::EmptyAggregate);
        }

        const HT20_NDBPS: [u32; 8] = [26, 52, 78, 104, 156, 208, 234, 260];
        const HT40_NDBPS: [u32; 8] = [54, 108, 162, 216, 324, 432, 486, 540];

        let ndbps = match rate.channel_width {
            HtChannelWidth::Mhz20 => HT20_NDBPS[rate.mcs.index() as usize],
            HtChannelWidth::Mhz40 => HT40_NDBPS[rate.mcs.index() as usize],
        };
        let coded_bits = SERVICE_BITS + aggregate.bytes as u32 * 8 + BCC_TAIL_BITS_PER_ENCODER;
        let symbols = coded_bits.div_ceil(ndbps);
        let symbol_hundred_nanoseconds = match rate.guard_interval {
            HtGuardInterval::Long800Ns => 40,
            HtGuardInterval::Short400Ns => 36,
        };

        Ok(Self {
            hundred_nanoseconds: HT_MIXED_PREAMBLE_HUNDRED_NANOSECONDS
                + symbols * symbol_hundred_nanoseconds,
        })
    }

    pub const fn hundred_nanoseconds(self) -> u32 {
        self.hundred_nanoseconds
    }

    pub const fn nanoseconds(self) -> u64 {
        self.hundred_nanoseconds as u64 * 100
    }

    /// Round upward only at a consumer boundary which requires microseconds.
    pub const fn micros_ceil(self) -> u32 {
        self.hundred_nanoseconds
            .div_ceil(HUNDRED_NANOSECONDS_PER_MICROSECOND)
    }

    /// Sum modeled PPDU durations without silently saturating accounting.
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self
            .hundred_nanoseconds
            .checked_add(other.hundred_nanoseconds)
        {
            Some(hundred_nanoseconds) => Some(Self {
                hundred_nanoseconds,
            }),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::tx::{HtChannelWidth, HtGuardInterval, HtMcs, HtRate};

    use super::*;

    const fn rate(width: HtChannelWidth, guard_interval: HtGuardInterval) -> HtRate {
        HtRate::new(HtMcs::Mcs7, guard_interval, width)
    }

    #[test]
    fn models_exact_ht40_lgi_ba32_data_ppdu() {
        let duration = ModeledHtAmpduPpduDuration::from_published_ampdu(
            rate(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns),
            HtAmpduLength {
                bytes: 48_512,
                subframes: 32,
            },
        )
        .unwrap();

        // ceil((16 + 48_512 * 8 + 6) / 540) = 719 symbols.
        assert_eq!(duration.hundred_nanoseconds(), 29_160);
        assert_eq!(duration.nanoseconds(), 2_916_000);
        assert_eq!(duration.micros_ceil(), 2_916);
    }

    #[test]
    fn retains_short_gi_fraction_until_the_consumer_boundary() {
        let duration = ModeledHtAmpduPpduDuration::from_published_ampdu(
            rate(HtChannelWidth::Mhz40, HtGuardInterval::Short400Ns),
            HtAmpduLength {
                bytes: 48_512,
                subframes: 32,
            },
        )
        .unwrap();

        assert_eq!(duration.hundred_nanoseconds(), 26_284);
        assert_eq!(duration.nanoseconds(), 2_628_400);
        assert_eq!(duration.micros_ceil(), 2_629);
    }

    #[test]
    fn channel_width_selects_data_bits_per_symbol_not_nominal_kbps_rounding() {
        let aggregate = HtAmpduLength {
            bytes: 1_500,
            subframes: 1,
        };
        let ht20 = ModeledHtAmpduPpduDuration::from_published_ampdu(
            rate(HtChannelWidth::Mhz20, HtGuardInterval::Long800Ns),
            aggregate,
        )
        .unwrap();
        let ht40 = ModeledHtAmpduPpduDuration::from_published_ampdu(
            rate(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns),
            aggregate,
        )
        .unwrap();

        assert_eq!(ht20.hundred_nanoseconds(), 2_280);
        assert_eq!(ht40.hundred_nanoseconds(), 1_320);
        assert!(ht40 < ht20);
    }

    #[test]
    fn rejects_absent_packet_or_byte_ownership() {
        let rate = rate(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns);
        assert_eq!(
            ModeledHtAmpduPpduDuration::from_published_ampdu(
                rate,
                HtAmpduLength {
                    bytes: 0,
                    subframes: 1,
                },
            ),
            Err(HtAmpduPpduDurationError::EmptyAggregate)
        );
        assert_eq!(
            ModeledHtAmpduPpduDuration::from_published_ampdu(
                rate,
                HtAmpduLength {
                    bytes: 1,
                    subframes: 0,
                },
            ),
            Err(HtAmpduPpduDurationError::EmptyAggregate)
        );
    }

    #[test]
    fn retry_publications_can_be_accumulated_without_claiming_measurement() {
        let first = ModeledHtAmpduPpduDuration::from_published_ampdu(
            rate(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns),
            HtAmpduLength {
                bytes: 48_512,
                subframes: 32,
            },
        )
        .unwrap();
        let retry = ModeledHtAmpduPpduDuration::from_published_ampdu(
            rate(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns),
            HtAmpduLength {
                bytes: 12_128,
                subframes: 8,
            },
        )
        .unwrap();

        assert_eq!(
            first.checked_add(retry).unwrap().hundred_nanoseconds(),
            36_760
        );
    }
}
