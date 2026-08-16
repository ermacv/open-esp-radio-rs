//! Finite ESP32-S31 receive BlockAck hardware leaf in the live MAC crate.
//!
//! The pinned vendor function programs one of eight reverse-addressed MAC
//! register banks. This module reproduces only the register transaction. It
//! owns no agreement state, allocates nothing, and never waits.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::RadioRuntimeOwner;
use open_esp_radio_esp32s31_hal::types::{
    MacInterface, MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence, MacRxBlockAckTid,
    MacRxBlockAckWindow,
};
use open_esp_radio_esp32s31_hal::wifi_mac::WifiMacHal;

const RX_BLOCK_ACK_CAPACITY: u8 = 8;
/// Highest receive BlockAck TID accepted by the vendor net80211 state machine.
///
/// SOURCE: complete `libnet80211.a[ieee80211_ht.o]::
/// ht_recv_action_ba_addba_request`. After extracting the four-bit TID from
/// ADDBA parameters, the vendor rejects requests with bit three set. Thus its
/// ordinary receive-BA path accepts TIDs 0 through 7, including TID 7 used by
/// the FRITZ!Box downlink queue.
pub const S31_RX_BLOCK_ACK_MAX_TID: u8 = 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct S31RxBlockAckAgreement {
    pub hardware_index: u8,
    pub interface: MacInterface,
    pub peer: [u8; 6],
    pub tid: u8,
    pub starting_sequence: u16,
    pub window: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum S31RxBlockAckAgreementError {
    HardwareIndex(u8),
    MulticastPeer,
    Tid(u8),
    StartingSequence(u16),
    Window(u16),
}

/// Narrow hardware capability shared by station and access-point RX
/// BlockAck control. It owns only one bank transaction at a time; agreement
/// lifecycle and retained frames remain above this leaf.
pub trait RxBlockAckHardware {
    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError>;

    fn clear_rx_block_ack(&mut self, hardware_index: u8)
    -> Result<(), S31RxBlockAckAgreementError>;
}

impl RxBlockAckHardware for WifiMacHal<'_> {
    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        program(self, agreement)
    }

    fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        clear(self, hardware_index)
    }
}

impl RxBlockAckHardware for RadioRuntimeOwner {
    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        program(&mut self.wifi_mac_hal(), agreement)
    }

    fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        clear(&mut self.wifi_mac_hal(), hardware_index)
    }
}

impl S31RxBlockAckAgreement {
    pub const fn validate(self) -> Result<Self, S31RxBlockAckAgreementError> {
        if self.hardware_index >= RX_BLOCK_ACK_CAPACITY {
            return Err(S31RxBlockAckAgreementError::HardwareIndex(
                self.hardware_index,
            ));
        }
        if self.peer[0] & 1 != 0 {
            return Err(S31RxBlockAckAgreementError::MulticastPeer);
        }
        if self.tid > S31_RX_BLOCK_ACK_MAX_TID {
            return Err(S31RxBlockAckAgreementError::Tid(self.tid));
        }
        if self.starting_sequence > 0x0fff {
            return Err(S31RxBlockAckAgreementError::StartingSequence(
                self.starting_sequence,
            ));
        }
        if self.window == 0 || self.window > 0x7f {
            return Err(S31RxBlockAckAgreementError::Window(self.window));
        }
        Ok(self)
    }
}

/// Program one receive BlockAck entry without entering vendor logging,
/// allocation, timer, or synchronization code.
///
/// The mutable PAC borrow serializes this transaction with other MAC register
/// operations. The caller must keep the matching software reorder state alive
/// until [`clear`] completes.
pub fn program(
    mmio: &mut WifiMacHal<'_>,
    agreement: S31RxBlockAckAgreement,
) -> Result<(), S31RxBlockAckAgreementError> {
    let agreement = agreement.validate()?;
    mmio.program_rx_block_ack_entry(
        MacRxBlockAckEntryIndex::new(u32::from(agreement.hardware_index))
            .expect("validated receive BlockAck hardware index"),
        agreement.interface,
        agreement.peer,
        MacRxBlockAckTid::new(u32::from(agreement.tid)).expect("validated receive BlockAck TID"),
        MacRxBlockAckStartingSequence::new(u32::from(agreement.starting_sequence))
            .expect("validated receive BlockAck starting sequence"),
        MacRxBlockAckWindow::new(u32::from(agreement.window))
            .expect("validated receive BlockAck window"),
    );
    Ok(())
}

/// Remove one extra SoftAP receive BlockAck entry.
///
/// The caller must recycle all retained software reorder slots first.
pub fn clear(
    mmio: &mut WifiMacHal<'_>,
    hardware_index: u8,
) -> Result<(), S31RxBlockAckAgreementError> {
    if hardware_index >= RX_BLOCK_ACK_CAPACITY {
        return Err(S31RxBlockAckAgreementError::HardwareIndex(hardware_index));
    }
    mmio.delete_rx_block_ack_entry(
        MacRxBlockAckEntryIndex::new(u32::from(hardware_index))
            .expect("validated receive BlockAck hardware index"),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGREEMENT: S31RxBlockAckAgreement = S31RxBlockAckAgreement {
        hardware_index: 3,
        interface: MacInterface::AccessPoint,
        peer: [0x70, 0x15, 0xfb, 0xa8, 0x48, 0xf0],
        tid: 6,
        starting_sequence: 0x0abc,
        window: 16,
    };

    #[test]
    fn validation_rejects_every_unrepresentable_field() {
        assert!(AGREEMENT.validate().is_ok());
        assert!(matches!(
            S31RxBlockAckAgreement {
                hardware_index: 8,
                ..AGREEMENT
            }
            .validate(),
            Err(S31RxBlockAckAgreementError::HardwareIndex(8))
        ));
        assert!(matches!(
            S31RxBlockAckAgreement {
                peer: [1, 0, 0, 0, 0, 0],
                ..AGREEMENT
            }
            .validate(),
            Err(S31RxBlockAckAgreementError::MulticastPeer)
        ));
        assert!(matches!(
            S31RxBlockAckAgreement {
                tid: 8,
                ..AGREEMENT
            }
            .validate(),
            Err(S31RxBlockAckAgreementError::Tid(8))
        ));
        assert!(matches!(
            S31RxBlockAckAgreement {
                window: 128,
                ..AGREEMENT
            }
            .validate(),
            Err(S31RxBlockAckAgreementError::Window(128))
        ));
    }
}
