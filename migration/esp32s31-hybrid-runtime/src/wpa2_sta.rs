//! Static STA link-state callbacks for the pinned S31 WPA table.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::channel::{BoundedChannel, Receive};

#[cfg(target_arch = "riscv32")]
use crate::wpa2_frames::{OwnedAssociationSecurityIes, OwnedRsnIe, Wpa2FrameError};

pub const WPA2_STA_LINK_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2StaLinkEvent {
    Connected { bssid: [u8; 6] },
    Disconnected { reason: u8 },
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2StaAssocRsnError {
    Missing,
    InvalidLength(usize),
    Invalid(Wpa2FrameError),
}

static EVENTS: BoundedChannel<Wpa2StaLinkEvent, WPA2_STA_LINK_CAPACITY> = BoundedChannel::new();
static REJECTED: AtomicUsize = AtomicUsize::new(0);
static HANDSHAKE_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn try_receive_wpa2_sta_link_event() -> Option<Wpa2StaLinkEvent> {
    EVENTS.try_receive()
}

pub fn receive_wpa2_sta_link_event() -> Receive<'static, Wpa2StaLinkEvent, WPA2_STA_LINK_CAPACITY> {
    EVENTS.receive()
}

pub fn rejected_wpa2_sta_link_events() -> usize {
    REJECTED.load(Ordering::Acquire)
}

/// Update the net80211-visible 4-way state after a Rust WPA2 transition.
/// Set it when association begins and clear it after M4 success or failure.
pub fn set_wpa2_sta_handshake_active(active: bool) {
    HANDSHAKE_ACTIVE.store(active, Ordering::Release);
}

#[cfg(target_arch = "riscv32")]
mod target {
    use core::{ffi::c_void, ptr};

    use super::*;

    const WPA_STA_CONNECTED_OFFSET: usize = 0x0c;
    const WPA_STA_DISCONNECTED_OFFSET: usize = 0x10;
    const WPA_STA_HANDSHAKE_OFFSET: usize = 0x18;
    // `pahole`/DWARF for the pinned `wpa.c.obj` reports a 0x488-byte
    // `struct wpa_sm`, with `assoc_wpa_ie` and its length at these offsets.
    const WPA_SM_SIZE: usize = 0x488;
    const WPA_SM_ASSOC_IE_OFFSET: usize = 868;
    const WPA_SM_ASSOC_IE_LEN_OFFSET: usize = 872;
    const WPA_SM_ASSOC_RSNXE_OFFSET: usize = 876;
    const WPA_SM_ASSOC_RSNXE_LEN_OFFSET: usize = 880;

    #[cfg(feature = "hil-vendor-tx")]
    const WPA_SM_TPTK_OFFSET: usize = 344;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_SM_TPTK_SET_OFFSET: usize = 624;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_SM_SNONCE_OFFSET: usize = 628;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_SM_ANONCE_OFFSET: usize = 660;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_SM_RENEW_SNONCE_OFFSET: usize = 692;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_SM_RX_REPLAY_OFFSET: usize = 696;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_SM_RX_REPLAY_SET_OFFSET: usize = 704;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_SM_STATE_OFFSET: usize = 908;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_SM_KEY_INSTALL_OFFSET: usize = 936;

    #[cfg(feature = "hil-vendor-tx")]
    const WPA_PTK_SIZE: usize = 276;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_PTK_KEK_OFFSET: usize = 32;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_PTK_TK_OFFSET: usize = 64;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_PTK_KCK_LEN_OFFSET: usize = 240;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_PTK_KEK_LEN_OFFSET: usize = 244;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_PTK_TK_LEN_OFFSET: usize = 248;
    #[cfg(feature = "hil-vendor-tx")]
    const WPA_PTK_LEN_OFFSET: usize = 264;

    type ConnectedCallback = unsafe extern "C" fn(*const u8);
    type DisconnectedCallback = unsafe extern "C" fn(u8);
    type HandshakeCallback = unsafe extern "C" fn() -> bool;

    static INSTALLED: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" {
        static mut gWpaSm: u8;
        static mut wpa_cb: *mut c_void;
        fn __esp_wpa_sta_connected_cb(bssid: *const u8);
        fn __esp_wpa_sta_connected_cb_end();
        fn __esp_wpa_sta_disconnected_cb(reason: u8);
        fn __esp_wpa_sta_disconnected_cb_end();
        fn wpa_sta_in_4way_handshake() -> bool;
    }

    /// Copy the exact RSN IE emitted in the active STA association request.
    ///
    /// This reads two pinned fields from the already initialized supplicant
    /// state and immediately copies the IE into Rust-owned fixed storage. It
    /// does not enter the supplicant, allocate, wait, or retain its pointer.
    ///
    /// # Safety
    /// Association setup must have completed and teardown/disconnect must not
    /// run concurrently with this bounded copy.
    pub unsafe fn copy_wpa2_sta_assoc_rsn_ie<const N: usize>(
    ) -> Result<OwnedRsnIe<N>, Wpa2StaAssocRsnError> {
        const _: () = assert!(WPA_SM_ASSOC_IE_LEN_OFFSET + 4 <= WPA_SM_SIZE);

        let state = ptr::addr_of!(gWpaSm).cast::<u8>();
        let source = state.add(WPA_SM_ASSOC_IE_OFFSET).cast::<*const u8>().read();
        let length = state.add(WPA_SM_ASSOC_IE_LEN_OFFSET).cast::<usize>().read();
        if source.is_null() {
            return Err(Wpa2StaAssocRsnError::Missing);
        }
        if !(2..=N).contains(&length) || length > u8::MAX as usize + 2 {
            return Err(Wpa2StaAssocRsnError::InvalidLength(length));
        }
        OwnedRsnIe::try_copy(core::slice::from_raw_parts(source, length))
            .map_err(Wpa2StaAssocRsnError::Invalid)
    }

    /// Copy the complete security IE sequence echoed by WPA2 message 2.
    ///
    /// The pinned vendor supplicant concatenates `assoc_wpa_ie` and the
    /// optional `assoc_rsnxe` before computing M2 MIC. Both inputs are copied
    /// into one Rust-owned bounded object without retaining either pointer.
    ///
    /// # Safety
    /// Association setup must have completed and teardown/disconnect must not
    /// run concurrently with this bounded copy.
    pub unsafe fn copy_wpa2_sta_assoc_security_ies<const N: usize>(
    ) -> Result<OwnedAssociationSecurityIes<N>, Wpa2StaAssocRsnError> {
        const _: () = assert!(WPA_SM_ASSOC_RSNXE_LEN_OFFSET + 4 <= WPA_SM_SIZE);

        let rsn = unsafe { copy_wpa2_sta_assoc_rsn_ie::<N>()? };
        let state = ptr::addr_of!(gWpaSm).cast::<u8>();
        let rsnxe_source = state
            .add(WPA_SM_ASSOC_RSNXE_OFFSET)
            .cast::<*const u8>()
            .read();
        let rsnxe_length = state
            .add(WPA_SM_ASSOC_RSNXE_LEN_OFFSET)
            .cast::<usize>()
            .read();
        let rsnxe = if rsnxe_length == 0 {
            &[][..]
        } else {
            if rsnxe_source.is_null()
                || rsn
                    .as_bytes()
                    .len()
                    .checked_add(rsnxe_length)
                    .is_none_or(|length| length > N)
            {
                return Err(Wpa2StaAssocRsnError::InvalidLength(rsnxe_length));
            }
            core::slice::from_raw_parts(rsnxe_source, rsnxe_length)
        };
        OwnedAssociationSecurityIes::try_copy(&rsn, rsnxe).map_err(Wpa2StaAssocRsnError::Invalid)
    }

    /// Publish the state produced by the stock M1 handler for a HIL A/B test.
    ///
    /// This is deliberately excluded from production strict builds. It lets a
    /// hardware test distinguish a hidden consumer of the vendor supplicant
    /// state from an error in the Rust EAPOL frame or the lower TX path. It
    /// performs only bounded stores and copies; no vendor function is called.
    ///
    /// # Safety
    /// The strict handoff must own the initialized supplicant state, and no
    /// vendor supplicant callback may execute concurrently. `ptk` and both
    /// nonces must describe the M2 which is about to be submitted.
    #[cfg(feature = "hil-vendor-tx")]
    pub unsafe fn publish_vendor_wpa2_sta_m2_diagnostic_state(
        supplicant_nonce: &[u8; 32],
        authenticator_nonce: &[u8; 32],
        replay_counter: u64,
        ptk: &crate::wpa2_crypto::Wpa2Ptk,
    ) {
        const _: () = assert!(WPA_SM_KEY_INSTALL_OFFSET < WPA_SM_SIZE);
        const _: () = assert!(WPA_PTK_LEN_OFFSET + 4 <= WPA_PTK_SIZE);

        let state = ptr::addr_of_mut!(gWpaSm).cast::<u8>();
        let temporary_ptk = unsafe { state.add(WPA_SM_TPTK_OFFSET) };
        unsafe {
            temporary_ptk.write_bytes(0, WPA_PTK_SIZE);
            ptr::copy_nonoverlapping(ptk.kck().as_ptr(), temporary_ptk, ptk.kck().len());
            ptr::copy_nonoverlapping(
                ptk.kek().as_ptr(),
                temporary_ptk.add(WPA_PTK_KEK_OFFSET),
                ptk.kek().len(),
            );
            ptr::copy_nonoverlapping(
                ptk.temporal_key().as_ptr(),
                temporary_ptk.add(WPA_PTK_TK_OFFSET),
                ptk.temporal_key().len(),
            );
            temporary_ptk
                .add(WPA_PTK_KCK_LEN_OFFSET)
                .cast::<usize>()
                .write(ptk.kck().len());
            temporary_ptk
                .add(WPA_PTK_KEK_LEN_OFFSET)
                .cast::<usize>()
                .write(ptk.kek().len());
            temporary_ptk
                .add(WPA_PTK_TK_LEN_OFFSET)
                .cast::<usize>()
                .write(ptk.temporal_key().len());
            temporary_ptk
                .add(WPA_PTK_LEN_OFFSET)
                .cast::<usize>()
                .write(ptk.kck().len() + ptk.kek().len() + ptk.temporal_key().len());

            state.add(WPA_SM_TPTK_SET_OFFSET).cast::<u32>().write(1);
            ptr::copy_nonoverlapping(
                supplicant_nonce.as_ptr(),
                state.add(WPA_SM_SNONCE_OFFSET),
                supplicant_nonce.len(),
            );
            ptr::copy_nonoverlapping(
                authenticator_nonce.as_ptr(),
                state.add(WPA_SM_ANONCE_OFFSET),
                authenticator_nonce.len(),
            );
            state.add(WPA_SM_RENEW_SNONCE_OFFSET).cast::<u32>().write(0);
            ptr::copy_nonoverlapping(
                replay_counter.to_be_bytes().as_ptr(),
                state.add(WPA_SM_RX_REPLAY_OFFSET),
                8,
            );
            state
                .add(WPA_SM_RX_REPLAY_SET_OFFSET)
                .cast::<u32>()
                .write(1);
            state.add(WPA_SM_STATE_OFFSET).cast::<u32>().write(7);
            state.add(WPA_SM_KEY_INSTALL_OFFSET).write(1);
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Wpa2StaInstallError {
        SupplicantNotInitialized,
        UnexpectedConnectedSize(usize),
        UnexpectedDisconnectedSize(usize),
        UnexpectedConnectedCallback(usize),
        UnexpectedDisconnectedCallback(usize),
        UnexpectedHandshakeCallback(usize),
        PendingEvents,
    }

    unsafe fn callback_slot<T>(callbacks: *mut c_void, offset: usize) -> *mut T {
        callbacks.cast::<u8>().add(offset).cast::<T>()
    }

    #[no_mangle]
    unsafe extern "C" fn __esp_wifi_async_wpa2_sta_connected(bssid: *const u8) {
        if bssid.is_null() {
            REJECTED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut owned = [0; 6];
        owned.copy_from_slice(core::slice::from_raw_parts(bssid, 6));
        HANDSHAKE_ACTIVE.store(true, Ordering::Release);
        if EVENTS
            .try_send(Wpa2StaLinkEvent::Connected { bssid: owned })
            .is_err()
        {
            REJECTED.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[no_mangle]
    unsafe extern "C" fn __esp_wifi_async_wpa2_sta_disconnected(reason: u8) {
        HANDSHAKE_ACTIVE.store(false, Ordering::Release);
        if EVENTS
            .try_send(Wpa2StaLinkEvent::Disconnected { reason })
            .is_err()
        {
            REJECTED.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[no_mangle]
    unsafe extern "C" fn __esp_wifi_async_wpa2_sta_in_4way() -> bool {
        HANDSHAKE_ACTIVE.load(Ordering::Acquire)
    }

    /// Patch runtime STA state notifications after `esp_supplicant_init` and
    /// before association/RX can execute.
    ///
    /// # Safety
    /// The pinned audited S31 archive and
    /// `ld/esp32s31-wpa2-sta-locals.x` must be linked. Installation must be
    /// exclusive with callback execution and supplicant teardown.
    pub unsafe fn install_async_wpa2_sta_callbacks() -> Result<(), Wpa2StaInstallError> {
        if !EVENTS.is_empty() {
            return Err(Wpa2StaInstallError::PendingEvents);
        }
        let connected_size = (__esp_wpa_sta_connected_cb_end as *const () as usize)
            .wrapping_sub(__esp_wpa_sta_connected_cb as *const () as usize);
        // The pinned input sections are 0x08/0xc0 bytes. Flash placement can
        // relax AUIPC/JALR pairs to JAL, yielding the smaller final sizes.
        if connected_size != 0x08 && connected_size != 0x04 {
            return Err(Wpa2StaInstallError::UnexpectedConnectedSize(connected_size));
        }
        let disconnected_size = (__esp_wpa_sta_disconnected_cb_end as *const () as usize)
            .wrapping_sub(__esp_wpa_sta_disconnected_cb as *const () as usize);
        if disconnected_size != 0xc0 && disconnected_size != 0xa8 {
            return Err(Wpa2StaInstallError::UnexpectedDisconnectedSize(
                disconnected_size,
            ));
        }

        let callbacks = ptr::addr_of!(wpa_cb).read();
        if callbacks.is_null() {
            return Err(Wpa2StaInstallError::SupplicantNotInitialized);
        }
        let connected = callback_slot::<ConnectedCallback>(callbacks, WPA_STA_CONNECTED_OFFSET);
        let disconnected =
            callback_slot::<DisconnectedCallback>(callbacks, WPA_STA_DISCONNECTED_OFFSET);
        let handshake = callback_slot::<HandshakeCallback>(callbacks, WPA_STA_HANDSHAKE_OFFSET);
        let current_connected = connected.read() as usize;
        let current_disconnected = disconnected.read() as usize;
        let current_handshake = handshake.read() as usize;
        if current_connected != __esp_wpa_sta_connected_cb as ConnectedCallback as usize
            && current_connected
                != __esp_wifi_async_wpa2_sta_connected as ConnectedCallback as usize
        {
            return Err(Wpa2StaInstallError::UnexpectedConnectedCallback(
                current_connected,
            ));
        }
        if current_disconnected != __esp_wpa_sta_disconnected_cb as DisconnectedCallback as usize
            && current_disconnected
                != __esp_wifi_async_wpa2_sta_disconnected as DisconnectedCallback as usize
        {
            return Err(Wpa2StaInstallError::UnexpectedDisconnectedCallback(
                current_disconnected,
            ));
        }
        if current_handshake != wpa_sta_in_4way_handshake as HandshakeCallback as usize
            && current_handshake != __esp_wifi_async_wpa2_sta_in_4way as HandshakeCallback as usize
        {
            return Err(Wpa2StaInstallError::UnexpectedHandshakeCallback(
                current_handshake,
            ));
        }

        connected.write(__esp_wifi_async_wpa2_sta_connected);
        disconnected.write(__esp_wifi_async_wpa2_sta_disconnected);
        handshake.write(__esp_wifi_async_wpa2_sta_in_4way);
        HANDSHAKE_ACTIVE.store(false, Ordering::Release);
        INSTALLED.store(true, Ordering::Release);
        Ok(())
    }

    pub fn async_wpa2_sta_callbacks_installed() -> bool {
        INSTALLED.load(Ordering::Acquire)
    }
}

#[cfg(target_arch = "riscv32")]
pub use target::{
    async_wpa2_sta_callbacks_installed, copy_wpa2_sta_assoc_rsn_ie,
    copy_wpa2_sta_assoc_security_ies, install_async_wpa2_sta_callbacks, Wpa2StaInstallError,
};

#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
pub use target::publish_vendor_wpa2_sta_m2_diagnostic_state;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_state_is_explicit() {
        set_wpa2_sta_handshake_active(true);
        assert!(HANDSHAKE_ACTIVE.load(Ordering::Acquire));
        set_wpa2_sta_handshake_active(false);
        assert!(!HANDSHAKE_ACTIVE.load(Ordering::Acquire));
    }
}
