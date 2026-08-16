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

/// ESP32-S31 register operations required by connected BlockAck control.
pub trait ConnectedControlHardware: TxHardware + RxBlockAckHardware {
    fn station_tsf(&mut self) -> u64;

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError>;

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

    fn replace_sta_group_ccmp(
        &mut self,
        slot: &mut StaGroupCcmpSlot,
        key_id: u8,
        temporal_key: &[u8; 16],
    ) -> Result<(), CryptoKeyError> {
        CooperativeRadioHardware::replace_sta_group_ccmp(self, slot, key_id, temporal_key)
    }
}
