use super::*;

/// Coherent mutable radio resources used by one finite attempt.
pub struct Esp32s31StaAttemptRadio<
    'hardware,
    'transmit,
    'storage,
    H,
    C,
    D,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    pub hardware: &'hardware mut H,
    pub channel: C,
    pub receive: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    pub rx_storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub transmit: &'transmit mut T,
}

impl<
    'hardware,
    'transmit,
    'storage,
    H,
    C,
    D,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31StaAttemptRadio<
        'hardware,
        'transmit,
        'storage,
        H,
        C,
        D,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub const fn new(
        hardware: &'hardware mut H,
        channel: C,
        receive: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
        rx_storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        transmit: &'transmit mut T,
    ) -> Self {
        Self {
            hardware,
            channel,
            receive,
            rx_storage,
            transmit,
        }
    }
}

/// Allocation-free frame scratch used by management and EAPOL parsing.
pub struct Esp32s31StaAttemptStorage<'scratch> {
    pub frame: &'scratch mut [u8],
}

impl<'scratch> Esp32s31StaAttemptStorage<'scratch> {
    pub const fn new(frame: &'scratch mut [u8]) -> Self {
        Self { frame }
    }
}
