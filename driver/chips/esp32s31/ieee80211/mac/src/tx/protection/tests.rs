use super::*;
use crate::{
    rx::HeGuardIntervalAndLtf,
    tx::{HeMcs, HeRate, HtGuardInterval, HtMcs, HtRate},
};

const fn ht(width: HtChannelWidth) -> TxPhyRate {
    TxPhyRate::Ht(HtRate::new(HtMcs::Mcs7, HtGuardInterval::Long800Ns, width))
}

const fn he() -> TxPhyRate {
    TxPhyRate::He(HeRate::new(
        HeMcs::Mcs7,
        HeGuardIntervalAndLtf::TwoLtf1600Ns,
    ))
}

#[test]
fn erp_protects_only_ofdm_and_keeps_the_physical_frontier_explicit() {
    let policy =
        WifiTxProtectionPolicy::new(ErpProtectionMode::CtsToSelf, HtProtectionMode::None, None);
    assert_eq!(
        policy.require_unprotected(
            TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
            TxProtectionReceiver::Individual,
            None,
        ),
        Ok(())
    );
    assert_eq!(
        policy.require_unprotected(
            TxPhyRate::Legacy(LegacyRate::Ofdm24M),
            TxProtectionReceiver::Individual,
            None,
        ),
        Err(TxProtectionAdmissionError::PhysicalPublicationUnverified {
            request: TxProtectionRequest {
                mechanism: TxProtectionMechanism::CtsToSelf,
                reason: TxProtectionReason::ErpUseProtection,
            }
        })
    );
}

#[test]
fn ht_modes_distinguish_twenty_and_forty_mhz_and_group_receivers() {
    let twenty =
        WifiTxProtectionPolicy::new(ErpProtectionMode::None, HtProtectionMode::TwentyMhz, None);
    assert_eq!(
        twenty.require_unprotected(
            ht(HtChannelWidth::Mhz20),
            TxProtectionReceiver::Individual,
            None,
        ),
        Ok(())
    );
    assert_eq!(
        twenty.require_unprotected(ht(HtChannelWidth::Mhz40), TxProtectionReceiver::Group, None,),
        Err(TxProtectionAdmissionError::PhysicalPublicationUnverified {
            request: TxProtectionRequest {
                mechanism: TxProtectionMechanism::CtsToSelf,
                reason: TxProtectionReason::Ht(HtProtectionMode::TwentyMhz),
            }
        })
    );
}

#[test]
fn he_threshold_accepts_only_a_proven_nonzero_duration_ceiling() {
    let threshold = HeTxopDurationRtsThreshold::new(64).unwrap();
    let policy = WifiTxProtectionPolicy::new(
        ErpProtectionMode::None,
        HtProtectionMode::None,
        Some(threshold),
    );
    assert_eq!(
        policy.require_unprotected(he(), TxProtectionReceiver::Individual, None),
        Err(TxProtectionAdmissionError::HePpduDurationUnowned { threshold })
    );
    assert_eq!(
        policy.require_unprotected(
            he(),
            TxProtectionReceiver::Individual,
            Some(HeEdcaTxopLimit::from_units_32_us(64).unwrap()),
        ),
        Ok(())
    );
    assert!(matches!(
        policy.require_unprotected(
            he(),
            TxProtectionReceiver::Individual,
            Some(HeEdcaTxopLimit::from_units_32_us(65).unwrap()),
        ),
        Err(TxProtectionAdmissionError::PhysicalPublicationUnverified { .. })
    ));
    assert!(matches!(
        policy.require_unprotected(
            he(),
            TxProtectionReceiver::Individual,
            Some(HeEdcaTxopLimit::DEFAULT),
        ),
        Err(TxProtectionAdmissionError::PhysicalPublicationUnverified { .. })
    ));
}

#[test]
fn ie_decoders_retain_native_protocol_encodings() {
    assert_eq!(
        ErpProtectionMode::from_information(Some(0x02)),
        ErpProtectionMode::CtsToSelf
    );
    let mut operation = [0_u8; 24];
    operation[..2].copy_from_slice(&[61, 22]);
    operation[4] = 3;
    assert_eq!(
        HtProtectionMode::from_operation_ie(Some(&operation)),
        HtProtectionMode::NonHtMixed
    );
    assert_eq!(HeTxopDurationRtsThreshold::new(0), None);
    assert_eq!(HeTxopDurationRtsThreshold::new(0x03ff), None);
    assert_eq!(HeTxopDurationRtsThreshold::new(7).unwrap().units_32_us(), 7);
}
