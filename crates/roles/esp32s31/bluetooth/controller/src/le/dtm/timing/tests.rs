use crate::le::dtm::{DtmPayloadLength, DtmPhy};

use super::DtmTxTimingMicros;

fn timing(length: u8, phy: DtmPhy) -> DtmTxTimingMicros {
    DtmTxTimingMicros::new(DtmPayloadLength::from_hci_image(length), phy, 0)
}

#[test]
fn zero_length_packets_match_all_four_complete_phy_branches() {
    assert_eq!(timing(0, DtmPhy::Le1M).packet_duration(), 96);
    assert_eq!(timing(0, DtmPhy::Le1M).interval(), 625);
    assert_eq!(timing(0, DtmPhy::Le2M).packet_duration(), 52);
    assert_eq!(timing(0, DtmPhy::Le2M).interval(), 625);
    assert_eq!(timing(0, DtmPhy::LeCoded).packet_duration(), 848);
    assert_eq!(timing(0, DtmPhy::LeCoded).interval(), 1_250);
    assert_eq!(timing(0, DtmPhy::LeCodedS2).packet_duration(), 494);
    assert_eq!(timing(0, DtmPhy::LeCodedS2).interval(), 1_250);
}

#[test]
fn maximum_length_packets_retain_exact_duration_and_rounding() {
    assert_eq!(timing(255, DtmPhy::Le1M).packet_duration(), 2_136);
    assert_eq!(timing(255, DtmPhy::Le1M).interval(), 2_500);
    assert_eq!(timing(255, DtmPhy::Le2M).packet_duration(), 1_072);
    assert_eq!(timing(255, DtmPhy::Le2M).interval(), 1_875);
    assert_eq!(timing(255, DtmPhy::LeCoded).packet_duration(), 17_168);
    assert_eq!(timing(255, DtmPhy::LeCoded).interval(), 17_500);
    assert_eq!(timing(255, DtmPhy::LeCodedS2).packet_duration(), 4_574);
    assert_eq!(timing(255, DtmPhy::LeCodedS2).interval(), 5_000);
}

#[test]
fn every_length_and_phy_matches_the_complete_vendor_integer_formula() {
    let phys = [
        DtmPhy::Le1M,
        DtmPhy::Le2M,
        DtmPhy::LeCoded,
        DtmPhy::LeCodedS2,
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
    let below = DtmTxTimingMicros::new(
        DtmPayloadLength::from_hci_image(255),
        DtmPhy::LeCoded,
        17_499,
    );
    assert_eq!(below.interval(), 17_500);

    let above = DtmTxTimingMicros::new(
        DtmPayloadLength::from_hci_image(255),
        DtmPhy::LeCoded,
        17_501,
    );
    assert_eq!(above.interval(), 17_501);

    let maximum =
        DtmTxTimingMicros::new(DtmPayloadLength::from_hci_image(0), DtmPhy::Le1M, u16::MAX);
    assert_eq!(maximum.interval(), u32::from(u16::MAX));
}

#[test]
fn complete_s31_tick_tail_is_identity_with_zero_remainder() {
    let phys = [
        DtmPhy::Le1M,
        DtmPhy::Le2M,
        DtmPhy::LeCoded,
        DtmPhy::LeCodedS2,
    ];
    let requests = [0, 1, 624, 625, 626, 17_501, u16::MAX];

    for phy in phys {
        for length in 0..=u8::MAX {
            for requested_interval in requests {
                let micros = DtmTxTimingMicros::new(
                    DtmPayloadLength::from_hci_image(length),
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
        (DtmPhy::Le1M, 2_136),
        (DtmPhy::Le2M, 1_072),
        (DtmPhy::LeCoded, 17_168),
        (DtmPhy::LeCodedS2, 4_574),
    ];

    for (phy, expected_window) in expected {
        for length in 0..=u8::MAX {
            let timing = timing(length, phy).scheduler_timing();
            assert_eq!(timing.packet_window_micros(), expected_window);
        }
    }
}
