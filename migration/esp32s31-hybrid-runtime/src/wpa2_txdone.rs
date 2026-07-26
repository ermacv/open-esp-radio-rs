//! Allocation-free STA EAPOL TX-completion bridge.

use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_arch = "riscv32")]
use core::sync::atomic::AtomicBool;

use crate::{
    channel::{BoundedChannel, Receive},
    wpa2::EapolKeyMessage,
};

#[cfg(any(test, target_arch = "riscv32"))]
use crate::wpa2::EapolKeyFrame;

pub const WPA2_STA_TX_DONE_CAPACITY: usize = 8;
#[cfg(any(test, target_arch = "riscv32"))]
const MAX_EAPOL_TX_FRAME: usize = 512;
#[cfg(any(test, target_arch = "riscv32"))]
const CCMP_HEADER_LEN: usize = 8;
#[cfg(any(test, target_arch = "riscv32"))]
const LLC_SNAP_EAPOL: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wpa2StaTxDone {
    pub message: EapolKeyMessage,
    pub replay_counter: u64,
    pub failed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2TxDoneInstallError {
    AlreadyInstalled,
    NotInstalled,
    NotRadioOwner,
    PendingEvents,
}

static EVENTS: BoundedChannel<Wpa2StaTxDone, WPA2_STA_TX_DONE_CAPACITY> = BoundedChannel::new();
#[cfg(target_arch = "riscv32")]
static INSTALLED: AtomicBool = AtomicBool::new(false);
static REJECTED: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_COMPLETED_LENGTH: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_COMPLETED_HASH: AtomicUsize = AtomicUsize::new(0);

#[cfg(feature = "hil-vendor-tx")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HilCompletedEapolSnapshot {
    pub length: usize,
    pub hash: u32,
}

#[cfg(feature = "hil-vendor-tx")]
pub fn hil_completed_eapol_snapshot() -> HilCompletedEapolSnapshot {
    HilCompletedEapolSnapshot {
        length: HIL_COMPLETED_LENGTH.load(Ordering::Acquire),
        hash: HIL_COMPLETED_HASH.load(Ordering::Acquire) as u32,
    }
}

pub fn try_receive_wpa2_sta_tx_done() -> Option<Wpa2StaTxDone> {
    EVENTS.try_receive()
}

pub fn receive_wpa2_sta_tx_done() -> Receive<'static, Wpa2StaTxDone, WPA2_STA_TX_DONE_CAPACITY> {
    EVENTS.receive()
}

pub fn rejected_wpa2_sta_tx_done() -> usize {
    REJECTED.load(Ordering::Acquire)
}

pub fn async_wpa2_sta_tx_done_installed() -> bool {
    #[cfg(target_arch = "riscv32")]
    {
        INSTALLED.load(Ordering::Acquire)
    }
    #[cfg(not(target_arch = "riscv32"))]
    {
        false
    }
}

#[cfg(any(test, target_arch = "riscv32"))]
fn ingest(frame: *const u8, length: usize, failed: bool) -> bool {
    if frame.is_null()
        || !(crate::wpa2::EAPOL_KEY_PACKET_LEN..=MAX_EAPOL_TX_FRAME).contains(&length)
    {
        REJECTED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let bytes = unsafe { core::slice::from_raw_parts(frame, length) };
    #[cfg(feature = "hil-vendor-tx")]
    {
        let mut hash = 0x811c_9dc5_u32;
        for byte in bytes {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
        HIL_COMPLETED_LENGTH.store(length, Ordering::Release);
        HIL_COMPLETED_HASH.store(hash as usize, Ordering::Release);
    }
    let Ok(key) = EapolKeyFrame::parse(bytes) else {
        REJECTED.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    let message = key.message();
    if !matches!(
        message,
        EapolKeyMessage::PairwiseMessage2 | EapolKeyMessage::PairwiseMessage4
    ) {
        REJECTED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    if EVENTS
        .try_send(Wpa2StaTxDone {
            message,
            replay_counter: key.replay_counter(),
            failed,
        })
        .is_err()
    {
        REJECTED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    true
}

#[cfg(any(test, target_arch = "riscv32"))]
fn completed_sta_eapol(packet: &[u8]) -> Option<&[u8]> {
    let frame_control = u16::from_le_bytes(packet.get(..2)?.try_into().ok()?);
    if frame_control & 0x008c != 0x0088 {
        return None;
    }
    let mut header_len = if frame_control & 0x0300 == 0x0300 {
        30
    } else {
        24
    };
    header_len += 2; // QoS Control.
    if frame_control & 0x8000 != 0 {
        header_len += 4;
    }
    if frame_control & 0x4000 != 0 {
        header_len += CCMP_HEADER_LEN;
    }
    if packet.get(header_len..header_len + LLC_SNAP_EAPOL.len())? != LLC_SNAP_EAPOL {
        return None;
    }
    let eapol = packet.get(header_len + LLC_SNAP_EAPOL.len()..)?;
    let body_len = usize::from(u16::from_be_bytes(eapol.get(2..4)?.try_into().ok()?));
    let eapol_len = body_len.checked_add(4)?;
    if eapol_len > MAX_EAPOL_TX_FRAME {
        return None;
    }
    eapol.get(..eapol_len)
}

/// Deliver one completed STA EAPOL MPDU without entering the vendor
/// connection-manager callback.
///
/// The stock `sta_eapol_txdone_cb` gates the registered callback on its own
/// association state and performs a stateful key-table lookup for protected
/// M4. Strict association is Rust-owned, so that gate can silently discard an
/// otherwise successful completion. The owned TX buffer already exposes the
/// finite QoS/CCMP/LLC layout; validate it and copy only parsed EAPOL metadata
/// into the bounded channel.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn ingest_completed_sta_frame(frame: *mut u8, failed: bool) -> bool {
    const FRAME_BUFFER_OFFSET: usize = 0x04;
    const FRAME_LAYOUT_OFFSET: usize = 0x24;
    const BUFFER_DATA_OFFSET: usize = 0x04;
    const PP_PREFIX_LEN: usize = 8;

    if frame.is_null() {
        REJECTED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let buffer = unsafe {
        frame
            .add(FRAME_BUFFER_OFFSET)
            .cast::<*mut u8>()
            .read_unaligned()
    };
    if buffer.is_null() {
        REJECTED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let mut packet = unsafe {
        buffer
            .add(BUFFER_DATA_OFFSET)
            .cast::<*mut u8>()
            .read_unaligned()
    };
    let layout = unsafe {
        frame
            .add(FRAME_LAYOUT_OFFSET)
            .cast::<u16>()
            .read_unaligned()
    };
    if packet.is_null() || layout & 0x2000 == 0 {
        REJECTED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let mpdu_len = unsafe { packet.cast::<u32>().read_unaligned() as usize } & 0x3fff;
    packet = unsafe { packet.add(PP_PREFIX_LEN) };
    if mpdu_len < 24 + LLC_SNAP_EAPOL.len() + crate::wpa2::EAPOL_KEY_PACKET_LEN {
        REJECTED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let packet = unsafe { core::slice::from_raw_parts(packet, mpdu_len) };
    let Some(eapol) = completed_sta_eapol(packet) else {
        REJECTED.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    ingest(eapol.as_ptr(), eapol.len(), failed)
}

#[cfg(target_arch = "riscv32")]
type EapolTxDoneCallback = unsafe extern "C" fn(*const u8, usize, bool);

#[cfg(target_arch = "riscv32")]
unsafe extern "C" {
    fn esp_wifi_register_eapol_txdonecb_internal(callback: Option<EapolTxDoneCallback>);
    fn eapol_txcb(frame: *const u8, length: usize, failed: bool);
}

#[cfg(target_arch = "riscv32")]
#[no_mangle]
unsafe extern "C" fn __esp_wifi_async_wpa2_sta_txdone(
    frame: *const u8,
    length: usize,
    failed: bool,
) {
    let _ = ingest(frame, length, failed);
}

/// Replace the stock STA `eapol_txcb` state transition after Wi-Fi setup.
///
/// # Safety
/// Registration must be serialized with supplicant initialization and
/// deinitialization and must finish before the executor/radio IRQ is enabled.
/// The callback ABI and the stock restore symbol are pinned by the S31
/// analyzer.
#[cfg(target_arch = "riscv32")]
pub unsafe fn install_async_wpa2_sta_tx_done() -> Result<(), Wpa2TxDoneInstallError> {
    if !EVENTS.is_empty() {
        return Err(Wpa2TxDoneInstallError::PendingEvents);
    }
    if INSTALLED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(Wpa2TxDoneInstallError::AlreadyInstalled);
    }
    esp_wifi_register_eapol_txdonecb_internal(Some(__esp_wifi_async_wpa2_sta_txdone));
    Ok(())
}

/// Restore the pinned stock callback before supplicant teardown.
///
/// # Safety
/// The same serialization requirements as
/// [`install_async_wpa2_sta_tx_done`] apply. The restored callback is not
/// strict-safe and must not run before teardown takes exclusive ownership.
#[cfg(target_arch = "riscv32")]
pub unsafe fn uninstall_async_wpa2_sta_tx_done() -> Result<(), Wpa2TxDoneInstallError> {
    if !crate::context::in_radio_context() {
        return Err(Wpa2TxDoneInstallError::NotRadioOwner);
    }
    if INSTALLED
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(Wpa2TxDoneInstallError::NotInstalled);
    }
    esp_wifi_register_eapol_txdonecb_internal(Some(eapol_txcb));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wpa2_frames::Wpa2TxFrame;

    fn qos_eapol_mpdu(eapol: &[u8], protected: bool) -> std::vec::Vec<u8> {
        let mut packet = std::vec![0_u8; 26 + usize::from(protected) * CCMP_HEADER_LEN];
        packet[..2].copy_from_slice(&(0x0188_u16 | u16::from(protected) << 14).to_le_bytes());
        packet.extend_from_slice(&LLC_SNAP_EAPOL);
        packet.extend_from_slice(eapol);
        packet
    }

    #[test]
    fn completed_mpdu_parser_accepts_plain_m2_and_ccmp_m4() {
        let m2 = Wpa2TxFrame::<128>::message2(
            [1; 6],
            7,
            [2; 32],
            &crate::wpa2_frames::OwnedRsnIe::<2>::try_copy(&[0x30, 0]).unwrap(),
        )
        .unwrap();
        let plain = qos_eapol_mpdu(m2.as_bytes(), false);
        assert_eq!(completed_sta_eapol(&plain), Some(m2.as_bytes()));

        let m4 = Wpa2TxFrame::<128>::message4([1; 6], 8).unwrap();
        let protected = qos_eapol_mpdu(m4.as_bytes(), true);
        assert_eq!(completed_sta_eapol(&protected), Some(m4.as_bytes()));
    }

    #[test]
    fn completed_mpdu_parser_rejects_wrong_llc_and_declared_length() {
        let m4 = Wpa2TxFrame::<128>::message4([1; 6], 8).unwrap();
        let mut packet = qos_eapol_mpdu(m4.as_bytes(), true);
        packet[26 + CCMP_HEADER_LEN] ^= 1;
        assert_eq!(completed_sta_eapol(&packet), None);

        let mut packet = qos_eapol_mpdu(m4.as_bytes(), true);
        let eapol_offset = 26 + CCMP_HEADER_LEN + LLC_SNAP_EAPOL.len();
        packet[eapol_offset + 2..eapol_offset + 4].copy_from_slice(&u16::MAX.to_be_bytes());
        assert_eq!(completed_sta_eapol(&packet), None);
    }

    #[test]
    fn callback_copies_only_valid_sta_handshake_metadata() {
        while try_receive_wpa2_sta_tx_done().is_some() {}
        let m2 = Wpa2TxFrame::<128>::message2(
            [1; 6],
            7,
            [2; 32],
            &crate::wpa2_frames::OwnedRsnIe::<2>::try_copy(&[0x30, 0]).unwrap(),
        )
        .unwrap();
        ingest(m2.as_bytes().as_ptr(), m2.as_bytes().len(), false);
        assert_eq!(
            try_receive_wpa2_sta_tx_done(),
            Some(Wpa2StaTxDone {
                message: EapolKeyMessage::PairwiseMessage2,
                replay_counter: 7,
                failed: false,
            })
        );

        let m1 = Wpa2TxFrame::<128>::message1([1; 6], 8, [3; 32]).unwrap();
        let rejected = rejected_wpa2_sta_tx_done();
        ingest(m1.as_bytes().as_ptr(), m1.as_bytes().len(), false);
        assert_eq!(rejected_wpa2_sta_tx_done(), rejected + 1);
        assert_eq!(try_receive_wpa2_sta_tx_done(), None);
    }
}
