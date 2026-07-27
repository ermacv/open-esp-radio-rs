//! Strict Rust replacement for net80211 WPA2 TX key selection and CCMP header
//! insertion.

use core::{ffi::c_void, ptr};
use open_esp_radio_ieee80211::ccmp;

const NET80211_CRYPTO_ENCAP_CALLBACK_SLOT: usize = 0x44 / core::mem::size_of::<usize>();
const ESF_STORAGE_OFFSET: usize = 0x04;
const ESF_LENGTH_OFFSET: usize = 0x16;
const ESF_DESCRIPTOR_OFFSET: usize = 0x34;
const BUFFER_DATA_OFFSET: usize = 0x04;
const DESCRIPTOR_MULTICAST_FLAG: u32 = 0x0000_0002;
const NODE_PAIRWISE_HARDWARE_INDEX_OFFSET: usize = 0x134;
const NODE_GROUP_HARDWARE_INDEX_OFFSET: usize = 0x135;
const KEY_HARDWARE_INDEX_OFFSET: usize = 0x00;
const KEY_TX_PN_LOW_OFFSET: usize = 0x98;
const KEY_TX_PN_HIGH_OFFSET: usize = 0x9c;
const KEY_CIPHER_POINTER_OFFSET: usize = 0xa0;
const KEY_LENGTH_OFFSET: usize = 0xa4;
const WPA2_CCMP_KEY_LENGTH: u32 = 16;

unsafe extern "C" {
    static ccmp: [u8; 24];
    static mut net80211_funcs: *mut usize;
    fn __real_ieee80211_crypto_encap(node: *mut u8, buffer: *mut u8) -> *mut c_void;
}

pub(crate) fn link_wrapper_active() -> bool {
    // Proven by the final-value ASSERT in esp32s31-rom-wrap-overrides.x.
    true
}

pub(crate) fn callback_active() -> bool {
    unsafe {
        let table = ptr::addr_of!(net80211_funcs).read_volatile();
        !table.is_null()
            && table
                .add(NET80211_CRYPTO_ENCAP_CALLBACK_SLOT)
                .read_volatile()
                == wifi_strict_ieee80211_crypto_encap as *const () as usize
    }
}

/// Adopt the ROM caller's function-table slot after cold initialization.
///
/// The previous value must be either the pinned vendor leaf or this exact
/// replacement. An unknown table owner fails closed.
pub(crate) unsafe fn adopt_callback() -> bool {
    let table = ptr::addr_of!(net80211_funcs).read_volatile();
    if table.is_null() {
        return false;
    }
    let slot = table.add(NET80211_CRYPTO_ENCAP_CALLBACK_SLOT);
    let current = slot.read_volatile();
    let vendor = __real_ieee80211_crypto_encap as *const () as usize;
    let replacement = wifi_strict_ieee80211_crypto_encap as *const () as usize;
    if current != vendor && current != replacement {
        return false;
    }
    slot.write_volatile(replacement);
    slot.read_volatile() == replacement
}

unsafe fn owned_ccmp_key(hardware_index: u8) -> Option<*mut u8> {
    let key = crate::wpa2_s31::owned_static_vendor_key_object(hardware_index)?;
    if key.add(KEY_HARDWARE_INDEX_OFFSET).cast::<u16>().read() != u16::from(hardware_index)
        || key
            .add(KEY_CIPHER_POINTER_OFFSET)
            .cast::<*const u8>()
            .read()
            != ptr::addr_of!(ccmp).cast::<u8>()
        || key.add(KEY_LENGTH_OFFSET).cast::<u32>().read() != WPA2_CCMP_KEY_LENGTH
    {
        return None;
    }
    Some(key)
}

unsafe fn insert_ccmp_header(key: *mut u8, buffer: *mut u8, key_id_bits: u8) -> Option<()> {
    let storage = buffer.add(ESF_STORAGE_OFFSET).cast::<*mut u8>().read();
    if storage.is_null() {
        return None;
    }
    let data_slot = storage.add(BUFFER_DATA_OFFSET).cast::<*mut u8>();
    let data = data_slot.read();
    let new_data = (data as usize).checked_sub(ccmp::CCMP_HEADER_LEN)? as *mut u8;
    let length_slot = buffer.add(ESF_LENGTH_OFFSET).cast::<u16>();
    let length = length_slot.read();
    let new_length = length.checked_add(ccmp::CCMP_HEADER_LEN as u16)?;

    let low_slot = key.add(KEY_TX_PN_LOW_OFFSET).cast::<u32>();
    let high_slot = key.add(KEY_TX_PN_HIGH_OFFSET).cast::<u32>();
    let (low, high) = ccmp::advance_vendor_tx_pn(low_slot.read(), high_slot.read());
    let header = ccmp::ccmp_header(low, high, key_id_bits);

    data_slot.write(new_data);
    length_slot.write(new_length);
    low_slot.write(low);
    high_slot.write(high);
    ptr::copy_nonoverlapping(header.as_ptr(), new_data, header.len());
    Some(())
}

/// Finite WPA2-CCMP port of the pinned
/// `ieee80211_crypto_encap -> ccmp_encap` path.
///
/// It resolves only Rust-owned static key objects, performs no indirect
/// cipher callback, and reproduces the exact eight-byte CCMP header and
/// packet-number update. Unsupported/foreign key state returns no key.
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn wifi_strict_ieee80211_crypto_encap(
    node: *mut u8,
    buffer: *mut u8,
) -> *mut c_void {
    if !crate::critical::strict_wifi_hart_armed() {
        return __real_ieee80211_crypto_encap(node, buffer);
    }
    if !crate::critical::on_strict_wifi_hart()
        || !crate::context::in_radio_context()
        || !crate::net80211_state::ordinary_sta_ap_profile()
        || node.is_null()
        || buffer.is_null()
    {
        return ptr::null_mut();
    }
    let descriptor = buffer.add(ESF_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    if descriptor.is_null() {
        return ptr::null_mut();
    }
    let multicast = descriptor.cast::<u32>().read() & DESCRIPTOR_MULTICAST_FLAG != 0;
    let hardware_index = node
        .add(if multicast {
            NODE_GROUP_HARDWARE_INDEX_OFFSET
        } else {
            NODE_PAIRWISE_HARDWARE_INDEX_OFFSET
        })
        .read();
    let Some(key) = owned_ccmp_key(hardware_index) else {
        return ptr::null_mut();
    };
    let key_id_bits = if multicast {
        ccmp::multicast_key_id_bits(hardware_index)
    } else {
        0
    };
    if insert_ccmp_header(key, buffer, key_id_bits).is_none() {
        return ptr::null_mut();
    }
    key.cast::<c_void>()
}
