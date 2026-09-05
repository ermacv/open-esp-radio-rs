use super::*;
use crate::RadioTimestamp;

#[test]
fn unknown_bits_fail_closed_and_known_sets_round_trip() {
    for bit in 0..16 {
        let image = 1_u16 << bit;
        assert_eq!(RadioCapabilities::from_bits(image).is_ok(), bit < 11);
    }
    let set = RadioCapabilities::CSMA_CA | RadioCapabilities::ENERGY_SCAN;
    assert_eq!(RadioCapabilities::from_bits(set.bits()), Ok(set));
    assert!(set.supports_tx_mode(TxMode::CsmaCa { max_backoffs: 4 }));
    assert!(!set.supports_tx_mode(TxMode::Scheduled {
        at: RadioTimestamp::from_micros(10),
    }));
}
