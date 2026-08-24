//! ESP32-S31 hardware contract used by connected-station control.
//!
//! The protocol state machine depends on this narrow interface rather than
//! the PAC owner directly.  The contract belongs to the chip composition: it
//! describes TSF, RX BlockAck and HE-TID operations, but no executor wakeups
//! or Embassy task lifecycle.

use open_esp_radio_esp32s31_wifi::cooperative_hardware::CooperativeRadioHardware;
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{CryptoKeyError, StaGroupCcmpSlot},
    rx_ampdu_hw::{RxBlockAckHardware, S31RxBlockAckAgreementError},
    tx::TxHardware,
};
use open_esp_radio_wifi_sta::power_save::StaDozePermit;
use open_esp_radio_wifi_sta::twt::{IndividualTwtAgreement, IndividualTwtProposal};

/// Deepest completed stage of one hardware doze attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationDozeHardwareStage {
    None,
    /// The station TBTT target plus two-register wake gate were programmed and
    /// restored through the affine PAC token.
    StationTbttWakePrefix {
        target_bits_35_10: u32,
    },
}

/// First hardware boundary whose semantics are not present in reviewed S31
/// evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationDozeUnsupportedStage {
    /// The four generic TSF timer WDEVPWR causes are known, but the low three
    /// timer-control bits do not identify which compare domain is the STA TSF.
    StationWdevpwrCompareBinding,
    /// No entered transaction exists for the generic default restore method.
    HardwareSleepEntry,
}

/// Fail-closed result retaining exact reached-stage evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationDozeHardwareError {
    WakePrepare(open_esp_radio_esp32s31_hal::StaTbttWakePrepareError),
    Unsupported {
        reached: StationDozeHardwareStage,
        missing: StationDozeUnsupportedStage,
    },
}

/// Deepest source-proven stage of one S31 individual-TWT handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationIndividualTwtHardwareStage {
    None,
}

/// First hardware semantic missing from reviewed S31 source/SVD evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationIndividualTwtUnsupportedStage {
    /// The MAC exposes an ITWT/PTI leaf, but reviewed evidence does not map
    /// individual flow IDs and accepted TSF schedules onto that register or
    /// prove coexistence admission ordering.
    ItwtCoexistenceAdmission,
    /// No reviewed source binds an accepted TWT service period to a station
    /// TSF wake compare plus RF/PHY/BB retention transaction.
    WakeScheduleProgramming,
    /// No successful installation exists whose exact register image can be
    /// restored while preserving another flow.
    WakeScheduleRestore,
}

/// Typed fail-closed S31 boundary; no variant implies that RF sleep occurred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationIndividualTwtHardwareError {
    Unsupported {
        reached: StationIndividualTwtHardwareStage,
        missing: StationIndividualTwtUnsupportedStage,
    },
}

/// ESP32-S31 register operations required by connected BlockAck control.
pub trait ConnectedControlHardware: TxHardware + RxBlockAckHardware {
    fn station_tsf(&mut self) -> u64;

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError>;

    /// Enter modem doze atomically. `Err` must leave every hardware owner in
    /// its awake pre-call state. Production currently exercises and rolls back
    /// the reviewed target/wake-gate prefix, then fails closed before binding
    /// it to a WDEVPWR compare cause or entering an RF/PHY power transition.
    fn enter_station_doze(
        &mut self,
        _permit: &StaDozePermit,
    ) -> Result<(), StationDozeHardwareError> {
        Err(StationDozeHardwareError::Unsupported {
            reached: StationDozeHardwareStage::None,
            missing: StationDozeUnsupportedStage::HardwareSleepEntry,
        })
    }

    /// Restore every hardware owner changed by `enter_station_doze`. Failure
    /// retains the affine restore obligation at the caller and must quarantine
    /// normal TX/teardown progress.
    fn restore_station_awake(&mut self) -> Result<(), StationDozeHardwareError> {
        Err(StationDozeHardwareError::Unsupported {
            reached: StationDozeHardwareStage::None,
            missing: StationDozeUnsupportedStage::HardwareSleepEntry,
        })
    }

    /// Prove that this exact proposal can be represented before the shared TX
    /// owner is allowed to publish a TWT Setup request. An error must not
    /// change MAC, coexistence, clock, RF, PHY or BB state.
    fn admit_station_individual_twt(
        &mut self,
        _proposal: &IndividualTwtProposal,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        Err(StationIndividualTwtHardwareError::Unsupported {
            reached: StationIndividualTwtHardwareStage::None,
            missing: StationIndividualTwtUnsupportedStage::ItwtCoexistenceAdmission,
        })
    }

    /// Install one peer-accepted agreement atomically. Failure must retain the
    /// awake pre-call state so connected control can send a teardown.
    fn install_station_individual_twt(
        &mut self,
        _agreement: &IndividualTwtAgreement,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        Err(StationIndividualTwtHardwareError::Unsupported {
            reached: StationIndividualTwtHardwareStage::None,
            missing: StationIndividualTwtUnsupportedStage::WakeScheduleProgramming,
        })
    }

    /// Remove an installed agreement before local teardown, stop or
    /// reconnect. Failure requires the outer owner to quarantine the epoch.
    fn remove_station_individual_twt(
        &mut self,
        _agreement: &IndividualTwtAgreement,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        Err(StationIndividualTwtHardwareError::Unsupported {
            reached: StationIndividualTwtHardwareStage::None,
            missing: StationIndividualTwtUnsupportedStage::WakeScheduleRestore,
        })
    }

    /// Atomically consume the current association's authority to replace its
    /// single active group-key entry. Hardware test doubles which do not
    /// exercise WPA2 rekey may retain the fail-closed default.
    fn replace_sta_group_ccmp(
        &mut self,
        _slot: &mut StaGroupCcmpSlot,
        _key_id: u8,
        _temporal_key: &[u8; 16],
    ) -> Result<(), CryptoKeyError> {
        Err(CryptoKeyError::HardwareRejected)
    }
}

impl ConnectedControlHardware for CooperativeRadioHardware<'_> {
    fn station_tsf(&mut self) -> u64 {
        CooperativeRadioHardware::station_tsf(self)
    }

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        CooperativeRadioHardware::set_he_tid_enabled(self, tid, enabled)
    }

    fn enter_station_doze(
        &mut self,
        permit: &StaDozePermit,
    ) -> Result<(), StationDozeHardwareError> {
        let target_bits_35_10 = self
            .probe_station_tbtt_wake_prefix(permit.wake_tsf)
            .map_err(StationDozeHardwareError::WakePrepare)?;
        Err(StationDozeHardwareError::Unsupported {
            reached: StationDozeHardwareStage::StationTbttWakePrefix { target_bits_35_10 },
            missing: StationDozeUnsupportedStage::StationWdevpwrCompareBinding,
        })
    }

    fn admit_station_individual_twt(
        &mut self,
        _proposal: &IndividualTwtProposal,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        // The reviewed SVD names MAC_COEX ITWT/PTI registers but does not
        // define their per-flow encoding, ownership or relation to the STA
        // TSF wake transaction. Do not probe or guess those bits.
        Err(StationIndividualTwtHardwareError::Unsupported {
            reached: StationIndividualTwtHardwareStage::None,
            missing: StationIndividualTwtUnsupportedStage::ItwtCoexistenceAdmission,
        })
    }

    fn replace_sta_group_ccmp(
        &mut self,
        slot: &mut StaGroupCcmpSlot,
        key_id: u8,
        temporal_key: &[u8; 16],
    ) -> Result<(), CryptoKeyError> {
        CooperativeRadioHardware::replace_sta_group_ccmp(self, slot, key_id, temporal_key)
    }
}
