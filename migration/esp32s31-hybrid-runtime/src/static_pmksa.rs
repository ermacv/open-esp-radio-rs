//! Fixed ownership for the empty STA PMKSA cache header.
//!
//! The pinned `pmksa_cache_init` allocates one zeroed 20-byte object. Its
//! recovered fields are the entry-list head, entry count, `wpa_sm` pointer,
//! entry-free callback and callback context. The strict Rust WPA2 path does
//! not create vendor PMKSA entries, so only the empty header is required.

use core::ptr;

const CACHE_WORDS: usize = 5;
const ENTRY_HEAD_WORD: usize = 0;
const ENTRY_COUNT_WORD: usize = 1;
const WPA_SM_WORD: usize = 2;
const FREE_CALLBACK_WORD: usize = 3;
const CALLBACK_CONTEXT_WORD: usize = 4;

#[repr(C, align(4))]
struct StaticPmksaCache([u32; CACHE_WORDS]);

#[link_section = ".critical.bss.wifi_strict.pmksa_cache"]
static mut STATIC_PMKSA_CACHE: StaticPmksaCache = StaticPmksaCache([0; CACHE_WORDS]);

unsafe fn cache_words() -> *mut u32 {
    ptr::addr_of_mut!(STATIC_PMKSA_CACHE).cast::<u32>()
}

unsafe fn cache_is_empty(cache: *mut u32) -> bool {
    cache.add(ENTRY_HEAD_WORD).read_volatile() == 0
        && cache.add(ENTRY_COUNT_WORD).read_volatile() == 0
}

/// Return whether the exact empty PMKSA header is initialized.
///
/// # Safety
///
/// This must be serialized with supplicant initialization/deinitialization.
pub unsafe fn static_pmksa_cache_bound() -> bool {
    let cache = cache_words();
    cache_is_empty(cache)
        && cache.add(WPA_SM_WORD).read_volatile() != 0
        && cache.add(FREE_CALLBACK_WORD).read_volatile() != 0
        && cache.add(CALLBACK_CONTEXT_WORD).read_volatile() != 0
}

/// Replace the one-allocation PMKSA cache constructor.
#[cfg(feature = "rust-static-pmksa-cache-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_pmksa_cache_init(
    free_callback: *const (),
    callback_context: *mut u8,
    wpa_sm: *mut u8,
) -> *mut u8 {
    let cache = cache_words();
    if free_callback.is_null()
        || callback_context.is_null()
        || wpa_sm.is_null()
        || cache.add(ENTRY_HEAD_WORD).read_volatile() != 0
        || cache.add(ENTRY_COUNT_WORD).read_volatile() != 0
        || cache.add(WPA_SM_WORD).read_volatile() != 0
        || cache.add(FREE_CALLBACK_WORD).read_volatile() != 0
        || cache.add(CALLBACK_CONTEXT_WORD).read_volatile() != 0
    {
        return ptr::null_mut();
    }

    cache.add(ENTRY_HEAD_WORD).write_volatile(0);
    cache.add(ENTRY_COUNT_WORD).write_volatile(0);
    cache
        .add(WPA_SM_WORD)
        .write_volatile(wpa_sm.addr() as u32);
    cache
        .add(FREE_CALLBACK_WORD)
        .write_volatile(free_callback.addr() as u32);
    cache
        .add(CALLBACK_CONTEXT_WORD)
        .write_volatile(callback_context.addr() as u32);
    cache.cast::<u8>()
}

/// Withdraw only the exact empty static PMKSA cache.
///
/// An unknown cache or a populated entry list is left untouched. In
/// particular, this boundary never enters the vendor expiration timer, an
/// indirect free callback or a deallocator.
#[cfg(feature = "rust-static-pmksa-cache-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_pmksa_cache_deinit(cache: *mut u8) {
    let static_cache = cache_words();
    if cache.cast::<u32>() != static_cache || !cache_is_empty(static_cache) {
        return;
    }
    static_cache.add(ENTRY_HEAD_WORD).write_volatile(0);
    static_cache.add(ENTRY_COUNT_WORD).write_volatile(0);
    static_cache.add(WPA_SM_WORD).write_volatile(0);
    static_cache.add(FREE_CALLBACK_WORD).write_volatile(0);
    static_cache
        .add(CALLBACK_CONTEXT_WORD)
        .write_volatile(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pmksa_header_has_exact_recovered_shape() {
        assert_eq!(size_of::<StaticPmksaCache>(), 20);
        assert_eq!(align_of::<StaticPmksaCache>(), 4);
        assert_eq!(CALLBACK_CONTEXT_WORD + 1, CACHE_WORDS);
    }
}
