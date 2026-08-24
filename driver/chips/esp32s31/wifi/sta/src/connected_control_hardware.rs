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

/// Deepest completed stage of one hardware doze attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationDozeHardwareStage {
    None,
    /// The two-register station TSF wake gate was programmed and restored
    /// through the affine PAC token.
    StationTsfWakeGate,
}

/// First hardware boundary whose semantics are not present in reviewed S31
/// evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationDozeUnsupportedStage {
    /// Target packing is reviewed, but the restricted generated PAC has no
    /// safe arbitrary 26-bit writer for the STA TBTT target field.
    StationTbttTargetProgramming,
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
    /// the reviewed wake-gate prefix, then fails closed before target compare,
    /// WDEVPWR arming or RF/PHY power transition.
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
        let _ = permit;
        self.probe_station_tbtt_wake_prefix()
            .map_err(StationDozeHardwareError::WakePrepare)?;
        Err(StationDozeHardwareError::Unsupported {
            reached: StationDozeHardwareStage::StationTsfWakeGate,
            missing: StationDozeUnsupportedStage::StationTbttTargetProgramming,
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
