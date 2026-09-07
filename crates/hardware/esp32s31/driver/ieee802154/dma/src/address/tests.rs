use super::*;

#[test]
fn exact_frame_range_boundaries_are_enforced() {
    assert_eq!(DmaFrameAddress::try_new(DMA_LOW).unwrap().as_u32(), DMA_LOW);
    assert_eq!(
        DmaFrameAddress::try_new(DMA_HIGH - FRAME_BUFFER_SIZE as u32)
            .unwrap()
            .as_u32(),
        DMA_HIGH - FRAME_BUFFER_SIZE as u32
    );
    assert_eq!(
        DmaFrameAddress::try_new(DMA_LOW - 4),
        Err(DmaAddressError::OutOfRange)
    );
    assert_eq!(
        DmaFrameAddress::try_new(DMA_HIGH - FRAME_BUFFER_SIZE as u32 + 4),
        Err(DmaAddressError::OutOfRange)
    );
    assert_eq!(
        DmaFrameAddress::try_new(DMA_LOW + 1),
        Err(DmaAddressError::Unaligned)
    );
}

#[test]
fn complete_pool_span_must_fit() {
    let base = DmaFrameAddress::try_new(DMA_HIGH - 3 * FRAME_BUFFER_SIZE as u32).unwrap();
    assert_eq!(base.validates_frame_count(3), Ok(()));
    assert_eq!(
        base.validates_frame_count(4),
        Err(DmaAddressError::OutOfRange)
    );
    assert_eq!(
        base.validates_frame_count(usize::MAX),
        Err(DmaAddressError::RegionTooLarge)
    );
}
