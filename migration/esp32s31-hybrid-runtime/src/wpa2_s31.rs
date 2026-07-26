//! Pinned ESP32-S31 static TX and CCMP backend.
//!
//! AP controlled-port state is owned by Rust because no independent
//! allocation-free authorization primitive is exported by the pinned blobs.

#![cfg_attr(not(any(test, target_arch = "riscv32")), allow(dead_code))]

use core::{
    cell::UnsafeCell,
    sync::atomic::{compiler_fence, AtomicBool, AtomicU8, Ordering},
};

#[cfg(target_arch = "riscv32")]
use core::{ffi::c_void, sync::atomic::AtomicUsize};

use crate::wpa2_crypto::WPA2_TK_LEN;

const VENDOR_KEY_PREFIX_LEN: usize = 0xa8;
const VENDOR_KEY_OBJECT_LEN: usize = VENDOR_KEY_PREFIX_LEN + WPA2_TK_LEN;
const KEY_INDEX_OFFSET: usize = 0x00;
const RECEIVE_SEQUENCE_OFFSET: usize = 0x98;
const CIPHER_POINTER_OFFSET: usize = 0xa0;
const KEY_LENGTH_OFFSET: usize = 0xa4;
const KEY_BYTES_OFFSET: usize = 0xa8;
const STA_PAIRWISE_HARDWARE_INDEX: u8 = 4;
const STA_GROUP_HARDWARE_INDEX: u8 = 1;
const AP_GROUP_HARDWARE_INDEX_BASE: u8 = 1;
const MAX_WPA2_GTK_ID: u8 = 3;
const CRYPTO_INTERFACE_COUNT: usize = 3;
#[cfg(target_arch = "riscv32")]
const MAX_VENDOR_KEY_INDEX: u8 = 24;

const fn crypto_control_address(interface: u32) -> Option<usize> {
    if interface < CRYPTO_INTERFACE_COUNT as u32 {
        Some(0x2010_4800 + interface as usize * 4)
    } else {
        None
    }
}

const fn crypto_enable_control(
    interface: u32,
    algorithm: u32,
    enable: u32,
    spp: u32,
) -> Option<(usize, u32, u32)> {
    let address = match crypto_control_address(interface) {
        Some(address) => address,
        None => return None,
    };
    let base = if interface == 2 && enable == 0 {
        0x0001_0000
    } else {
        0x0003_0000
    };
    let mut first = base | 0x103;
    if algorithm & 0xfb == 1 {
        first |= 0x1000_0000;
    }
    if spp != 0 {
        first |= 0x200;
    }
    let final_value = if algorithm == 4 {
        (first & 0x3fff_ffff) | 0x8000_0000
    } else {
        first & 0x3fff_ffff
    };
    Some((address, first, final_value))
}

#[cfg(target_arch = "riscv32")]
static STATIC_VENDOR_KEY_SLOTS: [AtomicUsize; MAX_VENDOR_KEY_INDEX as usize + 1] =
    [const { AtomicUsize::new(0) }; MAX_VENDOR_KEY_INDEX as usize + 1];

#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
static HIL_AP_WAITING_PEER: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];
#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
static HIL_AP_WAITING_PEER_VALID: AtomicBool = AtomicBool::new(false);

const fn hardware_key_direction(cipher: u32, hardware_index: u32) -> u32 {
    if cipher & 0x001c_0000 == 0x0004_0000 {
        7
    } else if hardware_index <= 3 {
        6
    } else {
        3
    }
}

const fn group_hardware_index(interface: crate::wpa2::Wpa2Interface, key_id: u8) -> Option<u8> {
    if key_id > MAX_WPA2_GTK_ID {
        return None;
    }
    match interface {
        crate::wpa2::Wpa2Interface::Station => Some(STA_GROUP_HARDWARE_INDEX),
        crate::wpa2::Wpa2Interface::AccessPoint => Some(AP_GROUP_HARDWARE_INDEX_BASE + key_id),
    }
}

#[repr(C, align(4))]
struct VendorCcmpKeyObject {
    bytes: [u8; VENDOR_KEY_OBJECT_LEN],
}

impl VendorCcmpKeyObject {
    const fn new() -> Self {
        Self {
            bytes: [0; VENDOR_KEY_OBJECT_LEN],
        }
    }

    fn wipe(&mut self) {
        for byte in &mut self.bytes {
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
        compiler_fence(Ordering::SeqCst);
    }

    fn initialize_like_wifi_init_key(&mut self) {
        self.bytes[..VENDOR_KEY_PREFIX_LEN].fill(0);
        self.bytes[8..0x98].fill(0xff);
    }
}

struct StaticVendorKeySlot {
    claimed: AtomicBool,
    hardware_index: AtomicU8,
    object: UnsafeCell<VendorCcmpKeyObject>,
}

impl StaticVendorKeySlot {
    const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            hardware_index: AtomicU8::new(u8::MAX),
            object: UnsafeCell::new(VendorCcmpKeyObject::new()),
        }
    }
}

unsafe impl Sync for StaticVendorKeySlot {}

struct StaticAuthorizedPeers<const N: usize> {
    peers: [Option<[u8; 6]>; N],
}

impl<const N: usize> StaticAuthorizedPeers<N> {
    const fn new() -> Self {
        Self {
            peers: [const { None }; N],
        }
    }

    fn contains(&self, peer: &[u8; 6]) -> bool {
        self.peers
            .iter()
            .any(|candidate| candidate.as_ref() == Some(peer))
    }

    fn any(&self) -> bool {
        self.peers.iter().any(Option::is_some)
    }

    fn set(&mut self, peer: [u8; 6], authorized: bool) -> Result<(), ()> {
        if let Some(slot) = self
            .peers
            .iter_mut()
            .find(|candidate| candidate.as_ref() == Some(&peer))
        {
            if !authorized {
                *slot = None;
            }
            return Ok(());
        }
        if !authorized {
            return Ok(());
        }
        let Some(slot) = self.peers.iter_mut().find(|candidate| candidate.is_none()) else {
            return Err(());
        };
        *slot = Some(peer);
        Ok(())
    }
}

struct StaticCancelledPeerGenerations<const N: usize> {
    peers: [Option<([u8; 6], usize)>; N],
}

impl<const N: usize> StaticCancelledPeerGenerations<N> {
    const fn new() -> Self {
        Self {
            peers: [const { None }; N],
        }
    }

    fn get(&self, peer: &[u8; 6]) -> Option<usize> {
        self.peers.iter().find_map(|candidate| {
            candidate
                .as_ref()
                .filter(|(candidate_peer, _)| candidate_peer == peer)
                .map(|(_, epoch)| *epoch)
        })
    }

    fn remove(&mut self, peer: &[u8; 6]) {
        if let Some(slot) = self.peers.iter_mut().find(|candidate| {
            candidate
                .as_ref()
                .is_some_and(|(candidate_peer, _)| candidate_peer == peer)
        }) {
            *slot = None;
        }
    }

    fn set(&mut self, peer: [u8; 6], epoch: usize) -> Result<(), ()> {
        if let Some(slot) = self.peers.iter_mut().find(|candidate| {
            candidate
                .as_ref()
                .is_some_and(|(candidate_peer, _)| *candidate_peer == peer)
        }) {
            *slot = Some((peer, epoch));
            return Ok(());
        }
        let Some(slot) = self.peers.iter_mut().find(|candidate| candidate.is_none()) else {
            return Err(());
        };
        *slot = Some((peer, epoch));
        Ok(())
    }

    fn retain(&mut self, mut keep: impl FnMut(&[u8; 6]) -> bool) {
        for slot in &mut self.peers {
            if slot.as_ref().is_some_and(|(peer, _)| !keep(peer)) {
                *slot = None;
            }
        }
    }
}

/// Stable-address storage referenced by the pinned net80211 key table.
pub struct S31StaticKeyStorage<const N: usize> {
    backend_taken: AtomicBool,
    slots: [StaticVendorKeySlot; N],
}

impl<const N: usize> S31StaticKeyStorage<N> {
    pub const fn new() -> Self {
        Self {
            backend_taken: AtomicBool::new(false),
            slots: [const { StaticVendorKeySlot::new() }; N],
        }
    }

    #[cfg(target_arch = "riscv32")]
    fn take_backend(&'static self) -> bool {
        self.backend_taken
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    #[cfg(any(test, target_arch = "riscv32"))]
    fn claim(&'static self, hardware_index: u8) -> Option<*mut VendorCcmpKeyObject> {
        for slot in &self.slots {
            if slot.claimed.load(Ordering::Acquire)
                && slot.hardware_index.load(Ordering::Acquire) == hardware_index
            {
                #[cfg(target_arch = "riscv32")]
                STATIC_VENDOR_KEY_SLOTS[usize::from(hardware_index)].store(
                    slot as *const StaticVendorKeySlot as usize,
                    Ordering::Release,
                );
                return Some(slot.object.get());
            }
        }
        for slot in &self.slots {
            if slot
                .claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                slot.hardware_index.store(hardware_index, Ordering::Release);
                #[cfg(target_arch = "riscv32")]
                STATIC_VENDOR_KEY_SLOTS[usize::from(hardware_index)].store(
                    slot as *const StaticVendorKeySlot as usize,
                    Ordering::Release,
                );
                return Some(slot.object.get());
            }
        }
        None
    }

    /// Wipe all slots after the Wi-Fi driver has been fully deinitialized.
    ///
    /// # Safety
    /// No vendor key-table pointer may reference this storage and no backend
    /// may be executing.
    pub unsafe fn reset_after_wifi_deinit(&'static self) {
        for slot in &self.slots {
            #[cfg(target_arch = "riscv32")]
            unregister_static_vendor_key_slot(slot);
            (*slot.object.get()).wipe();
            slot.hardware_index.store(u8::MAX, Ordering::Release);
            slot.claimed.store(false, Ordering::Release);
        }
        self.backend_taken.store(false, Ordering::Release);
    }
}

#[cfg(target_arch = "riscv32")]
fn unregister_static_vendor_key_slot(slot: &StaticVendorKeySlot) {
    let slot_address = slot as *const StaticVendorKeySlot as usize;
    for registered in &STATIC_VENDOR_KEY_SLOTS {
        let _ = registered.compare_exchange(slot_address, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn owned_static_vendor_key_object(hardware_index: u8) -> Option<*mut u8> {
    if hardware_index > MAX_VENDOR_KEY_INDEX {
        return None;
    }
    let slot_address = STATIC_VENDOR_KEY_SLOTS[usize::from(hardware_index)].load(Ordering::Acquire);
    if slot_address == 0 {
        return None;
    }
    let slot = &*(slot_address as *const StaticVendorKeySlot);
    (slot.claimed.load(Ordering::Acquire)
        && slot.hardware_index.load(Ordering::Acquire) == hardware_index)
        .then(|| slot.object.get().cast::<u8>())
}

#[cfg(target_arch = "riscv32")]
unsafe fn static_vendor_key_object_is_owned(hardware_index: u8, pointer: *mut c_void) -> bool {
    if pointer.is_null() {
        return false;
    }
    owned_static_vendor_key_object(hardware_index)
        .is_some_and(|object| core::ptr::eq(object.cast::<c_void>(), pointer))
}

/// Consume the vendor `free` performed after `ic_del_key` for a Rust-owned
/// static key object. The software-key table is cleared by the caller directly
/// after this callback returns; this only wipes and releases the backing slot.
///
/// # Safety
/// `pointer` must be the value passed by the serialized vendor key teardown on
/// the strict radio-owner stack.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn release_static_vendor_key_object(pointer: *mut c_void) -> bool {
    if pointer.is_null() {
        return false;
    }
    for registered in &STATIC_VENDOR_KEY_SLOTS {
        let slot_address = registered.load(Ordering::Acquire);
        if slot_address == 0 {
            continue;
        }
        let slot = &*(slot_address as *const StaticVendorKeySlot);
        if !core::ptr::eq(slot.object.get().cast::<c_void>(), pointer)
            || !slot.claimed.load(Ordering::Acquire)
        {
            continue;
        }
        if registered
            .compare_exchange(slot_address, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        (*slot.object.get()).wipe();
        slot.hardware_index.store(u8::MAX, Ordering::Release);
        slot.claimed.store(false, Ordering::Release);
        return true;
    }
    false
}

impl<const N: usize> Default for S31StaticKeyStorage<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum S31Wpa2IoError {
    StorageAlreadyTaken,
    NotRadioOwner,
    StaTxDoneCallbackMissing,
    TxLengthOverflow,
    TxPeerNotFound(u32),
    TxPeerPowerSaveUnsupported([u8; 6]),
    CachedTxRuntimeEnabled,
    StaticTxPoolExhausted,
    InvalidTxDescriptor,
    TxPostRejected(u32),
    VendorTxDiagnosticRejected(i32),
    TxBackendPoisoned,
    StaticKeySlotsFull,
    MissingApPeerHardwareIndex,
    InvalidGroupKeyId(u8),
    InvalidHardwareIndex(u8),
    ForeignSoftwareKeyPresent,
    MissingStaInterfaceState,
    UnexpectedStaPairwiseHardwareIndex,
    AuthorizationWithoutPairwiseKey,
    MissingApTransmitGroupKey,
    MissingApTransmitGroupNode,
    MissingApRateContext,
    StaPeerUnauthorized,
    ApPeerUnauthorized,
    AuthorizationSlotsFull,
    StaPeerMismatch,
    StaLinkResetBusy,
    InternalOwnershipMismatch,
    DataTxCreditMismatch,
}

/// Decide whether the radio-command owner must retain an AP data command
/// until a peer-bound active/PS-Poll edge.
///
/// Group traffic is deliberately excluded. It must cross the ESF ownership
/// boundary immediately so the bounded net80211 queue can retain several
/// frames and release the complete group FIFO on one Rust DTIM edge. Retrying
/// a group command here would wait on a nonexistent `ff:ff:ff:ff:ff:ff` peer
/// edge and serialize the FIFO to at most one frame per DTIM.
const fn ap_owner_power_save_retry(destination: &[u8; 6], sleeping: bool, flags: u32) -> bool {
    destination[0] & 1 == 0 && (sleeping || flags & 0x10 != 0)
}

#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HilStaPairwiseKeySnapshot {
    pub valid: bool,
    pub peer: [u8; 6],
    pub control: u16,
    pub key_matches: bool,
    pub crypto_gate: u16,
    pub node_hardware_index: u8,
    pub node_flags: u32,
    pub node_key_state: u8,
    pub station_privacy: u32,
    pub station_state: u8,
    pub global_connected: u8,
    pub global_auth_state: u8,
}

#[cfg(target_arch = "riscv32")]
mod target {
    use core::{ffi::c_void, mem::size_of, ptr};

    use esp_wifi_sys_esp32s31::include::{
        wifi_interface_t_WIFI_IF_AP, wifi_interface_t_WIFI_IF_STA,
    };

    use super::*;
    use crate::{
        command::PendingCommandAction,
        context::in_radio_context,
        data_rx::WifiDataInterface,
        data_tx::OwnedWifiDataTxFrame,
        wpa2::Wpa2Interface,
        wpa2_ap::WPA2_AP_ASSOC_CAPACITY,
        wpa2_io::{
            StaticWpa2Keys, TryWpa2Io, Wpa2IoCommand, Wpa2IoFailure, Wpa2KeyInstall, Wpa2KeyKind,
        },
        wpa2_txdone::async_wpa2_sta_tx_done_installed,
    };

    const CCMP_ALGORITHM: u32 = 3;
    const PAIRWISE_KEY_INDEX: u32 = 0;
    const CRYPTO_KEY_TABLE_BASE: usize = 0x2010_5800;
    const CRYPTO_KEY_ENTRY_STRIDE: usize = 40;
    const CRYPTO_KEY_VALID_BITMAP: *mut u32 = 0x2010_4814 as *mut u32;
    const CRYPTO_POLICY_CONTROL: *mut u32 = 0x2010_4810 as *mut u32;
    const MAX_HARDWARE_KEY_BYTES: usize = 32;
    unsafe extern "C" {
        static ccmp: [u8; 24];
        static mut g_ic: u8;
        static mut g_wifi_nvs: *mut u8;
        static mut g_sta_connected_flag: u8;
        static mut g_per_conn_trc: u8;
        #[cfg(feature = "hil-vendor-tx")]
        static mut gWpaSm: u8;

        #[link_name = "hal_crypto_set_key_entry"]
        fn linked_hal_crypto_set_key_entry(
            hardware_index: u32,
            key: *const u8,
            key_length: usize,
            metadata: *const u8,
        );

        fn cnx_node_search(peer: *const u8) -> *mut u8;
        #[link_name = "__real_cnx_node_alloc"]
        fn initialization_cnx_node_alloc(peer: *const u8) -> *mut u8;
        fn cnx_bss_init(node: *mut u8, interface: *mut u8);
        fn ieee80211_search_node(interface: u32, frame: *const u8, error: *mut u32) -> *mut u8;
        fn ieee80211_is_tx_allowed(node: *mut u8, authentication_frame: bool) -> bool;
        fn esf_buf_alloc(frame: *const u8, kind: u32, length: u32) -> *mut u8;
        fn ieee80211_post_hmac_tx(buffer: *mut u8) -> u32;
        fn ic_del_key(hardware_index: u32);
        #[cfg(feature = "hil-vendor-tx")]
        fn ieee80211_output_do(
            interface: u32,
            frame: *const u8,
            length: u32,
            flags: u32,
            netstack_buffer: *mut c_void,
        ) -> i32;
        #[cfg(feature = "hil-vendor-tx")]
        fn wpa_ether_send(
            state: *mut c_void,
            destination: *const u8,
            protocol: u16,
            data: *mut u8,
            data_len: usize,
        ) -> i32;
        fn ic_set_key(
            interface: u32,
            algorithm: u32,
            key_index: u32,
            peer: *const u8,
            hardware_index: u32,
            key: *const u8,
            key_length: usize,
            enable: u32,
            spp: u32,
        );
        #[cfg(feature = "hil-vendor-tx")]
        fn ets_printf(format: *const u8, ...) -> i32;
    }

    const SEARCH_ERROR_CACHED_TX_ENABLED: u32 = 0x3002;
    const SEARCH_ERROR_INVALID_INTERFACE: u32 = 0x3004;
    const SEARCH_ERROR_INTERFACE_NOT_RUNNING: u32 = 0x3006;
    const SEARCH_ERROR_INTERFACE_MISSING: u32 = 0x3007;
    const SEARCH_ERROR_NODE_MISSING: u32 = 0x3015;
    const SEARCH_ERROR_TX_DISALLOWED: u32 = 0x3016;
    const INTERFACE_STATE_OFFSET: usize = 0x98;
    const INTERFACE_PRIMARY_NODE_OFFSET: usize = 0xec;
    const STA_NODE_OFFSET: usize = 0xe4;
    const NODE_FLAGS_OFFSET: usize = 0x0c;
    const NODE_ASSOCIATION_ID_OFFSET: usize = 0x26;
    const MAX_CONNECTION_INDEX_OFFSET: usize = 0x3f6;
    const AP_NODE_LEN: usize = 0x510;
    const AP_NODE_HARDWARE_INDEX_OFFSET: usize = 0x134;
    const AP_NODE_BITMAP_OFFSET: usize = 0x118;
    const RATE_CONTEXT_PRIMARY_RATE_OFFSET: usize = 0x08;
    const RATE_CONTEXT_SECONDARY_RATE_OFFSET: usize = 0x09;
    const RATE_CONTEXT_MODE_OFFSET: usize = 0x0c;
    const RATE_CONTEXT_PRIMARY_SCHEDULE_OFFSET: usize = 0x64;
    const RATE_CONTEXT_SECONDARY_SCHEDULE_OFFSET: usize = 0x68;
    const AP_FIRST_PEER_RATE_CONTEXT: usize = 1;
    const AP_LAST_PEER_RATE_CONTEXT: usize = 16;
    const AP_DEFAULT_RATE_CONTEXT: usize = 20;

    fn station_interface() -> *mut u8 {
        crate::net80211_state::station_interface()
            .map(|interface| interface.as_ptr())
            .unwrap_or(ptr::null_mut())
    }

    fn access_point_interface() -> *mut u8 {
        crate::net80211_state::access_point_interface()
            .map(|interface| interface.as_ptr())
            .unwrap_or(ptr::null_mut())
    }

    #[repr(C, align(4))]
    struct StaticApNode {
        bytes: [u8; AP_NODE_LEN],
    }

    struct StaticApNodeSlot {
        claimed: AtomicBool,
        node: UnsafeCell<StaticApNode>,
    }

    impl StaticApNodeSlot {
        const fn new() -> Self {
            Self {
                claimed: AtomicBool::new(false),
                node: UnsafeCell::new(StaticApNode {
                    bytes: [0; AP_NODE_LEN],
                }),
            }
        }
    }

    unsafe impl Sync for StaticApNodeSlot {}

    #[link_section = ".critical.bss.wifi_strict.ap_nodes"]
    static AP_NODES: [StaticApNodeSlot; WPA2_AP_ASSOC_CAPACITY] =
        [const { StaticApNodeSlot::new() }; WPA2_AP_ASSOC_CAPACITY];

    unsafe fn set_search_error(error: *mut u32, value: u32) {
        if !error.is_null() {
            error.write(value);
        }
    }

    unsafe fn strict_ap_node_search(peer: *const u8) -> *mut u8 {
        if peer.is_null() {
            return ptr::null_mut();
        }
        let interface = access_point_interface();
        if interface.is_null() {
            return ptr::null_mut();
        }
        if peer.read() & 1 != 0 {
            return interface
                .add(INTERFACE_PRIMARY_NODE_OFFSET)
                .cast::<*mut u8>()
                .read_volatile();
        }
        let config = ptr::addr_of_mut!(g_wifi_nvs).read_volatile();
        if config.is_null() {
            return ptr::null_mut();
        }

        // The pinned blob increments an eight-bit index and can spin forever
        // when the configured limit is 255. Strict AP owns eight peer slots;
        // the extra entry is the interface/BSS node. Bound the identical
        // contiguous lookup to those statically provisioned entries.
        let configured_limit = usize::from(config.add(MAX_CONNECTION_INDEX_OFFSET).read_volatile());
        let slots = WPA2_AP_ASSOC_CAPACITY + 1;
        let mut index = 0_usize;
        while index < slots && index <= configured_limit {
            let node = interface
                .add(INTERFACE_PRIMARY_NODE_OFFSET + index * size_of::<*mut u8>())
                .cast::<*mut u8>()
                .read_volatile();
            if !node.is_null() {
                let mut byte = 0_usize;
                while byte < 6 && node.add(4 + byte).read_volatile() == peer.add(byte).read() {
                    byte += 1;
                }
                if byte == 6 {
                    return node;
                }
            }
            index += 1;
        }
        ptr::null_mut()
    }

    /// Finite replacement for the pinned AP node-table search.
    ///
    /// The final strict image must link with `-Wl,--wrap=cnx_node_search`.
    #[no_mangle]
    pub unsafe extern "C" fn __wrap_cnx_node_search(peer: *const u8) -> *mut u8 {
        strict_ap_node_search(peer)
    }

    /// Allocate an AP connection node from the fixed SRAM pool.
    ///
    /// The pinned allocator requests `0x510` bytes through the OSI heap on the
    /// first Open System Authentication frame. Strict mode instead claims the
    /// node corresponding to the bounded vendor table index, initializes it
    /// with the same finite constructor, and publishes the pointer only after
    /// initialization is complete.
    #[no_mangle]
    pub unsafe extern "C" fn __wrap_cnx_node_alloc(peer: *const u8) -> *mut u8 {
        if !crate::critical::strict_wifi_hart_armed() {
            return initialization_cnx_node_alloc(peer);
        }
        if peer.is_null()
            || peer.read() & 1 != 0
            || !crate::critical::on_strict_wifi_hart()
            || !crate::context::in_radio_context()
        {
            return ptr::null_mut();
        }
        let interface = access_point_interface();
        let config = ptr::addr_of_mut!(g_wifi_nvs).read_volatile();
        if interface.is_null() || config.is_null() {
            return ptr::null_mut();
        }

        let configured_limit = usize::from(config.add(MAX_CONNECTION_INDEX_OFFSET).read_volatile());
        let mut index = 1_usize;
        while index <= WPA2_AP_ASSOC_CAPACITY && index <= configured_limit {
            let table_entry = interface
                .add(INTERFACE_PRIMARY_NODE_OFFSET + index * size_of::<*mut u8>())
                .cast::<*mut u8>();
            if table_entry.read_volatile().is_null() {
                let slot = &AP_NODES[index - 1];
                if slot
                    .claimed
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return ptr::null_mut();
                }
                let node = slot.node.get().cast::<u8>();
                cnx_bss_init(node, interface);
                node.add(AP_NODE_HARDWARE_INDEX_OFFSET)
                    .write((index + 7) as u8);
                ptr::copy_nonoverlapping(peer, node.add(4), 6);
                table_entry.write_volatile(node);
                let bitmap = interface.add(AP_NODE_BITMAP_OFFSET).cast::<u32>();
                bitmap.write_volatile(bitmap.read_volatile() | (1_u32 << index));
                return node;
            }
            index += 1;
        }
        ptr::null_mut()
    }

    /// Return a vendor-released AP connection node to the fixed SRAM pool.
    ///
    /// The pinned node teardown clears the interface table before calling the
    /// OSI `free` callback. Strict mode must consume that `free` locally: the
    /// object was never heap-backed, but its bounded pool slot must become
    /// available for a later association.
    ///
    /// # Safety
    /// The caller must have received `node` from the vendor teardown after all
    /// table and hardware references to it were removed.
    pub(crate) unsafe fn release_static_ap_node(node: *mut c_void) -> bool {
        for slot in &AP_NODES {
            if ptr::eq(slot.node.get().cast::<c_void>(), node)
                && slot.claimed.load(Ordering::Acquire)
            {
                ptr::write_bytes(slot.node.get().cast::<u8>(), 0, AP_NODE_LEN);
                slot.claimed.store(false, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// STA/AP-only replacement for the path-insensitive vendor node lookup.
    ///
    /// NAN is unsupported by the strict runtime and is rejected without
    /// entering its assert loop. The final strict image must link with
    /// `-Wl,--wrap=ieee80211_search_node`.
    #[no_mangle]
    pub unsafe extern "C" fn __wrap_ieee80211_search_node(
        interface: u32,
        frame: *const u8,
        error: *mut u32,
    ) -> *mut u8 {
        if !crate::net80211_state::ordinary_sta_ap_profile() {
            set_search_error(error, SEARCH_ERROR_CACHED_TX_ENABLED);
            return ptr::null_mut();
        }
        let interface_state = if interface == wifi_interface_t_WIFI_IF_STA {
            station_interface()
        } else if interface == wifi_interface_t_WIFI_IF_AP {
            access_point_interface()
        } else {
            set_search_error(error, SEARCH_ERROR_INVALID_INTERFACE);
            return ptr::null_mut();
        };
        if interface_state.is_null() {
            set_search_error(error, SEARCH_ERROR_INTERFACE_MISSING);
            return ptr::null_mut();
        }
        if interface_state
            .add(INTERFACE_STATE_OFFSET)
            .cast::<u32>()
            .read_volatile()
            != 5
        {
            set_search_error(error, SEARCH_ERROR_INTERFACE_NOT_RUNNING);
            return ptr::null_mut();
        }

        let node = if interface == wifi_interface_t_WIFI_IF_STA {
            interface_state
                .add(STA_NODE_OFFSET)
                .cast::<*mut u8>()
                .read_volatile()
        } else if frame.is_null() {
            ptr::null_mut()
        } else {
            strict_ap_node_search(frame)
        };
        if node.is_null()
            || (node
                .add(NODE_ASSOCIATION_ID_OFFSET)
                .cast::<u16>()
                .read_volatile()
                == 0
                && node.add(NODE_FLAGS_OFFSET).cast::<u32>().read_volatile() & 0x0002_0000 != 0)
        {
            set_search_error(error, SEARCH_ERROR_NODE_MISSING);
            return ptr::null_mut();
        }

        let authentication_frame = if frame.is_null() {
            false
        } else {
            let protocol = u16::from_be_bytes([frame.add(12).read(), frame.add(13).read()]);
            matches!(protocol, 0x888e | 0x88b4)
        };
        if !ieee80211_is_tx_allowed(node, authentication_frame) {
            set_search_error(error, SEARCH_ERROR_TX_DISALLOWED);
            return ptr::null_mut();
        }
        node
    }

    pub(crate) fn runtime_key_link_wrapper_active() -> bool {
        core::ptr::eq(
            linked_hal_crypto_set_key_entry as *const (),
            __wrap_hal_crypto_set_key_entry as *const (),
        )
    }

    #[cfg(feature = "hil-vendor-tx")]
    pub fn hil_sta_pairwise_key_snapshot(
        expected_tk: &[u8; WPA2_TK_LEN],
    ) -> HilStaPairwiseKeySnapshot {
        unsafe {
            let entry = (CRYPTO_KEY_TABLE_BASE
                + usize::from(STA_PAIRWISE_HARDWARE_INDEX) * CRYPTO_KEY_ENTRY_STRIDE)
                as *const u8;
            let peer_low = entry.cast::<u32>().read_volatile().to_le_bytes();
            let peer_control = entry.add(4).cast::<u32>().read_volatile();
            let peer_high = peer_control.to_le_bytes();
            let mut peer = [0; 6];
            peer[..4].copy_from_slice(&peer_low);
            peer[4..].copy_from_slice(&peer_high[..2]);
            let mut key_matches = true;
            let mut index = 0;
            while index < expected_tk.len() {
                key_matches &= entry.add(8 + index).read_volatile() == expected_tk[index];
                index += 1;
            }
            let station = sta_interface_state();
            let node = sta_interface_node();
            HilStaPairwiseKeySnapshot {
                valid: CRYPTO_KEY_VALID_BITMAP.read_volatile()
                    & (1_u32 << STA_PAIRWISE_HARDWARE_INDEX)
                    != 0,
                peer,
                control: (peer_control >> 16) as u16,
                key_matches,
                crypto_gate: ptr::addr_of_mut!(g_ic)
                    .add(0x210)
                    .cast::<u16>()
                    .read_volatile(),
                node_hardware_index: if node.is_null() {
                    u8::MAX
                } else {
                    node.add(0x134).read_volatile()
                },
                node_flags: if node.is_null() {
                    0
                } else {
                    node.add(0x0c).cast::<u32>().read_volatile()
                },
                node_key_state: if node.is_null() {
                    u8::MAX
                } else {
                    node.add(0x24).read_volatile()
                },
                station_privacy: if station.is_null() {
                    0
                } else {
                    station.add(0xa4).cast::<u32>().read_volatile()
                },
                station_state: if station.is_null() {
                    u8::MAX
                } else {
                    station.add(0x140).read_volatile()
                },
                global_connected: ptr::addr_of_mut!(g_sta_connected_flag).read_volatile(),
                global_auth_state: ptr::addr_of_mut!(g_ic).add(0x274).read_volatile(),
            }
        }
    }

    unsafe fn software_key_slot(hardware_index: u8) -> Option<*mut *mut c_void> {
        if hardware_index > MAX_VENDOR_KEY_INDEX {
            return None;
        }
        Some(
            ptr::addr_of_mut!(g_ic)
                .add(0x148 + usize::from(hardware_index) * 4)
                .cast::<*mut c_void>(),
        )
    }

    unsafe fn ap_pairwise_hardware_index(peer: *const u8) -> u8 {
        let node = cnx_node_search(peer);
        if node.is_null() {
            0
        } else {
            node.add(0x134).read()
        }
    }

    unsafe fn peer_spp(interface: Wpa2Interface, peer: *const u8) -> u8 {
        let node = match interface {
            Wpa2Interface::AccessPoint => cnx_node_search(peer),
            Wpa2Interface::Station => {
                let interface = station_interface();
                if interface.is_null() {
                    return 0;
                }
                interface.add(0xe4).cast::<*mut u8>().read()
            }
        };
        if node.is_null() {
            0
        } else {
            node.add(0x2f8).read()
        }
    }

    unsafe fn sta_interface_state() -> *mut u8 {
        station_interface()
    }

    unsafe fn sta_interface_node() -> *mut u8 {
        let interface = sta_interface_state();
        if interface.is_null() {
            return ptr::null_mut();
        }
        interface.add(0xe4).cast::<*mut u8>().read()
    }

    unsafe fn read_metadata(metadata: *const u8, offset: usize) -> u32 {
        u32::from(metadata.add(offset).read())
    }

    unsafe fn write_key_word(
        destination: *mut u32,
        key: *const u8,
        key_length: usize,
        word: usize,
    ) {
        let offset = word * 4;
        if offset >= key_length {
            return;
        }
        let mut value = 0_u32;
        if offset < key_length {
            value |= u32::from(key.add(offset).read());
        }
        if offset + 1 < key_length {
            value |= u32::from(key.add(offset + 1).read()) << 8;
        }
        if offset + 2 < key_length {
            value |= u32::from(key.add(offset + 2).read()) << 16;
        }
        if offset + 3 < key_length {
            value |= u32::from(key.add(offset + 3).read()) << 24;
        }
        destination.add(word).write_volatile(value);
    }

    unsafe fn clear_hardware_key_entry(hardware_index: u32) {
        let valid = CRYPTO_KEY_VALID_BITMAP.read_volatile();
        CRYPTO_KEY_VALID_BITMAP.write_volatile(valid & !(1_u32 << hardware_index));
        let entry =
            (CRYPTO_KEY_TABLE_BASE + hardware_index as usize * CRYPTO_KEY_ENTRY_STRIDE) as *mut u32;
        let mut word = 0;
        while word < CRYPTO_KEY_ENTRY_STRIDE / size_of::<u32>() {
            entry.add(word).write_volatile(0);
            word += 1;
        }
    }

    unsafe fn enable_hardware_crypto(
        interface: u32,
        algorithm: u32,
        enable: u32,
        spp: u32,
    ) -> bool {
        let Some((control_address, first_control, final_control)) =
            crypto_enable_control(interface, algorithm, enable, spp)
        else {
            return false;
        };
        let control = control_address as *mut u32;
        control.write_volatile(first_control);
        let policy = CRYPTO_POLICY_CONTROL.read_volatile();
        if algorithm == 4 {
            CRYPTO_POLICY_CONTROL.write_volatile(policy | 0x003f_ffc0);
        } else {
            CRYPTO_POLICY_CONTROL.write_volatile(policy & 0xffc0_003f);
        }
        let current = control.read_volatile();
        let final_value = if algorithm == 4 {
            (current & 0x3fff_ffff) | 0x8000_0000
        } else {
            current & 0x3fff_ffff
        };
        debug_assert_eq!(final_value, final_control);
        control.write_volatile(final_value);
        true
    }

    /// Complete Rust replacement for
    /// `libpp.a[wdev.o]::wDev_Insert_KeyEntry`.
    ///
    /// The nine-byte metadata record, key-table writes and crypto-enable MMIO
    /// follow the pinned `0x8e` body. Its legacy `wDevCtrl` bitmap write is
    /// intentionally absent: `StaticWpa2Keys` and the static vendor-slot
    /// tokens are already the single Rust owner, while removal always clears
    /// the explicit hardware index. Mirroring that unused C teardown cache
    /// would add a second ownership ledger.
    #[no_mangle]
    pub unsafe extern "C" fn wifi_strict_wdev_insert_key_entry(
        algorithm: u32,
        interface: u32,
        logical_key_index: u32,
        peer: *const u8,
        hardware_index: u32,
        key: *const u8,
        key_length: usize,
        enable: u32,
        spp: u32,
    ) {
        if interface >= CRYPTO_INTERFACE_COUNT as u32
            || algorithm > u8::MAX as u32
            || logical_key_index > u8::MAX as u32
            || hardware_index > u32::from(MAX_VENDOR_KEY_INDEX)
            || peer.is_null()
            || key.is_null()
            || key_length == 0
            || key_length > MAX_HARDWARE_KEY_BYTES
        {
            return;
        }

        let mut metadata = [0_u8; 9];
        metadata[0] = interface as u8;
        metadata[1] = algorithm as u8;
        metadata[2] = logical_key_index as u8;
        ptr::copy_nonoverlapping(peer, metadata.as_mut_ptr().add(3), 6);
        __wrap_hal_crypto_set_key_entry(hardware_index, key, key_length, metadata.as_ptr());
        let _ = enable_hardware_crypto(interface, algorithm, enable, spp & 0xff);
    }

    /// Complete Rust replacement for `libpp.a[if_hwctrl.o]::ic_set_key`.
    ///
    /// The C `if_ctrl[interface].ptk_alg/gtk_alg` compatibility cache is not
    /// mirrored: the admitted WPA2 path carries cipher and key kind in its
    /// typed Rust key object. All hardware and ownership inputs are validated
    /// before MMIO is changed.
    #[no_mangle]
    pub unsafe extern "C" fn wifi_strict_ic_set_key(
        interface: u32,
        algorithm: u32,
        logical_key_index: u32,
        peer: *const u8,
        hardware_index: u32,
        key: *const u8,
        key_length: usize,
        enable: u32,
        spp: u32,
    ) {
        if interface >= CRYPTO_INTERFACE_COUNT as u32
            || algorithm > u8::MAX as u32
            || logical_key_index > u8::MAX as u32
            || hardware_index > u32::from(MAX_VENDOR_KEY_INDEX)
            || peer.is_null()
            || key.is_null()
            || key_length == 0
            || key_length > MAX_HARDWARE_KEY_BYTES
        {
            return;
        }

        wifi_strict_wdev_insert_key_entry(
            algorithm,
            interface,
            logical_key_index,
            peer,
            hardware_index,
            key,
            key_length,
            enable,
            spp,
        );
    }

    /// Complete Rust replacement for `libpp.a[if_hwctrl.o]::ic_del_key`.
    ///
    /// It performs the complete pinned `hal_crypto_clr_key_entry` bitmap
    /// update plus ten zeroing MMIO stores. The typed caller releases its
    /// static software-key token separately, so this leaf neither guesses nor
    /// duplicates software ownership. Invalid indices fail before any shift
    /// or address calculation.
    #[no_mangle]
    pub unsafe extern "C" fn wifi_strict_ic_del_key(hardware_index: u32) {
        if hardware_index > u32::from(MAX_VENDOR_KEY_INDEX) {
            return;
        }
        clear_hardware_key_entry(hardware_index);
    }

    /// Allocation-free replacement for the pinned `0x1c2`-byte HAL leaf.
    ///
    /// The stock implementation allocates a temporary buffer solely when the
    /// key pointer is not word-aligned. This replacement programs the same
    /// fixed key-table MMIO a byte at a time and supports the complete hardware
    /// maximum of 32 key bytes, so neither pointer alignment nor a future
    /// caller can expose the allocator branch. The surrounding pinned
    /// `wDev_Insert_KeyEntry` still performs its bookkeeping and calls
    /// `hal_crypto_enable`.
    ///
    /// The final firmware must link with
    /// `-Wl,--wrap=hal_crypto_set_key_entry`.
    #[no_mangle]
    pub unsafe extern "C" fn __wrap_hal_crypto_set_key_entry(
        hardware_index: u32,
        key: *const u8,
        key_length: usize,
        metadata: *const u8,
    ) {
        if hardware_index > u32::from(MAX_VENDOR_KEY_INDEX)
            || key.is_null()
            || metadata.is_null()
            || key_length == 0
            || key_length > MAX_HARDWARE_KEY_BYTES
        {
            return;
        }

        let interface = read_metadata(metadata, 0);
        let algorithm = read_metadata(metadata, 1);
        let logical_key_index = read_metadata(metadata, 2);
        let peer_low = read_metadata(metadata, 3)
            | (read_metadata(metadata, 4) << 8)
            | (read_metadata(metadata, 5) << 16)
            | (read_metadata(metadata, 6) << 24);
        let peer_high = read_metadata(metadata, 7) | (read_metadata(metadata, 8) << 8);

        let cipher = if algorithm == 5 {
            0x0005_0000
        } else if algorithm == 9 {
            0x0014_0000
                | if key_length == MAX_HARDWARE_KEY_BYTES {
                    0x0400_0000
                } else {
                    0
                }
        } else {
            (algorithm & 7) << 18
        };
        let direction = hardware_key_direction(cipher, hardware_index);
        let control = ((interface & 3) << 8)
            | (direction << 5)
            | (u32::from(logical_key_index != 3) << 11)
            | (logical_key_index << 14)
            | ((cipher >> 16) & 0x341f);

        let entry =
            (CRYPTO_KEY_TABLE_BASE + hardware_index as usize * CRYPTO_KEY_ENTRY_STRIDE) as *mut u32;
        entry.write_volatile(peer_low);
        entry.add(1).write_volatile(peer_high | (control << 16));
        let key_destination = entry.add(2);
        write_key_word(key_destination, key, key_length, 0);
        write_key_word(key_destination, key, key_length, 1);
        write_key_word(key_destination, key, key_length, 2);
        write_key_word(key_destination, key, key_length, 3);
        write_key_word(key_destination, key, key_length, 4);
        write_key_word(key_destination, key, key_length, 5);
        write_key_word(key_destination, key, key_length, 6);
        write_key_word(key_destination, key, key_length, 7);

        let valid = CRYPTO_KEY_VALID_BITMAP.read_volatile();
        CRYPTO_KEY_VALID_BITMAP.write_volatile(valid | (1_u32 << hardware_index));
    }

    pub struct S31StaticWpa2Io<const K: usize> {
        storage: &'static S31StaticKeyStorage<K>,
        keys: StaticWpa2Keys<K>,
        sta_authorized_peer: Option<[u8; 6]>,
        authorized_peers: StaticAuthorizedPeers<K>,
        tx_poisoned: bool,
        ap_active_epoch: usize,
        ap_ps_poll_epoch: usize,
        ap_removal_epoch: usize,
        ap_waiting_association_epoch: usize,
        ap_waiting_peer: [u8; 6],
        ap_retry_armed: bool,
        ap_cancelled_peers: StaticCancelledPeerGenerations<WPA2_AP_ASSOC_CAPACITY>,
        #[cfg(feature = "hil-vendor-tx")]
        vendor_tx_diagnostic: bool,
        #[cfg(feature = "hil-vendor-tx")]
        vendor_wpa_tx_diagnostic: bool,
    }

    impl<const K: usize> S31StaticWpa2Io<K> {
        /// Construct a backend after strict runtime preparation.
        ///
        /// # Safety
        /// `proof` must belong to the active driver. Controlled hardware key
        /// indices must contain no foreign heap object. The backend and static
        /// storage must remain exclusive until Wi-Fi deinitialization. No RX
        /// fragment assembled under an old key may be live when a key-install
        /// command is submitted; the stock fragment cleanup can call logging
        /// and is deliberately excluded from this strict backend.
        pub unsafe fn new(
            storage: &'static S31StaticKeyStorage<K>,
            _proof: &crate::policy::StrictRuntimeProof,
        ) -> Result<Self, S31Wpa2IoError> {
            if !storage.take_backend() {
                return Err(S31Wpa2IoError::StorageAlreadyTaken);
            }
            Ok(Self {
                storage,
                keys: StaticWpa2Keys::new(),
                sta_authorized_peer: None,
                authorized_peers: StaticAuthorizedPeers::new(),
                tx_poisoned: false,
                ap_active_epoch: 0,
                ap_ps_poll_epoch: 0,
                ap_removal_epoch: 0,
                ap_waiting_association_epoch: 0,
                ap_waiting_peer: [0; 6],
                ap_retry_armed: false,
                ap_cancelled_peers: StaticCancelledPeerGenerations::new(),
                #[cfg(feature = "hil-vendor-tx")]
                vendor_tx_diagnostic: false,
                #[cfg(feature = "hil-vendor-tx")]
                vendor_wpa_tx_diagnostic: false,
            })
        }

        /// Select one finite, prebuilt TX schedule for every AP connection.
        ///
        /// This is the allocation-free body reached by the pinned
        /// `rc_set_fix_rate(AP, true, rate)` path after its ioctl/task wrapper.
        /// Every live peer context in 1..=16 and the mandatory AP
        /// default/group context 20 are validated in full before the first
        /// write. Null unassociated peer slots are skipped exactly like the
        /// recovered vendor leaf. Both primary and secondary fixed-rate bits
        /// are enabled, so `rcGetSched` never enters adaptive rate control for
        /// a context owned at takeover.
        ///
        /// # Safety
        ///
        /// Vendor `trc_init` must have completed, and the caller must own the
        /// radio runtime so no concurrent vendor task can mutate the contexts.
        pub unsafe fn configure_ap_fixed_rate(&mut self, rate: u8) -> Result<(), S31Wpa2IoError> {
            let table = ptr::addr_of_mut!(g_per_conn_trc);

            let context_at = |index: usize| {
                table
                    .add(index * size_of::<*mut u8>())
                    .cast::<*mut u8>()
                    .read()
            };
            let validate = |context: *mut u8| {
                let primary = context
                    .add(RATE_CONTEXT_PRIMARY_SCHEDULE_OFFSET)
                    .cast::<*mut u8>()
                    .read_unaligned();
                let secondary = context
                    .add(RATE_CONTEXT_SECONDARY_SCHEDULE_OFFSET)
                    .cast::<*mut u8>()
                    .read_unaligned();
                !primary.is_null() && !secondary.is_null()
            };

            for index in AP_FIRST_PEER_RATE_CONTEXT..=AP_LAST_PEER_RATE_CONTEXT {
                let context = context_at(index);
                if !context.is_null() && !validate(context) {
                    return Err(S31Wpa2IoError::MissingApRateContext);
                }
            }
            let default_context = context_at(AP_DEFAULT_RATE_CONTEXT);
            if default_context.is_null() || !validate(default_context) {
                return Err(S31Wpa2IoError::MissingApRateContext);
            }

            let apply = |context: *mut u8| {
                context.add(RATE_CONTEXT_PRIMARY_RATE_OFFSET).write(rate);
                context.add(RATE_CONTEXT_SECONDARY_RATE_OFFSET).write(rate);
                let mode = context.add(RATE_CONTEXT_MODE_OFFSET).cast::<u16>();
                mode.write_unaligned(mode.read_unaligned() | 0x03);
            };
            for index in AP_FIRST_PEER_RATE_CONTEXT..=AP_LAST_PEER_RATE_CONTEXT {
                let context = context_at(index);
                if !context.is_null() {
                    apply(context);
                }
            }
            apply(default_context);
            Ok(())
        }

        /// Route HIL frames through the stock `ieee80211_output_do` oracle.
        ///
        /// This intentionally reintroduces the vendor global-lock callbacks
        /// and is only for locating differences in the reconstructed TX leaf.
        #[cfg(feature = "hil-vendor-tx")]
        pub unsafe fn enable_vendor_tx_diagnostic(&mut self) {
            self.vendor_tx_diagnostic = true;
        }

        /// Route HIL EAPOL through the complete stock `wpa_ether_send` leaf.
        ///
        /// This is strictly a differential oracle: it reintroduces the stock
        /// global-lock TX wrapper and must never be enabled in production.
        #[cfg(feature = "hil-vendor-tx")]
        pub unsafe fn enable_vendor_wpa_tx_diagnostic(&mut self) {
            self.vendor_wpa_tx_diagnostic = true;
        }

        pub const fn keys(&self) -> &StaticWpa2Keys<K> {
            &self.keys
        }

        /// Controlled-port gate for Rust-owned AP data channels.
        ///
        /// EAPOL ingress is intentionally handled separately. Every ordinary
        /// AP data frame must be checked here immediately before enqueue or
        /// transmit, so a prior authorization cannot be retained as a stale
        /// capability after deauthorization.
        pub fn is_ap_peer_authorized(&self, peer: &[u8; 6]) -> bool {
            self.authorized_peers.contains(peer)
        }

        /// Controlled-port state for the single associated STA peer.
        pub fn is_sta_peer_authorized(&self) -> bool {
            self.sta_authorized_peer.is_some()
        }

        fn has_ap_transmit_group_key(&self) -> bool {
            self.ap_transmit_group_hardware_index().is_some()
        }

        fn cancel_if_ap_peer_removed(&mut self, interface: Wpa2Interface, peer: &[u8; 6]) -> bool {
            if interface != Wpa2Interface::AccessPoint || peer[0] & 1 != 0 {
                return false;
            }
            let association_epoch = crate::wpa2_ap::wpa2_ap_peer_association_epoch(peer);
            if let Some(cancelled_epoch) = self.ap_cancelled_peers.get(peer) {
                if association_epoch.is_some_and(|epoch| epoch != cancelled_epoch) {
                    self.ap_cancelled_peers.remove(peer);
                    return false;
                }
            } else if association_epoch.is_some() {
                return false;
            }
            let _ = self.authorized_peers.set(*peer, false);
            crate::ap_power_save::record_cancelled_transmit();
            true
        }

        fn ap_transmit_group_hardware_index(&self) -> Option<u8> {
            (0..K).find_map(|index| {
                let key = self.keys.get(index)?;
                match key.kind() {
                    Wpa2KeyKind::Group {
                        key_id,
                        transmit: true,
                    } if key.interface() == Wpa2Interface::AccessPoint => {
                        group_hardware_index(Wpa2Interface::AccessPoint, key_id)
                    }
                    _ => None,
                }
            })
        }

        /// Submit one frame received from the fixed application TX channel.
        ///
        /// This performs one immediate static-pool attempt. AP traffic is
        /// checked against the current controlled-port table at the point of
        /// submission, so queueing a frame cannot retain stale authorization.
        pub fn try_transmit_wifi_data(
            &mut self,
            frame: &OwnedWifiDataTxFrame,
        ) -> Result<(), S31Wpa2IoError> {
            if !in_radio_context() {
                return Err(S31Wpa2IoError::NotRadioOwner);
            }
            let interface = match frame.interface() {
                WifiDataInterface::Station => Wpa2Interface::Station,
                WifiDataInterface::AccessPoint => Wpa2Interface::AccessPoint,
            };
            match interface {
                Wpa2Interface::Station if !self.is_sta_peer_authorized() => {
                    return Err(S31Wpa2IoError::StaPeerUnauthorized);
                }
                Wpa2Interface::AccessPoint => {
                    let destination = frame.destination();
                    let authorized = if destination[0] & 1 != 0 {
                        self.authorized_peers.any() && self.has_ap_transmit_group_key()
                    } else {
                        self.is_ap_peer_authorized(destination)
                    };
                    if !authorized {
                        return Err(S31Wpa2IoError::ApPeerUnauthorized);
                    }
                }
                _ => {}
            }
            self.submit_frame(interface, frame.as_bytes(), Some(frame))
        }

        fn has_pairwise_key(&self, interface: Wpa2Interface, peer: &[u8; 6]) -> bool {
            (0..K).any(|index| {
                self.keys.get(index).is_some_and(|key| {
                    key.interface() == interface
                        && key.peer() == peer
                        && key.kind() == Wpa2KeyKind::Pairwise
                })
            })
        }

        fn set_peer_authorized(
            &mut self,
            interface: Wpa2Interface,
            peer: [u8; 6],
            authorized: bool,
        ) -> Result<(), S31Wpa2IoError> {
            if authorized && !self.has_pairwise_key(interface, &peer) {
                return Err(S31Wpa2IoError::AuthorizationWithoutPairwiseKey);
            }
            match interface {
                Wpa2Interface::Station => {
                    if authorized {
                        self.activate_sta_ptk()?;
                        self.sta_authorized_peer = Some(peer);
                    } else if self.sta_authorized_peer == Some(peer) {
                        self.sta_authorized_peer = None;
                    }
                    Ok(())
                }
                Wpa2Interface::AccessPoint => {
                    if authorized {
                        let hardware_index = self
                            .ap_transmit_group_hardware_index()
                            .ok_or(S31Wpa2IoError::MissingApTransmitGroupKey)?;
                        unsafe {
                            let node = cnx_node_search(peer.as_ptr());
                            if node.is_null() || node.add(0x134).read() == hardware_index {
                                return Err(S31Wpa2IoError::MissingApPeerHardwareIndex);
                            }
                            // `ieee80211_crypto_encap` selects multicast AP
                            // traffic through this byte. The stock AP key
                            // setter stores `key_id + 1`; publish the same
                            // already-installed fixed hardware slot without
                            // entering its allocator-backed wrapper.
                            node.add(0x135).write(hardware_index);

                            // Finite state tail of
                            // `esp_wifi_wpa_ptk_init_done_internal`: publish
                            // the installed PTK to the ordinary data path and
                            // clear its pre-authorization marker. The omitted
                            // remainder only constructs and posts a vendor
                            // event; controlled-port publication is owned by
                            // the bounded Rust table below.
                            let flags = node.add(0x0c).cast::<u32>();
                            flags.write((flags.read() & 0xfdff_ffff) | 1);
                            node.add(0x24).write(0);
                        }
                    }
                    let result = self
                        .authorized_peers
                        .set(peer, authorized)
                        .map_err(|()| S31Wpa2IoError::AuthorizationSlotsFull);
                    result
                }
            }
        }

        fn reset_sta_link(&mut self, peer: [u8; 6]) -> Result<(), S31Wpa2IoError> {
            if self
                .sta_authorized_peer
                .is_some_and(|authorized| authorized != peer)
            {
                return Err(S31Wpa2IoError::StaPeerMismatch);
            }
            if !unsafe { crate::sta_link::can_reset_static_sta_link() } {
                return Err(S31Wpa2IoError::StaLinkResetBusy);
            }

            // Preflight every software-key pointer before changing the
            // controlled port or hardware. This prevents a foreign key object
            // from turning teardown into a partially committed transaction.
            for index in 0..K {
                let Some(key) = self.keys.get(index) else {
                    continue;
                };
                if key.interface() != Wpa2Interface::Station {
                    continue;
                }
                if key.peer() != &peer && key.kind() == Wpa2KeyKind::Pairwise {
                    return Err(S31Wpa2IoError::StaPeerMismatch);
                }
                let hardware_index = match key.kind() {
                    Wpa2KeyKind::Pairwise => STA_PAIRWISE_HARDWARE_INDEX,
                    Wpa2KeyKind::Group { .. } => STA_GROUP_HARDWARE_INDEX,
                };
                let registered = unsafe { software_key_slot(hardware_index).unwrap().read() };
                if registered.is_null()
                    || !unsafe { static_vendor_key_object_is_owned(hardware_index, registered) }
                {
                    return Err(S31Wpa2IoError::ForeignSoftwareKeyPresent);
                }
            }

            self.sta_authorized_peer = None;
            for index in 0..K {
                let Some(key) = self.keys.get(index) else {
                    continue;
                };
                if key.interface() != Wpa2Interface::Station {
                    continue;
                }
                let hardware_index = match key.kind() {
                    Wpa2KeyKind::Pairwise => STA_PAIRWISE_HARDWARE_INDEX,
                    Wpa2KeyKind::Group { .. } => STA_GROUP_HARDWARE_INDEX,
                };
                unsafe {
                    ic_del_key(hardware_index.into());
                    let slot = software_key_slot(hardware_index).unwrap();
                    let object = slot.read();
                    slot.write(ptr::null_mut());
                    let released = release_static_vendor_key_object(object);
                    debug_assert!(released);
                }
                drop(self.keys.remove(index));
            }

            unsafe {
                let station = sta_interface_state();
                let node = sta_interface_node();
                if !station.is_null() {
                    let privacy = station.add(0xa4).cast::<u32>();
                    privacy.write(privacy.read() & !0x10);
                    station.add(0x140).write(0);
                }
                if !node.is_null() {
                    node.add(0x134).write(0);
                    node.add(0x135).write(0);
                    ptr::write_bytes(node.add(0x137), 0, 4);
                    let flags = node.add(0x0c).cast::<u32>();
                    flags.write(flags.read() & !(1 | 0x40 | 0x8000));
                    node.add(0x24).write(0);
                }
                ptr::addr_of_mut!(g_ic).add(0x274).write(0);
                ptr::addr_of_mut!(g_sta_connected_flag).write(0);
                crate::sta_link::reset_static_sta_link();
            }
            Ok(())
        }

        #[inline(never)]
        fn activate_sta_ptk(&self) -> Result<(), S31Wpa2IoError> {
            let station = unsafe { sta_interface_state() };
            let node = unsafe { sta_interface_node() };
            if station.is_null() || node.is_null() {
                return Err(S31Wpa2IoError::MissingStaInterfaceState);
            }
            let hardware_index = unsafe { node.add(0x134).read() };
            if hardware_index != STA_PAIRWISE_HARDWARE_INDEX {
                return Err(S31Wpa2IoError::UnexpectedStaPairwiseHardwareIndex);
            }
            unsafe {
                // Finite state tail of the pinned PTK-ready and STA privacy
                // callbacks. It runs only after M4 TX completion; their
                // event/log side effects are deliberately omitted.
                let flags = node.add(0x0c).cast::<u32>();
                flags.write((flags.read() & 0xfdff_ffff) | 1);
                node.add(0x24).write(0);
                let privacy = station.add(0xa4).cast::<u32>();
                privacy.write(privacy.read() | 0x10);

                // Minimal finite connection-state commit recovered from
                // `cnx_auth_done`. Its omitted remainder enters NVS, event
                // posting, power-save and AMPDU control; ordinary STA data
                // paths consume only these state facts.
                ptr::addr_of_mut!(g_ic).add(0x274).write(1);
                ptr::addr_of_mut!(g_sta_connected_flag).write(1);
                station.add(0x140).write(5);
            }
            // Negotiate the protocol boundary now that protected data is
            // authorized. The current step does not enable aggregation; it
            // only exercises the Rust-owned ADDBA state and async timeout.
            unsafe {
                let _ = crate::sta_link::start_sta_tx_block_ack();
            }
            Ok(())
        }

        fn submit_eapol<const N: usize>(
            &mut self,
            frame: &crate::wpa2_frames::Wpa2EthernetFrame<N>,
        ) -> Result<(), S31Wpa2IoError> {
            self.submit_frame(frame.interface(), frame.as_bytes(), None)
        }

        fn submit_frame(
            &mut self,
            frame_interface: Wpa2Interface,
            frame: &[u8],
            data_owner: Option<&OwnedWifiDataTxFrame>,
        ) -> Result<(), S31Wpa2IoError> {
            if self.tx_poisoned {
                return Err(S31Wpa2IoError::TxBackendPoisoned);
            }
            let length =
                u32::try_from(frame.len()).map_err(|_| S31Wpa2IoError::TxLengthOverflow)?;
            let interface = match frame_interface {
                Wpa2Interface::Station => wifi_interface_t_WIFI_IF_STA,
                Wpa2Interface::AccessPoint => wifi_interface_t_WIFI_IF_AP,
            };

            #[cfg(feature = "hil-vendor-tx")]
            if self.vendor_wpa_tx_diagnostic && frame_interface == Wpa2Interface::Station {
                if frame.len() < 14 || frame.len() > crate::wpa2_frames::WPA2_TX_ETHERNET_CAPACITY {
                    return Err(S31Wpa2IoError::TxLengthOverflow);
                }
                let mut vendor_frame = [0_u8; crate::wpa2_frames::WPA2_TX_ETHERNET_CAPACITY];
                vendor_frame[..frame.len()].copy_from_slice(frame);
                let protocol = u16::from_be_bytes([vendor_frame[12], vendor_frame[13]]);
                let result = unsafe {
                    wpa_ether_send(
                        ptr::addr_of_mut!(gWpaSm).cast(),
                        vendor_frame.as_ptr(),
                        protocol,
                        vendor_frame.as_mut_ptr().add(14),
                        frame.len() - 14,
                    )
                };
                return if result == 0 {
                    Ok(())
                } else {
                    Err(S31Wpa2IoError::VendorTxDiagnosticRejected(result))
                };
            }

            #[cfg(feature = "hil-vendor-tx")]
            if self.vendor_tx_diagnostic && data_owner.is_some() {
                let mut peer_error = 0_u32;
                let diagnostic_node =
                    unsafe { ieee80211_search_node(interface, frame.as_ptr(), &mut peer_error) };
                let diagnostic_interface = if interface == wifi_interface_t_WIFI_IF_STA {
                    station_interface()
                } else if interface == wifi_interface_t_WIFI_IF_AP {
                    access_point_interface()
                } else {
                    ptr::null_mut()
                };
                unsafe {
                    ets_printf(
                        c"HIL data node: node=%08x err=%08x nf=%08x n24=%02x n134=%02x n135=%02x if138=%08x\r\n"
                            .as_ptr()
                            .cast(),
                        diagnostic_node.addr(),
                        peer_error,
                        if diagnostic_node.is_null() {
                            0
                        } else {
                            diagnostic_node.add(0x0c).cast::<u32>().read()
                        },
                        if diagnostic_node.is_null() {
                            0xff_u32
                        } else {
                            u32::from(diagnostic_node.add(0x24).read())
                        },
                        if diagnostic_node.is_null() {
                            0xff_u32
                        } else {
                            u32::from(diagnostic_node.add(0x134).read())
                        },
                        if diagnostic_node.is_null() {
                            0xff_u32
                        } else {
                            u32::from(diagnostic_node.add(0x135).read())
                        },
                        if diagnostic_interface.is_null() {
                            0
                        } else {
                            diagnostic_interface.add(0x138).cast::<u32>().read()
                        },
                    );
                }
                let result = unsafe {
                    ieee80211_output_do(interface, frame.as_ptr(), length, 0, ptr::null_mut())
                };
                unsafe {
                    ets_printf(c"HIL vendor data result=%d\r\n".as_ptr().cast(), result);
                }
                return if result == 0 {
                    Ok(())
                } else {
                    Err(S31Wpa2IoError::VendorTxDiagnosticRejected(result))
                };
            }

            let mut peer_error = 0_u32;
            let node = unsafe { ieee80211_search_node(interface, frame.as_ptr(), &mut peer_error) };
            if node.is_null() {
                return Err(S31Wpa2IoError::TxPeerNotFound(peer_error));
            }
            if frame_interface == Wpa2Interface::AccessPoint {
                // The stock output wrapper branches into the AP power-save
                // queue when either field is set. Strict mode rejects that
                // branch instead of entering its separate scheduling graph.
                let sleeping = unsafe { node.add(0x2fe).read() != 0 };
                let flags = unsafe { node.add(0x0c).cast::<u32>().read() };
                let peer = [frame[0], frame[1], frame[2], frame[3], frame[4], frame[5]];
                // A PS-Poll is a credit only for a command that was already
                // deferred for this peer. An unsolicited/stale PS-Poll must
                // never authorize a later frame.
                let retry_armed = self.ap_retry_armed && self.ap_waiting_peer == peer;
                let ps_poll_credit = retry_armed
                    .then(|| {
                        crate::ap_power_save::ps_poll_credit_after(self.ap_ps_poll_epoch, &peer)
                    })
                    .flatten();
                if ap_owner_power_save_retry(&peer, sleeping, flags) && ps_poll_credit.is_none() {
                    // Publish the exact recovered AID bit through the finite
                    // Rust leaf. The owned command remains with the Rust radio
                    // owner; no vendor PS queue or OSI primitive is entered.
                    unsafe { crate::wpa2_ap::strict_update_ap_tim(node, true) };
                    crate::ap_power_save::record_deferred_transmit(&peer);
                    return Err(S31Wpa2IoError::TxPeerPowerSaveUnsupported(peer));
                }
                if let Some(epoch) = ps_poll_credit {
                    // Consume one peer-bound PS-Poll edge. This bypasses only
                    // the vendor dynamic PS queue; the ordinary fixed TX path
                    // below still owns, encrypts, and completes the frame.
                    self.ap_ps_poll_epoch = epoch;
                    unsafe { crate::wpa2_ap::strict_update_ap_tim(node, false) };
                }
                self.ap_retry_armed = false;
            }
            if !crate::net80211_state::ordinary_sta_ap_profile() {
                return Err(S31Wpa2IoError::CachedTxRuntimeEnabled);
            }

            // Call the mandatory ESF wrapper directly. Kind 1 is the
            // initialized static TX free list; this bypasses the stock cache
            // selector and its optional netstack callback.
            let buffer = unsafe { esf_buf_alloc(frame.as_ptr(), 1, length) };
            if buffer.is_null() {
                return Err(S31Wpa2IoError::StaticTxPoolExhausted);
            }

            let descriptor = unsafe { buffer.add(0x34).cast::<*mut u8>().read() };
            if descriptor.is_null() {
                self.tx_poisoned = true;
                return Err(S31Wpa2IoError::InvalidTxDescriptor);
            }
            unsafe {
                let control = descriptor.add(0x10).cast::<u32>();
                control.write((control.read() & 0xfff3_ffff) | ((interface & 3) << 18));
                let flags = buffer.add(0x1c).cast::<u32>();
                // `ieee80211_output_do(..., flags = 0, netstack = 0)` clears
                // the entire upper halfword here, not only the by-reference
                // ownership bit. Retaining stale allocator metadata can make
                // a successfully completed EAPOL buffer take the wrong TX
                // treatment downstream.
                flags.write(flags.read() & 0x0000_ffff);
            }

            let result = unsafe { ieee80211_post_hmac_tx(buffer) };
            if result != 0 {
                // The leaf recycles on its ordinary rejection branches. If
                // pp_post itself rejects after list insertion, its ownership
                // cannot be recovered synchronously; poison the backend and
                // require Wi-Fi deinit instead of retrying or duplicating TX.
                self.tx_poisoned = true;
                return Err(S31Wpa2IoError::TxPostRejected(result));
            }
            if let Some(owner) = data_owner {
                if owner.commit_hardware_credit(buffer).is_err() {
                    self.tx_poisoned = true;
                    return Err(S31Wpa2IoError::DataTxCreditMismatch);
                }
            }
            Ok(())
        }

        fn logical_slot_for(&self, install: &Wpa2KeyInstall) -> Result<usize, S31Wpa2IoError> {
            if install.interface() == Wpa2Interface::Station
                && matches!(install.kind(), Wpa2KeyKind::Group { .. })
            {
                // This chip has one active STA GTK hardware slot. Rekeying
                // to another logical GTK id replaces the old owned secret.
                if let Some(index) = (0..K).find(|&index| {
                    self.keys.get(index).is_some_and(|old| {
                        old.interface() == Wpa2Interface::Station
                            && matches!(old.kind(), Wpa2KeyKind::Group { .. })
                    })
                }) {
                    return Ok(index);
                }
            }
            self.keys
                .slot_for(install)
                .map_err(|_| S31Wpa2IoError::StaticKeySlotsFull)
        }

        fn install_ccmp(
            &mut self,
            install: Wpa2KeyInstall,
        ) -> Result<(), (S31Wpa2IoError, Wpa2KeyInstall)> {
            let interface = install.interface();
            let peer = *install.peer();
            let kind = install.kind();
            let (hardware_index, key_index, spp, gtk_node) = match kind {
                Wpa2KeyKind::Pairwise => {
                    let hardware_index = unsafe {
                        match interface {
                            Wpa2Interface::Station => STA_PAIRWISE_HARDWARE_INDEX,
                            Wpa2Interface::AccessPoint => ap_pairwise_hardware_index(peer.as_ptr()),
                        }
                    };
                    if interface == Wpa2Interface::AccessPoint && hardware_index == 0 {
                        return Err((S31Wpa2IoError::MissingApPeerHardwareIndex, install));
                    }
                    let spp = unsafe { peer_spp(interface, peer.as_ptr()) };
                    (
                        hardware_index,
                        PAIRWISE_KEY_INDEX,
                        u32::from(spp != 0),
                        None,
                    )
                }
                Wpa2KeyKind::Group { key_id, .. } => {
                    let Some(hardware_index) = group_hardware_index(interface, key_id) else {
                        return Err((S31Wpa2IoError::InvalidGroupKeyId(key_id), install));
                    };
                    let gtk_node = match interface {
                        Wpa2Interface::Station => {
                            let node = unsafe { sta_interface_node() };
                            if node.is_null() {
                                return Err((S31Wpa2IoError::MissingStaInterfaceState, install));
                            }
                            Some((node, true))
                        }
                        Wpa2Interface::AccessPoint => {
                            // AP group installs carry the broadcast peer. The
                            // pinned `ieee80211_set_gtk` resolves it to the
                            // interface/BSS node rather than an associated
                            // station node.
                            let node = unsafe { strict_ap_node_search(peer.as_ptr()) };
                            if node.is_null() {
                                return Err((S31Wpa2IoError::MissingApTransmitGroupNode, install));
                            }
                            Some((node, false))
                        }
                    };
                    (hardware_index, u32::from(key_id), 0, gtk_node)
                }
            };
            if hardware_index > MAX_VENDOR_KEY_INDEX {
                return Err((
                    S31Wpa2IoError::InvalidHardwareIndex(hardware_index),
                    install,
                ));
            }
            let logical_index = match self.logical_slot_for(&install) {
                Ok(index) => index,
                Err(error) => return Err((error, install)),
            };
            let Some(object) = self.storage.claim(hardware_index) else {
                return Err((S31Wpa2IoError::StaticKeySlotsFull, install));
            };
            let software_key_slot = unsafe { software_key_slot(hardware_index) }.unwrap();
            let registered = unsafe { software_key_slot.read() };
            if !registered.is_null() && registered != object.cast::<c_void>() {
                return Err((S31Wpa2IoError::ForeignSoftwareKeyPresent, install));
            }
            let key = match self.keys.replace_at(logical_index, install) {
                Ok(key) => key,
                Err(install) => {
                    return Err((S31Wpa2IoError::InternalOwnershipMismatch, install));
                }
            };

            let interface_number = match interface {
                Wpa2Interface::Station => wifi_interface_t_WIFI_IF_STA,
                Wpa2Interface::AccessPoint => wifi_interface_t_WIFI_IF_AP,
            };
            unsafe {
                // Exact bounded hardware replacement prefix from the pinned
                // non-delete `ppInstallKey` branch. `ic_del_key` is a finite
                // bitmap update plus ten key-register clears; it has no loop,
                // lock, allocation, callback, or wait edge.
                ic_del_key(hardware_index.into());
                let crypto_enable = u32::from(
                    ptr::addr_of_mut!(g_ic)
                        .add(0x210)
                        .cast::<u16>()
                        .read_volatile()
                        == 0,
                );
                ic_set_key(
                    interface_number,
                    CCMP_ALGORITHM,
                    key_index,
                    peer.as_ptr(),
                    hardware_index.into(),
                    key.key().as_bytes().as_ptr(),
                    WPA2_TK_LEN,
                    crypto_enable,
                    spp,
                );

                let object = &mut *object;
                // Exact two bounded memset operations from the pinned
                // 0x30-byte `wifi_init_key`, performed in Rust-owned storage.
                object.initialize_like_wifi_init_key();
                object.bytes[KEY_INDEX_OFFSET..KEY_INDEX_OFFSET + 2]
                    .copy_from_slice(&(hardware_index as u16).to_le_bytes());
                object.bytes[RECEIVE_SEQUENCE_OFFSET..RECEIVE_SEQUENCE_OFFSET + 8]
                    .copy_from_slice(key.receive_sequence());
                object
                    .bytes
                    .as_mut_ptr()
                    .add(CIPHER_POINTER_OFFSET)
                    .cast::<*const u8>()
                    .write(core::ptr::addr_of!(ccmp).cast::<u8>());
                object.bytes[KEY_LENGTH_OFFSET..KEY_LENGTH_OFFSET + 4]
                    .copy_from_slice(&(WPA2_TK_LEN as u32).to_le_bytes());
                object.bytes[KEY_BYTES_OFFSET..].copy_from_slice(key.key().as_bytes());

                // Exact non-freeing branch of the pinned key-table setter.
                // The foreign-pointer case was rejected before any mutation.
                software_key_slot.write(object as *mut _ as *mut c_void);
                if let Wpa2KeyKind::Group { key_id, .. } = kind {
                    if let Some((node, station_mapping)) = gtk_node {
                        // `ieee80211_set_gtk` proves the active selector for
                        // both roles. STA additionally keeps the recovered
                        // logical-id mapping used while receiving a rekey.
                        node.add(0x135).write(hardware_index);
                        if station_mapping {
                            node.add(0x137 + usize::from(key_id)).write(hardware_index);
                        }
                    }
                }
            }
            Ok(())
        }
    }

    impl<const K: usize, const N: usize> TryWpa2Io<N> for S31StaticWpa2Io<K> {
        type Error = S31Wpa2IoError;

        fn poll_internal(&mut self, cx: &mut core::task::Context<'_>) -> bool {
            crate::wpa2_ap::poll_deferred_ap_management(cx)
        }

        fn try_execute(
            &mut self,
            command: Wpa2IoCommand<N>,
        ) -> Result<(), Wpa2IoFailure<Self::Error, N>> {
            if !in_radio_context() {
                return Err(Wpa2IoFailure {
                    error: S31Wpa2IoError::NotRadioOwner,
                    command,
                });
            }
            match command {
                Wpa2IoCommand::Transmit(frame) => {
                    if frame.interface() == Wpa2Interface::Station
                        && !async_wpa2_sta_tx_done_installed()
                    {
                        return Err(Wpa2IoFailure {
                            error: S31Wpa2IoError::StaTxDoneCallbackMissing,
                            command: Wpa2IoCommand::Transmit(frame),
                        });
                    }
                    let Some(peer) = frame
                        .as_bytes()
                        .get(..6)
                        .and_then(|bytes| <&[u8; 6]>::try_from(bytes).ok())
                    else {
                        return Err(Wpa2IoFailure {
                            error: S31Wpa2IoError::TxLengthOverflow,
                            command: Wpa2IoCommand::Transmit(frame),
                        });
                    };
                    if self.cancel_if_ap_peer_removed(frame.interface(), peer) {
                        return Ok(());
                    }
                    if let Err(error) = self.submit_eapol(&frame) {
                        return Err(Wpa2IoFailure {
                            error,
                            command: Wpa2IoCommand::Transmit(frame),
                        });
                    }
                    Ok(())
                }
                Wpa2IoCommand::TransmitData(frame) => {
                    if self.cancel_if_ap_peer_removed(
                        match frame.interface() {
                            WifiDataInterface::Station => Wpa2Interface::Station,
                            WifiDataInterface::AccessPoint => Wpa2Interface::AccessPoint,
                        },
                        frame.destination(),
                    ) {
                        return Ok(());
                    }
                    match self.try_transmit_wifi_data(&frame) {
                        Ok(()) => Ok(()),
                        Err(S31Wpa2IoError::TxPeerNotFound(peer_error)) => {
                            // Network ownership and peer ownership are sampled
                            // at different async boundaries. A peer can vanish
                            // after this frame was admitted but before the
                            // radio owner handles it. Reject this one owned
                            // frame and release its static slot; the radio
                            // executor itself remains immortal.
                            crate::data_tx::reject_wifi_data_tx_missing_peer(peer_error);
                            Ok(())
                        }
                        Err(error) => Err(Wpa2IoFailure {
                            error,
                            command: Wpa2IoCommand::TransmitData(frame),
                        }),
                    }
                }
                Wpa2IoCommand::InstallKey(install) => {
                    self.install_ccmp(install)
                        .map_err(|(error, install)| Wpa2IoFailure {
                            error,
                            command: Wpa2IoCommand::InstallKey(install),
                        })
                }
                Wpa2IoCommand::SetPeerAuthorized {
                    interface,
                    peer,
                    authorized,
                } => self
                    .set_peer_authorized(interface, peer, authorized)
                    .map_err(|error| Wpa2IoFailure {
                        error,
                        command: Wpa2IoCommand::SetPeerAuthorized {
                            interface,
                            peer,
                            authorized,
                        },
                    }),
                Wpa2IoCommand::ResetStaLink { peer } => {
                    self.reset_sta_link(peer).map_err(|error| Wpa2IoFailure {
                        error,
                        command: Wpa2IoCommand::ResetStaLink { peer },
                    })
                }
                #[cfg(feature = "hil-rx-ampdu")]
                Wpa2IoCommand::ExpireRxAmpduGap { generation } => {
                    let _ = crate::rx::expire_rx_ampdu_gap(generation);
                    Ok(())
                }
                #[cfg(feature = "hil-rx-ampdu")]
                Wpa2IoCommand::RemoveRxAmpduPeer { peer } => {
                    crate::rx_ampdu_ap::remove_peer(peer);
                    Ok(())
                }
            }
        }

        fn prepare_retry(&mut self, error: &Self::Error) -> bool {
            let S31Wpa2IoError::TxPeerPowerSaveUnsupported(peer) = *error else {
                return false;
            };
            self.ap_waiting_peer = peer;
            self.ap_active_epoch = crate::ap_power_save::active_epoch(&peer);
            self.ap_ps_poll_epoch = crate::ap_power_save::ps_poll_epoch(&peer);
            self.ap_removal_epoch = crate::ap_power_save::removal_epoch(&peer);
            self.ap_waiting_association_epoch =
                crate::wpa2_ap::wpa2_ap_peer_association_epoch(&peer).unwrap_or(0);
            self.ap_retry_armed = true;
            #[cfg(feature = "hil-vendor-tx")]
            {
                for (destination, byte) in HIL_AP_WAITING_PEER.iter().zip(peer) {
                    destination.store(byte, Ordering::Relaxed);
                }
                HIL_AP_WAITING_PEER_VALID.store(true, Ordering::Release);
            }
            true
        }

        fn poll_retry_ready(
            &mut self,
            cx: &mut core::task::Context<'_>,
        ) -> core::task::Poll<PendingCommandAction> {
            if crate::wpa2_ap::wpa2_ap_peer_association_epoch(&self.ap_waiting_peer)
                != Some(self.ap_waiting_association_epoch)
            {
                #[cfg(feature = "hil-vendor-tx")]
                HIL_AP_WAITING_PEER_VALID.store(false, Ordering::Release);
                return core::task::Poll::Ready(PendingCommandAction::Cancel);
            }
            match crate::ap_power_save::poll_peer_edge(
                self.ap_active_epoch,
                self.ap_ps_poll_epoch,
                self.ap_removal_epoch,
                &self.ap_waiting_peer,
                cx,
            ) {
                core::task::Poll::Ready(crate::ap_power_save::PeerEdge::Retry) => {
                    #[cfg(feature = "hil-vendor-tx")]
                    HIL_AP_WAITING_PEER_VALID.store(false, Ordering::Release);
                    core::task::Poll::Ready(PendingCommandAction::Retry)
                }
                core::task::Poll::Ready(crate::ap_power_save::PeerEdge::Removed) => {
                    #[cfg(feature = "hil-vendor-tx")]
                    HIL_AP_WAITING_PEER_VALID.store(false, Ordering::Release);
                    core::task::Poll::Ready(PendingCommandAction::Cancel)
                }
                core::task::Poll::Pending => core::task::Poll::Pending,
            }
        }

        fn cancel_retry(&mut self, command: &Wpa2IoCommand<N>) {
            let matches_waiting_peer = match command {
                Wpa2IoCommand::Transmit(frame) => {
                    frame.interface() == Wpa2Interface::AccessPoint
                        && frame.as_bytes().get(..6) == Some(&self.ap_waiting_peer)
                }
                Wpa2IoCommand::TransmitData(frame) => {
                    frame.interface() == WifiDataInterface::AccessPoint
                        && frame.destination() == &self.ap_waiting_peer
                }
                _ => false,
            };
            if matches_waiting_peer {
                let _ = self.authorized_peers.set(self.ap_waiting_peer, false);
                self.ap_retry_armed = false;
                self.ap_cancelled_peers
                    .retain(|peer| crate::wpa2_ap::wpa2_ap_peer_association_epoch(peer).is_some());
                let inserted = self
                    .ap_cancelled_peers
                    .set(self.ap_waiting_peer, self.ap_waiting_association_epoch);
                debug_assert!(inserted.is_ok());
                crate::ap_power_save::record_cancelled_transmit();
            }
        }
    }

    /// Publish the real peer-removal readiness edge for the AP command that is
    /// currently deferred on a sleeping peer.
    ///
    /// This is a deterministic HIL trigger for the async owner's cancellation
    /// path. It is deliberately unavailable without `hil-vendor-tx` and does
    /// not mutate the vendor association table or disconnect a station.
    #[cfg(feature = "hil-vendor-tx")]
    pub fn hil_cancel_deferred_ap_transmit() -> bool {
        if !HIL_AP_WAITING_PEER_VALID.load(Ordering::Acquire) {
            return false;
        }
        let mut peer = [0; 6];
        for (byte, source) in peer.iter_mut().zip(HIL_AP_WAITING_PEER.iter()) {
            *byte = source.load(Ordering::Acquire);
        }
        if HIL_AP_WAITING_PEER_VALID
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        crate::ap_power_save::observe_peer_removed(&peer);
        true
    }
}

#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
pub use target::hil_cancel_deferred_ap_transmit;
#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
pub use target::hil_sta_pairwise_key_snapshot;
#[cfg(target_arch = "riscv32")]
pub use target::S31StaticWpa2Io;
#[cfg(target_arch = "riscv32")]
pub(crate) use target::{release_static_ap_node, runtime_key_link_wrapper_active};

#[cfg(test)]
mod tests {
    use super::*;

    static STORAGE: S31StaticKeyStorage<1> = S31StaticKeyStorage::new();

    #[test]
    fn pinned_vendor_key_layout_is_exact_and_aligned() {
        assert_eq!(core::mem::size_of::<VendorCcmpKeyObject>(), 0xb8);
        assert_eq!(core::mem::align_of::<VendorCcmpKeyObject>(), 4);
        assert_eq!(KEY_INDEX_OFFSET, 0);
        assert_eq!(RECEIVE_SEQUENCE_OFFSET, 0x98);
        assert_eq!(CIPHER_POINTER_OFFSET, 0xa0);
        assert_eq!(KEY_LENGTH_OFFSET, 0xa4);
        assert_eq!(KEY_BYTES_OFFSET, 0xa8);
    }

    #[test]
    fn rust_key_initialization_matches_pinned_wifi_init_key() {
        let mut object = VendorCcmpKeyObject {
            bytes: [0x5a; VENDOR_KEY_OBJECT_LEN],
        };

        object.initialize_like_wifi_init_key();

        assert!(object.bytes[..8].iter().all(|byte| *byte == 0));
        assert!(object.bytes[8..0x98].iter().all(|byte| *byte == 0xff));
        assert!(object.bytes[0x98..0xa8].iter().all(|byte| *byte == 0));
        assert!(object.bytes[0xa8..].iter().all(|byte| *byte == 0x5a));
    }

    #[test]
    fn static_vendor_slot_has_stable_address_and_fixed_capacity() {
        let first = STORAGE.claim(4).unwrap();
        let same = STORAGE.claim(4).unwrap();
        assert_eq!(first, same);
        assert!(STORAGE.claim(5).is_none());
        assert_eq!(first.addr() & 3, 0);
    }

    #[test]
    fn group_hardware_slots_are_fixed_and_disjoint_from_sta_pairwise() {
        use crate::wpa2::Wpa2Interface;

        for key_id in 0..=MAX_WPA2_GTK_ID {
            assert_eq!(
                group_hardware_index(Wpa2Interface::Station, key_id),
                Some(STA_GROUP_HARDWARE_INDEX)
            );
            assert_eq!(
                group_hardware_index(Wpa2Interface::AccessPoint, key_id),
                Some(AP_GROUP_HARDWARE_INDEX_BASE + key_id)
            );
        }
        assert_ne!(STA_GROUP_HARDWARE_INDEX, STA_PAIRWISE_HARDWARE_INDEX);
        assert_eq!(group_hardware_index(Wpa2Interface::Station, 4), None);
    }

    #[test]
    fn hardware_key_direction_matches_pinned_hal_branches() {
        let ccmp = 3 << 18;
        assert_eq!(hardware_key_direction(ccmp, 4), 3);
        assert_eq!(hardware_key_direction(ccmp, 1), 6);
        assert_eq!(hardware_key_direction(0x0004_0000, 4), 7);
    }

    #[test]
    fn crypto_enable_plan_matches_all_three_pinned_interface_branches() {
        assert_eq!(
            crypto_enable_control(0, 3, 0, 0),
            Some((0x2010_4800, 0x0003_0103, 0x0003_0103))
        );
        assert_eq!(
            crypto_enable_control(1, 4, 0, 1),
            Some((0x2010_4804, 0x0003_0303, 0x8003_0303))
        );
        assert_eq!(
            crypto_enable_control(2, 3, 0, 0),
            Some((0x2010_4808, 0x0001_0103, 0x0001_0103))
        );
        assert_eq!(
            crypto_enable_control(2, 1, 1, 0),
            Some((0x2010_4808, 0x1003_0103, 0x1003_0103))
        );
        assert_eq!(crypto_enable_control(3, 3, 1, 0), None);
    }

    #[test]
    fn authorization_table_is_fixed_and_deauthorization_is_idempotent() {
        let first = [1, 2, 3, 4, 5, 6];
        let second = [6, 5, 4, 3, 2, 1];
        let mut peers = StaticAuthorizedPeers::<1>::new();

        assert!(!peers.contains(&first));
        assert!(!peers.any());
        assert_eq!(peers.set(first, true), Ok(()));
        assert!(peers.contains(&first));
        assert!(peers.any());
        assert_eq!(peers.set(second, true), Err(()));
        assert_eq!(peers.set(first, false), Ok(()));
        assert!(!peers.any());
        assert_eq!(peers.set(first, false), Ok(()));
        assert_eq!(peers.set(second, true), Ok(()));
        assert!(peers.contains(&second));
    }

    #[test]
    fn cancelled_generations_are_peer_bound_and_fixed_capacity() {
        let first = [1, 2, 3, 4, 5, 6];
        let second = [6, 5, 4, 3, 2, 1];
        let third = [7, 7, 7, 7, 7, 7];
        let mut peers = StaticCancelledPeerGenerations::<2>::new();

        assert_eq!(peers.get(&first), None);
        assert_eq!(peers.set(first, 10), Ok(()));
        assert_eq!(peers.set(second, 20), Ok(()));
        assert_eq!(peers.get(&first), Some(10));
        assert_eq!(peers.get(&second), Some(20));
        assert_eq!(peers.set(first, 11), Ok(()));
        assert_eq!(peers.get(&first), Some(11));
        assert_eq!(peers.set(third, 30), Err(()));

        peers.retain(|peer| *peer != second);
        assert_eq!(peers.get(&second), None);
        assert_eq!(peers.set(third, 30), Ok(()));
        peers.remove(&first);
        assert_eq!(peers.get(&first), None);
        assert_eq!(peers.get(&third), Some(30));
    }

    #[test]
    fn group_power_save_ownership_moves_to_bounded_dtim_queue() {
        assert!(!ap_owner_power_save_retry(
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            true,
            0x10,
        ));
        assert!(!ap_owner_power_save_retry(
            &[0x33, 0x33, 0, 0, 0, 2],
            true,
            0x10,
        ));
        assert!(ap_owner_power_save_retry(
            &[0x92, 0xd6, 0x0e, 0x4c, 0x09, 0x75],
            true,
            0,
        ));
    }
}
