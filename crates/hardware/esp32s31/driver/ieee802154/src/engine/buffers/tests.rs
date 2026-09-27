use super::*;

/// A block is DMA-visible only when it lies wholly in internal SRAM; PSRAM
/// and a block crossing the window's end are not.
#[test]
fn only_internal_sram_is_dma_visible() {
    let size = core::mem::size_of::<Ieee802154EngineBuffers>();
    assert!(dma_window_contains(DMA_WINDOW.start, size));
    assert!(dma_window_contains(DMA_WINDOW.end - size, size));
    assert!(!dma_window_contains(DMA_WINDOW.end - size + 1, size));
    assert!(!dma_window_contains(DMA_WINDOW.start - 4, size));
    // Where the PSRAM data profile placed the buffers on the stand.
    assert!(!dma_window_contains(0x500f_142c, size));
    assert!(!dma_window_contains(usize::MAX - 2, size));
}
