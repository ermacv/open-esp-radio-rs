//! The Espressif estimate of the HE SU APEP length that fits below a TXOP
//! duration RTS threshold.

use oer_ieee80211_mac::phy::{HeGiLtf, HeRate};
use oer_ieee80211_upper_mac::{HePacketPadding, HeTxopDurationRtsThreshold, HeTxopRtsBudget};

/// The nominal packet padding code the vendor stores from the peer's HE
/// Capabilities.
///
/// SOURCE(esp32s31): complete `libnet80211.a[ieee80211_he.o]::ieee80211_parse_hecap`
/// stores this code at node offset `0x354`; complete
/// `libpp.a[if_hwctrl.o]::ic_set_he_rts_threshold_bytes_tab` subtracts 8 µs
/// for code one, 16 µs for code two and nothing for any other code.
pub const fn packet_padding_from_code(code: u8) -> HePacketPadding {
    match code {
        1 => HePacketPadding::Us8,
        2 => HePacketPadding::Us16,
        _ => HePacketPadding::None,
    }
}

/// The largest HE SU APEP length whose TXOP stays below the threshold.
///
/// SOURCE(esp32s31): complete `libpp.a[if_hwctrl.o]::ic_set_he_rts_threshold_bytes_tab`
/// and its `.data` tables in `libpp.a[hal_mac_ctl.o]` (`he_preamble_su`,
/// `he_time_per_sym`, `he_data_bits_per_sym`), plus complete
/// `libpp.a[pp_he.o]::get_estimated_batime`. The vendor evaluates, in single
/// precision, `threshold * 32 - 20 - preamble - 16 - padding`, subtracts the
/// BlockAck estimate, divides by the symbol duration, floors, multiplies by
/// the RU242 data bits per symbol, subtracts the 22 SERVICE/tail bits and
/// shifts right by three. DCM halves the data bits for MCS 0, 1, 3 and 4.
/// The halfword table stores the arithmetic result modulo 2^16; a budget no
/// larger than the BlockAck estimate stores zero. Complete
/// `ic_get_he_rts_threshold_bytes` selects row one for both 0.8 µs
/// guard-interval configurations.
///
/// The vendor tables hold HE20 MCS 0-9; a rate beyond them gets a budget of
/// zero, so the PPDU is protected.
pub fn maximum_unprotected_apep_bytes(
    threshold: HeTxopDurationRtsThreshold,
    padding: HePacketPadding,
    rate: HeRate,
) -> u16 {
    const PREAMBLE_US: [f32; 3] = [23.2, 24.0, 32.0];
    const SYMBOL_US: [f32; 3] = [13.6, 14.4, 16.0];
    const DATA_BITS_PER_SYMBOL_RU242: [i32; 10] =
        [117, 234, 351, 468, 702, 936, 1_053, 1_170, 1_404, 1_560];
    let row = match rate.gi_ltf() {
        HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns => 0,
        HeGiLtf::Ltf2xGi1600Ns => 1,
        HeGiLtf::Ltf4xGi3200Ns => 2,
    };
    let mcs = rate.mcs().index();
    let Some(&data_bits) = DATA_BITS_PER_SYMBOL_RU242.get(usize::from(mcs)) else {
        return 0;
    };
    let block_ack_us = match mcs {
        0 => 68,
        1 | 2 => 44,
        _ => 32,
    } as f32;
    let budget = (i32::from(threshold.units_32_us()) * 32 - 20) as f32
        - PREAMBLE_US[row]
        - 16.0
        - f32::from(padding.micros());
    if block_ack_us >= budget {
        return 0;
    }
    // The quotient is positive here, so truncation equals `floor`.
    let symbols = ((budget - block_ack_us) / SYMBOL_US[row]) as i32;
    let mut bits = data_bits;
    if rate.dcm() {
        bits /= 2;
    }
    ((bits * symbols - 22) >> 3) as u16
}

/// [`maximum_unprotected_apep_bytes`] as the portable protection policy's
/// budget.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EspressifHeTxopRtsBudget;

impl HeTxopRtsBudget for EspressifHeTxopRtsBudget {
    fn max_unprotected_apep_bytes(
        &self,
        threshold: HeTxopDurationRtsThreshold,
        padding: HePacketPadding,
        rate: HeRate,
    ) -> u16 {
        maximum_unprotected_apep_bytes(threshold, padding, rate)
    }
}

#[cfg(test)]
mod tests {
    use oer_ieee80211_mac::phy::{FecCoding, HeMcs, PpduBandwidth, SpatialStreams};

    use super::*;

    fn he(mcs: u8) -> HeRate {
        HeRate::new(
            HeMcs::new(mcs).unwrap(),
            SpatialStreams::ONE,
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi800Ns,
            FecCoding::Bcc,
            false,
        )
        .unwrap()
    }

    #[test]
    fn the_budget_matches_the_vendor_byte_table() {
        let threshold = HeTxopDurationRtsThreshold::new(64).unwrap();
        // 64 * 32 - 20 - 23.2 - 16 = 1988.8 µs; minus the 32 µs BlockAck
        // estimate is 143 whole 13.6 µs symbols of 1170 bits at MCS 7.
        assert_eq!(
            maximum_unprotected_apep_bytes(threshold, HePacketPadding::None, he(7)),
            20_911
        );
        assert!(maximum_unprotected_apep_bytes(threshold, HePacketPadding::Us16, he(7)) < 20_911);
        let short = HeTxopDurationRtsThreshold::new(2).unwrap();
        assert_eq!(
            EspressifHeTxopRtsBudget.max_unprotected_apep_bytes(
                short,
                HePacketPadding::None,
                he(0)
            ),
            0
        );
        assert_eq!(
            maximum_unprotected_apep_bytes(threshold, HePacketPadding::None, he(11)),
            0
        );
        assert_eq!(packet_padding_from_code(2), HePacketPadding::Us16);
        assert_eq!(packet_padding_from_code(3), HePacketPadding::None);
    }
}
