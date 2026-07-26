//! Finite ESP32-S31 receive BlockAck hardware leaf.
//!
//! The pinned vendor function programs six MAC registers and then logs the
//! readback. This module reproduces only the register transaction. It owns no
//! agreement state, allocates nothing, and never waits.

#[cfg(target_arch = "riscv32")]
use core::sync::atomic::{compiler_fence, Ordering};

const RX_BA_CAPACITY: u8 = 8;
const EXTRA_SOFTAP_RX_BA_INDEX_MASK: u32 = 0x0000_03e0;
const EXTRA_SOFTAP_RX_BA_TID_MASK: u32 = 0x0000_f000;
const EXTRA_SOFTAP_RX_BA_WINDOW_MASK: u32 = 0x01fc_0000;
const EXTRA_SOFTAP_RX_BA_VALID: u32 = 0x8000_0000;
const EXTRA_SOFTAP_RX_BA_WRITE: u32 = 0x4000_0000;
const EXTRA_SOFTAP_RX_BA_ENABLE: u32 = 1;

#[cfg(target_arch = "riscv32")]
const EXTRA_SOFTAP_RX_BA_CONTROL: *mut u32 = 0x2010_4ea4 as *mut u32;
#[cfg(target_arch = "riscv32")]
const EXTRA_SOFTAP_RX_BA_PEER_TAIL: *mut u32 = 0x2010_4ea8 as *mut u32;
#[cfg(target_arch = "riscv32")]
const EXTRA_SOFTAP_RX_BA_PEER_HEAD: *mut u32 = 0x2010_4eac as *mut u32;
#[cfg(target_arch = "riscv32")]
const EXTRA_SOFTAP_RX_BA_START_SEQUENCE: *mut u32 = 0x2010_4eb0 as *mut u32;
#[cfg(target_arch = "riscv32")]
const EXTRA_SOFTAP_RX_BA_BITMAP_LOW: *mut u32 = 0x2010_4eb4 as *mut u32;
#[cfg(target_arch = "riscv32")]
const EXTRA_SOFTAP_RX_BA_BITMAP_HIGH: *mut u32 = 0x2010_4eb8 as *mut u32;
#[cfg(target_arch = "riscv32")]
const MAC_AGREEMENT_UPDATE: *mut u32 = 0x2010_4298 as *mut u32;

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
        if self.hardware_index >= RX_BA_CAPACITY {
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
/// # Safety
///
/// The caller must serialize this finite transaction with every other MAC
/// agreement update, run on the radio-owner hart after MAC initialization, and
/// keep the matching software reorder state alive until `clear` completes.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_ampdu_hw"]
pub unsafe fn program(
    agreement: S31RxBlockAckAgreement,
) -> Result<(), S31RxBlockAckAgreementError> {
    let agreement = agreement.validate()?;
    let selected = selected_control(
        EXTRA_SOFTAP_RX_BA_CONTROL.read_volatile(),
        agreement.hardware_index,
    );
    EXTRA_SOFTAP_RX_BA_CONTROL.write_volatile(selected);
    EXTRA_SOFTAP_RX_BA_PEER_HEAD.write_volatile(peer_head(agreement.peer));
    EXTRA_SOFTAP_RX_BA_PEER_TAIL.write_volatile(peer_tail(
        agreement.interface,
        agreement.peer,
        agreement.window,
    ));
    EXTRA_SOFTAP_RX_BA_START_SEQUENCE.write_volatile(u32::from(agreement.starting_sequence));
    EXTRA_SOFTAP_RX_BA_BITMAP_LOW.write_volatile(0);
    EXTRA_SOFTAP_RX_BA_BITMAP_HIGH.write_volatile(0);
    EXTRA_SOFTAP_RX_BA_CONTROL.write_volatile(active_control(
        agreement.hardware_index,
        agreement.tid,
    ));

    let update = MAC_AGREEMENT_UPDATE.read_volatile();
    MAC_AGREEMENT_UPDATE.write_volatile(update | 0x100);
    MAC_AGREEMENT_UPDATE.write_volatile(MAC_AGREEMENT_UPDATE.read_volatile() & !0x100);
    MAC_AGREEMENT_UPDATE.write_volatile(MAC_AGREEMENT_UPDATE.read_volatile() | 0x200);
    compiler_fence(Ordering::SeqCst);
    let _ = EXTRA_SOFTAP_RX_BA_PEER_TAIL.read_volatile();
    let _ = EXTRA_SOFTAP_RX_BA_PEER_HEAD.read_volatile();
    let _ = EXTRA_SOFTAP_RX_BA_START_SEQUENCE.read_volatile();
    let _ = EXTRA_SOFTAP_RX_BA_CONTROL.read_volatile();
    compiler_fence(Ordering::SeqCst);
    MAC_AGREEMENT_UPDATE.write_volatile(MAC_AGREEMENT_UPDATE.read_volatile() & !0x200);
    Ok(())
}

/// Remove one extra SoftAP receive BlockAck entry.
///
/// # Safety
///
/// The caller must satisfy the same MAC ownership and serialization contract
/// as `program` and must recycle all retained software reorder slots first.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.rx_ampdu_hw"]
pub unsafe fn clear(hardware_index: u8) -> Result<(), S31RxBlockAckAgreementError> {
    if hardware_index >= RX_BA_CAPACITY {
        return Err(S31RxBlockAckAgreementError::HardwareIndex(hardware_index));
    }
    let selected = selected_control(EXTRA_SOFTAP_RX_BA_CONTROL.read_volatile(), hardware_index);
    EXTRA_SOFTAP_RX_BA_CONTROL.write_volatile(selected);
    MAC_AGREEMENT_UPDATE.write_volatile(MAC_AGREEMENT_UPDATE.read_volatile() | 0x200);
    compiler_fence(Ordering::SeqCst);
    let _ = EXTRA_SOFTAP_RX_BA_CONTROL.read_volatile();
    compiler_fence(Ordering::SeqCst);
    MAC_AGREEMENT_UPDATE.write_volatile(MAC_AGREEMENT_UPDATE.read_volatile() & !0x200);
    EXTRA_SOFTAP_RX_BA_CONTROL
        .write_volatile(EXTRA_SOFTAP_RX_BA_CONTROL.read_volatile() & !EXTRA_SOFTAP_RX_BA_VALID);
    EXTRA_SOFTAP_RX_BA_BITMAP_LOW.write_volatile(0);
    EXTRA_SOFTAP_RX_BA_BITMAP_HIGH.write_volatile(0);
    MAC_AGREEMENT_UPDATE.write_volatile(MAC_AGREEMENT_UPDATE.read_volatile() | 0x100);
    MAC_AGREEMENT_UPDATE.write_volatile(MAC_AGREEMENT_UPDATE.read_volatile() & !0x100);
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
