//! Pinned descriptor and backing ownership for one TX A-MPDU.
//!
//! Internally buffered aggregates retain their static arena directly.
//! External zero-copy aggregates receive prepare/start authority only through
//! an owner which also retains every referenced [`StableDmaBacking`] lease.

use core::{marker::PhantomPinned, mem, pin::Pin};

use open_esp_radio_dma::{HardwareOwnedTxDma, PreparedTxDma, StableDmaBacking, StableDmaRange};
use open_esp_radio_esp32s31_registers::MacTxQueueDetached;
use pin_project::pin_project;

use crate::descriptor::{
    BIT_30, DESCRIPTOR_BYTES, Descriptor, descriptor_address_valid, dma_range_valid, tx_owned_word,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AmpduDmaState {
    #[default]
    Free,
    Reserved,
    HardwareOwned,
    Completed,
    Detached,
    ResetRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduDmaStorageError {
    Address,
    Busy,
    Count,
    InvalidLength,
    State,
}

/// One internal A-MPDU descriptor entry, excluding its derived addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduInternalDescriptor {
    pub buffer_capacity: u32,
    pub transfer_length: u32,
}

/// One descriptor backed by a retained external DMA lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduExternalDescriptor {
    pub backing_index: u8,
    pub dma_offset: usize,
    pub buffer_capacity: u32,
    pub transfer_length: u32,
}

#[repr(C, align(16))]
struct AmpduDmaBuffer<const BUFFER_SIZE: usize>([u8; BUFFER_SIZE]);

/// Final static allocation for an internal-buffer A-MPDU arena.
#[pin_project]
pub struct AmpduDmaStorage<const SLOTS: usize, const BUFFER_SIZE: usize> {
    #[pin]
    descriptors: [Descriptor; SLOTS],
    #[pin]
    buffers: [AmpduDmaBuffer<BUFFER_SIZE>; SLOTS],
    state: AmpduDmaState,
    #[pin]
    _pin: PhantomPinned,
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> AmpduDmaStorage<SLOTS, BUFFER_SIZE> {
    pub const fn new() -> Self {
        Self {
            descriptors: [const { Descriptor::new() }; SLOTS],
            buffers: [const { AmpduDmaBuffer([0; BUFFER_SIZE]) }; SLOTS],
            state: AmpduDmaState::Free,
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_pointer_width = "32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>, AmpduDmaStorageError> {
        let descriptor_base = u32::try_from(storage.descriptors.as_ptr().addr())
            .map_err(|_| AmpduDmaStorageError::Address)?;
        let buffer_base = if BUFFER_SIZE == 0 {
            0
        } else {
            u32::try_from(storage.buffers.as_ptr().addr())
                .map_err(|_| AmpduDmaStorageError::Address)?
        };
        Self::pin_static_inner(storage, descriptor_base, buffer_base)
    }

    /// Bind deterministic target-like addresses in a native model.
    #[cfg(not(target_pointer_width = "32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        descriptor_base: u32,
        buffer_base: u32,
    ) -> Result<PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>, AmpduDmaStorageError> {
        Self::pin_static_inner(storage, descriptor_base, buffer_base)
    }

    #[allow(
        unsafe_code,
        reason = "unique static aggregate storage becomes retained DMA authority once"
    )]
    fn pin_static_inner(
        storage: &'static mut Self,
        descriptor_base: u32,
        buffer_base: u32,
    ) -> Result<PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>, AmpduDmaStorageError> {
        let descriptor_count = u8::try_from(SLOTS).map_err(|_| AmpduDmaStorageError::Count)?;
        if descriptor_count == 0 || !descriptor_address_valid(descriptor_base) {
            return Err(AmpduDmaStorageError::Count);
        }
        let descriptor_len = u32::try_from(core::mem::size_of_val(&storage.descriptors))
            .map_err(|_| AmpduDmaStorageError::Address)?;
        if !dma_range_valid(descriptor_base, descriptor_len) {
            return Err(AmpduDmaStorageError::Address);
        }
        let descriptor_range = unsafe {
            // SAFETY: the returned owner permanently retains this unique
            // static descriptor allocation and owns every state transition.
            StableDmaRange::from_raw_parts(descriptor_base, descriptor_len)
                .ok_or(AmpduDmaStorageError::Address)?
        };

        let buffer_stride = u32::try_from(core::mem::size_of::<AmpduDmaBuffer<BUFFER_SIZE>>())
            .map_err(|_| AmpduDmaStorageError::Address)?;
        let buffer_capacity =
            u32::try_from(BUFFER_SIZE).map_err(|_| AmpduDmaStorageError::Address)?;
        let buffer_range = if BUFFER_SIZE == 0 {
            None
        } else {
            let buffer_len = u32::try_from(core::mem::size_of_val(&storage.buffers))
                .map_err(|_| AmpduDmaStorageError::Address)?;
            if !dma_range_valid(buffer_base, buffer_len) {
                return Err(AmpduDmaStorageError::Address);
            }
            Some(unsafe {
                // SAFETY: the same static owner retains the entire aligned
                // internal buffer arena for every DMA lifecycle state.
                StableDmaRange::from_raw_parts(buffer_base, buffer_len)
                    .ok_or(AmpduDmaStorageError::Address)?
            })
        };

        Ok(PinnedAmpduDmaStorage {
            storage: Pin::static_mut(storage),
            binding: AmpduDmaBinding {
                descriptor_base,
                descriptor_count,
                descriptor_range,
                buffer_base,
                buffer_stride,
                buffer_capacity,
                buffer_range,
            },
        })
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> Default for AmpduDmaStorage<SLOTS, BUFFER_SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

/// Private-range authority for one statically retained aggregate arena.
pub struct AmpduDmaBinding {
    descriptor_base: u32,
    descriptor_count: u8,
    descriptor_range: StableDmaRange<'static>,
    buffer_base: u32,
    buffer_stride: u32,
    buffer_capacity: u32,
    buffer_range: Option<StableDmaRange<'static>>,
}

impl AmpduDmaBinding {
    pub const fn descriptor_head(&self) -> u32 {
        self.descriptor_base
    }

    pub const fn descriptor_count(&self) -> u8 {
        self.descriptor_count
    }

    pub fn descriptor_address(&self, index: usize) -> Option<u32> {
        (index < usize::from(self.descriptor_count))
            .then(|| {
                self.descriptor_base
                    .checked_add(u32::try_from(index).ok()?.checked_mul(DESCRIPTOR_BYTES)?)
            })
            .flatten()
            .filter(|address| self.descriptor_range.contains(*address, DESCRIPTOR_BYTES))
    }

    pub fn internal_buffer_address(&self, index: usize) -> Option<u32> {
        let range = self.buffer_range.as_ref()?;
        (index < usize::from(self.descriptor_count))
            .then(|| {
                self.buffer_base
                    .checked_add(u32::try_from(index).ok()?.checked_mul(self.buffer_stride)?)
            })
            .flatten()
            .filter(|address| range.contains(*address, self.buffer_capacity))
    }

    pub const fn internal_buffer_capacity(&self) -> u32 {
        self.buffer_capacity
    }
}

pub struct AmpduDmaStart<'owner> {
    binding: &'owner AmpduDmaBinding,
}

#[allow(
    unsafe_code,
    reason = "start token exists only after aggregate hardware ownership is recorded"
)]
unsafe impl HardwareOwnedTxDma for AmpduDmaStart<'_> {
    fn descriptor_head(&self) -> u32 {
        self.binding.descriptor_base
    }
}

pub struct PinnedAmpduDmaStorage<const SLOTS: usize, const BUFFER_SIZE: usize> {
    storage: Pin<&'static mut AmpduDmaStorage<SLOTS, BUFFER_SIZE>>,
    binding: AmpduDmaBinding,
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE> {
    pub fn state(&self) -> AmpduDmaState {
        self.storage.as_ref().get_ref().state
    }

    pub const fn binding(&self) -> &AmpduDmaBinding {
        &self.binding
    }

    pub fn begin(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.transition(AmpduDmaState::Free, AmpduDmaState::Reserved)
    }

    pub fn buffer_mut(
        &mut self,
        index: usize,
    ) -> Result<&mut [u8; BUFFER_SIZE], AmpduDmaStorageError> {
        if !matches!(self.state(), AmpduDmaState::Free | AmpduDmaState::Reserved) {
            return Err(AmpduDmaStorageError::Busy);
        }
        self.storage
            .as_mut()
            .project()
            .buffers
            .get_mut()
            .get_mut(index)
            .map(|buffer| &mut buffer.0)
            .ok_or(AmpduDmaStorageError::Count)
    }

    /// Publish a complete chain backed only by this owner's internal buffers.
    pub fn publish_internal_chain(
        &mut self,
        entries: &[AmpduInternalDescriptor],
    ) -> Result<AmpduDmaPublication<'_, SLOTS, BUFFER_SIZE>, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        if entries.is_empty() || entries.len() > SLOTS {
            return Err(AmpduDmaStorageError::Count);
        }

        for (index, entry) in entries.iter().enumerate() {
            if entry.buffer_capacity == 0
                || entry.buffer_capacity > self.binding.buffer_capacity
                || entry.transfer_length == 0
                || entry.transfer_length > entry.buffer_capacity
            {
                return Err(AmpduDmaStorageError::InvalidLength);
            }
            self.binding
                .internal_buffer_address(index)
                .ok_or(AmpduDmaStorageError::Address)?;
            self.binding
                .descriptor_address(index)
                .ok_or(AmpduDmaStorageError::Address)?;
        }

        for (index, entry) in entries.iter().enumerate() {
            let buffer_address = self
                .binding
                .internal_buffer_address(index)
                .ok_or(AmpduDmaStorageError::Address)?;
            let next_address = if index + 1 == entries.len() {
                0
            } else {
                self.binding
                    .descriptor_address(index + 1)
                    .ok_or(AmpduDmaStorageError::Address)?
            };
            let mut word0 = tx_owned_word(entry.buffer_capacity, entry.transfer_length)
                .ok_or(AmpduDmaStorageError::InvalidLength)?;
            if next_address != 0 {
                word0 &= !BIT_30;
            }
            self.storage.as_ref().get_ref().descriptors[index].publish(
                word0,
                buffer_address,
                next_address,
            );
        }

        Ok(AmpduDmaPublication { owner: self })
    }

    pub fn descriptor_word0(&self, index: usize) -> Option<u32> {
        self.storage
            .as_ref()
            .get_ref()
            .descriptors
            .get(index)
            .map(Descriptor::word0)
    }

    pub fn descriptor_next_address(&self, index: usize) -> Option<u32> {
        self.storage
            .as_ref()
            .get_ref()
            .descriptors
            .get(index)
            .map(Descriptor::next_address)
    }

    pub fn descriptor_buffer_address(&self, index: usize) -> Option<u32> {
        self.storage
            .as_ref()
            .get_ref()
            .descriptors
            .get(index)
            .map(Descriptor::buffer_address)
    }

    pub fn cancel(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.clear_to_free(AmpduDmaState::Reserved)
    }

    pub fn mark_completed(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.transition(AmpduDmaState::HardwareOwned, AmpduDmaState::Completed)
    }

    pub fn mark_detached(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        if !detached.confirms_descriptor_head(self.binding.descriptor_base) {
            return Err(AmpduDmaStorageError::Address);
        }
        self.transition(AmpduDmaState::Completed, AmpduDmaState::Detached)
    }

    pub fn detached_buffer(
        &self,
        index: usize,
    ) -> Result<&[u8; BUFFER_SIZE], AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Detached {
            return Err(AmpduDmaStorageError::State);
        }
        self.storage
            .as_ref()
            .get_ref()
            .buffers
            .get(index)
            .map(|buffer| &buffer.0)
            .ok_or(AmpduDmaStorageError::Count)
    }

    pub fn release_detached(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.clear_to_free(AmpduDmaState::Detached)
    }

    pub fn release_aborted(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        if !detached.confirms_descriptor_head(self.binding.descriptor_base) {
            return Err(AmpduDmaStorageError::Address);
        }
        self.clear_to_free(AmpduDmaState::HardwareOwned)
    }

    pub fn quarantine(&mut self) {
        *self.storage.as_mut().project().state = AmpduDmaState::ResetRequired;
    }

    fn transition(
        &mut self,
        expected: AmpduDmaState,
        next: AmpduDmaState,
    ) -> Result<(), AmpduDmaStorageError> {
        let storage = self.storage.as_mut().project();
        if *storage.state != expected {
            return Err(AmpduDmaStorageError::State);
        }
        *storage.state = next;
        Ok(())
    }

    fn clear_to_free(&mut self, expected: AmpduDmaState) -> Result<(), AmpduDmaStorageError> {
        if self.state() != expected {
            return Err(AmpduDmaStorageError::State);
        }
        for descriptor in &self.storage.as_ref().get_ref().descriptors {
            descriptor.publish(0, 0, 0);
        }
        *self.storage.as_mut().project().state = AmpduDmaState::Free;
        Ok(())
    }
}

/// Aggregate DMA owner which retains every externally referenced allocation.
///
/// The descriptor arena remains in static chip storage while `B` may be a
/// movable lease object whose underlying allocation is stable. Moving this
/// owner is safe: descriptor addresses never point at the lease object, only
/// at the allocation guaranteed by its unsafe [`StableDmaBacking`] contract.
pub struct RetainedAmpduDma<B, const SLOTS: usize, const BUFFER_SIZE: usize> {
    dma: Option<PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>>,
    backings: [Option<B>; SLOTS],
    held: usize,
}

impl<B, const SLOTS: usize, const BUFFER_SIZE: usize> RetainedAmpduDma<B, SLOTS, BUFFER_SIZE> {
    pub fn new(dma: PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>) -> Self {
        Self {
            dma: Some(dma),
            backings: [const { None }; SLOTS],
            held: 0,
        }
    }

    pub fn state(&self) -> AmpduDmaState {
        self.dma().state()
    }

    pub const fn held_backing_count(&self) -> usize {
        self.held
    }

    pub fn descriptor_head(&self) -> u32 {
        self.dma().binding().descriptor_head()
    }

    pub fn begin(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().begin()
    }

    /// Re-reserve a detached aggregate for a selective BlockAck retry.
    ///
    /// All leases stay retained. The new descriptor chain may reference a
    /// compacted subset of them, while acknowledged frames remain owned until
    /// the retry reaches its final detach/release edge.
    pub fn begin_retry(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut()
            .transition(AmpduDmaState::Detached, AmpduDmaState::Reserved)
    }

    pub fn push_backing(&mut self, backing: B) -> Result<u8, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        let index = self.held;
        let backing_index = u8::try_from(index).map_err(|_| AmpduDmaStorageError::Count)?;
        let slot = self
            .backings
            .get_mut(index)
            .ok_or(AmpduDmaStorageError::Count)?;
        if slot.is_some() {
            return Err(AmpduDmaStorageError::Busy);
        }
        *slot = Some(backing);
        self.held += 1;
        Ok(backing_index)
    }

    /// Undo the most recent reserved backing insertion.
    ///
    /// Aggregate metadata is prepared by the upper MAC after the lease has
    /// entered this owner.  If that preparation rejects the frame, this
    /// strictly-LIFO edge returns the lease without leaving a hole in the
    /// backing-index space later published to DMA descriptors.
    pub fn pop_last_backing(&mut self, index: u8) -> Result<B, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        let index = usize::from(index);
        if self.held.checked_sub(1) != Some(index) {
            return Err(AmpduDmaStorageError::Count);
        }
        let backing = self.backings[index]
            .take()
            .ok_or(AmpduDmaStorageError::Count)?;
        self.held = index;
        Ok(backing)
    }

    pub fn reserved_backing_mut(&mut self, index: u8) -> Result<&mut B, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        self.backings
            .get_mut(usize::from(index))
            .and_then(Option::as_mut)
            .ok_or(AmpduDmaStorageError::Count)
    }

    pub fn detached_backing_mut(&mut self, index: u8) -> Result<&mut B, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Detached {
            return Err(AmpduDmaStorageError::State);
        }
        self.backings
            .get_mut(usize::from(index))
            .and_then(Option::as_mut)
            .ok_or(AmpduDmaStorageError::Count)
    }

    pub fn cancel(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().cancel()?;
        self.drop_backings();
        Ok(())
    }

    pub fn mark_completed(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().mark_completed()
    }

    pub fn mark_detached(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().mark_detached(detached)
    }

    pub fn release_detached(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().release_detached()?;
        self.drop_backings();
        Ok(())
    }

    pub fn release_aborted(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().release_aborted(detached)?;
        self.drop_backings();
        Ok(())
    }

    pub fn quarantine(&mut self) {
        self.dma_mut().quarantine();
    }

    #[allow(clippy::result_large_err)]
    pub fn try_into_dma(mut self) -> Result<PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>, Self> {
        if self.state() != AmpduDmaState::Free || self.held != 0 {
            return Err(self);
        }
        Ok(self
            .dma
            .take()
            .expect("retained aggregate owner contains its DMA arena"))
    }

    fn dma(&self) -> &PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE> {
        self.dma
            .as_ref()
            .expect("retained aggregate owner contains its DMA arena")
    }

    fn dma_mut(&mut self) -> &mut PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE> {
        self.dma
            .as_mut()
            .expect("retained aggregate owner contains its DMA arena")
    }

    fn drop_backings(&mut self) {
        for backing in &mut self.backings[..self.held] {
            drop(backing.take());
        }
        self.held = 0;
    }

    fn forget_backings(&mut self) {
        for backing in &mut self.backings[..self.held] {
            if let Some(backing) = backing.take() {
                mem::forget(backing);
            }
        }
        self.held = 0;
    }
}

impl<B: StableDmaBacking, const SLOTS: usize, const BUFFER_SIZE: usize>
    RetainedAmpduDma<B, SLOTS, BUFFER_SIZE>
{
    /// Resolve an upper-MAC frame range to a retained backing and offset.
    ///
    /// The caller supplies only integer identity metadata. This owner obtains
    /// the actual slice from the lease and proves that the complete descriptor
    /// capacity remains inside it before returning a publication entry.
    pub fn reserved_external_descriptor(
        &mut self,
        address: usize,
        buffer_capacity: u32,
        transfer_length: u32,
    ) -> Result<AmpduExternalDescriptor, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        let capacity =
            usize::try_from(buffer_capacity).map_err(|_| AmpduDmaStorageError::Address)?;
        for (index, backing) in self.backings[..self.held].iter_mut().enumerate() {
            let Some(backing) = backing.as_mut() else {
                continue;
            };
            let bytes = backing.stable_dma_region().into_mut_slice();
            let Some(offset) = address.checked_sub(bytes.as_ptr().addr()) else {
                continue;
            };
            if offset
                .checked_add(capacity)
                .is_some_and(|end| end <= bytes.len())
            {
                return Ok(AmpduExternalDescriptor {
                    backing_index: u8::try_from(index).map_err(|_| AmpduDmaStorageError::Count)?,
                    dma_offset: offset,
                    buffer_capacity,
                    transfer_length,
                });
            }
        }
        Err(AmpduDmaStorageError::Address)
    }

    /// Borrow a detached range only after the queue-detach proof was consumed.
    pub fn detached_region_mut(
        &mut self,
        address: usize,
        capacity: usize,
    ) -> Result<&mut [u8], AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Detached {
            return Err(AmpduDmaStorageError::State);
        }
        for backing in &mut self.backings[..self.held] {
            let Some(backing) = backing.as_mut() else {
                continue;
            };
            let bytes = backing.stable_dma_region().into_mut_slice();
            let Some(offset) = address.checked_sub(bytes.as_ptr().addr()) else {
                continue;
            };
            let Some(end) = offset.checked_add(capacity) else {
                continue;
            };
            if end <= bytes.len() {
                return Ok(&mut bytes[offset..end]);
            }
        }
        Err(AmpduDmaStorageError::Address)
    }

    /// Publish a complete chain whose entries resolve into retained leases.
    pub fn publish_external_chain(
        &mut self,
        entries: &[AmpduExternalDescriptor],
    ) -> Result<RetainedAmpduDmaPublication<'_, B, SLOTS, BUFFER_SIZE>, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        if entries.is_empty() || entries.len() > SLOTS {
            return Err(AmpduDmaStorageError::Count);
        }

        let mut buffer_addresses = [0_u32; SLOTS];
        for (index, entry) in entries.iter().enumerate() {
            if entry.buffer_capacity == 0
                || entry.transfer_length == 0
                || entry.transfer_length > entry.buffer_capacity
            {
                return Err(AmpduDmaStorageError::InvalidLength);
            }
            let backing = self
                .backings
                .get_mut(usize::from(entry.backing_index))
                .and_then(Option::as_mut)
                .ok_or(AmpduDmaStorageError::Count)?;
            let bytes = backing.stable_dma_region().into_mut_slice();
            let capacity = usize::try_from(entry.buffer_capacity)
                .map_err(|_| AmpduDmaStorageError::Address)?;
            let _end = entry
                .dma_offset
                .checked_add(capacity)
                .filter(|end| *end <= bytes.len())
                .ok_or(AmpduDmaStorageError::Address)?;
            let address = bytes
                .as_ptr()
                .addr()
                .checked_add(entry.dma_offset)
                .ok_or(AmpduDmaStorageError::Address)?;
            buffer_addresses[index] = external_dma_address(address, entry.buffer_capacity)?;
            self.dma()
                .binding
                .descriptor_address(index)
                .ok_or(AmpduDmaStorageError::Address)?;
        }

        for (index, entry) in entries.iter().enumerate() {
            let next_address = if index + 1 == entries.len() {
                0
            } else {
                self.dma()
                    .binding
                    .descriptor_address(index + 1)
                    .ok_or(AmpduDmaStorageError::Address)?
            };
            let mut word0 = tx_owned_word(entry.buffer_capacity, entry.transfer_length)
                .ok_or(AmpduDmaStorageError::InvalidLength)?;
            if next_address != 0 {
                word0 &= !BIT_30;
            }
            self.dma().storage.as_ref().get_ref().descriptors[index].publish(
                word0,
                buffer_addresses[index],
                next_address,
            );
        }

        Ok(RetainedAmpduDmaPublication { owner: self })
    }
}

impl<B, const SLOTS: usize, const BUFFER_SIZE: usize> Drop
    for RetainedAmpduDma<B, SLOTS, BUFFER_SIZE>
{
    fn drop(&mut self) {
        if self.dma.is_none() {
            self.drop_backings();
            return;
        }
        if matches!(
            self.state(),
            AmpduDmaState::HardwareOwned | AmpduDmaState::Completed | AmpduDmaState::ResetRequired
        ) {
            self.forget_backings();
        } else {
            self.drop_backings();
        }
    }
}

#[cfg(target_pointer_width = "32")]
fn external_dma_address(address: usize, capacity: u32) -> Result<u32, AmpduDmaStorageError> {
    let address = u32::try_from(address).map_err(|_| AmpduDmaStorageError::Address)?;
    dma_range_valid(address, capacity)
        .then_some(address)
        .ok_or(AmpduDmaStorageError::Address)
}

#[cfg(not(target_pointer_width = "32"))]
fn external_dma_address(address: usize, _capacity: u32) -> Result<u32, AmpduDmaStorageError> {
    // Native models have no asynchronous DMA actor. Truncation supplies only
    // a model descriptor image for host state-machine tests.
    Ok(address as u32)
}

pub struct RetainedAmpduDmaPublication<'owner, B, const SLOTS: usize, const BUFFER_SIZE: usize> {
    owner: &'owner mut RetainedAmpduDma<B, SLOTS, BUFFER_SIZE>,
}

#[allow(
    unsafe_code,
    reason = "publication token is coupled to the owner retaining every external lease"
)]
unsafe impl<B: StableDmaBacking, const SLOTS: usize, const BUFFER_SIZE: usize> PreparedTxDma
    for RetainedAmpduDmaPublication<'_, B, SLOTS, BUFFER_SIZE>
{
    fn descriptor_head(&self) -> u32 {
        self.owner.dma().binding.descriptor_base
    }
}

impl<B, const SLOTS: usize, const BUFFER_SIZE: usize>
    RetainedAmpduDmaPublication<'_, B, SLOTS, BUFFER_SIZE>
{
    pub fn commit<F>(self, start: F)
    where
        F: FnOnce(&AmpduDmaStart<'_>),
    {
        let owner = self.owner;
        let dma = owner.dma_mut();
        *dma.storage.as_mut().project().state = AmpduDmaState::HardwareOwned;
        start(&AmpduDmaStart {
            binding: &dma.binding,
        });
    }
}

pub struct AmpduDmaPublication<'owner, const SLOTS: usize, const BUFFER_SIZE: usize> {
    owner: &'owner mut PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>,
}

#[allow(
    unsafe_code,
    reason = "publication token retains a validated internal aggregate chain"
)]
unsafe impl<const SLOTS: usize, const BUFFER_SIZE: usize> PreparedTxDma
    for AmpduDmaPublication<'_, SLOTS, BUFFER_SIZE>
{
    fn descriptor_head(&self) -> u32 {
        self.owner.binding.descriptor_base
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> AmpduDmaPublication<'_, SLOTS, BUFFER_SIZE> {
    pub fn commit<F>(self, start: F)
    where
        F: FnOnce(&AmpduDmaStart<'_>),
    {
        let owner = self.owner;
        *owner.storage.as_mut().project().state = AmpduDmaState::HardwareOwned;
        start(&AmpduDmaStart {
            binding: &owner.binding,
        });
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use open_esp_radio_dma::{
        HardwareOwnedTxDma, PreparedTxDma, StableDmaBacking, StableDmaRegion,
    };
    use std::{boxed::Box, cell::Cell, rc::Rc};

    use super::*;
    use crate::descriptor::{length, size};

    const DESCRIPTOR_BASE: u32 = 0x2f00_1000;
    const BUFFER_BASE: u32 = 0x2f01_0000;

    fn storage() -> PinnedAmpduDmaStorage<4, 256> {
        AmpduDmaStorage::pin_static_model(
            std::boxed::Box::leak(std::boxed::Box::new(AmpduDmaStorage::new())),
            DESCRIPTOR_BASE,
            BUFFER_BASE,
        )
        .unwrap()
    }

    struct TestBacking {
        bytes: Box<[u8; 128]>,
        drops: Rc<Cell<usize>>,
    }

    impl TestBacking {
        fn new(drops: Rc<Cell<usize>>) -> Self {
            Self {
                bytes: Box::new([0; 128]),
                drops,
            }
        }
    }

    impl Drop for TestBacking {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[allow(
        unsafe_code,
        reason = "test backing owns one non-moving boxed allocation"
    )]
    unsafe impl StableDmaBacking for TestBacking {
        fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
            // SAFETY: moving `TestBacking` does not move its boxed allocation.
            unsafe { StableDmaRegion::new(&mut self.bytes[..]) }
        }
    }

    fn descriptor_only_storage() -> PinnedAmpduDmaStorage<2, 0> {
        AmpduDmaStorage::pin_static_model(
            Box::leak(Box::new(AmpduDmaStorage::new())),
            DESCRIPTOR_BASE,
            0,
        )
        .unwrap()
    }

    #[test]
    fn internal_chain_publishes_phased_dma_authority() {
        let mut storage = storage();
        storage.begin().unwrap();
        storage.buffer_mut(0).unwrap()[0] = 0x11;
        storage.buffer_mut(1).unwrap()[0] = 0x22;
        let entries = [
            AmpduInternalDescriptor {
                buffer_capacity: 256,
                transfer_length: 100,
            },
            AmpduInternalDescriptor {
                buffer_capacity: 128,
                transfer_length: 80,
            },
        ];
        let publication = storage.publish_internal_chain(&entries).unwrap();
        assert_eq!(publication.descriptor_head(), DESCRIPTOR_BASE);

        let mut start_head = 0;
        publication.commit(|start| start_head = start.descriptor_head());
        assert_eq!(start_head, DESCRIPTOR_BASE);
        assert_eq!(storage.state(), AmpduDmaState::HardwareOwned);
        assert_eq!(size(storage.descriptor_word0(0).unwrap()), 256);
        assert_eq!(length(storage.descriptor_word0(0).unwrap()), 100);
        assert_eq!(
            storage.descriptor_next_address(0),
            Some(DESCRIPTOR_BASE + DESCRIPTOR_BYTES)
        );
        assert_eq!(storage.descriptor_next_address(1), Some(0));

        storage.mark_completed().unwrap();
        assert!(storage.detached_buffer(0).is_err());
        storage
            .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
            .unwrap();
        assert_eq!(storage.detached_buffer(0).unwrap()[0], 0x11);
        storage.release_detached().unwrap();
        assert_eq!(storage.state(), AmpduDmaState::Free);
    }

    #[test]
    fn invalid_internal_chain_does_not_publish_capability() {
        let mut storage = storage();
        storage.begin().unwrap();
        assert!(
            storage
                .publish_internal_chain(&[AmpduInternalDescriptor {
                    buffer_capacity: 257,
                    transfer_length: 1,
                }])
                .is_err()
        );
        assert_eq!(storage.state(), AmpduDmaState::Reserved);
        storage.cancel().unwrap();
    }

    #[test]
    fn reset_quarantine_has_no_release_edge() {
        let mut storage = storage();
        storage.begin().unwrap();
        storage.quarantine();
        assert_eq!(storage.state(), AmpduDmaState::ResetRequired);
        assert_eq!(storage.cancel(), Err(AmpduDmaStorageError::State));
        assert_eq!(storage.begin(), Err(AmpduDmaStorageError::State));
    }

    #[test]
    fn detach_proof_must_name_the_aggregate_head() {
        let mut storage = storage();
        storage.begin().unwrap();
        storage
            .publish_internal_chain(&[AmpduInternalDescriptor {
                buffer_capacity: 256,
                transfer_length: 64,
            }])
            .unwrap()
            .commit(|_| {});
        storage.mark_completed().unwrap();

        assert_eq!(
            storage.mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE + 4)),
            Err(AmpduDmaStorageError::Address)
        );
        assert_eq!(storage.state(), AmpduDmaState::Completed);
        storage
            .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
            .unwrap();
        storage.release_detached().unwrap();
    }

    #[test]
    fn zero_slot_arena_is_rejected() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(AmpduDmaStorage::<0, 64>::new()));
        assert!(AmpduDmaStorage::pin_static_model(storage, DESCRIPTOR_BASE, BUFFER_BASE).is_err());
    }

    #[test]
    fn external_chain_retains_backings_through_detach() {
        let drops = Rc::new(Cell::new(0));
        let mut owner = RetainedAmpduDma::new(descriptor_only_storage());
        owner.begin().unwrap();
        let first = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
        let second = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
        owner.reserved_backing_mut(first).unwrap().bytes[8] = 0x11;
        owner.reserved_backing_mut(second).unwrap().bytes[16] = 0x22;
        let entries = [
            AmpduExternalDescriptor {
                backing_index: first,
                dma_offset: 8,
                buffer_capacity: 64,
                transfer_length: 40,
            },
            AmpduExternalDescriptor {
                backing_index: second,
                dma_offset: 16,
                buffer_capacity: 64,
                transfer_length: 48,
            },
        ];
        let publication = owner.publish_external_chain(&entries).unwrap();
        assert_eq!(publication.descriptor_head(), DESCRIPTOR_BASE);
        publication.commit(|start| assert_eq!(start.descriptor_head(), DESCRIPTOR_BASE));
        assert_eq!(owner.state(), AmpduDmaState::HardwareOwned);
        assert_eq!(drops.get(), 0);

        owner.mark_completed().unwrap();
        assert!(owner.detached_backing_mut(first).is_err());
        owner
            .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
            .unwrap();
        assert_eq!(owner.detached_backing_mut(first).unwrap().bytes[8], 0x11);
        owner.release_detached().unwrap();
        assert_eq!(owner.state(), AmpduDmaState::Free);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn detached_external_chain_can_be_republished_for_retry() {
        let drops = Rc::new(Cell::new(0));
        let mut owner = RetainedAmpduDma::new(descriptor_only_storage());
        owner.begin().unwrap();
        let backing = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
        let address = owner
            .reserved_backing_mut(backing)
            .unwrap()
            .bytes
            .as_ptr()
            .addr()
            + 8;
        let entry = owner.reserved_external_descriptor(address, 64, 40).unwrap();
        assert_eq!(entry.backing_index, backing);
        assert_eq!(entry.dma_offset, 8);
        owner
            .publish_external_chain(&[entry])
            .unwrap()
            .commit(|_| {});
        owner.mark_completed().unwrap();
        owner
            .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
            .unwrap();

        owner.begin_retry().unwrap();
        assert_eq!(owner.held_backing_count(), 1);
        let retry = owner.reserved_external_descriptor(address, 64, 40).unwrap();
        owner
            .publish_external_chain(&[retry])
            .unwrap()
            .commit(|_| {});
        owner.mark_completed().unwrap();
        owner
            .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
            .unwrap();
        assert_eq!(owner.detached_region_mut(address, 64).unwrap().len(), 64);
        owner.release_detached().unwrap();
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn dropping_hardware_owned_external_chain_forgets_backings() {
        let drops = Rc::new(Cell::new(0));
        let mut owner = RetainedAmpduDma::new(descriptor_only_storage());
        owner.begin().unwrap();
        let backing = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
        owner
            .publish_external_chain(&[AmpduExternalDescriptor {
                backing_index: backing,
                dma_offset: 0,
                buffer_capacity: 64,
                transfer_length: 32,
            }])
            .unwrap()
            .commit(|_| {});

        drop(owner);
        assert_eq!(drops.get(), 0);
    }

    #[test]
    fn dropping_reserved_external_chain_releases_backings() {
        let drops = Rc::new(Cell::new(0));
        let mut owner = RetainedAmpduDma::new(descriptor_only_storage());
        owner.begin().unwrap();
        owner.push_backing(TestBacking::new(drops.clone())).unwrap();

        drop(owner);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn reserved_backing_insert_can_be_rolled_back_transactionally() {
        let drops = Rc::new(Cell::new(0));
        let mut owner = RetainedAmpduDma::new(descriptor_only_storage());
        owner.begin().unwrap();
        let first = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
        let second = owner.push_backing(TestBacking::new(drops.clone())).unwrap();

        assert!(owner.pop_last_backing(first).is_err());
        assert_eq!(owner.held_backing_count(), 2);
        drop(owner.pop_last_backing(second).unwrap());
        assert_eq!(owner.held_backing_count(), 1);
        assert_eq!(drops.get(), 1);

        owner.cancel().unwrap();
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn invalid_external_chain_is_not_partially_published() {
        let drops = Rc::new(Cell::new(0));
        let mut owner = RetainedAmpduDma::new(descriptor_only_storage());
        owner.begin().unwrap();
        let backing = owner.push_backing(TestBacking::new(drops.clone())).unwrap();

        assert!(matches!(
            owner.publish_external_chain(&[AmpduExternalDescriptor {
                backing_index: backing,
                dma_offset: 96,
                buffer_capacity: 64,
                transfer_length: 32,
            }]),
            Err(AmpduDmaStorageError::Address)
        ));
        assert_eq!(owner.state(), AmpduDmaState::Reserved);
        assert_eq!(owner.held_backing_count(), 1);
        assert_eq!(owner.dma().descriptor_word0(0), Some(0));
        assert_eq!(owner.dma().descriptor_buffer_address(0), Some(0));
        assert_eq!(drops.get(), 0);

        owner.cancel().unwrap();
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn free_retained_owner_returns_its_dma_arena() {
        let owner = RetainedAmpduDma::<TestBacking, 2, 0>::new(descriptor_only_storage());
        let dma = match owner.try_into_dma() {
            Ok(dma) => dma,
            Err(_) => panic!("free owner must return its arena"),
        };

        assert_eq!(dma.state(), AmpduDmaState::Free);
    }
}
