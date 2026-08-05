//! Hardware and TX ports consumed by connected-station control.
//!
//! This module is the narrow integration boundary around the protocol state
//! machine. It contains the ESP32-S31 PAC binding and the pinned-TX binding,
//! but owns no mailbox, BlockAck session, beacon policy, or power-save state.

use core::future::Future;

use open_esp_radio_esp32s31_registers::{MacHeTid, RadioRegisters};
use open_esp_radio_esp32s31_wifi_lmac::{
    rx_ampdu_hw::{self, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    tx::TxHardware,
};
use open_esp_radio_ieee80211::station_power_save::StaPowerManagement;

use crate::{
    runner::{WifiControlProgress, WifiControlProgress::TxPending},
    single_mpdu_tx::{
        ActionTxConfig, Esp32s31SingleMpduTx, SingleMpduTxError, SingleMpduTxOutcome,
    },
};

/// PAC authority required by connected BlockAck control.
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

/// Minimal connected-TX capability consumed by the BlockAck control plane.
///
/// Keeping this interface independent of buffer sizes and concrete TX owners
/// lets the same control state machine drive both an ordinary-only bring-up
/// fixture and the production aggregate owner. The implementation remains
/// monomorphized; this is not a dynamic runtime adapter.
pub trait ConnectedControlTx {
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome>;

    fn now_micros(&self) -> u64;

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_;

    fn peek_qos_sequence(&self, tid: u8) -> Option<u16>;

    fn start_action<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<WifiControlProgress, SingleMpduTxError>;

    fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<WifiControlProgress, SingleMpduTxError>;

    /// Mirror the protocol session into the data scheduler. Ordinary-only
    /// fixtures deliberately ignore this edge; aggregate owners use it as
    /// the sole permission to publish an A-MPDU for the TID.
    fn set_tx_block_ack_operational(&mut self, tid: u8, operational: bool);
}

impl<P, E, T, const BUFFER_SIZE: usize> ConnectedControlTx
    for Esp32s31SingleMpduTx<'_, P, E, T, BUFFER_SIZE>
where
    P: crate::ordinary_tx::WifiTxPowerProfile,
    E: crate::ordinary_tx::WifiTxEntropy,
    T: crate::ordinary_tx::WifiTxTimer,
{
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        Esp32s31SingleMpduTx::take_last_outcome(self)
    }

    fn now_micros(&self) -> u64 {
        Esp32s31SingleMpduTx::now_micros(self)
    }

    fn wait_until_micros(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        Esp32s31SingleMpduTx::wait_until_micros(self, deadline_micros)
    }

    fn peek_qos_sequence(&self, tid: u8) -> Option<u16> {
        Esp32s31SingleMpduTx::peek_qos_sequence(self, tid)
    }

    fn start_action<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<WifiControlProgress, SingleMpduTxError> {
        Esp32s31SingleMpduTx::start_action(self, hardware, body, config).map(|_| TxPending)
    }

    fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<WifiControlProgress, SingleMpduTxError> {
        Esp32s31SingleMpduTx::start_power_management_null(self, hardware, power_management)
            .map(|_| TxPending)
    }

    fn set_tx_block_ack_operational(&mut self, _tid: u8, _operational: bool) {}
}
