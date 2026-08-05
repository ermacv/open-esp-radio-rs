//! Pinned descriptor and internal-buffer ownership for one TX A-MPDU.
//!
//! This lower owner deliberately covers only internally backed aggregates.
//! External zero-copy leases need an additional retained-backing proof before
//! they may receive the same prepare/start capabilities.

use core::{marker::PhantomPinned, pin::Pin};

use open_esp_radio_dma::{HardwareOwnedTxDma, PreparedTxDma, StableDmaRange};
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

    pub fn cancel(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.clear_to_free(AmpduDmaState::Reserved)
    }

    pub fn mark_completed(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.transition(AmpduDmaState::HardwareOwned, AmpduDmaState::Completed)
    }

    pub fn mark_detached(&mut self) -> Result<(), AmpduDmaStorageError> {
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

    pub fn release_aborted(&mut self) -> Result<(), AmpduDmaStorageError> {
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

    use open_esp_radio_dma::{HardwareOwnedTxDma, PreparedTxDma};

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
        storage.mark_detached().unwrap();
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
    fn zero_slot_arena_is_rejected() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(AmpduDmaStorage::<0, 64>::new()));
        assert!(AmpduDmaStorage::pin_static_model(storage, DESCRIPTOR_BASE, BUFFER_BASE).is_err());
    }
}
