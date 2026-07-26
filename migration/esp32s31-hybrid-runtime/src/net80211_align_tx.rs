//! Strict Rust replacement for the finite ESP32-S31 net80211 ESF alignment
//! leaf.

use core::ptr;

const ESF_STORAGE_OFFSET: usize = 0x04;
const ESF_HEADER_LENGTH_OFFSET: usize = 0x14;
const ESF_REMAINING_LENGTH_OFFSET: usize = 0x16;
const STORAGE_DATA_OFFSET: usize = 0x04;

unsafe extern "C" {
    fn __real_ieee80211_align_eb(buffer: *mut u8, reserve: u32);
}

pub(crate) fn link_wrapper_active() -> bool {
    // Proven by the final-value ASSERT in esp32s31-rom-wrap-overrides.x.
    true
}

#[inline(always)]
unsafe fn trap_invalid_alignment(buffer: *mut u8, reserve: u32) -> ! {
    core::arch::asm!(
        "ebreak",
        in("a0") buffer,
        in("a1") reserve,
        options(noreturn)
    )
}

/// Reserve and align one ordinary STA/AP 802.11 header.
///
/// The pinned vendor leaf mutates only the ESF storage pointer and its packed
/// length word. The strict replacement computes every fallible value before
/// moving bytes or publishing either mutation.
///
/// # Safety
///
/// `buffer` must be a live, exclusively owned ESF TX object whose storage has
/// the headroom promised by the pinned allocator ABI.
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn wifi_strict_ieee80211_align_eb(buffer: *mut u8, reserve: u32) {
    if !crate::critical::strict_wifi_hart_armed() {
        __real_ieee80211_align_eb(buffer, reserve);
        return;
    }
    if !crate::critical::on_strict_wifi_hart()
        || !crate::context::in_radio_context()
        || !crate::net80211_state::ordinary_sta_ap_profile()
        || buffer.is_null()
    {
        trap_invalid_alignment(buffer, reserve);
    }

    let storage = buffer.add(ESF_STORAGE_OFFSET).cast::<*mut u8>().read();
    if storage.is_null() {
        trap_invalid_alignment(buffer, reserve);
    }
    let data_slot = storage.add(STORAGE_DATA_OFFSET).cast::<*mut u8>();
    let data = data_slot.read();
    if data.is_null() {
        trap_invalid_alignment(buffer, reserve);
    }
    let header_len = buffer.add(ESF_HEADER_LENGTH_OFFSET).cast::<u16>().read();
    let remaining_len = buffer.add(ESF_REMAINING_LENGTH_OFFSET).cast::<u16>().read();
    let storage_word = storage.cast::<u32>().read();
    let Some(plan) = crate::net80211_align::plan(
        data as usize,
        reserve as usize,
        header_len,
        remaining_len,
        storage_word,
    ) else {
        trap_invalid_alignment(buffer, reserve);
    };

    if plan.aligned_start != plan.reserved_start {
        ptr::copy(
            plan.reserved_start as *const u8,
            plan.aligned_start as *mut u8,
            plan.move_len,
        );
    }
    data_slot.write(plan.aligned_start as *mut u8);
    storage.cast::<u32>().write(plan.storage_word);
}
