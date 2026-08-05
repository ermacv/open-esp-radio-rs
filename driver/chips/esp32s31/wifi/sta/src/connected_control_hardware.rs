//! ESP32-S31 hardware contract used by connected-station control.
//!
//! The protocol state machine depends on this narrow interface rather than
//! the PAC owner directly.  The contract belongs to the chip composition: it
//! describes TSF, RX BlockAck and HE-TID operations, but no executor wakeups
//! or Embassy task lifecycle.

use open_esp_radio_esp32s31_registers::{MacHeTid, RadioRegisters};
use open_esp_radio_esp32s31_wifi_mac::{
    rx_ampdu_hw::{self, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    tx::TxHardware,
};

/// ESP32-S31 register operations required by connected BlockAck control.
pub trait ConnectedControlHardware: TxHardware {
    fn station_tsf(&mut self) -> u64;

    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError>;

    fn clear_rx_block_ack(&mut self, hardware_index: u8)
    -> Result<(), S31RxBlockAckAgreementError>;

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError>;
}

impl ConnectedControlHardware for RadioRegisters {
    fn station_tsf(&mut self) -> u64 {
        RadioRegisters::station_tsf(self)
    }

    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::program(self, agreement)
    }

    fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::clear(self, hardware_index)
    }

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        let tid = MacHeTid::new(tid).ok_or(S31RxBlockAckAgreementError::Tid(tid))?;
        RadioRegisters::set_he_trigger_based_tid_enabled(self, tid, enabled);
        Ok(())
    }
}
