//! Role-consistent composition of reviewed DTM event words into bound memory.
//!
//! This layer combines already validated LLL transforms with the lower
//! consuming memory transaction. The resulting state remains CPU-only and
//! retains TX packet readiness where that role requires it. It does not prove
//! list insertion, a visibility fence, a hardware latch or completion
//! ownership.

#![forbid(unsafe_code)]

use core::{convert::Infallible, marker::PhantomData};

use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphPositionalEventPrepared,
    BluetoothDtmMemoryGraphPrepareError, BluetoothDtmMemoryGraphPrepareFailure,
    BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared, BluetoothDtmPositionalEventWords,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerLockModifyRequest,
};

use crate::{
    BluetoothDtmLinkStateReset, BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern,
    BluetoothDtmPreparedTxGraph, BluetoothDtmRole, BluetoothSchedulerReservation,
    BluetoothSchedulerSequenceReady, dtm_scheduler_item::apply_overlap_insertion_power,
    scheduler_lock_modify::BluetoothSchedulerLockModifyPublicationResult,
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
            packet: BluetoothDtmEventPacket::Transmitter { pattern, length },
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

    /// Apply this RX plan to one exact ordinary CPU-owned graph.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc failure retains both the unchanged SRAM graph and affine reservation"
    )]
    pub fn prepare(
        self,
        owner: BluetoothDtmMemoryGraphCpuOwned,
    ) -> Result<
        BluetoothDtmReviewedEventWordsPrepared<BluetoothDtmReceiverEvent>,
        BluetoothDtmReviewedEventPrepareFailure<BluetoothDtmReceiverEvent>,
    > {
        let plan = self;
        let prepared = match owner
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(plan.apply_to_seed(seed)))
        {
            Ok(prepared) => prepared,
            Err(memory) => {
                return Err(BluetoothDtmReviewedEventPrepareFailure { memory, plan });
            }
        };

        Ok(BluetoothDtmReviewedEventWordsPrepared {
            memory: prepared,
            packet: BluetoothDtmEventPacket::Receiver,
            reservation: plan.reservation,
            _role: PhantomData,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothDtmEventPacket {
    Transmitter {
        pattern: BluetoothDtmPayloadPattern,
        length: BluetoothDtmPayloadLength,
    },
    Receiver,
}

/// CPU-owned bound graph containing one role-consistent reviewed word image.
///
/// The role marker preserves whether TX packet readiness was consumed into the
/// event. This type exposes no packet mutation or publication operation.
pub struct BluetoothDtmReviewedEventWordsPrepared<Role> {
    memory: BluetoothDtmMemoryGraphPositionalEventPrepared,
    packet: BluetoothDtmEventPacket,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

impl<Role> BluetoothDtmReviewedEventWordsPrepared<Role> {
    /// Return the role shared by both applied transforms.
    pub const fn role(&self) -> BluetoothDtmRole {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventPacket::Receiver => BluetoothDtmRole::Receiver,
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
            packet: self.packet,
            reservation: self.reservation,
            _role: PhantomData,
        }
    }

    /// Cancel before publication and recover both CPU-owned resources.
    pub fn cancel(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    ) {
        (self.memory.cancel(), self.reservation)
    }
}

impl BluetoothDtmReviewedEventWordsPrepared<BluetoothDtmTransmitterEvent> {
    /// Return the exact standard pattern retained from packet preparation.
    pub const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { pattern, .. } => pattern,
            BluetoothDtmEventPacket::Receiver => unreachable!(),
        }
    }

    /// Return the exact payload length retained from packet preparation.
    pub const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { length, .. } => length,
            BluetoothDtmEventPacket::Receiver => unreachable!(),
        }
    }
}

/// CPU-owned DTM graph after the reviewed scheduler bookkeeping prefix.
///
/// Forming its lock/modify request is still not hardware publication. The
/// complete descriptor, private packet-engine latch and visibility fence are
/// deliberately absent from this state.
#[must_use = "the scheduler-prepared DTM graph must remain owned or be cancelled"]
pub struct BluetoothDtmSchedulerBookkeepingPrepared<Role> {
    memory: BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared,
    packet: BluetoothDtmEventPacket,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

/// Why a completed lock/modify transaction cannot belong to a prepared DTM
/// graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmLockModifyJoinError {
    /// The transaction names a different scheduler-item allocation.
    SchedulerItemIdentityMismatch,
    /// The transaction names a different scheduler hardware list.
    HardwareListMismatch,
}

/// Rejected lock/modify join retaining both affine inputs.
#[must_use = "a rejected scheduler publication still owns the prepared graph and result"]
pub struct BluetoothDtmLockModifyJoinFailure<Role> {
    prepared: BluetoothDtmSchedulerBookkeepingPrepared<Role>,
    publication: BluetoothSchedulerLockModifyPublicationResult,
    error: BluetoothDtmLockModifyJoinError,
}

/// Outcome of joining one prepared DTM graph with a lock/modify result.
///
/// A dedicated enum keeps both large affine states inline without turning the
/// failure into an oversized `Result::Err` ABI or requiring allocation.
#[must_use = "the joined or rejected DTM scheduler state must remain owned"]
pub enum BluetoothDtmLockModifyJoin<Role> {
    /// The exact graph address and hardware-list identity matched.
    Joined(BluetoothDtmLockModifyCompleted<Role>),
    /// The graph and result remain separately recoverable after a mismatch.
    Rejected(BluetoothDtmLockModifyJoinFailure<Role>),
}

impl<Role> BluetoothDtmLockModifyJoinFailure<Role> {
    /// Exact identity mismatch that rejected the join.
    pub const fn error(&self) -> BluetoothDtmLockModifyJoinError {
        self.error
    }

    /// Recover both unchanged affine inputs.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmSchedulerBookkeepingPrepared<Role>,
        BluetoothSchedulerLockModifyPublicationResult,
    ) {
        (self.prepared, self.publication)
    }
}

impl<Role> core::fmt::Debug for BluetoothDtmLockModifyJoinFailure<Role> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmLockModifyJoinFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// CPU-owned DTM graph joined to its exact completed lock/modify transaction.
///
/// This state cannot be cancelled back to an unpublished graph because the
/// request has crossed the MMIO boundary. It still proves neither hardware-
/// list head publication nor descriptor visibility, RUN, hardware ownership,
/// radio completion or ownership return.
#[must_use = "the joined DTM scheduler transaction must advance through head publication"]
pub struct BluetoothDtmLockModifyCompleted<Role> {
    memory: BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared,
    packet: BluetoothDtmEventPacket,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    publication: BluetoothSchedulerLockModifyPublicationResult,
    _role: PhantomData<Role>,
}

impl<Role> BluetoothDtmLockModifyCompleted<Role> {
    /// DTM role retained by the joined graph.
    pub const fn role(&self) -> BluetoothDtmRole {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventPacket::Receiver => BluetoothDtmRole::Receiver,
        }
    }

    /// Source-owned scheduler window retained until completion or abort.
    pub const fn scheduler_window(&self) -> crate::BluetoothSchedulerRawWindow {
        self.reservation.window()
    }

    /// Scheduler-item address retained by both joined inputs.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    /// Hardware list retained by both joined inputs.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.publication.request().hardware_list_index()
    }

    /// Positional result of the completed request-publication transaction.
    ///
    /// This is diagnostic information, not a radio or descriptor status.
    pub const fn publication_result_code(&self) -> u8 {
        self.publication.code()
    }
}

impl<Role> BluetoothDtmSchedulerBookkeepingPrepared<Role> {
    /// Return the role shared by the retained DTM transforms.
    pub const fn role(&self) -> BluetoothDtmRole {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { .. } => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventPacket::Receiver => BluetoothDtmRole::Receiver,
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

    /// Form the reviewed list-zero DTM insertion request for this graph.
    ///
    /// The separately allocated 0x48-byte DTM scheduler context is zeroed and
    /// embeds its scheduler manager at +0x2c. The manager hardware-list index
    /// at context +0x44 is therefore zero. This method performs no MMIO and
    /// does not change CPU ownership.
    ///
    /// This remains crate-private until the lower memory layer represents the
    /// complete hardware-consumed descriptor and publication boundary. It is
    /// currently used only to retain the exact request identity in ownership
    /// tests; external controller code cannot submit an incomplete graph.
    #[allow(
        dead_code,
        reason = "publication stays closed until the complete hardware-consumed descriptor exists"
    )]
    pub(crate) const fn scheduler_lock_modify_request(
        &self,
    ) -> BluetoothSchedulerLockModifyRequest {
        BluetoothSchedulerLockModifyRequest::new(
            self.scheduler_item_address(),
            self.hardware_list_index(),
        )
    }

    /// Join the graph to the exact completed lock/modify request.
    ///
    /// A result for another address or hardware list is returned with this
    /// graph unchanged. Success deliberately exposes no cancellation or
    /// hardware-owned transition: head publication and the descriptor memory
    /// contract are still separate prerequisites.
    pub fn join_lock_modify_publication(
        self,
        publication: BluetoothSchedulerLockModifyPublicationResult,
    ) -> BluetoothDtmLockModifyJoin<Role> {
        let request = publication.request();
        let error = if request.address() != self.scheduler_item_address() {
            Some(BluetoothDtmLockModifyJoinError::SchedulerItemIdentityMismatch)
        } else if request.hardware_list_index() != self.hardware_list_index() {
            Some(BluetoothDtmLockModifyJoinError::HardwareListMismatch)
        } else {
            None
        };
        if let Some(error) = error {
            return BluetoothDtmLockModifyJoin::Rejected(BluetoothDtmLockModifyJoinFailure {
                prepared: self,
                publication,
                error,
            });
        }

        BluetoothDtmLockModifyJoin::Joined(BluetoothDtmLockModifyCompleted {
            memory: self.memory,
            packet: self.packet,
            reservation: self.reservation,
            publication,
            _role: PhantomData,
        })
    }

    /// Cancel before publication and recover the prepared event words.
    pub fn cancel(self) -> BluetoothDtmReviewedEventWordsPrepared<Role> {
        BluetoothDtmReviewedEventWordsPrepared {
            memory: self.memory.cancel(),
            packet: self.packet,
            reservation: self.reservation,
            _role: PhantomData,
        }
    }
}

impl BluetoothDtmSchedulerBookkeepingPrepared<BluetoothDtmTransmitterEvent> {
    /// Return the exact standard pattern retained through bookkeeping.
    pub const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { pattern, .. } => pattern,
            BluetoothDtmEventPacket::Receiver => unreachable!(),
        }
    }

    /// Return the exact payload length retained through bookkeeping.
    pub const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        match self.packet {
            BluetoothDtmEventPacket::Transmitter { length, .. } => length,
            BluetoothDtmEventPacket::Receiver => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDtmBoundSramLinkAddress, BluetoothDtmMemoryGraphModelAddress,
        BluetoothDtmMemoryGraphStorage, BluetoothDtmSchedulerAllocationConfig,
    };
    use open_esp_radio_esp32s31_hal::{
        BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex,
        BluetoothSchedulerLockModifyRequest,
    };
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::{BluetoothDtmReviewedEventWordsPlan, BluetoothDtmReviewedEventWordsPlanError};
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
        BluetoothDtmLinkStateReset, BluetoothDtmLockModifyJoinError, BluetoothDtmPayloadLength,
        BluetoothDtmPayloadPattern, BluetoothDtmPhy, BluetoothDtmRole,
        BluetoothDtmSchedulerItemEvent, BluetoothDtmSchedulerTimingPolicy,
        BluetoothDtmTxGraphPrepare, BluetoothSchedulerLockModifyPublicationResult,
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
        let request = scheduler_prepared.scheduler_lock_modify_request();
        assert_eq!(
            request.address(),
            scheduler_prepared.scheduler_item_address()
        );
        assert_eq!(
            request.hardware_list_index(),
            scheduler_prepared.hardware_list_index()
        );
        assert_eq!(
            request.hardware_list_index(),
            BluetoothSchedulerHardwareListIndex::ZERO
        );
        let wrong_address = BluetoothControllerSramAddress::new(0x2f00_0040)
            .expect("test address is representable");
        let wrong_request = BluetoothSchedulerLockModifyRequest::new(
            wrong_address,
            BluetoothSchedulerHardwareListIndex::ZERO,
        );
        let failure = match scheduler_prepared.join_lock_modify_publication(
            BluetoothSchedulerLockModifyPublicationResult::for_validation(0, wrong_request),
        ) {
            super::BluetoothDtmLockModifyJoin::Rejected(failure) => failure,
            super::BluetoothDtmLockModifyJoin::Joined(_) => {
                panic!("a result for another scheduler item consumed this graph")
            }
        };
        assert_eq!(
            failure.error(),
            BluetoothDtmLockModifyJoinError::SchedulerItemIdentityMismatch
        );
        let (scheduler_prepared, _wrong_publication) = failure.into_parts();
        let wrong_list = BluetoothSchedulerHardwareListIndex::new(1)
            .expect("list one is in the scheduler domain");
        let wrong_request = BluetoothSchedulerLockModifyRequest::new(request.address(), wrong_list);
        let failure = match scheduler_prepared.join_lock_modify_publication(
            BluetoothSchedulerLockModifyPublicationResult::for_validation(0, wrong_request),
        ) {
            super::BluetoothDtmLockModifyJoin::Rejected(failure) => failure,
            super::BluetoothDtmLockModifyJoin::Joined(_) => {
                panic!("a result for another hardware list consumed this graph")
            }
        };
        assert_eq!(
            failure.error(),
            BluetoothDtmLockModifyJoinError::HardwareListMismatch
        );
        let (scheduler_prepared, _wrong_publication) = failure.into_parts();
        let prepared = scheduler_prepared.cancel();
        let words = prepared.words();
        assert_eq!(words.link_state().word_00, 0x8ff1_c057);
        assert_eq!(words.link_state().tx_head_link_image(), 0x1c057);
        assert_eq!(words.link_state().word_08, 0x0ff1_c04b);
        assert_eq!(words.scheduler_item().link_state_link_image(), 0x1c000);
        assert_eq!(words.scheduler_item().word_14, 0x5150_0000);
        assert_eq!(words.scheduler_item().word_2c, 0);
        let (_owner, reservation) = prepared.cancel();
        assert!(timeline.release(reservation));
        assert!(timeline.is_empty());
    }

    #[test]
    fn completed_lock_modify_transaction_retains_the_exact_dtm_graph_identity() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let reset = BluetoothDtmLinkStateReset::new(None, None, 0, 0, BluetoothDtmRole::Receiver)
            .expect("zero dynamic fields are valid");
        let plan = BluetoothDtmReviewedEventWordsPlan::new_receiver(
            reset,
            reservation(&mut timeline, BluetoothDtmRole::Receiver),
        )
        .expect("both transforms encode RX");
        let scheduler_prepared = plan
            .prepare(owner(0x2f00_2100))
            .expect("current graph anchors satisfy the generated RX image")
            .prepare_scheduler_bookkeeping();
        let request = scheduler_prepared.scheduler_lock_modify_request();

        let completed = match scheduler_prepared.join_lock_modify_publication(
            BluetoothSchedulerLockModifyPublicationResult::for_validation(7, request),
        ) {
            super::BluetoothDtmLockModifyJoin::Joined(completed) => completed,
            super::BluetoothDtmLockModifyJoin::Rejected(_) => {
                panic!("the exact request did not rejoin its prepared graph")
            }
        };

        assert_eq!(completed.role(), BluetoothDtmRole::Receiver);
        assert_eq!(completed.scheduler_window().start(), 103);
        assert_eq!(completed.scheduler_item_address(), request.address());
        assert_eq!(
            completed.hardware_list_index(),
            BluetoothSchedulerHardwareListIndex::ZERO
        );
        assert_eq!(completed.publication_result_code(), 7);
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
            .prepare(owner(0x2f00_0100))
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
        assert!(timeline.release(reservation));
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
            .prepare(owner(0x2f00_1100))
            .expect("the bound graph accepts the resolved event");
        let scheduler_words = prepared.words().scheduler_item();

        assert_eq!(scheduler_words.word_44, 105);
        assert_eq!(scheduler_words.word_48, 107);
        assert_eq!(scheduler_words.word_0c, 116);
        assert_eq!(scheduler_words.word_10, 2);

        let (_owner, resolved) = prepared.cancel();
        assert!(timeline.release(resolved));
        assert!(timeline.release(occupied));
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
        assert!(timeline.release(failure.into_reservation()));
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
        assert!(timeline.release(failure.into_reservation()));
    }
}
