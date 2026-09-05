//! Transfer lifecycle and exclusive retention of channel, payloads, and descriptors.

#[cfg(feature = "psram-dma-diagnostic")]
use super::registers::terminal_status;
use super::{
    completion::channel0_interrupt,
    descriptor::{
        AxiGdmaDescriptor, BurstSize, build_chain, build_segment_chains, required_descriptors,
        validate_descriptors,
    },
    registers::{
        INTERNAL_SRAM_END, INTERNAL_SRAM_START, PSRAM_END, PSRAM_START, RX_ERRORS, TX_ERRORS,
        disable_channel_interrupts, dma_fence, enable_and_configure_group,
    },
};
use crate::{PsramCacheWritebackError, writeback_psram_for_dma_read};
use core::marker::PhantomData;
use esp_hal::interrupt::{InterruptHandler, Priority};
use esp_hal::peripherals::DMA_AXI_CH0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AxiGdmaMem2MemError {
    Empty,
    LengthMismatch,
    SourceOutsideDmaMemory,
    DestinationOutsideInternalSram,
    SourceAlignment,
    DestinationAlignment,
    DescriptorAlignment,
    DescriptorOutsideInternalSram,
    InsufficientDescriptors,
    AddressOverflow,
    CacheWriteback(PsramCacheWritebackError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AxiGdmaMem2MemTransferError {
    Timeout,
    Hardware { rx_raw: u32, tx_raw: u32 },
    DescriptorWriteback,
    ReceivedLength { expected: usize, actual: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AxiGdmaMem2MemReport {
    pub bytes: usize,
    pub descriptors: usize,
    pub rx_raw: u32,
    pub tx_raw: u32,
}

/// One discontiguous source/destination pair retained by an M2M transfer.
///
/// The mutable borrows are the ownership proof: neither allocation can be
/// observed, recycled or mutated until the prepared/active transfer is
/// dropped. A PSRAM source must own every cache line touched by its range,
/// because preparation writes those complete lines back for the DMA
/// reader.
pub struct AxiGdmaMem2MemSegment<'buffer> {
    pub(super) destination: &'buffer mut [u8],
    pub(super) source: &'buffer mut [u8],
}

impl<'buffer> AxiGdmaMem2MemSegment<'buffer> {
    pub fn new(destination: &'buffer mut [u8], source: &'buffer mut [u8]) -> Self {
        Self {
            destination,
            source,
        }
    }

    pub fn destination(&self) -> &[u8] {
        self.destination
    }

    pub fn source(&self) -> &[u8] {
        self.source
    }

    pub fn len(&self) -> usize {
        self.source.len()
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }
}

/// Exclusive owner of ESP32-S31 AXI-GDMA channel zero in M2M mode.
pub struct AxiGdmaMem2Mem<'d> {
    _channel: DMA_AXI_CH0<'d>,
    _not_send: PhantomData<*mut ()>,
}

enum PayloadOwner<'transfer, 'buffer> {
    Contiguous {
        destination: &'transfer mut [u8],
        source: &'transfer mut [u8],
    },
    Segments(&'transfer mut [AxiGdmaMem2MemSegment<'buffer>]),
}

impl PayloadOwner<'_, '_> {
    fn retain(&mut self) {
        match self {
            Self::Contiguous {
                destination,
                source,
            } => {
                core::hint::black_box(destination);
                core::hint::black_box(source);
            }
            Self::Segments(segments) => {
                core::hint::black_box(segments);
            }
        }
    }
}

/// A configured transfer that has not yet been published to hardware.
pub struct AxiGdmaMem2MemPreparedOwner<'transfer, 'buffer, 'd> {
    driver: &'transfer mut AxiGdmaMem2Mem<'d>,
    payload: PayloadOwner<'transfer, 'buffer>,
    rx_descriptors: &'transfer mut [AxiGdmaDescriptor],
    tx_descriptors: &'transfer mut [AxiGdmaDescriptor],
    descriptor_count: usize,
    expected_bytes: usize,
}

pub type AxiGdmaMem2MemPrepared<'a, 'd> = AxiGdmaMem2MemPreparedOwner<'a, 'a, 'd>;

pub type AxiGdmaMem2MemSegmentsPrepared<'transfer, 'buffer, 'd> =
    AxiGdmaMem2MemPreparedOwner<'transfer, 'buffer, 'd>;

/// An active transfer retaining exclusive ownership of all DMA memory.
pub struct AxiGdmaMem2MemTransferOwner<'transfer, 'buffer, 'd> {
    driver: &'transfer mut AxiGdmaMem2Mem<'d>,
    payload: PayloadOwner<'transfer, 'buffer>,
    rx_descriptors: &'transfer mut [AxiGdmaDescriptor],
    tx_descriptors: &'transfer mut [AxiGdmaDescriptor],
    descriptor_count: usize,
    expected_bytes: usize,
    pub(super) active: bool,
}

pub type AxiGdmaMem2MemTransfer<'a, 'd> = AxiGdmaMem2MemTransferOwner<'a, 'a, 'd>;

pub type AxiGdmaMem2MemSegmentsTransfer<'transfer, 'buffer, 'd> =
    AxiGdmaMem2MemTransferOwner<'transfer, 'buffer, 'd>;

impl<'d> AxiGdmaMem2Mem<'d> {
    pub fn new(channel: DMA_AXI_CH0<'d>) -> Self {
        enable_and_configure_group();
        let mut this = Self {
            _channel: channel,
            _not_send: PhantomData,
        };
        this.stop_and_reset_channel();
        let handler = InterruptHandler::new(channel0_interrupt, Priority::Priority1);
        this._channel.bind_dma_in_interrupt(handler);
        this._channel.bind_dma_out_interrupt(handler);
        this._channel.enable_dma_in_interrupt(Priority::Priority1);
        this._channel.enable_dma_out_interrupt(Priority::Priority1);
        this
    }

    pub fn prepare<'a>(
        &'a mut self,
        destination: &'a mut [u8],
        source: &'a mut [u8],
        rx_descriptors: &'a mut [AxiGdmaDescriptor],
        tx_descriptors: &'a mut [AxiGdmaDescriptor],
        burst: BurstSize,
    ) -> Result<AxiGdmaMem2MemPrepared<'a, 'd>, AxiGdmaMem2MemError> {
        let source_is_psram = validate_payloads(destination, source, burst)?;
        let expected_bytes = source.len();
        let descriptor_count = required_descriptors(source.len(), burst);
        validate_descriptors(rx_descriptors, descriptor_count)?;
        validate_descriptors(tx_descriptors, descriptor_count)?;

        if source_is_psram {
            writeback_psram_for_dma_read(source).map_err(AxiGdmaMem2MemError::CacheWriteback)?;
        }
        build_chain(
            tx_descriptors,
            source.as_mut_ptr(),
            source.len(),
            descriptor_count,
            burst,
            true,
        );
        build_chain(
            rx_descriptors,
            destination.as_mut_ptr(),
            destination.len(),
            descriptor_count,
            burst,
            false,
        );
        self.configure_channel(
            rx_descriptors.as_mut_ptr() as u32,
            tx_descriptors.as_mut_ptr() as u32,
            burst,
        );

        Ok(AxiGdmaMem2MemPrepared {
            driver: self,
            payload: PayloadOwner::Contiguous {
                destination,
                source,
            },
            rx_descriptors,
            tx_descriptors,
            descriptor_count,
            expected_bytes,
        })
    }

    /// Prepare one hardware transaction over discontiguous packet pairs.
    ///
    /// The descriptor chains preserve segment boundaries, but publish one
    /// terminal EOF for the whole batch. This is the primitive needed by
    /// the Wi-Fi TXQ materializer: every packet remains a separate typed
    /// allocation while AXI-GDMA receives one kick.
    pub fn prepare_segments<'transfer, 'buffer>(
        &'transfer mut self,
        segments: &'transfer mut [AxiGdmaMem2MemSegment<'buffer>],
        rx_descriptors: &'transfer mut [AxiGdmaDescriptor],
        tx_descriptors: &'transfer mut [AxiGdmaDescriptor],
        burst: BurstSize,
    ) -> Result<AxiGdmaMem2MemSegmentsPrepared<'transfer, 'buffer, 'd>, AxiGdmaMem2MemError> {
        if segments.is_empty() {
            return Err(AxiGdmaMem2MemError::Empty);
        }

        let mut descriptor_count = 0usize;
        let mut expected_bytes = 0usize;
        for segment in segments.iter_mut() {
            let source_is_psram =
                validate_payloads(&*segment.destination, &*segment.source, burst)?;
            descriptor_count = descriptor_count
                .checked_add(required_descriptors(segment.len(), burst))
                .ok_or(AxiGdmaMem2MemError::AddressOverflow)?;
            expected_bytes = expected_bytes
                .checked_add(segment.len())
                .ok_or(AxiGdmaMem2MemError::AddressOverflow)?;
            if source_is_psram {
                writeback_psram_for_dma_read(&mut *segment.source)
                    .map_err(AxiGdmaMem2MemError::CacheWriteback)?;
            }
        }
        validate_descriptors(rx_descriptors, descriptor_count)?;
        validate_descriptors(tx_descriptors, descriptor_count)?;

        build_segment_chains(
            tx_descriptors,
            rx_descriptors,
            segments,
            descriptor_count,
            burst,
        );
        self.configure_channel(
            rx_descriptors.as_mut_ptr() as u32,
            tx_descriptors.as_mut_ptr() as u32,
            burst,
        );

        Ok(AxiGdmaMem2MemPreparedOwner {
            driver: self,
            payload: PayloadOwner::Segments(segments),
            rx_descriptors,
            tx_descriptors,
            descriptor_count,
            expected_bytes,
        })
    }
}

impl<'transfer, 'buffer, 'd> AxiGdmaMem2MemPreparedOwner<'transfer, 'buffer, 'd> {
    /// Publish both descriptor chains and start the RX side before TX.
    ///
    /// The returned owner can remain alive while the CPU prepares work
    /// that does not alias either payload or either descriptor list.
    pub fn start(self) -> AxiGdmaMem2MemTransferOwner<'transfer, 'buffer, 'd> {
        self.driver.start();
        AxiGdmaMem2MemTransferOwner {
            driver: self.driver,
            payload: self.payload,
            rx_descriptors: self.rx_descriptors,
            tx_descriptors: self.tx_descriptors,
            descriptor_count: self.descriptor_count,
            expected_bytes: self.expected_bytes,
            active: true,
        }
    }
}

impl AxiGdmaMem2MemTransferOwner<'_, '_, '_> {
    /// Diagnostic baseline that deliberately burns the caller's budget.
    ///
    /// Product code must await the transfer's [`core::future::Future`] implementation.
    /// This method exists only so the HIL probe can quantify the cost of
    /// blocking against interrupt-driven completion.
    #[cfg(feature = "psram-dma-diagnostic")]
    #[allow(
        clippy::disallowed_methods,
        reason = "the diagnostic compares an explicit blocking baseline with the async product path"
    )]
    pub fn wait_blocking(
        mut self,
        spin_budget: u32,
    ) -> Result<AxiGdmaMem2MemReport, AxiGdmaMem2MemTransferError> {
        let mut remaining = spin_budget;
        let (rx_raw, tx_raw) = loop {
            if let Some(status) = terminal_status() {
                break status;
            }
            if remaining == 0 {
                self.driver.stop_and_reset_channel();
                self.active = false;
                return Err(AxiGdmaMem2MemTransferError::Timeout);
            }
            remaining -= 1;
            core::hint::spin_loop();
        };

        self.finish(rx_raw, tx_raw)
    }

    pub(super) fn finish(
        &mut self,
        rx_raw: u32,
        tx_raw: u32,
    ) -> Result<AxiGdmaMem2MemReport, AxiGdmaMem2MemTransferError> {
        disable_channel_interrupts();
        dma_fence();
        self.driver.stop_and_reset_channel();
        self.active = false;
        if rx_raw & RX_ERRORS != 0 || tx_raw & TX_ERRORS != 0 {
            return Err(AxiGdmaMem2MemTransferError::Hardware { rx_raw, tx_raw });
        }

        let mut received = 0usize;
        for descriptor in &self.rx_descriptors[..self.descriptor_count] {
            if descriptor.owner_is_dma() {
                return Err(AxiGdmaMem2MemTransferError::DescriptorWriteback);
            }
            received += descriptor.received_bytes();
        }
        for descriptor in &self.tx_descriptors[..self.descriptor_count] {
            if descriptor.owner_is_dma() {
                return Err(AxiGdmaMem2MemTransferError::DescriptorWriteback);
            }
        }
        if received != self.expected_bytes {
            return Err(AxiGdmaMem2MemTransferError::ReceivedLength {
                expected: self.expected_bytes,
                actual: received,
            });
        }

        // Keep every payload borrow live until all hardware and descriptor
        // observations have completed.
        self.payload.retain();
        Ok(AxiGdmaMem2MemReport {
            bytes: received,
            descriptors: self.descriptor_count,
            rx_raw,
            tx_raw,
        })
    }
}

impl Drop for AxiGdmaMem2MemTransferOwner<'_, '_, '_> {
    fn drop(&mut self) {
        if self.active {
            disable_channel_interrupts();
            self.driver.stop_and_reset_channel();
        }
    }
}

impl Drop for AxiGdmaMem2Mem<'_> {
    fn drop(&mut self) {
        self.stop_and_reset_channel();
    }
}

fn validate_payloads(
    destination: &[u8],
    source: &[u8],
    burst: BurstSize,
) -> Result<bool, AxiGdmaMem2MemError> {
    if source.is_empty() {
        return Err(AxiGdmaMem2MemError::Empty);
    }
    if source.len() != destination.len() {
        return Err(AxiGdmaMem2MemError::LengthMismatch);
    }
    let source_address = source.as_ptr() as usize;
    let source_is_psram = range_within(source_address, source.len(), PSRAM_START, PSRAM_END)?;
    let source_is_internal = range_within(
        source_address,
        source.len(),
        INTERNAL_SRAM_START,
        INTERNAL_SRAM_END,
    )?;
    if !source_is_psram && !source_is_internal {
        return Err(AxiGdmaMem2MemError::SourceOutsideDmaMemory);
    }
    validate_range(
        destination.as_ptr() as usize,
        destination.len(),
        INTERNAL_SRAM_START,
        INTERNAL_SRAM_END,
    )
    .map_err(|error| match error {
        AxiGdmaMem2MemError::AddressOverflow => error,
        _ => AxiGdmaMem2MemError::DestinationOutsideInternalSram,
    })?;

    // AXI-GDMA accepts arbitrary terminal lengths and word-aligned payload
    // addresses even with descriptor/data bursting enabled. Requiring a
    // complete burst here would reject normal Ethernet lengths, while a
    // cache-line requirement belongs to the owning PSRAM allocation, not
    // to the initialized packet prefix passed to this driver.
    let _ = burst;
    const PAYLOAD_ALIGNMENT: usize = core::mem::align_of::<u32>();
    if !(source.as_ptr() as usize).is_multiple_of(PAYLOAD_ALIGNMENT) {
        return Err(AxiGdmaMem2MemError::SourceAlignment);
    }
    if !(destination.as_ptr() as usize).is_multiple_of(PAYLOAD_ALIGNMENT) {
        return Err(AxiGdmaMem2MemError::DestinationAlignment);
    }
    Ok(source_is_psram)
}

pub(super) fn validate_range(
    address: usize,
    size: usize,
    start: usize,
    end: usize,
) -> Result<(), AxiGdmaMem2MemError> {
    let range_end = address
        .checked_add(size)
        .ok_or(AxiGdmaMem2MemError::AddressOverflow)?;
    if address < start || range_end > end {
        return Err(AxiGdmaMem2MemError::SourceOutsideDmaMemory);
    }
    Ok(())
}

fn range_within(
    address: usize,
    size: usize,
    start: usize,
    end: usize,
) -> Result<bool, AxiGdmaMem2MemError> {
    let range_end = address
        .checked_add(size)
        .ok_or(AxiGdmaMem2MemError::AddressOverflow)?;
    Ok(address >= start && range_end <= end)
}
