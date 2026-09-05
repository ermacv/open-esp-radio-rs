//! Static CPU-owned storage for the recovered ESP32-S31 DTM memory graph.
//!
//! The types in this module reserve the complete finite per-event link-graph
//! footprint and reproduce only reviewed CPU-side initialization transforms.
//! The separate `0x28`-byte DTM environment remains LLL state above this
//! boundary. A target-only binding derives the real addresses of one static
//! allocation, rejects storage outside physical internal SRAM and retains the
//! allocation behind one movable owner. A matching affine PAC head-publication
//! token consumes the last CPU rollback state into a controller-visible graph.
//! Four-byte alignment is the minimum proven by compressed controller links;
//! it is not a cache-coherency claim.

#![forbid(unsafe_code)]

use core::{convert::Infallible, num::NonZeroU32, pin::Pin};

use crate::{
    dtm_event_image::{
        BluetoothDtmPositionalEventWords, BluetoothDtmRxHeaderTailProjection,
        BluetoothDtmTxHeaderHeadProjection,
    },
    dtm_rx_result::{BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError},
    le_tx_packet::BluetoothLeTxPacketPreparedLength,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerSoftwareListRemovalReady,
};

mod codec;

pub use codec::{
    BLUETOOTH_DTM_LINK_STATE_BYTES, BLUETOOTH_DTM_MAX_PACKET_CAPACITY,
    BLUETOOTH_DTM_RX_PACKET_BYTES, BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES,
    BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES, BLUETOOTH_DTM_TX_PACKET_BYTES,
    BluetoothDtmMemoryGraphStorage,
};
use codec::{
    BluetoothDtmEmptyListRollback, BluetoothDtmMemoryGraphBinding, BluetoothDtmRxRotationPlan,
    BluetoothDtmSchedulerBookkeepingRollback, BluetoothLeTestPduHeader,
};

/// Product-owned limits contributing to the DTM scheduler allocation field.
///
/// The current allocator forms scheduler-item `+0x20[11:0]` from three public
/// ESP-IDF controller limits and target-private additions. The private
/// halfword recovered at options offset `+0x14` is fixed by this S31
/// implementation; it is not an application policy or part of this API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerAllocationConfig {
    extended_advertising_instances: u16,
    connections: u16,
    periodic_syncs: u8,
}

impl BluetoothDtmSchedulerAllocationConfig {
    /// Capture the exact source-owned inputs used by the S31 DTM allocator.
    pub const fn new(
        extended_advertising_instances: u16,
        connections: u16,
        periodic_syncs: u8,
    ) -> Self {
        Self {
            extended_advertising_instances,
            connections,
            periodic_syncs,
        }
    }
}

/// Why a lower-layer selector cannot form a standard LE Test PDU header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmTxPacketPrepareError {
    /// The LE Test PDU Type field admits exactly the eight standard DTM types.
    UnsupportedPayloadType,
}

/// Why static DTM graph storage cannot become an address-bound CPU owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmMemoryGraphBindError {
    /// A target pointer cannot be represented by the 32-bit S31 address space.
    AddressWidth,
    /// The proposed base is outside the compressed controller-pointer domain.
    InvalidBase(BluetoothControllerSramAddressError),
    /// Some byte of the complete graph is outside physical internal SRAM.
    ExtentOutsidePhysicalSram,
    /// A required graph link would encode as the unbound zero image.
    ZeroCompressedLink,
    /// A reviewed packet extent is inconsistent with the bound graph layout.
    InvalidPacketExtent,
}

/// Failed static binding that retains the exact, unchanged allocation.
///
/// Address validation completes before any allocation-time field is written.
/// The failure is linear and cannot be duplicated:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphBindFailure;
///
/// fn duplicate(failure: BluetoothDtmMemoryGraphBindFailure) {
///     let moved = failure;
///     let _ = failure.error();
///     drop(moved);
/// }
/// ```
pub struct BluetoothDtmMemoryGraphBindFailure {
    storage: &'static mut BluetoothDtmMemoryGraphStorage,
    error: BluetoothDtmMemoryGraphBindError,
}

impl BluetoothDtmMemoryGraphBindFailure {
    fn new(
        storage: &'static mut BluetoothDtmMemoryGraphStorage,
        error: BluetoothDtmMemoryGraphBindError,
    ) -> Self {
        Self { storage, error }
    }

    /// Return the finite reason without releasing the allocation.
    pub const fn error(&self) -> BluetoothDtmMemoryGraphBindError {
        self.error
    }

    /// Recover the unchanged static allocation and binding error.
    pub fn into_parts(
        self,
    ) -> (
        &'static mut BluetoothDtmMemoryGraphStorage,
        BluetoothDtmMemoryGraphBindError,
    ) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for BluetoothDtmMemoryGraphBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
///
/// Construction proves the compressed-pointer syntax only. The graph binding
/// still validates the complete extent against physical S31 SRAM. Production
/// RISC-V code has no access to this type and must derive real field addresses
/// from its static allocation.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmMemoryGraphModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl BluetoothDtmMemoryGraphModelAddress {
    /// Validate one synthetic base against the controller encoding domain.
    pub const fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        match BluetoothControllerSramAddress::new(address) {
            Ok(address) => Ok(Self(address)),
            Err(error) => Err(error),
        }
    }

    const fn address(self) -> u32 {
        self.0.address()
    }
}

/// Opaque identity of one exact statically pinned DTM graph storage object.
///
/// This value is an equality witness only: it exposes neither the storage
/// pointer nor controller-SRAM addresses and grants no access or publication
/// authority. The memory layer mints it while consuming the unique static
/// storage borrow, and every graph typestate retains it inside its binding.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct BluetoothDtmMemoryGraphIdentity(usize);

impl BluetoothDtmMemoryGraphIdentity {
    fn for_storage(storage: &BluetoothDtmMemoryGraphStorage) -> Self {
        Self(core::ptr::addr_of!(*storage).addr())
    }
}

impl core::fmt::Debug for BluetoothDtmMemoryGraphIdentity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphIdentity")
            .finish_non_exhaustive()
    }
}

/// Snapshot supplied to one in-place positional event-word builder.
///
/// Both private links are sampled from this graph's current link-state words,
/// not reconstructed from an earlier binding or another graph. The builder is
/// invoked while the unique graph owner is consumed, so its output cannot be
/// applied later to a different owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmPositionalEventSeed {
    words: BluetoothDtmPositionalEventWords,
    tx_header_head: BluetoothDtmTxHeaderHeadProjection,
    rx_header_tail: BluetoothDtmRxHeaderTailProjection,
}

impl BluetoothDtmPositionalEventSeed {
    /// Return the current values of exactly the nineteen writable words.
    pub const fn words(self) -> BluetoothDtmPositionalEventWords {
        self.words
    }

    /// Return the current graph-bound TX-header head projection.
    pub const fn tx_header_head_projection(self) -> BluetoothDtmTxHeaderHeadProjection {
        self.tx_header_head
    }

    /// Return the current graph-bound RX-header tail projection.
    pub const fn rx_header_tail_projection(self) -> BluetoothDtmRxHeaderTailProjection {
        self.rx_header_tail
    }
}

/// Why CPU-owned positional event words were not committed to the graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmMemoryGraphPrepareError<BuildError = Infallible> {
    /// The upper builder rejected its semantic inputs before any graph write.
    Build(BuildError),
    /// The current private TX-head word contains the unbound zero link.
    CurrentTxHeadUnbound,
    /// The current private TX head does not name this graph's bound TX header.
    CurrentTxHeadIdentityMismatch,
    /// The selected TX header no longer names this graph's packet allocation.
    CurrentTxHeaderPacketBaseMismatch,
    /// The selected TX header no longer names this graph's LE Test PDU.
    CurrentTxHeaderPduTargetMismatch,
    /// The selected TX header lost the reviewed full-capacity allocation profile.
    CurrentTxHeaderAllocationExtentMismatch,
    /// The current private RX-tail word contains the unbound zero link.
    CurrentRxTailUnbound,
    /// The current private RX tail names neither of this graph's two RX headers.
    CurrentRxTailIdentityMismatch,
    /// The selected RX-tail header no longer names this graph's packet allocation.
    CurrentRxTailPacketMismatch,
    /// Link-state `+0x00` does not retain this graph's freshly sampled TX head.
    LinkStateTxHeadMismatch {
        /// Current private-chain projection required by this graph.
        expected: BluetoothDtmTxHeaderHeadProjection,
        /// Candidate projection returned by the builder.
        observed: BluetoothDtmTxHeaderHeadProjection,
    },
    /// Link-state `+0x08` does not retain this graph's freshly sampled RX tail.
    LinkStateRxTailMismatch {
        /// Current private-chain projection required by this graph.
        expected: BluetoothDtmRxHeaderTailProjection,
        /// Candidate projection returned by the builder.
        observed: BluetoothDtmRxHeaderTailProjection,
    },
    /// Scheduler-item `+0x08` no longer points to this graph's link-state.
    SchedulerItemLinkStateMismatch,
}

/// Failed positional preparation retaining the exact CPU-owned graph.
///
/// Current header-to-packet bindings, builder execution and all three
/// candidate descriptor anchors are checked before the first backing-storage
/// write. `into_parts` therefore returns the byte-unchanged owner for retry or
/// explicit shutdown.
pub struct BluetoothDtmMemoryGraphPrepareFailure<BuildError = Infallible> {
    owner: BluetoothDtmMemoryGraphCpuOwned,
    error: BluetoothDtmMemoryGraphPrepareError<BuildError>,
}

impl<BuildError> BluetoothDtmMemoryGraphPrepareFailure<BuildError> {
    /// Borrow the finite failure reason without releasing the owner.
    pub const fn error(&self) -> &BluetoothDtmMemoryGraphPrepareError<BuildError> {
        &self.error
    }

    /// Recover the unchanged owner and the exact builder or anchor error.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothDtmMemoryGraphPrepareError<BuildError>,
    ) {
        (self.owner, self.error)
    }
}

impl<BuildError: core::fmt::Debug> core::fmt::Debug
    for BluetoothDtmMemoryGraphPrepareFailure<BuildError>
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// CPU-owned graph after exactly nineteen positional event words were stored.
///
/// This state proves that the selected TX/RX headers retain this graph's
/// packet allocations, that the TX header retains its PDU target and reviewed
/// allocation extent, and that the candidate retained the three descriptor
/// links. It does not by itself prove fresh TX packet contents and has no list
/// insertion, fence, publication or hardware-ownership authority.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphPositionalEventPrepared;
///
/// fn cannot_mutate_packet(prepared: &mut BluetoothDtmMemoryGraphPositionalEventPrepared) {
///     let _packet = prepared.tx_packet_mut();
/// }
/// ```
pub struct BluetoothDtmMemoryGraphPositionalEventPrepared {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
    previous: BluetoothDtmPositionalEventWords,
}

impl BluetoothDtmMemoryGraphPositionalEventPrepared {
    /// Return the typed identity of this graph's prepared scheduler item.
    ///
    /// The returned address does not publish the item or change graph
    /// ownership. The upper scheduler must retain this consumed graph while a
    /// request carrying the identity is admitted.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Read back exactly the positional subset while it remains CPU-owned.
    pub fn words(&self) -> BluetoothDtmPositionalEventWords {
        self.storage.as_ref().get_ref().reviewed_event_words()
    }

    /// Prepare the scheduler-owned bookkeeping fields that precede insertion.
    ///
    /// This consumes the event-word owner, clears the reviewed control byte
    /// and software completed-item link, and installs the in-flight status
    /// sentinel. The returned graph remains CPU-owned: the complete
    /// hardware-consumed descriptor, release fence and scheduler-head
    /// publication are still separate prerequisites.
    pub fn prepare_scheduler_bookkeeping(
        mut self,
    ) -> BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
        let rollback = self.storage.as_mut().prepare_scheduler_bookkeeping();

        BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
            storage: self.storage,
            binding: self.binding,
            previous: self.previous,
            rollback,
        }
    }

    /// Cancel before publication and restore all nineteen prior words.
    ///
    /// This restoration is complete because this state never exposed mutable
    /// storage and the preparing transition wrote no other graph offset.
    pub fn cancel(mut self) -> BluetoothDtmMemoryGraphCpuOwned {
        self.storage
            .as_mut()
            .restore_positional_words(self.previous);

        BluetoothDtmMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

/// CPU-owned graph with the common scheduler bookkeeping prefix installed.
///
/// This state is deliberately not called published or hardware-owned. It has
/// no operation that exposes packet storage or permits the controller to
/// consume the graph before the remaining descriptor and visibility contract
/// is proven.
#[must_use = "the scheduler-prepared graph must remain owned or be cancelled"]
pub struct BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
    previous: BluetoothDtmPositionalEventWords,
    rollback: BluetoothDtmSchedulerBookkeepingRollback,
}

impl BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
    /// Return the typed identity of this graph's scheduler item.
    ///
    /// This value still carries no publication or CPU-access authority.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Prepare this item as the sole member of a proven-empty hardware list.
    ///
    /// SOURCE: complete current `r_sym_bt_YRnBzKlWCjsIbotqvNyS` and
    /// instruction-corresponding same-chip named
    /// `r_btdm_sched_merge_list_remove_overlap`. On the empty-list edge the
    /// selected item is the submitted item and its compressed hardware-next
    /// link is cleared before any hardware-head publication.
    ///
    /// This memory-only transition does not prove that a scheduler list is
    /// empty and performs no fence or MMIO. The Controller must consume it
    /// together with its exclusive empty-list epoch before publication.
    #[doc(hidden)]
    pub fn prepare_empty_list_link(mut self) -> BluetoothDtmMemoryGraphEmptyListLinkPrepared {
        let rollback = self.storage.as_mut().prepare_empty_list_link(self.rollback);

        BluetoothDtmMemoryGraphEmptyListLinkPrepared {
            storage: self.storage,
            binding: self.binding,
            previous: self.previous,
            rollback,
        }
    }

    /// Cancel before publication and recover the positional event owner.
    pub fn cancel(mut self) -> BluetoothDtmMemoryGraphPositionalEventPrepared {
        self.storage
            .as_mut()
            .restore_scheduler_bookkeeping(self.rollback);

        BluetoothDtmMemoryGraphPositionalEventPrepared {
            storage: self.storage,
            binding: self.binding,
            previous: self.previous,
        }
    }
}

/// CPU-owned graph whose scheduler item has a null hardware-next link.
///
/// This is only descriptor preparation for the empty-list merge case. It does
/// not carry list ownership, visibility, publication or hardware-consumption
/// authority; those belong to the upper Controller lifecycle.
#[must_use = "the empty-list candidate must remain owned or be cancelled"]
pub struct BluetoothDtmMemoryGraphEmptyListLinkPrepared {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
    previous: BluetoothDtmPositionalEventWords,
    rollback: BluetoothDtmEmptyListRollback,
}

impl BluetoothDtmMemoryGraphEmptyListLinkPrepared {
    /// Return the exact scheduler-item identity retained by this graph.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Consume CPU rollback ownership after the exact list head is published.
    ///
    /// This transition accepts only the affine PAC publication proof for DTM's
    /// fixed list zero and verifies that its head names this pinned graph. The
    /// resulting owner deliberately discards every rollback image: hardware
    /// may already retain and mutate the graph, so cancellation and CPU-side
    /// preparation must no longer be expressible.
    #[doc(hidden)]
    pub fn into_head_published(
        self,
        publication: &BluetoothSchedulerHardwareListHeadPublished,
    ) -> BluetoothDtmMemoryGraphHeadPublished {
        assert_eq!(
            publication.index(),
            BluetoothSchedulerHardwareListIndex::ZERO,
            "a DTM graph can only consume its fixed hardware-list publication"
        );
        assert_eq!(
            publication.head().address(),
            Some(self.scheduler_item_address()),
            "the published scheduler head must name the retained DTM graph"
        );

        BluetoothDtmMemoryGraphHeadPublished {
            storage: self.storage,
            binding: self.binding,
        }
    }

    /// Cancel before visibility or publication and recover bookkeeping state.
    pub fn cancel(mut self) -> BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
        let rollback = self.storage.as_mut().restore_empty_list_link(self.rollback);

        BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared {
            storage: self.storage,
            binding: self.binding,
            previous: self.previous,
            rollback,
        }
    }
}

/// Pinned DTM graph visible to and exclusively retained for controller use.
///
/// The matching affine PAC publication has completed both visibility fences
/// and installed this graph as hardware-list zero's head. This type exposes
/// identity only: it grants neither CPU mutation nor completion visibility,
/// and it has no cancellation or conversion back into a prepared owner.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphHeadPublished;
/// use open_esp_radio_esp32s31_hal::BluetoothSchedulerFinishedHardwareListObserved;
///
/// fn observe_before_run(
///     graph: BluetoothDtmMemoryGraphHeadPublished,
///     observed: BluetoothSchedulerFinishedHardwareListObserved,
/// ) {
///     graph.observe_completion(observed);
/// }
/// ```
#[must_use = "the published graph must either reach RUN or remain fail-stop owned"]
pub struct BluetoothDtmMemoryGraphHeadPublished {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
}

impl BluetoothDtmMemoryGraphHeadPublished {
    /// Return the exact scheduler-item identity visible through the list head.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Consume the exact RUN proof and admit completion observation.
    ///
    /// Head publication alone only makes the graph controller-visible. This
    /// transition requires the complete interrupt-preparation and RUN suffix
    /// for the same list and head before hardware-written status can be read.
    #[doc(hidden)]
    pub fn into_running(
        self,
        run: &BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothDtmMemoryGraphRunning {
        assert_eq!(
            run.index(),
            BluetoothSchedulerHardwareListIndex::ZERO,
            "a DTM graph can only run on its fixed hardware list"
        );
        assert_eq!(
            run.head().address(),
            Some(self.scheduler_item_address()),
            "the RUN proof must retain the published DTM graph"
        );

        BluetoothDtmMemoryGraphRunning {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

/// Pinned DTM graph admitted through the complete scheduler RUN transaction.
///
/// This is the first memory state that may consume a fenced finished-list
/// observation. It still exposes no CPU mutation or cancellation path.
#[must_use = "the running graph must advance through proven completion"]
pub struct BluetoothDtmMemoryGraphRunning {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
}

impl BluetoothDtmMemoryGraphRunning {
    /// Return the exact scheduler-item identity retained during execution.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.binding.scheduler_item_address().controller_address()
    }

    /// Observe the sole DTM item's status after one affine finished-list event.
    ///
    /// The current source-owned scheduler epoch assigns DTM exclusively to
    /// list zero and admits exactly one item. A token for another list returns
    /// both owners unchanged and performs no status read. A matching token is
    /// consumed by exactly one volatile load after the PAC transfer's trailing
    /// device fence. The in-flight sentinel retains hardware ownership; every
    /// other value advances only to completion-observed, not CPU-owned.
    #[doc(hidden)]
    pub fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> BluetoothDtmMemoryGraphCompletionObservation {
        if observed.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            return BluetoothDtmMemoryGraphCompletionObservation::ListMismatch {
                owner: self,
                observed,
            };
        }

        match self.storage.as_ref().get_ref().observe_completion_status() {
            Some(status) => BluetoothDtmMemoryGraphCompletionObservation::CompletionObserved(
                BluetoothDtmMemoryGraphCompletionObserved {
                    owner: self,
                    status,
                },
            ),
            None => BluetoothDtmMemoryGraphCompletionObservation::StillInFlight(self),
        }
    }
}

/// Reviewed DTM interpretation of one non-sentinel scheduler-item status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmSchedulerItemCompletionStatus {
    /// The role-specific accounting path accepts positional status zero.
    Zero,
    /// Hardware reported a positional nonzero status.
    NonZero(NonZeroU32),
}

/// Hardware-owned graph after a non-sentinel status was observed.
///
/// The descriptor has not yet been unlinked from the hardware list or removed
/// from the software completion queue. This state therefore exposes status and
/// identity only and cannot reclaim, mutate or republish the graph.
#[must_use = "completion observation must advance through unlink and recycle ownership"]
pub struct BluetoothDtmMemoryGraphCompletionObserved {
    owner: BluetoothDtmMemoryGraphRunning,
    status: BluetoothDtmSchedulerItemCompletionStatus,
}

impl BluetoothDtmMemoryGraphCompletionObserved {
    /// Exact item whose status was observed after the fenced transfer.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.owner.scheduler_item_address()
    }

    /// Semantic non-sentinel status retained by this completion observation.
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
        self.status
    }

    #[doc(hidden)]
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphRunning,
        BluetoothDtmSchedulerItemCompletionStatus,
    ) {
        (self.owner, self.status)
    }

    /// Prepare reclamation after exact hardware-head retirement and the
    /// post-unlink software-list removal gate without mutating SRAM.
    ///
    /// The two affine lower tokens are consumed together and the retained RUN
    /// head must name this graph on list zero. Success binds them into one
    /// affine prepared transaction; its later infallible commit performs the
    /// reviewed SRAM cleanup and returns the retained status with CPU ownership.
    #[doc(hidden)]
    pub fn prepare_recycle_after_software_list_removal(
        self,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<BluetoothDtmMemoryGraphRecyclePrepared, BluetoothDtmMemoryGraphRecycleFailure> {
        let error = if removal.index() != BluetoothSchedulerHardwareListIndex::ZERO {
            Some(BluetoothDtmMemoryGraphRecycleError::HardwareListMismatch)
        } else if removal.completed_head().address() != Some(self.scheduler_item_address()) {
            Some(BluetoothDtmMemoryGraphRecycleError::SchedulerItemMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(BluetoothDtmMemoryGraphRecycleFailure {
                error,
                completed: self,
                removal,
            });
        }

        Ok(BluetoothDtmMemoryGraphRecyclePrepared {
            completed: self,
            removal,
        })
    }
}

/// Validated but not yet mutated completed-graph recycle transaction.
///
/// This affine owner binds the exact completed graph, retired head and
/// post-unlink removal proof. It can be rolled back losslessly until
/// [`Self::commit`] performs the reviewed SRAM cleanup.
#[must_use = "the prepared recycle must be committed or returned to its owners"]
pub struct BluetoothDtmMemoryGraphRecyclePrepared {
    completed: BluetoothDtmMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

impl BluetoothDtmMemoryGraphRecyclePrepared {
    /// Recover every unchanged owner before the first SRAM mutation.
    #[doc(hidden)]
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }

    /// Perform the infallible reviewed recycle suffix.
    ///
    /// Vendor order removes the completed-queue link first and then clears the
    /// compressed hardware-next link while preserving its non-link prefix.
    #[doc(hidden)]
    pub fn commit(self) -> BluetoothDtmMemoryGraphRecycleCleaned {
        let Self {
            completed,
            removal: _,
        } = self;
        let BluetoothDtmMemoryGraphCompletionObserved { owner, status } = completed;
        let BluetoothDtmMemoryGraphRunning {
            mut storage,
            binding,
        } = owner;
        storage.as_mut().commit_scheduler_recycle();

        BluetoothDtmMemoryGraphRecycleCleaned {
            storage,
            binding,
            status,
        }
    }

    /// Validate the exact two-header RX-success topology without mutating it.
    ///
    /// The finished-list fence and removal proof are already retained by this
    /// transaction. This operation additionally proves either that no packet
    /// was returned or that one completed tail can execute the bounded
    /// two-slot rotation. Every malformed topology is returned unchanged.
    pub fn prepare_receiver_success(
        self,
    ) -> Result<
        BluetoothDtmMemoryGraphRxSuccessRecyclePrepared,
        BluetoothDtmMemoryGraphRxSuccessRecycleFailure,
    > {
        let owner = &self.completed.owner;
        let plan = match owner.storage.as_ref().get_ref().validate_rx_rotation(
            &owner.binding,
            &self,
            self.completed.status,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                return Err(BluetoothDtmMemoryGraphRxSuccessRecycleFailure {
                    error,
                    recycle: self,
                });
            }
        };
        Ok(BluetoothDtmMemoryGraphRxSuccessRecyclePrepared {
            recycle: self,
            plan,
        })
    }
}

/// Validated RX-success recycle retaining the exact two-slot rotation plan.
#[must_use = "the RX-success recycle must be committed or returned unchanged"]
pub struct BluetoothDtmMemoryGraphRxSuccessRecyclePrepared {
    recycle: BluetoothDtmMemoryGraphRecyclePrepared,
    plan: BluetoothDtmRxRotationPlan,
}

impl BluetoothDtmMemoryGraphRxSuccessRecyclePrepared {
    /// Recover the byte-unchanged generic recycle transaction.
    pub fn into_recycle_prepared(self) -> BluetoothDtmMemoryGraphRecyclePrepared {
        self.recycle
    }

    /// Commit generic scheduler cleanup and consume the validated returned-list
    /// read into one affine observation.
    ///
    /// This clears only the already-retired scheduler links. The returned owner
    /// alone can perform the RX append/re-arm suffix after the upper session
    /// consumes the observation. No fallible operation may follow this edge.
    pub fn observe(self) -> BluetoothDtmMemoryGraphRxSuccessObserved {
        let Self { recycle, plan } = self;
        BluetoothDtmMemoryGraphRxSuccessObserved {
            memory: recycle.commit(),
            plan,
        }
    }
}

/// Affine result of the bounded RX returned-list observation.
#[must_use = "the observed RX-success graph must be accounted and re-armed"]
pub struct BluetoothDtmMemoryGraphRxSuccessObserved {
    memory: BluetoothDtmMemoryGraphRecycleCleaned,
    plan: BluetoothDtmRxRotationPlan,
}

impl BluetoothDtmMemoryGraphRxSuccessObserved {
    /// Consume the semantic observation before the infallible RX re-arm.
    ///
    /// The callback result is retained while this method consumes the sole
    /// graph token. There is no public path that can re-arm either variant
    /// without first executing the callback.
    pub fn consume_then_commit<ResultValue>(
        self,
        consume: impl FnOnce(
            Option<Result<BluetoothDtmRxResultProjection, BluetoothDtmRxResultProjectionError>>,
        ) -> ResultValue,
    ) -> (BluetoothDtmMemoryGraphRecycleCleaned, ResultValue) {
        let Self { mut memory, plan } = self;
        let result = consume(plan.projection());
        memory
            .storage
            .as_mut()
            .commit_rx_rotation(&memory.binding, plan);
        (memory, result)
    }
}

/// Why an RX-success graph cannot enter the two-slot recycle suffix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmMemoryGraphRxSuccessRecycleError {
    /// The specialized path received a non-success scheduler status.
    CompletionStatusMismatch,
    /// The private RX head names neither bound header slot.
    RxHeadIdentityMismatch,
    /// The private RX tail names neither bound header slot.
    RxTailIdentityMismatch,
    /// The detached reserve does not name the other bound header slot.
    RxSwapIdentityMismatch,
    /// The initial detached reserve still carries a list or packet link.
    ReserveNotDetached,
    /// The initial packet-bearing header unexpectedly carries a backlink.
    InitialBacklinkUnexpected,
    /// A steady two-header chain unexpectedly retains a detached reserve.
    SwapReserveUnexpected,
    /// The steady predecessor still owns the packet.
    PredecessorPacketStillBound,
    /// The steady predecessor does not lead to the packet-bearing tail.
    PredecessorSuccessorMismatch,
    /// The steady predecessor was not returned by the prior event.
    PredecessorNotCompleted,
    /// The steady tail backlink does not name its exact predecessor.
    SuccessorBacklinkMismatch,
    /// The packet-bearing returned tail unexpectedly has a successor.
    ReturnedHasSuccessor,
    /// The returned tail does not retain this graph's sole RX packet.
    ReturnedPacketMismatch,
    /// Hardware left the packet result in its re-arm sentinel state.
    ReturnedResultNotProduced,
    /// Hardware left the positional auxiliary halfword in its re-arm sentinel state.
    ReturnedAuxiliaryNotProduced,
}

/// Lossless RX-success preflight rejection.
#[must_use = "RX-success rejection retains the unchanged recycle transaction"]
pub struct BluetoothDtmMemoryGraphRxSuccessRecycleFailure {
    error: BluetoothDtmMemoryGraphRxSuccessRecycleError,
    recycle: BluetoothDtmMemoryGraphRecyclePrepared,
}

impl BluetoothDtmMemoryGraphRxSuccessRecycleFailure {
    /// Return the semantic reason preflight rejected the unchanged graph.
    pub const fn error(&self) -> BluetoothDtmMemoryGraphRxSuccessRecycleError {
        self.error
    }

    /// Recover the byte-unchanged generic recycle transaction.
    pub fn into_recycle_prepared(self) -> BluetoothDtmMemoryGraphRecyclePrepared {
        self.recycle
    }
}

impl core::fmt::Debug for BluetoothDtmMemoryGraphRxSuccessRecycleFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphRxSuccessRecycleFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Graph whose reviewed hardware-release and SRAM recycle suffix is complete.
///
/// At this lower memory boundary the consumed empty-head/removal proof already
/// establishes that hardware can no longer reach the graph. The production
/// Controller deliberately keeps this intermediate opaque while it releases
/// its separate software timeline and source-list bookkeeping.
#[must_use = "the cleaned graph must return to its memory owner"]
pub struct BluetoothDtmMemoryGraphRecycleCleaned {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
    status: BluetoothDtmSchedulerItemCompletionStatus,
}

impl BluetoothDtmMemoryGraphRecycleCleaned {
    /// Recover lower-layer CPU memory ownership after hardware release.
    ///
    /// This does not claim that any upper scheduler timeline or source-list
    /// bookkeeping has been released.
    #[doc(hidden)]
    pub fn into_cpu_owned(self) -> BluetoothDtmMemoryGraphRecycled {
        BluetoothDtmMemoryGraphRecycled {
            owner: BluetoothDtmMemoryGraphCpuOwned {
                storage: self.storage,
                binding: self.binding,
            },
            status: self.status,
        }
    }
}

/// Why the completed memory graph rejected recycle authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmMemoryGraphRecycleError {
    /// The empty-head proof belongs to a hardware list other than DTM list zero.
    HardwareListMismatch,
    /// The retained RUN head names a different scheduler item.
    SchedulerItemMismatch,
}

/// Lossless rejection of one completed-graph recycle attempt.
#[must_use = "recycle rejection retains the graph and both affine authorizations"]
pub struct BluetoothDtmMemoryGraphRecycleFailure {
    error: BluetoothDtmMemoryGraphRecycleError,
    completed: BluetoothDtmMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

impl BluetoothDtmMemoryGraphRecycleFailure {
    /// Exact identity mismatch that prevented CPU ownership return.
    pub const fn error(&self) -> BluetoothDtmMemoryGraphRecycleError {
        self.error
    }

    /// Recover the unchanged completion and both affine authorizations.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }
}

/// CPU-owned graph after the reviewed scheduler-item recycle suffix.
#[must_use = "the recycled graph and completion status must reach the role owner"]
pub struct BluetoothDtmMemoryGraphRecycled {
    owner: BluetoothDtmMemoryGraphCpuOwned,
    status: BluetoothDtmSchedulerItemCompletionStatus,
}

impl BluetoothDtmMemoryGraphRecycled {
    /// Recover the ordinary CPU graph and its retained completion status.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothDtmSchedulerItemCompletionStatus,
    ) {
        (self.owner, self.status)
    }
}

/// Result of matching one fenced finished-list observation to the DTM graph.
#[must_use = "the retained hardware graph and any unmatched list token must be handled"]
pub enum BluetoothDtmMemoryGraphCompletionObservation {
    /// The token belongs to another list; neither owner was consumed.
    ListMismatch {
        /// Unchanged running DTM owner.
        owner: BluetoothDtmMemoryGraphRunning,
        /// Unchanged affine observation for its actual list owner.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// List zero was observed but hardware still reports the in-flight sentinel.
    StillInFlight(BluetoothDtmMemoryGraphRunning),
    /// A non-sentinel status was observed after the transfer fence.
    CompletionObserved(BluetoothDtmMemoryGraphCompletionObserved),
}

/// Unique CPU owner of one statically located and allocation-initialized graph.
///
/// This state is still unreachable by hardware. It contains no `arm`,
/// `publish`, raw-pointer or head-published transition; the future LLL must
/// first prove the remaining descriptor contract and visibility fences.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_bluetooth_memory::{
///     BluetoothDtmMemoryGraphModelAddress, BluetoothDtmMemoryGraphStorage,
/// };
///
/// let storage = Box::leak(Box::new(BluetoothDtmMemoryGraphStorage::new()));
/// let base = BluetoothDtmMemoryGraphModelAddress::new(0x2f00_0100).unwrap();
/// let config = open_esp_radio_esp32s31_bluetooth_memory::
///     BluetoothDtmSchedulerAllocationConfig::new(1, 1, 1);
/// let owner = BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base, config).unwrap();
/// let moved = owner;
/// let _identity = owner.identity();
/// drop(moved);
/// ```
pub struct BluetoothDtmMemoryGraphCpuOwned {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
}

/// Ended CPU-owned DTM allocation awaiting either reuse or permanent retention.
///
/// This is not a vendor allocator or `free` operation. The caller's pinned
/// static storage remains owned by this affine value, stays at the same bound
/// address and is not dropped, unpinned or exposed for mutation. Construction
/// is available only from [`BluetoothDtmMemoryGraphCpuOwned`], so a graph still
/// in a prepared, head-published, running, completion-observed or recycle-intermediate
/// state cannot enter this edge.
///
/// An upper Test End owner must first establish its scheduler, callback and
/// source-list quiescence before it exposes the CPU-owned graph consumed here.
/// This lower memory typestate does not fabricate that upper proof.
#[must_use = "the reclaimed static graph must be retained or reinitialized"]
pub struct BluetoothDtmMemoryGraphReclaimed {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
}

impl BluetoothDtmMemoryGraphReclaimed {
    /// Opaque identity retained from the completed allocation epoch.
    pub const fn identity(&self) -> BluetoothDtmMemoryGraphIdentity {
        self.binding.identity()
    }

    /// Start a fresh CPU-owned allocation epoch in the same pinned storage.
    ///
    /// Reinitialization consumes the reclaimed token exactly once and installs
    /// the complete reviewed allocation defaults from the configuration bound
    /// to this graph's first epoch. No caller can cross-wire a configuration
    /// from another graph. This performs no MMIO, fence, hardware publication,
    /// heap allocation or vendor `free`/`alloc` call.
    pub fn reinitialize(self) -> BluetoothDtmMemoryGraphCpuOwned {
        let config = self.binding.allocation_config();
        let mut owner = BluetoothDtmMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        };
        owner.initialize_reviewed_allocation(config);
        owner
    }
}

/// CPU-owned graph whose sole TX packet slot has a complete DTM image.
///
/// Construction consumes the graph owner and copies every declared payload
/// byte. The state remains unreachable by hardware and grants no scheduler or
/// publication authority.
#[must_use = "the prepared TX graph must remain owned or be explicitly discarded"]
pub struct BluetoothDtmMemoryGraphTxPacketPrepared {
    storage: Pin<&'static mut BluetoothDtmMemoryGraphStorage>,
    binding: BluetoothDtmMemoryGraphBinding,
    _pdu_header: BluetoothLeTestPduHeader,
    packet_length: BluetoothLeTxPacketPreparedLength<BLUETOOTH_DTM_TX_PACKET_BYTES>,
}

impl BluetoothDtmMemoryGraphTxPacketPrepared {
    /// Return the declared packet payload length.
    pub const fn payload_length(&self) -> u8 {
        self.packet_length.payload_bytes()
    }

    /// Borrow the complete reviewed prefix and declared payload.
    pub fn prepared_packet_bytes(&self) -> &[u8] {
        self.storage
            .as_ref()
            .get_ref()
            .prepared_tx_packet_bytes(self.packet_length)
    }

    /// Build and commit one positional event image while consuming readiness.
    ///
    /// A successful upper TX transition can retain this construction path in
    /// its own typestate. On failure the ordinary CPU owner is returned and
    /// the caller must explicitly prepare the packet again before retrying TX.
    pub fn try_prepare_positional_event<BuildError>(
        self,
        build: impl FnOnce(
            BluetoothDtmPositionalEventSeed,
        ) -> Result<BluetoothDtmPositionalEventWords, BuildError>,
    ) -> Result<
        BluetoothDtmMemoryGraphPositionalEventPrepared,
        BluetoothDtmMemoryGraphPrepareFailure<BuildError>,
    > {
        self.discard_packet_readiness()
            .try_prepare_positional_event(build)
    }

    /// Discard only the readiness proof and recover the CPU-owned graph.
    ///
    /// Packet bytes are retained, but another TX event must prepare them again
    /// to obtain a fresh proof for its exact pattern and length.
    pub fn discard_packet_readiness(self) -> BluetoothDtmMemoryGraphCpuOwned {
        BluetoothDtmMemoryGraphCpuOwned {
            storage: self.storage,
            binding: self.binding,
        }
    }
}

/// Failed TX packet preparation retaining the byte-unchanged graph owner.
///
/// The LE Test PDU header is validated before any packet byte is written.
pub struct BluetoothDtmMemoryGraphTxPacketPrepareFailure {
    owner: BluetoothDtmMemoryGraphCpuOwned,
    error: BluetoothDtmTxPacketPrepareError,
}

impl BluetoothDtmMemoryGraphTxPacketPrepareFailure {
    /// Return the finite packet-header validation failure.
    pub const fn error(&self) -> BluetoothDtmTxPacketPrepareError {
        self.error
    }

    /// Recover the byte-unchanged owner and exact validation failure.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothDtmTxPacketPrepareError,
    ) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for BluetoothDtmMemoryGraphTxPacketPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmMemoryGraphTxPacketPrepareFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl BluetoothDtmMemoryGraphCpuOwned {
    /// Opaque identity retained across every affine allocation epoch.
    pub const fn identity(&self) -> BluetoothDtmMemoryGraphIdentity {
        self.binding.identity()
    }

    /// Product-owned allocation policy bound to this exact graph.
    pub const fn allocation_config(&self) -> BluetoothDtmSchedulerAllocationConfig {
        self.binding.allocation_config()
    }

    /// Physical extent of the complete pinned graph.
    pub const fn range(&self) -> (u32, u32) {
        self.binding.range()
    }

    /// End this CPU-owned graph epoch without releasing its static allocation.
    ///
    /// The transition is intentionally mutation-free. It consumes the only
    /// ordinary CPU owner and removes every event-preparation operation until
    /// [`BluetoothDtmMemoryGraphReclaimed::reinitialize`] starts a fresh epoch.
    /// No equivalent operation exists on any hardware or completion typestate.
    pub fn into_reclaimed(self) -> BluetoothDtmMemoryGraphReclaimed {
        BluetoothDtmMemoryGraphReclaimed {
            storage: self.storage,
            binding: self.binding,
        }
    }

    /// Consume this graph and install one complete CPU-owned TX packet image.
    ///
    /// The LE Test PDU Type is validated before the first mutation; rejection
    /// returns this exact byte-unchanged owner for retry or shutdown.
    ///
    /// The fixed payload array makes the copy total for every accepted `u8`
    /// length; no callback can claim readiness without supplying all declared
    /// bytes. Bytes after the declared payload remain unchanged.
    pub fn prepare_tx_packet(
        mut self,
        payload_type: u8,
        payload_length: u8,
        payload: &[u8; BLUETOOTH_DTM_MAX_PACKET_CAPACITY],
    ) -> Result<
        BluetoothDtmMemoryGraphTxPacketPrepared,
        BluetoothDtmMemoryGraphTxPacketPrepareFailure,
    > {
        let pdu_header = match BluetoothLeTestPduHeader::without_cte(payload_type) {
            Ok(pdu_header) => pdu_header,
            Err(error) => {
                return Err(BluetoothDtmMemoryGraphTxPacketPrepareFailure { owner: self, error });
            }
        };
        let packet_length = self
            .storage
            .as_mut()
            .prepare_tx_packet(pdu_header, &payload[..payload_length as usize]);

        Ok(BluetoothDtmMemoryGraphTxPacketPrepared {
            storage: self.storage,
            binding: self.binding,
            _pdu_header: pdu_header,
            packet_length,
        })
    }

    /// Validate the complete private packet chain, then build and commit one
    /// role-neutral positional event-word image.
    ///
    /// The consuming closure receives current words and current private links
    /// from this exact owner. Header-binding, builder and descriptor-anchor
    /// failures return the byte-unchanged owner. Once validation succeeds,
    /// committing all nineteen offsets is infallible and remains entirely
    /// CPU-owned.
    pub fn try_prepare_positional_event<BuildError>(
        mut self,
        build: impl FnOnce(
            BluetoothDtmPositionalEventSeed,
        ) -> Result<BluetoothDtmPositionalEventWords, BuildError>,
    ) -> Result<
        BluetoothDtmMemoryGraphPositionalEventPrepared,
        BluetoothDtmMemoryGraphPrepareFailure<BuildError>,
    > {
        let seed = match self
            .storage
            .as_ref()
            .get_ref()
            .positional_event_seed(&self.binding)
        {
            Ok(seed) => seed,
            Err(error) => {
                return Err(BluetoothDtmMemoryGraphPrepareFailure { owner: self, error });
            }
        };
        let previous = seed.words();
        let candidate = match build(seed) {
            Ok(candidate) => candidate,
            Err(error) => {
                return Err(BluetoothDtmMemoryGraphPrepareFailure {
                    owner: self,
                    error: BluetoothDtmMemoryGraphPrepareError::Build(error),
                });
            }
        };

        if let Err(error) = self.storage.as_mut().validate_and_commit_positional_event(
            &self.binding,
            seed,
            candidate,
        ) {
            return Err(BluetoothDtmMemoryGraphPrepareFailure { owner: self, error });
        }

        Ok(BluetoothDtmMemoryGraphPositionalEventPrepared {
            storage: self.storage,
            binding: self.binding,
            previous,
        })
    }

    fn initialize_reviewed_allocation(&mut self, config: BluetoothDtmSchedulerAllocationConfig) {
        self.storage
            .as_mut()
            .initialize_reviewed_allocation(&self.binding, config);
    }
}

impl BluetoothDtmMemoryGraphStorage {
    /// Bind the real addresses of one unique static S31 allocation.
    ///
    /// All address and extent checks finish before the first mutation. The
    /// returned CPU owner installs only reviewed allocation-time headers and
    /// private-chain anchors; the graph remains unreachable by hardware.
    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
        config: BluetoothDtmSchedulerAllocationConfig,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphBindFailure> {
        let base = match u32::try_from(core::ptr::addr_of!(*storage).addr()) {
            Ok(base) => base,
            Err(_) => {
                return Err(BluetoothDtmMemoryGraphBindFailure::new(
                    storage,
                    BluetoothDtmMemoryGraphBindError::AddressWidth,
                ));
            }
        };
        Self::pin_static_inner(storage, base, config)
    }

    /// Bind a deterministic physical-SRAM address to a native ownership model.
    ///
    /// The model never derives an address from its host pointer. Passing a raw
    /// integer is rejected by the type system:
    ///
    /// ```compile_fail
    /// use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphStorage;
    ///
    /// let storage = Box::leak(Box::new(BluetoothDtmMemoryGraphStorage::new()));
    /// let config = open_esp_radio_esp32s31_bluetooth_memory::
    ///     BluetoothDtmSchedulerAllocationConfig::new(1, 1, 1);
    /// let _ = BluetoothDtmMemoryGraphStorage::pin_static_model(
    ///     storage,
    ///     0x2f00_0100,
    ///     config,
    /// );
    /// ```
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: BluetoothDtmMemoryGraphModelAddress,
        config: BluetoothDtmSchedulerAllocationConfig,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphBindFailure> {
        Self::pin_static_inner(storage, base.address(), config)
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: u32,
        config: BluetoothDtmSchedulerAllocationConfig,
    ) -> Result<BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphBindFailure> {
        let identity = BluetoothDtmMemoryGraphIdentity::for_storage(storage);
        let binding = match BluetoothDtmMemoryGraphBinding::new(identity, base, config) {
            Ok(binding) => binding,
            Err(error) => return Err(BluetoothDtmMemoryGraphBindFailure::new(storage, error)),
        };
        let mut owner = BluetoothDtmMemoryGraphCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        };
        owner.initialize_reviewed_allocation(config);
        Ok(owner)
    }
}

#[cfg(test)]
mod tests;
