//! Finite ESP32-S31 receive BlockAck hardware leaf in the live MAC crate.
//!
//! The pinned vendor function programs one of eight reverse-addressed MAC
//! register banks. This module reproduces only the register transaction. It
//! owns no agreement state, allocates nothing, and never waits.

use open_esp_radio_esp32s31_pac::RadioRegisters;

const RX_BLOCK_ACK_CAPACITY: u8 = 8;
/// Highest receive BlockAck TID accepted by the vendor net80211 state machine.
///
/// SOURCE: complete `_oracles/libnet80211.a[ieee80211_ht.o]::
/// ht_recv_action_ba_addba_request`. After extracting the four-bit TID from
/// ADDBA parameters, the vendor rejects requests with bit three set. Thus its
/// ordinary receive-BA path accepts TIDs 0 through 7, including TID 7 used by
/// the FRITZ!Box downlink queue.
pub const S31_RX_BLOCK_ACK_MAX_TID: u8 = 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct S31RxBlockAckAgreement {
    pub hardware_index: u8,
    pub interface: u8,
    pub peer: [u8; 6],
    pub tid: u8,
    pub starting_sequence: u16,
    pub window: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum S31RxBlockAckAgreementError {
    HardwareIndex(u8),
    Interface(u8),
    MulticastPeer,
    Tid(u8),
    StartingSequence(u16),
    Window(u16),
}

impl S31RxBlockAckAgreement {
    pub const fn validate(self) -> Result<Self, S31RxBlockAckAgreementError> {
        if self.hardware_index >= RX_BLOCK_ACK_CAPACITY {
            return Err(S31RxBlockAckAgreementError::HardwareIndex(
                self.hardware_index,
            ));
        }
        if self.interface > 3 {
            return Err(S31RxBlockAckAgreementError::Interface(self.interface));
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
#[unsafe(link_section = ".rwtext.wifi_strict.rx_ampdu_hw")]
pub fn program(
    mmio: &mut RadioRegisters,
    agreement: S31RxBlockAckAgreement,
) -> Result<(), S31RxBlockAckAgreementError> {
    let agreement = agreement.validate()?;
    mmio.program_rx_block_ack_entry(
        agreement.hardware_index,
        agreement.interface,
        agreement.peer,
        agreement.tid,
        agreement.starting_sequence,
        agreement.window,
    );
    Ok(())
}

/// Remove one extra SoftAP receive BlockAck entry.
///
/// The caller must recycle all retained software reorder slots first.
#[unsafe(link_section = ".rwtext.wifi_strict.rx_ampdu_hw")]
pub fn clear(
    mmio: &mut RadioRegisters,
    hardware_index: u8,
) -> Result<(), S31RxBlockAckAgreementError> {
    if hardware_index >= RX_BLOCK_ACK_CAPACITY {
        return Err(S31RxBlockAckAgreementError::HardwareIndex(hardware_index));
    }
    mmio.delete_rx_block_ack_entry(hardware_index);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGREEMENT: S31RxBlockAckAgreement = S31RxBlockAckAgreement {
        hardware_index: 3,
        interface: 1,
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
