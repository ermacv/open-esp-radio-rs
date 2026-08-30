//! Explicit cached-PSRAM ownership handoff to non-coherent peripheral readers.

use esp_hal::peripherals::CACHE;

const CACHE_LINE_SIZE: usize = 64;
const PSRAM_LOW: usize = 0x5000_0000;
const PSRAM_HIGH: usize = 0x5400_0000;
const CACHE_MAP_L1_DCACHE: u8 = 1 << 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PsramCacheWritebackError {
    Empty,
    AddressOverflow,
    OutsidePsram,
}

fn cache_aligned_psram_range(
    address: usize,
    size: usize,
) -> Result<(u32, u32), PsramCacheWritebackError> {
    if size == 0 {
        return Err(PsramCacheWritebackError::Empty);
    }
    let end = address
        .checked_add(size)
        .ok_or(PsramCacheWritebackError::AddressOverflow)?;
    if address < PSRAM_LOW || end > PSRAM_HIGH {
        return Err(PsramCacheWritebackError::OutsidePsram);
    }
    let start = address & !(CACHE_LINE_SIZE - 1);
    let aligned_end = end
        .checked_add(CACHE_LINE_SIZE - 1)
        .ok_or(PsramCacheWritebackError::AddressOverflow)?
        & !(CACHE_LINE_SIZE - 1);
    Ok((start as u32, (aligned_end - start) as u32))
}

/// Write CPU-dirty cached PSRAM lines back before a peripheral reads them.
///
/// The mutable borrow is the software-ownership proof for the affected data.
/// Callers must additionally ensure that the complete cache lines containing
/// `storage` do not contain data concurrently owned by another actor. After
/// this function returns, the caller must not mutate `storage` until the
/// peripheral has completed. ESP32-S31 requires the register-level writeback
/// operation to be issued twice; this is the exact source-side operation used
/// by ESP-IDF's async memcpy path. Source data is deliberately not invalidated.
#[allow(
    unsafe_code,
    reason = "the PAC marks raw CACHE field images unsafe even though the validated values are fixed by this semantic operation"
)]
#[inline(never)]
#[unsafe(link_section = ".rwtext.cache_maintenance")]
pub fn writeback_psram_for_dma_read(storage: &mut [u8]) -> Result<(), PsramCacheWritebackError> {
    let (start, size) = cache_aligned_psram_range(storage.as_ptr() as usize, storage.len())?;

    critical_section::with(|_| {
        let cache = CACHE::regs();
        cache
            .sync_map()
            .write(|writer| unsafe { writer.sync_map().bits(CACHE_MAP_L1_DCACHE) });
        cache
            .sync_addr()
            .write(|writer| unsafe { writer.sync_addr().bits(start) });
        cache
            .sync_size()
            .write(|writer| unsafe { writer.sync_size().bits(size) });
        for _ in 0..2 {
            // `SYNC_CTRL` resets to `INVALIDATE_ENA = 1`. A normal PAC
            // `write` starts from that reset image and would therefore
            // publish the invalid, mutually-exclusive value 0b0101.
            // ESP-IDF writes exactly `CACHE_WRITEBACK_ENA` (0b0100).
            unsafe {
                cache
                    .sync_ctrl()
                    .write_with_zero(|writer| writer.writeback_ena().set_bit());
            }
            while cache.sync_ctrl().read().sync_done().bit_is_clear() {}
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psram_range_expands_to_complete_cache_lines() {
        assert_eq!(
            cache_aligned_psram_range(PSRAM_LOW + 65, 64),
            Ok(((PSRAM_LOW + 64) as u32, 128))
        );
    }

    #[test]
    fn invalid_or_wrapping_ranges_fail_closed() {
        assert_eq!(
            cache_aligned_psram_range(PSRAM_LOW, 0),
            Err(PsramCacheWritebackError::Empty)
        );
        assert_eq!(
            cache_aligned_psram_range(PSRAM_LOW - 1, 1),
            Err(PsramCacheWritebackError::OutsidePsram)
        );
        assert_eq!(
            cache_aligned_psram_range(usize::MAX, 2),
            Err(PsramCacheWritebackError::AddressOverflow)
        );
    }
}
