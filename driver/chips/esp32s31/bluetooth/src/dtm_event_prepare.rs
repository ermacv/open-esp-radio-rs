//! Role-consistent composition of reviewed DTM event words into bound memory.
//!
//! This layer combines already validated LLL transforms with the lower
//! consuming memory transaction. The resulting state remains CPU-only and
//! retains TX packet readiness where that role requires it. It does not prove
//! list insertion, a visibility fence, a hardware latch or completion
//! ownership.

#![forbid(unsafe_code)]

use core::{convert::Infallible, marker::PhantomData};

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCompletionObservation, BluetoothDtmMemoryGraphCompletionObserved,
    BluetoothDtmMemoryGraphRecycleCleaned, BluetoothDtmMemoryGraphRecycleError,
    BluetoothDtmMemoryGraphRxSuccessRecycleError, BluetoothDtmSchedulerItemCompletionStatus,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphPositionalEventPrepared,
    BluetoothDtmMemoryGraphPrepareError, BluetoothDtmMemoryGraphPrepareFailure,
    BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared, BluetoothDtmPositionalEventWords,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphEmptyListLinkPrepared, BluetoothDtmMemoryGraphHardwareOwned,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerSoftwareListRemovalReady,
};

use crate::{
    BluetoothDtmLinkStateReset, BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern,
    BluetoothDtmPreparedTxGraph, BluetoothDtmRole, BluetoothSchedulerReservation,
    BluetoothSchedulerSequenceReady, dtm_rx_completion::BluetoothDtmReceiverSession,
    dtm_scheduler_item::apply_overlap_insertion_power,
};

/// Type marker for a transmitter event with a prepared packet prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmTransmitterEvent {}

/// Type marker for a receiver event without a transmitter packet prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmReceiverEvent {}

/// Why two validated DTM transforms cannot describe one event plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmReviewedEventWordsPlanError {
    /// Link-state and scheduler-item transforms encode different DTM roles.
    RoleMismatch {
        /// Role required by the selected constructor.
        expected: BluetoothDtmRole,
        /// Role selected by the link-state reset.
        link_state: BluetoothDtmRole,
        /// Role selected by the scheduler-item transform.
        scheduler_item: BluetoothDtmRole,
    },
}

/// Rejected role composition retaining the exact sequence-ready reservation.
pub struct BluetoothDtmReviewedEventWordsPlanFailure {
    error: BluetoothDtmReviewedEventWordsPlanError,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
}

impl BluetoothDtmReviewedEventWordsPlanFailure {
    /// Borrow the finite composition failure reason.
    pub const fn error(&self) -> BluetoothDtmReviewedEventWordsPlanError {
        self.error
    }

    /// Recover the reservation for explicit scheduler release.
    pub fn into_reservation(
        self,
    ) -> BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady> {
        self.reservation
    }
}

impl core::fmt::Debug for BluetoothDtmReviewedEventWordsPlanFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmReviewedEventWordsPlanFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Validated role-consistent plan for the nineteen reviewed event words.
///
/// Private chain links are deliberately absent from plan identity. They are
/// replaced inside `prepare` with fresh links sampled from the consumed graph.
/// Construction consumes an affine reservation that already passed strict
/// overlap resolution and both fresh Controller-time deadline gates. Sequence
/// timing can therefore only be formed from the resolved window retained by
/// that reservation.
pub struct BluetoothDtmReviewedEventWordsPlan<Role> {
    link_state: BluetoothDtmLinkStateReset,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

impl<Role> BluetoothDtmReviewedEventWordsPlan<Role> {
    fn new_for_role(
        link_state: BluetoothDtmLinkStateReset,
        reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
        expected: BluetoothDtmRole,
    ) -> Result<Self, BluetoothDtmReviewedEventWordsPlanFailure> {
        let link_role = link_state.role();
        let scheduler_role = reservation.event().role();
        if link_role != expected || scheduler_role != expected {
            return Err(BluetoothDtmReviewedEventWordsPlanFailure {
                error: BluetoothDtmReviewedEventWordsPlanError::RoleMismatch {
                    expected,
                    link_state: link_role,
                    scheduler_item: scheduler_role,
                },
                reservation,
            });
        }
        Ok(Self {
            link_state,
            reservation,
            _role: PhantomData,
        })
    }

    fn apply_to_seed(
        &self,
        seed: open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmPositionalEventSeed,
    ) -> BluetoothDtmPositionalEventWords {
        let current = seed.words();
        let event = self.reservation.event();
        let epoch = self.reservation.epoch();
        let resolved_window = self.reservation.window();
        let link_state = self
            .link_state
            .with_private_links(seed.tx_head(), seed.rx_tail())
            .apply(current.link_state())
            .apply_event_context(self.link_state.role(), epoch.raw_time_for_scheduler_time(0));
        let scheduler_item = event.apply_raw_window(
            current.scheduler_item(),
            resolved_window.start(),
            resolved_window.end(),
        );
        let scheduler_item = apply_overlap_insertion_power(scheduler_item, link_state)
            .apply_sequence_timing(self.reservation.timing_policy().sequence_lead_raw_delta());
        BluetoothDtmPositionalEventWords::new(link_state, scheduler_item)
    }
}

/// Failed graph preparation retaining the sequence-ready scheduler plan.
pub struct BluetoothDtmReviewedEventPrepareFailure<Role> {
    memory: BluetoothDtmMemoryGraphPrepareFailure,
    plan: BluetoothDtmReviewedEventWordsPlan<Role>,
}

impl<Role> BluetoothDtmReviewedEventPrepareFailure<Role> {
    /// Borrow the lower graph-preparation reason.
    pub const fn error(&self) -> &BluetoothDtmMemoryGraphPrepareError {
        self.memory.error()
    }

    /// Recover both the unchanged graph failure and the reusable plan.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphPrepareFailure,
        BluetoothDtmReviewedEventWordsPlan<Role>,
    ) {
        (self.memory, self.plan)
    }
}

impl<Role> core::fmt::Debug for BluetoothDtmReviewedEventPrepareFailure<Role> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmReviewedEventPrepareFailure")
            .field("error", self.memory.error())
            .finish_non_exhaustive()
    }
}

impl BluetoothDtmReviewedEventWordsPlan<BluetoothDtmTransmitterEvent> {
    /// Pair a transmitter reset with its sequence-ready scheduler reservation.
    pub fn new_transmitter(
        link_state: BluetoothDtmLinkStateReset,
        reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    ) -> Result<Self, BluetoothDtmReviewedEventWordsPlanFailure> {
        Self::new_for_role(link_state, reservation, BluetoothDtmRole::Transmitter)
    }

    /// Apply this TX plan only to a graph carrying a complete standard packet.
    ///
    /// Any lower validation failure returns an ordinary CPU owner. A retry
    /// must deliberately prepare a fresh packet-readiness proof.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc failure retains both the unchanged SRAM graph and affine reservation"
    )]
    pub fn prepare(
        self,
        owner: BluetoothDtmPreparedTxGraph,
    ) -> Result<
        BluetoothDtmReviewedEventWordsPrepared<BluetoothDtmTransmitterEvent>,
        BluetoothDtmReviewedEventPrepareFailure<BluetoothDtmTransmitterEvent>,
    > {
        let plan = self;
        let (memory, pattern, length) = owner.into_parts();
        let prepared = match memory
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(plan.apply_to_seed(seed)))
        {
            Ok(prepared) => prepared,
            Err(memory) => {
                return Err(BluetoothDtmReviewedEventPrepareFailure { memory, plan });
            }
        };

        Ok(BluetoothDtmReviewedEventWordsPrepared {
            memory: prepared,
            context: BluetoothDtmEventContext::Transmitter { pattern, length },
            reservation: plan.reservation,
            _role: PhantomData,
        })
    }
}

impl BluetoothDtmReviewedEventWordsPlan<BluetoothDtmReceiverEvent> {
    /// Pair a receiver reset with its sequence-ready scheduler reservation.
    pub fn new_receiver(
        link_state: BluetoothDtmLinkStateReset,
        reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    ) -> Result<Self, BluetoothDtmReviewedEventWordsPlanFailure> {
        Self::new_for_role(link_state, reservation, BluetoothDtmRole::Receiver)
    }

    /// Apply this RX plan to one exact graph/session aggregate.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc failure retains both the unchanged SRAM graph and affine reservation"
    )]
    pub fn prepare(
        self,
        owner: BluetoothDtmReceiverCpuOwned,
    ) -> Result<
        BluetoothDtmReviewedEventWordsPrepared<BluetoothDtmReceiverEvent>,
        BluetoothDtmReceiverEventPrepareFailure,
    > {
        let plan = self;
        let BluetoothDtmReceiverCpuOwned { memory, session } = owner;
        let prepared = match memory
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(plan.apply_to_seed(seed)))
        {
            Ok(prepared) => prepared,
            Err(memory) => {
                return Err(BluetoothDtmReceiverEventPrepareFailure {
                    memory,
                    plan,
                    session,
                });
            }
        };

        Ok(BluetoothDtmReviewedEventWordsPrepared {
            memory: prepared,
            context: BluetoothDtmEventContext::Receiver { session },
            reservation: plan.reservation,
            _role: PhantomData,
        })
    }
}

/// CPU-owned receiver graph bound to one non-copyable DTM test session.
#[must_use = "the receiver graph and its accumulated test state must stay together"]
pub struct BluetoothDtmReceiverCpuOwned {
    memory: BluetoothDtmMemoryGraphCpuOwned,
    session: BluetoothDtmReceiverSession,
}

impl BluetoothDtmReceiverCpuOwned {
    /// Start one fresh receiver session on an ordinary CPU-owned graph.
    pub fn new(memory: BluetoothDtmMemoryGraphCpuOwned) -> Self {
        Self {
            memory,
            session: BluetoothDtmReceiverSession::new(),
        }
    }

    /// Current received-packet count retained for LE Test End.
    pub const fn received_packet_count(&self) -> u16 {
        self.session.received_packet_count()
    }

    /// Split an idle receiver aggregate into its memory and accumulated count.
    ///
    /// This is only an ownership operation. It does not issue LE Test End,
    /// stop hardware or prove controller quiescence.
    pub fn into_memory_and_packet_count(self) -> (BluetoothDtmMemoryGraphCpuOwned, u16) {
        (self.memory, self.session.received_packet_count())
    }
}

/// Failed RX graph preparation retaining the exact session and plan.
#[must_use = "RX preparation failure retains the graph, session and reservation plan"]
pub struct BluetoothDtmReceiverEventPrepareFailure {
    memory: BluetoothDtmMemoryGraphPrepareFailure,
    plan: BluetoothDtmReviewedEventWordsPlan<BluetoothDtmReceiverEvent>,
    session: BluetoothDtmReceiverSession,
}

impl BluetoothDtmReceiverEventPrepareFailure {
    /// Borrow the lower graph-preparation reason.
    pub const fn error(&self) -> &BluetoothDtmMemoryGraphPrepareError {
        self.memory.error()
    }

    /// Recover the unchanged aggregate, error and reservation plan for retry.
    pub fn into_retry(
        self,
    ) -> (
        BluetoothDtmReceiverCpuOwned,
        BluetoothDtmMemoryGraphPrepareError,
        BluetoothDtmReviewedEventWordsPlan<BluetoothDtmReceiverEvent>,
    ) {
        let (memory, error) = self.memory.into_parts();
        (
            BluetoothDtmReceiverCpuOwned {
                memory,
                session: self.session,
            },
            error,
            self.plan,
        )
    }
}

impl core::fmt::Debug for BluetoothDtmReceiverEventPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmReceiverEventPrepareFailure")
            .field("error", self.memory.error())
            .finish_non_exhaustive()
    }
}

enum BluetoothDtmEventContext {
    Transmitter {
        pattern: BluetoothDtmPayloadPattern,
        length: BluetoothDtmPayloadLength,
    },
    Receiver {
        session: BluetoothDtmReceiverSession,
    },
}

/// CPU-owned bound graph containing one role-consistent reviewed word image.
///
/// The role marker preserves whether TX packet readiness was consumed into the
/// event. This type exposes no packet mutation or publication operation.
pub struct BluetoothDtmReviewedEventWordsPrepared<Role> {
    memory: BluetoothDtmMemoryGraphPositionalEventPrepared,
    context: BluetoothDtmEventContext,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

impl<Role> BluetoothDtmReviewedEventWordsPrepared<Role> {
    /// Return the role shared by both applied transforms.
    pub const fn role(&self) -> BluetoothDtmRole {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver { .. } => BluetoothDtmRole::Receiver,
        }
    }

    /// Return the typed controller-SRAM identity of the prepared item.
    ///
    /// This identity is derived from the retained non-forgeable graph binding;
    /// it does not grant publication or hardware ownership.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    /// Read back the exact nineteen CPU-owned positional words.
    pub fn words(&self) -> BluetoothDtmPositionalEventWords {
        self.memory.words()
    }

    /// Install the common scheduler bookkeeping prefix for this exact graph.
    ///
    /// The resulting state remains CPU-owned and cancellable. Only that later
    /// state may form a scheduler request; this prevents event words without
    /// the in-flight sentinel and cleared completion link from being admitted.
    pub fn prepare_scheduler_bookkeeping(self) -> BluetoothDtmSchedulerBookkeepingPrepared<Role> {
        BluetoothDtmSchedulerBookkeepingPrepared {
            memory: self.memory.prepare_scheduler_bookkeeping(),
            context: self.context,
            reservation: self.reservation,
            _role: PhantomData,
        }
    }
}

impl BluetoothDtmReviewedEventWordsPrepared<BluetoothDtmTransmitterEvent> {
    /// Return the exact standard pattern retained from packet preparation.
    pub const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { pattern, .. } => *pattern,
            BluetoothDtmEventContext::Receiver { .. } => unreachable!(),
        }
    }

    /// Return the exact payload length retained from packet preparation.
    pub const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { length, .. } => *length,
            BluetoothDtmEventContext::Receiver { .. } => unreachable!(),
        }
    }

    /// Cancel before publication and recover both TX CPU-owned resources.
    pub fn cancel(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    ) {
        (self.memory.cancel(), self.reservation)
    }
}

impl BluetoothDtmReviewedEventWordsPrepared<BluetoothDtmReceiverEvent> {
    /// Cancel before publication without detaching the RX session from memory.
    pub fn cancel(
        self,
    ) -> (
        BluetoothDtmReceiverCpuOwned,
        BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    ) {
        let BluetoothDtmEventContext::Receiver { session } = self.context else {
            unreachable!()
        };
        (
            BluetoothDtmReceiverCpuOwned {
                memory: self.memory.cancel(),
                session,
            },
            self.reservation,
        )
    }
}

/// CPU-owned DTM graph after the reviewed scheduler bookkeeping prefix.
///
/// The complete descriptor, common-scheduler insertion/merge transaction,
/// private packet-engine latch and visibility fence are deliberately absent
/// from this state.
#[must_use = "the scheduler-prepared DTM graph must remain owned or be cancelled"]
pub struct BluetoothDtmSchedulerBookkeepingPrepared<Role> {
    memory: BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared,
    context: BluetoothDtmEventContext,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

impl<Role> BluetoothDtmSchedulerBookkeepingPrepared<Role> {
    /// Return the role shared by the retained DTM transforms.
    pub const fn role(&self) -> BluetoothDtmRole {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver { .. } => BluetoothDtmRole::Receiver,
        }
    }

    /// Return the typed controller-SRAM identity of the retained item.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    /// Hardware list assigned to the zeroed DTM scheduler context.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        BluetoothSchedulerHardwareListIndex::ZERO
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn prepare_empty_list_link(self) -> BluetoothDtmEmptyListLinkPrepared<Role> {
        BluetoothDtmEmptyListLinkPrepared {
            memory: self.memory.prepare_empty_list_link(),
            context: self.context,
            reservation: self.reservation,
            _role: PhantomData,
        }
    }

    /// Cancel before publication and recover the prepared event words.
    pub fn cancel(self) -> BluetoothDtmReviewedEventWordsPrepared<Role> {
        BluetoothDtmReviewedEventWordsPrepared {
            memory: self.memory.cancel(),
            context: self.context,
            reservation: self.reservation,
            _role: PhantomData,
        }
    }
}

/// Internal join candidate after the item-side empty-list transform.
///
/// Only the scheduler module can combine this memory owner with its affine
/// exclusive empty-list epoch. Keeping this type crate-private prevents a
/// memory-only transition from being mistaken for list ownership.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothDtmEmptyListLinkPrepared<Role> {
    memory: BluetoothDtmMemoryGraphEmptyListLinkPrepared,
    context: BluetoothDtmEventContext,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Role> BluetoothDtmEmptyListLinkPrepared<Role> {
    pub(crate) const fn role(&self) -> BluetoothDtmRole {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver { .. } => BluetoothDtmRole::Receiver,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        BluetoothSchedulerHardwareListIndex::ZERO
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_hardware_owned(
        self,
        publication: &BluetoothSchedulerHardwareListHeadPublished,
    ) -> BluetoothDtmHardwareOwnedEvent<Role> {
        BluetoothDtmHardwareOwnedEvent {
            memory: self.memory.into_hardware_owned(publication),
            context: self.context,
            _reservation: self.reservation,
            _role: PhantomData,
        }
    }

    pub(crate) fn cancel(self) -> BluetoothDtmSchedulerBookkeepingPrepared<Role> {
        BluetoothDtmSchedulerBookkeepingPrepared {
            memory: self.memory.cancel(),
            context: self.context,
            reservation: self.reservation,
            _role: PhantomData,
        }
    }
}

/// Internal DTM event whose pinned graph has crossed the hardware-head edge.
///
/// Only the scheduler lifecycle can create this owner by pairing the prepared
/// event with its exact affine PAC publication. It intentionally has no
/// cancellation path or mutable access to controller-owned storage.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothDtmHardwareOwnedEvent<Role> {
    memory: BluetoothDtmMemoryGraphHardwareOwned,
    context: BluetoothDtmEventContext,
    _reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Role> BluetoothDtmHardwareOwnedEvent<Role> {
    pub(crate) const fn role(&self) -> BluetoothDtmRole {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver { .. } => BluetoothDtmRole::Receiver,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> BluetoothDtmHardwareOwnedEventCompletionObservation<Role> {
        let Self {
            memory,
            context,
            _reservation: reservation,
            _role: _,
        } = self;
        match memory.observe_completion(observed) {
            BluetoothDtmMemoryGraphCompletionObservation::ListMismatch { owner, observed } => {
                BluetoothDtmHardwareOwnedEventCompletionObservation::ListMismatch {
                    item: Self {
                        memory: owner,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                    observed,
                }
            }
            BluetoothDtmMemoryGraphCompletionObservation::StillInFlight(memory) => {
                BluetoothDtmHardwareOwnedEventCompletionObservation::StillInFlight(Self {
                    memory,
                    context,
                    _reservation: reservation,
                    _role: PhantomData,
                })
            }
            BluetoothDtmMemoryGraphCompletionObservation::CompletionObserved(memory) => {
                BluetoothDtmHardwareOwnedEventCompletionObservation::CompletionObserved(
                    BluetoothDtmCompletionObservedEvent {
                        memory,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                )
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothDtmHardwareOwnedEventCompletionObservation<Role> {
    ListMismatch {
        item: BluetoothDtmHardwareOwnedEvent<Role>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(BluetoothDtmHardwareOwnedEvent<Role>),
    CompletionObserved(BluetoothDtmCompletionObservedEvent<Role>),
}

/// Internal event retaining every owner after a non-sentinel status read.
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothDtmCompletionObservedEvent<Role> {
    memory: BluetoothDtmMemoryGraphCompletionObserved,
    context: BluetoothDtmEventContext,
    _reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothDtmCompletionObservedEvent<Role> {
    pub(crate) const fn role(&self) -> BluetoothDtmRole {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver { .. } => BluetoothDtmRole::Receiver,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
        self.memory.status()
    }

    pub(crate) fn recycle<const CAPACITY: usize>(
        self,
        timeline: &mut crate::BluetoothSchedulerTimeline<CAPACITY>,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<
        BluetoothDtmRecycleTimelineReleasedEvent<Role>,
        BluetoothDtmCompletionRecycleFailure<Role>,
    > {
        let Self {
            memory,
            context,
            _reservation: reservation,
            _role: _,
        } = self;
        let prepared = match memory.prepare_recycle_after_software_list_removal(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (memory, removal) = failure.into_parts();
                return Err(BluetoothDtmCompletionRecycleFailure {
                    error: BluetoothDtmCompletionRecycleError::MemoryIdentity(error),
                    item: BluetoothDtmCompletionObservedEvent {
                        memory,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                    removal,
                });
            }
        };
        let release = match timeline.prepare_release(reservation) {
            Ok(release) => release,
            Err(failure) => {
                let reservation = failure.into_reservation();
                let (memory, removal) = prepared.into_parts();
                return Err(BluetoothDtmCompletionRecycleFailure {
                    error: BluetoothDtmCompletionRecycleError::ReservationIdentityMismatch,
                    item: BluetoothDtmCompletionObservedEvent {
                        memory,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                    removal,
                });
            }
        };
        let memory = prepared.commit();
        release.commit();
        Ok(BluetoothDtmRecycleTimelineReleasedEvent {
            memory,
            context,
            _role: PhantomData,
        })
    }
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmCompletionObservedEvent<BluetoothDtmReceiverEvent> {
    pub(crate) fn recycle_receiver_success<const CAPACITY: usize>(
        self,
        timeline: &mut crate::BluetoothSchedulerTimeline<CAPACITY>,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<
        BluetoothDtmRxSuccessRecycleTimelineReleasedEvent,
        BluetoothDtmCompletionRecycleFailure<BluetoothDtmReceiverEvent>,
    > {
        let Self {
            memory,
            context,
            _reservation: reservation,
            _role: _,
        } = self;
        let prepared = match memory.prepare_recycle_after_software_list_removal(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (memory, removal) = failure.into_parts();
                return Err(BluetoothDtmCompletionRecycleFailure {
                    error: BluetoothDtmCompletionRecycleError::MemoryIdentity(error),
                    item: BluetoothDtmCompletionObservedEvent {
                        memory,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                    removal,
                });
            }
        };
        let rx_prepared = match prepared.prepare_receiver_success() {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (memory, removal) = failure.into_recycle_prepared().into_parts();
                return Err(BluetoothDtmCompletionRecycleFailure {
                    error: BluetoothDtmCompletionRecycleError::ReceiverSuccessMemory(error),
                    item: BluetoothDtmCompletionObservedEvent {
                        memory,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                    removal,
                });
            }
        };
        let release = match timeline.prepare_release(reservation) {
            Ok(release) => release,
            Err(failure) => {
                let reservation = failure.into_reservation();
                let (memory, removal) = rx_prepared.into_recycle_prepared().into_parts();
                return Err(BluetoothDtmCompletionRecycleFailure {
                    error: BluetoothDtmCompletionRecycleError::ReservationIdentityMismatch,
                    item: BluetoothDtmCompletionObservedEvent {
                        memory,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                    removal,
                });
            }
        };
        let BluetoothDtmEventContext::Receiver { mut session } = context else {
            unreachable!()
        };
        let (memory, outcome) = rx_prepared.observe().consume_then_commit(|projection| {
            projection.map_or(
                crate::BluetoothDtmRxCompletionOutcome::NoReturnedPacket,
                |result| session.account_projection(result),
            )
        });
        release.commit();
        Ok(BluetoothDtmRxSuccessRecycleTimelineReleasedEvent {
            memory,
            session,
            outcome,
        })
    }
}

/// Internal reason the complete DTM recycle transaction rejected ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothDtmCompletionRecycleError {
    MemoryIdentity(BluetoothDtmMemoryGraphRecycleError),
    ReceiverSuccessMemory(BluetoothDtmMemoryGraphRxSuccessRecycleError),
    ReservationIdentityMismatch,
}

/// Lossless rejection before either memory or timeline ownership changed.
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothDtmCompletionRecycleFailure<Role> {
    error: BluetoothDtmCompletionRecycleError,
    item: BluetoothDtmCompletionObservedEvent<Role>,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothDtmCompletionRecycleFailure<Role> {
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothDtmCompletionRecycleError,
        BluetoothDtmCompletionObservedEvent<Role>,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.error, self.item, self.removal)
    }
}

/// SRAM-cleaned event after the exact timeline reservation was released.
///
/// CPU graph ownership is still withheld until the source scheduler list has
/// committed its removal-ready epoch back to Empty.
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothDtmRecycleTimelineReleasedEvent<Role> {
    memory: BluetoothDtmMemoryGraphRecycleCleaned,
    context: BluetoothDtmEventContext,
    _role: PhantomData<Role>,
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothDtmRecycleTimelineReleasedEvent<Role> {
    pub(crate) fn finish_source_list_release(self) -> BluetoothDtmRecycledEvent<Role> {
        let (memory, status) = self.memory.into_cpu_owned().into_parts();
        BluetoothDtmRecycledEvent {
            memory,
            context: self.context,
            status,
            _role: PhantomData,
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothDtmRxSuccessRecycleTimelineReleasedEvent {
    memory: BluetoothDtmMemoryGraphRecycleCleaned,
    session: BluetoothDtmReceiverSession,
    outcome: crate::BluetoothDtmRxCompletionOutcome,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmRxSuccessRecycleTimelineReleasedEvent {
    pub(crate) fn finish_source_list_release(self) -> BluetoothDtmRxRearmedEvent {
        let (memory, _) = self.memory.into_cpu_owned().into_parts();
        BluetoothDtmRxRearmedEvent {
            memory,
            session: self.session,
            outcome: self.outcome,
        }
    }
}

/// CPU-owned graph after one exact completion/removal/recycle transaction.
#[must_use = "the recycled DTM graph must be retained by the role owner"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothDtmRecycledEvent<Role> {
    memory: BluetoothDtmMemoryGraphCpuOwned,
    context: BluetoothDtmEventContext,
    status: BluetoothDtmSchedulerItemCompletionStatus,
    _role: PhantomData<Role>,
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothDtmRecycledEvent<Role> {
    /// Role retained by the recycled event.
    pub const fn role(&self) -> BluetoothDtmRole {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver { .. } => BluetoothDtmRole::Receiver,
        }
    }

    /// Completion status retained across recycle.
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
        self.status
    }
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmRecycledEvent<BluetoothDtmTransmitterEvent> {
    /// Transmitter packet pattern retained by the recycled event.
    pub const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { pattern, .. } => *pattern,
            BluetoothDtmEventContext::Receiver { .. } => unreachable!(),
        }
    }

    /// Transmitter payload length retained by the recycled event.
    pub const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { length, .. } => *length,
            BluetoothDtmEventContext::Receiver { .. } => unreachable!(),
        }
    }

    /// Recover the TX graph for a later packet preparation or shutdown.
    pub fn into_memory(self) -> BluetoothDtmMemoryGraphCpuOwned {
        self.memory
    }
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmRecycledEvent<BluetoothDtmReceiverEvent> {
    /// Recover the unchanged RX session after a non-success scheduler event.
    pub fn into_next(self) -> BluetoothDtmReceiverCpuOwned {
        let BluetoothDtmEventContext::Receiver { session } = self.context else {
            unreachable!()
        };
        BluetoothDtmReceiverCpuOwned {
            memory: self.memory,
            session,
        }
    }
}

/// RX graph/session after one successful bounded drain and re-arm.
#[must_use = "the re-armed receiver session must continue or finish explicitly"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothDtmRxRearmedEvent {
    memory: BluetoothDtmMemoryGraphCpuOwned,
    session: BluetoothDtmReceiverSession,
    outcome: crate::BluetoothDtmRxCompletionOutcome,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmRxRearmedEvent {
    /// Semantic result of this event's bounded returned-buffer drain.
    pub const fn outcome(&self) -> crate::BluetoothDtmRxCompletionOutcome {
        self.outcome
    }

    /// Accumulated packet count retained for LE Test End.
    pub const fn received_packet_count(&self) -> u16 {
        self.session.received_packet_count()
    }

    /// Consume this re-arm proof into the sole next-event aggregate.
    pub fn into_next(self) -> BluetoothDtmReceiverCpuOwned {
        BluetoothDtmReceiverCpuOwned {
            memory: self.memory,
            session: self.session,
        }
    }
}

impl BluetoothDtmSchedulerBookkeepingPrepared<BluetoothDtmTransmitterEvent> {
    /// Return the exact standard pattern retained through bookkeeping.
    pub const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { pattern, .. } => *pattern,
            BluetoothDtmEventContext::Receiver { .. } => unreachable!(),
        }
    }

    /// Return the exact payload length retained through bookkeeping.
    pub const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        match &self.context {
            BluetoothDtmEventContext::Transmitter { length, .. } => *length,
            BluetoothDtmEventContext::Receiver { .. } => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDtmBoundSramLinkAddress, BluetoothDtmMemoryGraphModelAddress,
        BluetoothDtmMemoryGraphStorage, BluetoothDtmSchedulerAllocationConfig,
    };
    use open_esp_radio_esp32s31_hal::BluetoothSchedulerHardwareListIndex;
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::{
        BluetoothDtmReceiverCpuOwned, BluetoothDtmReviewedEventWordsPlan,
        BluetoothDtmReviewedEventWordsPlanError,
    };
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
        BluetoothDtmLinkStateReset, BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern,
        BluetoothDtmPhy, BluetoothDtmRole, BluetoothDtmSchedulerItemEvent,
        BluetoothDtmSchedulerTimingPolicy, BluetoothDtmTxGraphPrepare,
        BluetoothSchedulerReservation, BluetoothSchedulerSequenceAuthorizationError,
        BluetoothSchedulerSequenceReady, BluetoothSchedulerTimeline,
    };

    fn owner(base: u32) -> crate::BluetoothDtmMemoryGraphCpuOwned {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let base = BluetoothDtmMemoryGraphModelAddress::new(base)
            .expect("test base has valid compressed-pointer syntax");
        BluetoothDtmMemoryGraphStorage::pin_static_model(
            storage,
            base,
            BluetoothDtmSchedulerAllocationConfig::new(2, 3, 5, 4),
        )
        .expect("test graph fits physical controller SRAM")
    }

    fn epoch() -> BluetoothControllerSchedulerEpoch {
        BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
        )
    }

    fn item(role: BluetoothDtmRole) -> BluetoothDtmSchedulerItemEvent {
        BluetoothDtmSchedulerItemEvent::new(
            BluetoothDtmChannel::new(5).expect("channel five is valid"),
            match role {
                BluetoothDtmRole::Transmitter => BluetoothDtmPhy::Le2M,
                BluetoothDtmRole::Receiver => BluetoothDtmPhy::LeCoded,
            },
            role,
            1_012,
            1_020,
        )
        .expect("selected PHY is valid for its role")
    }

    fn timing_policy() -> BluetoothDtmSchedulerTimingPolicy {
        BluetoothDtmSchedulerTimingPolicy::from_scheduler_config(
            crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
        )
    }

    fn admission_sample() -> BluetoothControllerTimeSample {
        BluetoothControllerTimeSample::for_validation(92)
    }

    fn reservation<const CAPACITY: usize>(
        timeline: &mut BluetoothSchedulerTimeline<CAPACITY>,
        role: BluetoothDtmRole,
    ) -> BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady> {
        timeline
            .reserve_dtm_event(item(role), epoch(), timing_policy(), admission_sample())
            .expect("the first guarded deadline is open")
            .authorize_sequence(admission_sample())
            .expect("the second guarded deadline is open")
    }

    #[test]
    fn tx_plan_requires_and_retains_the_prepared_packet_identity() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let stale = BluetoothDtmBoundSramLinkAddress::new(0x2f00_0400)
            .expect("stale model link remains syntactically valid");
        let reset = BluetoothDtmLinkStateReset::new(
            Some(stale),
            Some(stale),
            0x15,
            0x2a,
            BluetoothDtmRole::Transmitter,
        )
        .expect("bounded reset fields are valid");
        let plan = BluetoothDtmReviewedEventWordsPlan::new_transmitter(
            reset,
            reservation(&mut timeline, BluetoothDtmRole::Transmitter),
        )
        .expect("both transforms encode TX");

        let packet = owner(0x2f07_0000).prepare_dtm_tx_packet(
            BluetoothDtmPayloadPattern::Repeated11110000,
            BluetoothDtmPayloadLength::from_hci_image(3),
        );
        assert_eq!(&packet.prepared_bytes()[0x12..], &[0x0f; 3]);

        let prepared = plan
            .prepare(packet)
            .expect("fresh private links replace both stale plan links");
        assert_eq!(prepared.role(), BluetoothDtmRole::Transmitter);
        assert_eq!(
            prepared.packet_pattern(),
            BluetoothDtmPayloadPattern::Repeated11110000
        );
        assert_eq!(prepared.packet_length().hci_image(), 3);
        let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
        assert_eq!(
            scheduler_prepared.packet_pattern(),
            BluetoothDtmPayloadPattern::Repeated11110000
        );
        assert_eq!(scheduler_prepared.packet_length().hci_image(), 3);
        assert_eq!(
            scheduler_prepared.hardware_list_index(),
            BluetoothSchedulerHardwareListIndex::ZERO
        );
        let prepared = scheduler_prepared.cancel();
        let words = prepared.words();
        assert_eq!(words.link_state().word_00, 0x8ff1_c057);
        assert_eq!(words.link_state().tx_head_link_image(), 0x1c057);
        assert_eq!(words.link_state().word_08, 0x0ff1_c04b);
        assert_eq!(words.scheduler_item().link_state_link_image(), 0x1c000);
        assert_eq!(words.scheduler_item().word_14, 0x5150_0000);
        assert_eq!(words.scheduler_item().word_2c, 0);
        let (_owner, reservation) = prepared.cancel();
        assert!(timeline.release(reservation).is_ok());
        assert!(timeline.is_empty());
    }

    #[test]
    fn rx_plan_applies_both_role_specific_transforms_to_the_bound_graph() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let reset = BluetoothDtmLinkStateReset::new(None, None, 0, 0, BluetoothDtmRole::Receiver)
            .expect("zero dynamic fields are valid");
        let plan = BluetoothDtmReviewedEventWordsPlan::new_receiver(
            reset,
            reservation(&mut timeline, BluetoothDtmRole::Receiver),
        )
        .expect("both transforms encode RX");

        let prepared = plan
            .prepare(BluetoothDtmReceiverCpuOwned::new(owner(0x2f00_0100)))
            .expect("current graph anchors satisfy the generated RX image");
        assert_eq!(prepared.role(), BluetoothDtmRole::Receiver);
        let words = prepared.words();
        assert_eq!(words.link_state().tx_head_link_image(), 0x097);
        assert_eq!(words.link_state().rx_tail_link_image(), 0x08b);
        assert_eq!(words.scheduler_item().link_state_link_image(), 0x040);
        assert_eq!(words.scheduler_item().word_14, 0xf000_0000);
        assert_eq!(words.scheduler_item().word_2c, 0x000f_0001);
        assert_eq!(words.scheduler_item().word_44, 103);
        assert_eq!(words.scheduler_item().word_48, 105);
        assert_eq!(words.scheduler_item().word_0c, 114);
        assert_eq!(words.scheduler_item().word_10, 2);
        assert_eq!(
            words.link_state().word_34,
            epoch().raw_time_for_scheduler_time(0)
        );
        let (_owner, reservation) = prepared.cancel();
        assert!(timeline.release(reservation).is_ok());
    }

    #[test]
    fn plan_forms_sequence_timing_from_the_overlap_resolved_window() {
        let mut timeline = BluetoothSchedulerTimeline::<2>::new();
        let occupied = timeline
            .reserve_dtm_event(
                item(BluetoothDtmRole::Receiver),
                epoch(),
                timing_policy(),
                admission_sample(),
            )
            .expect("the first window is admissible");
        let resolved = reservation(&mut timeline, BluetoothDtmRole::Receiver);
        assert_eq!(resolved.window().start(), occupied.window().end());

        let reset = BluetoothDtmLinkStateReset::new(None, None, 0, 0, BluetoothDtmRole::Receiver)
            .expect("zero dynamic fields are valid");
        let plan = BluetoothDtmReviewedEventWordsPlan::new_receiver(reset, resolved)
            .expect("the resolved event and reset share the RX role");
        let prepared = plan
            .prepare(BluetoothDtmReceiverCpuOwned::new(owner(0x2f00_1100)))
            .expect("the bound graph accepts the resolved event");
        let scheduler_words = prepared.words().scheduler_item();

        assert_eq!(scheduler_words.word_44, 105);
        assert_eq!(scheduler_words.word_48, 107);
        assert_eq!(scheduler_words.word_0c, 116);
        assert_eq!(scheduler_words.word_10, 2);

        let (_owner, resolved) = prepared.cancel();
        assert!(timeline.release(resolved).is_ok());
        assert!(timeline.release(occupied).is_ok());
        assert!(timeline.is_empty());
    }

    #[test]
    fn plan_rejects_mixed_roles_before_it_can_consume_memory() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let reset =
            BluetoothDtmLinkStateReset::new(None, None, 0, 0, BluetoothDtmRole::Transmitter)
                .expect("zero dynamic fields are valid");
        let failure = match BluetoothDtmReviewedEventWordsPlan::new_transmitter(
            reset,
            reservation(&mut timeline, BluetoothDtmRole::Receiver),
        ) {
            Ok(_) => panic!("a receiver reservation cannot form a transmitter plan"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothDtmReviewedEventWordsPlanError::RoleMismatch {
                expected: BluetoothDtmRole::Transmitter,
                link_state: BluetoothDtmRole::Transmitter,
                scheduler_item: BluetoothDtmRole::Receiver,
            }
        );
        assert!(timeline.release(failure.into_reservation()).is_ok());
    }

    #[test]
    fn sequence_authorization_rejects_the_second_guarded_deadline() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let reservation = timeline
            .reserve_dtm_event(
                item(BluetoothDtmRole::Receiver),
                epoch(),
                timing_policy(),
                admission_sample(),
            )
            .expect("the first guarded deadline is open");
        let failure = reservation
            .authorize_sequence(BluetoothControllerTimeSample::for_validation(93))
            .expect_err("the second sample reaches the guarded start");
        assert_eq!(
            failure.error(),
            BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired
        );
        assert!(timeline.release(failure.into_reservation()).is_ok());
    }
}
