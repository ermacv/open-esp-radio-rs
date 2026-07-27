//! Finite ESP32-S31 receive BlockAck hardware leaf in the live MAC crate.
//!
//! The pinned vendor function programs six MAC registers and then logs the
//! readback. This module reproduces only the register transaction. It owns no
//! agreement state, allocates nothing, and never waits.

use open_esp_radio_pac_esp32s31::{mac::rx_block_ack as registers, RadioRegisters};

const EXTRA_SOFTAP_RX_BA_INDEX_MASK: u32 = registers::control::INDEX.mask();
const EXTRA_SOFTAP_RX_BA_TID_MASK: u32 = registers::control::TID.mask();
const EXTRA_SOFTAP_RX_BA_WINDOW_MASK: u32 = registers::peer_tail_and_policy::WINDOW.mask();
const EXTRA_SOFTAP_RX_BA_VALID: u32 = registers::control::VALID.mask();
const EXTRA_SOFTAP_RX_BA_WRITE: u32 = registers::control::WRITE.mask();
const EXTRA_SOFTAP_RX_BA_ENABLE: u32 = registers::control::ENABLE.mask();

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
        if self.hardware_index >= registers::CAPACITY {
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
        if self.tid > 15 {
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

const fn selected_control(previous: u32, hardware_index: u8) -> u32 {
    (previous & !EXTRA_SOFTAP_RX_BA_INDEX_MASK)
        | ((hardware_index as u32) << 5 & EXTRA_SOFTAP_RX_BA_INDEX_MASK)
}

const fn active_control(hardware_index: u8, tid: u8) -> u32 {
    EXTRA_SOFTAP_RX_BA_VALID
        | EXTRA_SOFTAP_RX_BA_WRITE
        | EXTRA_SOFTAP_RX_BA_ENABLE
        | ((hardware_index as u32) << 5 & EXTRA_SOFTAP_RX_BA_INDEX_MASK)
        // The pinned caller passes its per-TID table byte offset (`tid * 4`)
        // to the HAL leaf, which then shifts the low nibble into this field.
        | ((tid as u32) << 14 & EXTRA_SOFTAP_RX_BA_TID_MASK)
}

const fn peer_head(peer: [u8; 6]) -> u32 {
    u32::from_le_bytes([peer[0], peer[1], peer[2], peer[3]])
}

const fn peer_tail(interface: u8, peer: [u8; 6], window: u16) -> u32 {
    u32::from_le_bytes([peer[4], peer[5], 0, 0])
        | ((interface as u32) << 16 & 0x0003_0000)
        | ((window as u32) << 18 & EXTRA_SOFTAP_RX_BA_WINDOW_MASK)
}

/// Program one receive BlockAck entry without entering vendor logging,
/// allocation, timer, or synchronization code.
///
/// The mutable PAC borrow serializes this transaction with other MAC register
/// operations. The caller must keep the matching software reorder state alive
/// until [`clear`] completes.
#[link_section = ".rwtext.wifi_strict.rx_ampdu_hw"]
pub fn program(
    mmio: &mut RadioRegisters,
    agreement: S31RxBlockAckAgreement,
) -> Result<(), S31RxBlockAckAgreementError> {
    let agreement = agreement.validate()?;
    let selected = selected_control(mmio.read32(registers::CONTROL), agreement.hardware_index);
    mmio.write32(registers::CONTROL, selected);
    mmio.write32(registers::PEER_HEAD, peer_head(agreement.peer));
    mmio.write32(
        registers::PEER_TAIL_AND_POLICY,
        peer_tail(agreement.interface, agreement.peer, agreement.window),
    );
    mmio.write32(
        registers::START_SEQUENCE,
        u32::from(agreement.starting_sequence),
    );
    mmio.write32(registers::BITMAP_LOW, 0);
    mmio.write32(registers::BITMAP_HIGH, 0);
    mmio.write32(
        registers::CONTROL,
        active_control(agreement.hardware_index, agreement.tid),
    );

    let update = mmio.read32(registers::AGREEMENT_UPDATE);
    mmio.write32(
        registers::AGREEMENT_UPDATE,
        update | registers::agreement_update::COMMIT.mask(),
    );
    mmio.modify32(
        registers::AGREEMENT_UPDATE,
        registers::agreement_update::COMMIT.mask(),
        0,
    );
    mmio.modify32(
        registers::AGREEMENT_UPDATE,
        registers::agreement_update::READBACK_LATCH.mask(),
        registers::agreement_update::READBACK_LATCH.mask(),
    );
    mmio.fence();
    let _ = mmio.read32(registers::PEER_TAIL_AND_POLICY);
    let _ = mmio.read32(registers::PEER_HEAD);
    let _ = mmio.read32(registers::START_SEQUENCE);
    let _ = mmio.read32(registers::CONTROL);
    mmio.fence();
    mmio.modify32(
        registers::AGREEMENT_UPDATE,
        registers::agreement_update::READBACK_LATCH.mask(),
        0,
    );
    Ok(())
}

/// Remove one extra SoftAP receive BlockAck entry.
///
/// The caller must recycle all retained software reorder slots first.
#[link_section = ".rwtext.wifi_strict.rx_ampdu_hw"]
pub fn clear(
    mmio: &mut RadioRegisters,
    hardware_index: u8,
) -> Result<(), S31RxBlockAckAgreementError> {
    if hardware_index >= registers::CAPACITY {
        return Err(S31RxBlockAckAgreementError::HardwareIndex(hardware_index));
    }
    let selected = selected_control(mmio.read32(registers::CONTROL), hardware_index);
    mmio.write32(registers::CONTROL, selected);
    mmio.modify32(
        registers::AGREEMENT_UPDATE,
        registers::agreement_update::READBACK_LATCH.mask(),
        registers::agreement_update::READBACK_LATCH.mask(),
    );
    mmio.fence();
    let _ = mmio.read32(registers::CONTROL);
    mmio.fence();
    mmio.modify32(
        registers::AGREEMENT_UPDATE,
        registers::agreement_update::READBACK_LATCH.mask(),
        0,
    );
    mmio.modify32(registers::CONTROL, EXTRA_SOFTAP_RX_BA_VALID, 0);
    mmio.write32(registers::BITMAP_LOW, 0);
    mmio.write32(registers::BITMAP_HIGH, 0);
    mmio.modify32(
        registers::AGREEMENT_UPDATE,
        registers::agreement_update::COMMIT.mask(),
        registers::agreement_update::COMMIT.mask(),
    );
    mmio.modify32(
        registers::AGREEMENT_UPDATE,
        registers::agreement_update::COMMIT.mask(),
        0,
    );
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
    fn packs_the_recovered_register_layout() {
        assert_eq!(selected_control(0xffff_ffff, 3), 0xffff_fc7f);
        assert_eq!(active_control(3, 6), 0xc000_8061);
        assert_eq!(peer_head(AGREEMENT.peer), 0xa8fb_1570);
        assert_eq!(peer_tail(1, AGREEMENT.peer, 16), 0x0041_f048);
    }

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
                window: 128,
                ..AGREEMENT
            }
            .validate(),
            Err(S31RxBlockAckAgreementError::Window(128))
        ));
    }
}
