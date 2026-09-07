//! Role-consistent composition of reviewed DTM event words into bound memory.
//!
//! This layer combines already validated LLL transforms with the lower
//! consuming memory transaction. The resulting state remains CPU-only and
//! retains TX packet readiness where that role requires it. It does not prove
//! the remaining descriptor-consumption contract, list insertion, visibility
//! fences or completion ownership.

#![forbid(unsafe_code)]

use core::{convert::Infallible, marker::PhantomData};

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    DtmMemoryGraphCompletionObservation, DtmMemoryGraphCompletionObserved,
    DtmMemoryGraphRecycleCleaned, DtmMemoryGraphRecycleError, DtmMemoryGraphRxSuccessRecycleError,
};

use oer_esp32s31_bluetooth_memory::{
    DtmMemoryGraphCpuOwned, DtmMemoryGraphPositionalEventPrepared, DtmMemoryGraphPrepareError,
    DtmMemoryGraphPrepareFailure, DtmMemoryGraphReclaimed,
    DtmMemoryGraphSchedulerBookkeepingPrepared, DtmPositionalEventWords,
    DtmSchedulerItemCompletionStatus,
};
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_bluetooth_memory::{
    DtmMemoryGraphEmptyListLinkPrepared, DtmMemoryGraphHeadPublished, DtmMemoryGraphRunning,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareRunCommandPublished, BluetoothSchedulerSoftwareListRemovalReady,
};

use crate::{
    DtmLinkStateReset, DtmRxInitialEventWindow, DtmRxRecurringEventWindow, DtmSchedulerReservation,
    DtmTxEventWindow,
    le::dtm::{
        DtmChannel, DtmPayloadLength, DtmPayloadPattern, DtmPhy, DtmPreparedTxGraph, DtmRole,
        DtmTxSchedulerTiming, rx::DtmReceiverSession,
        scheduler::item::apply_overlap_insertion_power,
    },
    scheduler::{
        DtmInitialSchedulerItemPhase, DtmRecurringSchedulerItemPhase, SchedulerSequenceReady,
    },
};

use oer_esp32s31_hal::{
    bluetooth::BluetoothSchedulerHardwareListIndex, types::BluetoothControllerSramAddress,
};

/// Type marker for a transmitter event with a prepared packet prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmTransmitterEvent {}

/// Type marker for a receiver event without a transmitter packet prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmReceiverEvent {}

/// Immutable command identity retained across every transmitter event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DtmTransmitterCommandFacts {
    link_state: DtmLinkStateReset,
    channel: DtmChannel,
    phy: DtmPhy,
    timing: DtmTxSchedulerTiming,
    margin: u32,
    pattern: DtmPayloadPattern,
    length: DtmPayloadLength,
}

/// Immutable command identity retained across every receiver event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DtmReceiverCommandFacts {
    link_state: DtmLinkStateReset,
    channel: DtmChannel,
    phy: DtmPhy,
    margin: u32,
}

/// Exact receiver window most recently committed by the scheduler lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRxCommittedWindow {
    /// Initial-window identity committed by the first completed RX event.
    Initial(DtmRxInitialEventWindow),
    /// Recurring-window identity committed by a later completed RX event.
    Recurring(DtmRxRecurringEventWindow),
}

/// Semantic LE Test End result retained by the affine command owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub enum DtmTestEndReport {
    /// A transmitter test reports zero packets through HCI.
    Transmitter,
    /// A receiver test reports its accumulated accepted-packet count.
    Receiver { received_packets: u16 },
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmTestEndReport {
    /// Packet count serialized in the LE Test End Command Complete event.
    pub const fn reported_packet_count(self) -> u16 {
        match self {
            Self::Transmitter => 0,
            Self::Receiver { received_packets } => received_packets,
        }
    }
}

/// Terminal-neutral proof that an active DTM graph is fully reclaimed.
///
/// Construction is restricted to active TX/RX owners after scheduler removal,
/// unlink, timeline release and recycle. Command policy may turn this proof into
/// a Test End report or return it directly to the idle runtime for Reset.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the reclaimed graph must reach one terminal session owner"]
pub(crate) struct DtmQuiescedCpuOwned {
    memory: DtmMemoryGraphReclaimed,
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmQuiescedCpuOwned {
    pub(crate) fn into_reclaimed_graph(self) -> DtmMemoryGraphReclaimed {
        self.memory
    }

    #[cfg(test)]
    pub(crate) fn from_cpu_owned_for_test(memory: DtmMemoryGraphCpuOwned) -> Self {
        Self {
            memory: memory.into_reclaimed(),
        }
    }
}

/// Completed DTM command retaining its static graph until response handoff.
///
/// This state is constructible only from an active role after its event has
/// completed the hardware-head retirement, software-list removal, timeline
/// release and memory recycle chain. It therefore cannot end a prepared or
/// in-flight event. The graph remains pinned and unavailable for another test
/// until the response owner consumes this value.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the Test End result and reclaimed graph must reach the session owner"]
pub struct DtmTestEndedCpuOwned {
    quiesced: DtmQuiescedCpuOwned,
    report: DtmTestEndReport,
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmTestEndedCpuOwned {
    /// Borrow the role-specific Test End report without releasing the graph.
    pub const fn report(&self) -> DtmTestEndReport {
        self.report
    }

    /// Release the pinned graph to the idle session after response handoff.
    pub fn into_reclaimed_graph(self) -> DtmMemoryGraphReclaimed {
        self.quiesced.into_reclaimed_graph()
    }
}

/// Why two validated DTM transforms cannot describe one event plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtmReviewedEventWordsPlanError {
    /// Link-state and scheduler-item transforms encode different DTM roles.
    RoleMismatch {
        /// Role required by the selected constructor.
        expected: DtmRole,
        /// Role selected by the link-state reset.
        link_state: DtmRole,
        /// Role selected by the scheduler-item transform.
        scheduler_item: DtmRole,
    },
}

/// Rejected role composition retaining the exact sequence-ready reservation.
pub(crate) struct DtmReviewedEventWordsPlanFailure {
    error: DtmReviewedEventWordsPlanError,
    reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
}

impl DtmReviewedEventWordsPlanFailure {
    /// Borrow the finite composition failure reason.
    #[cfg(test)]
    pub const fn error(&self) -> DtmReviewedEventWordsPlanError {
        self.error
    }

    /// Recover the reservation for explicit scheduler release.
    pub fn into_reservation(self) -> DtmSchedulerReservation<SchedulerSequenceReady> {
        self.reservation
    }
}

impl core::fmt::Debug for DtmReviewedEventWordsPlanFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmReviewedEventWordsPlanFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Validated role-consistent plan for the nineteen reviewed event words.
///
/// Private chain links are deliberately absent from plan identity. They are
/// replaced inside `prepare` with fresh links sampled from the consumed graph.
/// Construction consumes an affine reservation that already passed its
/// phase-specific pre-sequence policy and fresh Controller-time sequence gate.
/// Initial insertion includes admission and bounded overlap displacement;
/// recurring insertion retains its exact collision-free window. Sequence timing
/// can therefore only be formed from the window retained by that reservation.
pub(crate) struct DtmReviewedEventWordsPlan<Role> {
    link_state: DtmLinkStateReset,
    reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

impl<Role> DtmReviewedEventWordsPlan<Role> {
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_reservation(self) -> DtmSchedulerReservation<SchedulerSequenceReady> {
        self.reservation
    }

    fn new_for_role(
        link_state: DtmLinkStateReset,
        reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
        expected: DtmRole,
    ) -> Result<Self, DtmReviewedEventWordsPlanFailure> {
        let link_role = link_state.role();
        let scheduler_role = reservation.event().role();
        if link_role != expected || scheduler_role != expected {
            return Err(DtmReviewedEventWordsPlanFailure {
                error: DtmReviewedEventWordsPlanError::RoleMismatch {
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
        seed: oer_esp32s31_bluetooth_memory::DtmPositionalEventSeed,
    ) -> DtmPositionalEventWords {
        let current = seed.words();
        let event = self.reservation.event();
        let epoch = self.reservation.epoch();
        let retained_window = self.reservation.window();
        let link_state = self
            .link_state
            .with_private_links(
                seed.tx_header_head_projection(),
                seed.rx_header_tail_projection(),
            )
            .apply(current.link_state())
            .apply_event_context(self.link_state.role(), epoch.raw_ticks_for_micros(0));
        let scheduler_item = event.apply_raw_window(
            current.scheduler_item(),
            retained_window.start(),
            retained_window.end(),
        );
        let scheduler_item = apply_overlap_insertion_power(scheduler_item, link_state)
            .apply_sequence_timing(self.reservation.timing_policy().sequence_lead_raw_delta());
        DtmPositionalEventWords::new(link_state, scheduler_item)
    }
}

/// Failed graph preparation retaining the sequence-ready scheduler plan.
pub(crate) struct DtmReviewedEventPrepareFailure<Role> {
    memory: DtmMemoryGraphPrepareFailure,
    _plan: DtmReviewedEventWordsPlan<Role>,
    _pattern: DtmPayloadPattern,
    _length: DtmPayloadLength,
}

#[cfg(target_arch = "riscv32")]
impl DtmReviewedEventPrepareFailure<DtmTransmitterEvent> {
    /// Recover the byte-unchanged graph, complete TX program and reusable plan.
    ///
    /// Keeping the pattern and length here is required for a failed admission
    /// to rebuild the consumed packet-readiness proof without losing the DTM
    /// command that produced it.
    pub fn into_retry(
        self,
    ) -> (
        DtmMemoryGraphCpuOwned,
        DtmMemoryGraphPrepareError,
        DtmPayloadPattern,
        DtmPayloadLength,
        DtmReviewedEventWordsPlan<DtmTransmitterEvent>,
    ) {
        let (memory, error) = self.memory.into_parts();
        (memory, error, self._pattern, self._length, self._plan)
    }
}

impl<Role> core::fmt::Debug for DtmReviewedEventPrepareFailure<Role> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmReviewedEventPrepareFailure")
            .field("error", self.memory.error())
            .finish_non_exhaustive()
    }
}

impl DtmReviewedEventWordsPlan<DtmTransmitterEvent> {
    /// Pair a transmitter reset with its sequence-ready scheduler reservation.
    pub(crate) fn new_transmitter(
        link_state: DtmLinkStateReset,
        reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
    ) -> Result<Self, DtmReviewedEventWordsPlanFailure> {
        Self::new_for_role(link_state, reservation, DtmRole::Transmitter)
    }

    /// Apply this TX plan only to a graph carrying a complete standard packet.
    ///
    /// Any lower validation failure returns an ordinary CPU owner. A retry
    /// must deliberately prepare a fresh packet-readiness proof.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc failure retains both the unchanged SRAM graph and affine reservation"
    )]
    pub(crate) fn prepare_first(
        self,
        owner: DtmPreparedTxGraph,
        channel: DtmChannel,
        phy: DtmPhy,
        timing: DtmTxSchedulerTiming,
        margin: u32,
        window: DtmTxEventWindow,
    ) -> Result<
        DtmReviewedEventWordsPrepared<DtmTransmitterEvent, DtmInitialSchedulerItemPhase>,
        DtmReviewedEventPrepareFailure<DtmTransmitterEvent>,
    > {
        let plan = self;
        let (memory, pattern, length) = owner.into_parts();
        let facts = DtmTransmitterCommandFacts {
            link_state: plan.link_state,
            channel,
            phy,
            timing,
            margin,
            pattern,
            length,
        };
        let prepared = match memory
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(plan.apply_to_seed(seed)))
        {
            Ok(prepared) => prepared,
            Err(memory) => {
                return Err(DtmReviewedEventPrepareFailure {
                    memory,
                    _plan: plan,
                    _pattern: pattern,
                    _length: length,
                });
            }
        };

        Ok(DtmReviewedEventWordsPrepared {
            memory: prepared,
            context: DtmEventContext::Transmitter(DtmTransmitterEventContext {
                facts,
                event_window: window,
            }),
            rollback: (),
            reservation: plan.reservation,
            _state: PhantomData,
        })
    }

    /// Apply one recurring TX plan without rebuilding packet readiness or
    /// crossing the first-event admission edge.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc failure retains the active graph, command facts and reservation"
    )]
    pub(crate) fn prepare_recurring(
        self,
        owner: DtmActiveTransmitterCpuOwned,
        window: DtmTxEventWindow,
    ) -> Result<
        DtmReviewedEventWordsPrepared<DtmTransmitterEvent, DtmRecurringSchedulerItemPhase>,
        DtmRecurringTransmitterEventPrepareFailure,
    > {
        let plan = self;
        let DtmActiveTransmitterCpuOwned {
            memory,
            facts,
            last_committed_window,
            status,
        } = owner;
        let prepared = match memory
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(plan.apply_to_seed(seed)))
        {
            Ok(prepared) => prepared,
            Err(memory) => {
                return Err(DtmRecurringTransmitterEventPrepareFailure {
                    memory,
                    plan,
                    facts,
                    last_committed_window,
                    status,
                });
            }
        };

        Ok(DtmReviewedEventWordsPrepared {
            memory: prepared,
            context: DtmEventContext::Transmitter(DtmTransmitterEventContext {
                facts,
                event_window: window,
            }),
            rollback: (last_committed_window, status),
            reservation: plan.reservation,
            _state: PhantomData,
        })
    }
}

impl DtmReviewedEventWordsPlan<DtmReceiverEvent> {
    /// Pair a receiver reset with its sequence-ready scheduler reservation.
    pub(crate) fn new_receiver(
        link_state: DtmLinkStateReset,
        reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
    ) -> Result<Self, DtmReviewedEventWordsPlanFailure> {
        Self::new_for_role(link_state, reservation, DtmRole::Receiver)
    }

    /// Apply this RX plan to one exact graph/session aggregate.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc failure retains both the unchanged SRAM graph and affine reservation"
    )]
    pub(crate) fn prepare_first(
        self,
        owner: DtmReceiverCpuOwned,
        channel: DtmChannel,
        phy: DtmPhy,
        margin: u32,
        window: DtmRxInitialEventWindow,
    ) -> Result<
        DtmReviewedEventWordsPrepared<DtmReceiverEvent, DtmInitialSchedulerItemPhase>,
        DtmReceiverEventPrepareFailure,
    > {
        let plan = self;
        let DtmReceiverCpuOwned { memory, session } = owner;
        let facts = DtmReceiverCommandFacts {
            link_state: plan.link_state,
            channel,
            phy,
            margin,
        };
        let prepared = match memory
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(plan.apply_to_seed(seed)))
        {
            Ok(prepared) => prepared,
            Err(memory) => {
                return Err(DtmReceiverEventPrepareFailure {
                    memory,
                    _plan: plan,
                    _session: session,
                });
            }
        };

        Ok(DtmReviewedEventWordsPrepared {
            memory: prepared,
            context: DtmEventContext::Receiver(DtmReceiverEventContext {
                facts,
                session,
                event_window: DtmRxCommittedWindow::Initial(window),
            }),
            rollback: (),
            reservation: plan.reservation,
            _state: PhantomData,
        })
    }

    /// Apply one recurring RX plan without crossing the initial descriptor
    /// path or detaching the accumulated Test End count from its graph.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc failure retains the active graph, command facts and reservation"
    )]
    pub(crate) fn prepare_recurring(
        self,
        owner: DtmActiveReceiverCpuOwned,
        window: DtmRxRecurringEventWindow,
    ) -> Result<
        DtmReviewedEventWordsPrepared<DtmReceiverEvent, DtmRecurringSchedulerItemPhase>,
        DtmRecurringReceiverEventPrepareFailure,
    > {
        let plan = self;
        let DtmActiveReceiverCpuOwned {
            memory,
            facts,
            session,
            last_committed_window,
        } = owner;
        let prepared = match memory
            .try_prepare_positional_event(|seed| Ok::<_, Infallible>(plan.apply_to_seed(seed)))
        {
            Ok(prepared) => prepared,
            Err(memory) => {
                return Err(DtmRecurringReceiverEventPrepareFailure {
                    memory,
                    plan,
                    facts,
                    session,
                    last_committed_window,
                });
            }
        };

        Ok(DtmReviewedEventWordsPrepared {
            memory: prepared,
            context: DtmEventContext::Receiver(DtmReceiverEventContext {
                facts,
                session,
                event_window: DtmRxCommittedWindow::Recurring(window),
            }),
            rollback: last_committed_window,
            reservation: plan.reservation,
            _state: PhantomData,
        })
    }
}

/// Fresh CPU-owned receiver graph before the first DTM event.
///
/// Recycled events return [`DtmActiveReceiverCpuOwned`] instead, so
/// an active session cannot re-enter the initial descriptor path.
#[must_use = "the fresh receiver graph and its test state must stay together"]
pub struct DtmReceiverCpuOwned {
    memory: DtmMemoryGraphCpuOwned,
    session: DtmReceiverSession,
}

/// CPU-owned receiver graph belonging to an already active DTM session.
///
/// This type deliberately has no conversion back to
/// [`DtmReceiverCpuOwned`]. The immutable command and last committed
/// window travel with the graph, so only the recurring Controller operation or
/// a proven Test End path can consume it.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the active receiver graph must recur or enter a proven Test End path"]
pub struct DtmActiveReceiverCpuOwned {
    memory: DtmMemoryGraphCpuOwned,
    facts: DtmReceiverCommandFacts,
    session: DtmReceiverSession,
    last_committed_window: DtmRxCommittedWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmActiveReceiverCpuOwned {
    pub(crate) const fn link_state(&self) -> DtmLinkStateReset {
        self.facts.link_state
    }

    pub(crate) const fn channel(&self) -> DtmChannel {
        self.facts.channel
    }

    pub(crate) const fn phy(&self) -> DtmPhy {
        self.facts.phy
    }

    #[cfg(test)]
    pub(crate) const fn margin(&self) -> u32 {
        self.facts.margin
    }

    /// Current received-packet count retained for LE Test End.
    pub const fn received_packet_count(&self) -> u16 {
        self.session.received_packet_count()
    }

    /// Finish this receiver test at its fully recycled CPU-owned boundary.
    ///
    /// A Test End request received earlier must remain pending until the
    /// in-flight owner reaches this type. The returned value retains the graph
    /// while the caller stages the Command Complete response.
    pub fn into_test_ended(self) -> DtmTestEndedCpuOwned {
        let report = DtmTestEndReport::Receiver {
            received_packets: self.session.received_packet_count(),
        };
        DtmTestEndedCpuOwned {
            quiesced: DtmQuiescedCpuOwned {
                memory: self.memory.into_reclaimed(),
            },
            report,
        }
    }

    /// End active hardware ownership without attaching HCI terminal policy.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_quiesced(self) -> DtmQuiescedCpuOwned {
        DtmQuiescedCpuOwned {
            memory: self.memory.into_reclaimed(),
        }
    }
}

impl DtmReceiverCpuOwned {
    /// Start one fresh receiver session on an ordinary CPU-owned graph.
    pub fn new(memory: DtmMemoryGraphCpuOwned) -> Self {
        Self {
            memory,
            session: DtmReceiverSession::new(),
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
    pub fn into_memory_and_packet_count(self) -> (DtmMemoryGraphCpuOwned, u16) {
        (self.memory, self.session.received_packet_count())
    }
}

/// Failed RX graph preparation retaining the exact session and plan.
#[must_use = "RX preparation failure retains the graph, session and reservation plan"]
pub(crate) struct DtmReceiverEventPrepareFailure {
    memory: DtmMemoryGraphPrepareFailure,
    _plan: DtmReviewedEventWordsPlan<DtmReceiverEvent>,
    _session: DtmReceiverSession,
}

#[cfg(target_arch = "riscv32")]
impl DtmReceiverEventPrepareFailure {
    /// Recover the unchanged aggregate, error and reservation plan for retry.
    pub fn into_retry(
        self,
    ) -> (
        DtmReceiverCpuOwned,
        DtmMemoryGraphPrepareError,
        DtmReviewedEventWordsPlan<DtmReceiverEvent>,
    ) {
        let (memory, error) = self.memory.into_parts();
        (
            DtmReceiverCpuOwned {
                memory,
                session: self._session,
            },
            error,
            self._plan,
        )
    }
}

impl core::fmt::Debug for DtmReceiverEventPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmReceiverEventPrepareFailure")
            .field("error", self.memory.error())
            .finish_non_exhaustive()
    }
}

/// Failed recurring TX graph preparation retaining the complete active owner.
#[cfg(any(target_arch = "riscv32", test))]
#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "target-only scheduler consumes recurring recovery on production builds"
    )
)]
#[must_use = "recurring TX failure retains the active graph and reservation plan"]
pub(crate) struct DtmRecurringTransmitterEventPrepareFailure {
    memory: DtmMemoryGraphPrepareFailure,
    plan: DtmReviewedEventWordsPlan<DtmTransmitterEvent>,
    facts: DtmTransmitterCommandFacts,
    last_committed_window: DtmTxEventWindow,
    status: DtmSchedulerItemCompletionStatus,
}

#[cfg(any(target_arch = "riscv32", test))]
#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "target-only scheduler consumes recurring recovery on production builds"
    )
)]
impl DtmRecurringTransmitterEventPrepareFailure {
    pub(crate) fn into_retry(
        self,
    ) -> (
        DtmActiveTransmitterCpuOwned,
        DtmMemoryGraphPrepareError,
        DtmReviewedEventWordsPlan<DtmTransmitterEvent>,
    ) {
        let (memory, error) = self.memory.into_parts();
        (
            DtmActiveTransmitterCpuOwned {
                memory,
                facts: self.facts,
                last_committed_window: self.last_committed_window,
                status: self.status,
            },
            error,
            self.plan,
        )
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl core::fmt::Debug for DtmRecurringTransmitterEventPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmRecurringTransmitterEventPrepareFailure")
            .field("error", self.memory.error())
            .finish_non_exhaustive()
    }
}

/// Failed recurring RX graph preparation retaining the complete active owner.
#[cfg(any(target_arch = "riscv32", test))]
#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "target-only scheduler consumes recurring recovery on production builds"
    )
)]
#[must_use = "recurring RX failure retains the active graph and reservation plan"]
pub(crate) struct DtmRecurringReceiverEventPrepareFailure {
    memory: DtmMemoryGraphPrepareFailure,
    plan: DtmReviewedEventWordsPlan<DtmReceiverEvent>,
    facts: DtmReceiverCommandFacts,
    session: DtmReceiverSession,
    last_committed_window: DtmRxCommittedWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "target-only scheduler consumes recurring recovery on production builds"
    )
)]
impl DtmRecurringReceiverEventPrepareFailure {
    pub(crate) fn into_retry(
        self,
    ) -> (
        DtmActiveReceiverCpuOwned,
        DtmMemoryGraphPrepareError,
        DtmReviewedEventWordsPlan<DtmReceiverEvent>,
    ) {
        let (memory, error) = self.memory.into_parts();
        (
            DtmActiveReceiverCpuOwned {
                memory,
                facts: self.facts,
                session: self.session,
                last_committed_window: self.last_committed_window,
            },
            error,
            self.plan,
        )
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl core::fmt::Debug for DtmRecurringReceiverEventPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmRecurringReceiverEventPrepareFailure")
            .field("error", self.memory.error())
            .finish_non_exhaustive()
    }
}

struct DtmTransmitterEventContext {
    facts: DtmTransmitterCommandFacts,
    event_window: DtmTxEventWindow,
}

struct DtmReceiverEventContext {
    facts: DtmReceiverCommandFacts,
    session: DtmReceiverSession,
    event_window: DtmRxCommittedWindow,
}

enum DtmEventContext {
    Transmitter(DtmTransmitterEventContext),
    Receiver(DtmReceiverEventContext),
}

mod phase_sealed {
    pub trait Sealed<Role> {}
}

impl phase_sealed::Sealed<DtmTransmitterEvent> for DtmInitialSchedulerItemPhase {}

impl phase_sealed::Sealed<DtmTransmitterEvent> for DtmRecurringSchedulerItemPhase {}

impl phase_sealed::Sealed<DtmReceiverEvent> for DtmInitialSchedulerItemPhase {}

impl phase_sealed::Sealed<DtmReceiverEvent> for DtmRecurringSchedulerItemPhase {}

/// Valid relation between a DTM event role and its preparation phase.
///
/// Initial preparation has no prior active owner. Recurring preparation must
/// retain the committed window (and TX completion status) until publication,
/// so cancellation can reconstruct that owner without inspecting runtime
/// phase data. This trait is sealed: only the four supported TX/RX and
/// initial/recurring relations can be formed.
pub trait DtmSchedulerItemPhase<Role>: phase_sealed::Sealed<Role> {
    #[doc(hidden)]
    type Rollback;
}

impl DtmSchedulerItemPhase<DtmTransmitterEvent> for DtmInitialSchedulerItemPhase {
    type Rollback = ();
}

impl DtmSchedulerItemPhase<DtmTransmitterEvent> for DtmRecurringSchedulerItemPhase {
    type Rollback = (DtmTxEventWindow, DtmSchedulerItemCompletionStatus);
}

impl DtmSchedulerItemPhase<DtmReceiverEvent> for DtmInitialSchedulerItemPhase {
    type Rollback = ();
}

impl DtmSchedulerItemPhase<DtmReceiverEvent> for DtmRecurringSchedulerItemPhase {
    type Rollback = DtmRxCommittedWindow;
}

/// CPU-owned bound graph containing one role-consistent reviewed word image.
///
/// The role marker preserves whether TX packet readiness was consumed into the
/// event. This type exposes no packet mutation or publication operation.
pub(crate) struct DtmReviewedEventWordsPrepared<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    memory: DtmMemoryGraphPositionalEventPrepared,
    context: DtmEventContext,
    rollback: Phase::Rollback,
    reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
    _state: PhantomData<(Role, Phase)>,
}

impl<Role, Phase> DtmReviewedEventWordsPrepared<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    /// Install the common scheduler bookkeeping prefix for this exact graph.
    ///
    /// The resulting state remains CPU-owned and cancellable. Only that later
    /// state may form a scheduler request; this prevents event words without
    /// the in-flight sentinel and cleared completion link from being admitted.
    pub fn prepare_scheduler_bookkeeping(self) -> DtmSchedulerBookkeepingPrepared<Role, Phase> {
        DtmSchedulerBookkeepingPrepared {
            memory: self.memory.prepare_scheduler_bookkeeping(),
            context: self.context,
            rollback: self.rollback,
            reservation: self.reservation,
            _state: PhantomData,
        }
    }
}

impl DtmReviewedEventWordsPrepared<DtmTransmitterEvent, DtmInitialSchedulerItemPhase> {
    /// Cancel a first event before publication and recover ordinary ownership.
    pub(crate) fn cancel_first(
        self,
    ) -> (
        DtmMemoryGraphCpuOwned,
        DtmSchedulerReservation<SchedulerSequenceReady>,
    ) {
        let DtmEventContext::Transmitter(_) = self.context else {
            unreachable!()
        };
        (self.memory.cancel(), self.reservation)
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmReviewedEventWordsPrepared<DtmTransmitterEvent, DtmRecurringSchedulerItemPhase> {
    /// Cancel a recurring event and reconstruct the exact prior active owner.
    pub(crate) fn cancel_recurring(
        self,
    ) -> (
        DtmActiveTransmitterCpuOwned,
        DtmSchedulerReservation<SchedulerSequenceReady>,
    ) {
        let DtmEventContext::Transmitter(context) = self.context else {
            unreachable!()
        };
        let (last_committed_window, status) = self.rollback;
        (
            DtmActiveTransmitterCpuOwned {
                memory: self.memory.cancel(),
                facts: context.facts,
                last_committed_window,
                status,
            },
            self.reservation,
        )
    }
}

impl DtmReviewedEventWordsPrepared<DtmReceiverEvent, DtmInitialSchedulerItemPhase> {
    /// Cancel a first event without detaching the RX session from memory.
    pub(crate) fn cancel_first(
        self,
    ) -> (
        DtmReceiverCpuOwned,
        DtmSchedulerReservation<SchedulerSequenceReady>,
    ) {
        let DtmEventContext::Receiver(context) = self.context else {
            unreachable!()
        };
        (
            DtmReceiverCpuOwned {
                memory: self.memory.cancel(),
                session: context.session,
            },
            self.reservation,
        )
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmReviewedEventWordsPrepared<DtmReceiverEvent, DtmRecurringSchedulerItemPhase> {
    /// Cancel a recurring event and reconstruct the exact prior active owner.
    pub(crate) fn cancel_recurring(
        self,
    ) -> (
        DtmActiveReceiverCpuOwned,
        DtmSchedulerReservation<SchedulerSequenceReady>,
    ) {
        let DtmEventContext::Receiver(context) = self.context else {
            unreachable!()
        };
        (
            DtmActiveReceiverCpuOwned {
                memory: self.memory.cancel(),
                facts: context.facts,
                session: context.session,
                last_committed_window: self.rollback,
            },
            self.reservation,
        )
    }
}

/// CPU-owned DTM graph after the reviewed scheduler bookkeeping prefix.
///
/// The remaining descriptor-consumption contract, common-scheduler
/// insertion/merge transaction and visibility fence are deliberately absent
/// from this state.
#[must_use = "the scheduler-prepared DTM graph must remain owned or be cancelled"]
pub(crate) struct DtmSchedulerBookkeepingPrepared<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    memory: DtmMemoryGraphSchedulerBookkeepingPrepared,
    context: DtmEventContext,
    rollback: Phase::Rollback,
    reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
    _state: PhantomData<(Role, Phase)>,
}

impl<Role, Phase> DtmSchedulerBookkeepingPrepared<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    /// Return the typed controller-SRAM identity of the retained item.
    #[cfg(target_arch = "riscv32")]
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn prepare_empty_list_link(self) -> DtmEmptyListLinkPrepared<Role, Phase> {
        DtmEmptyListLinkPrepared {
            memory: self.memory.prepare_empty_list_link(),
            context: self.context,
            rollback: self.rollback,
            _reservation: self.reservation,
            _state: PhantomData,
        }
    }

    /// Cancel before publication and recover the prepared event words.
    pub fn cancel(self) -> DtmReviewedEventWordsPrepared<Role, Phase> {
        DtmReviewedEventWordsPrepared {
            memory: self.memory.cancel(),
            context: self.context,
            rollback: self.rollback,
            reservation: self.reservation,
            _state: PhantomData,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<Phase> DtmSchedulerBookkeepingPrepared<DtmTransmitterEvent, Phase>
where
    Phase: DtmSchedulerItemPhase<DtmTransmitterEvent>,
{
    /// Return the exact standard pattern retained from packet preparation.
    pub(crate) const fn packet_pattern(&self) -> DtmPayloadPattern {
        match &self.context {
            DtmEventContext::Transmitter(context) => context.facts.pattern,
            DtmEventContext::Receiver(_) => unreachable!(),
        }
    }

    /// Return the exact payload length retained from packet preparation.
    pub(crate) const fn packet_length(&self) -> DtmPayloadLength {
        match &self.context {
            DtmEventContext::Transmitter(context) => context.facts.length,
            DtmEventContext::Receiver(_) => unreachable!(),
        }
    }
}

/// Internal join candidate after the item-side empty-list transform.
///
/// Only the scheduler module can combine this memory owner with its affine
/// exclusive empty-list epoch. Keeping this type crate-private prevents a
/// memory-only transition from being mistaken for list ownership.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct DtmEmptyListLinkPrepared<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    memory: DtmMemoryGraphEmptyListLinkPrepared,
    context: DtmEventContext,
    rollback: Phase::Rollback,
    _reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
    _state: PhantomData<(Role, Phase)>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Role, Phase> DtmEmptyListLinkPrepared<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    pub(crate) const fn role(&self) -> DtmRole {
        match &self.context {
            DtmEventContext::Transmitter(_) => DtmRole::Transmitter,
            DtmEventContext::Receiver(_) => DtmRole::Receiver,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        BluetoothSchedulerHardwareListIndex::ZERO
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_head_published(
        self,
        publication: &BluetoothSchedulerHardwareListHeadPublished,
    ) -> DtmHeadPublishedEvent<Role> {
        // The publication proof is the sole edge that may discard the
        // pre-publication rollback phase. Head-published events can no longer
        // cancel back into either the fresh or active CPU owner.
        let Self {
            memory,
            context,
            rollback: _,
            _reservation,
            _state: _,
        } = self;
        DtmHeadPublishedEvent {
            memory: memory.into_head_published(publication),
            context,
            _reservation,
            _role: PhantomData,
        }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel(self) -> DtmSchedulerBookkeepingPrepared<Role, Phase> {
        DtmSchedulerBookkeepingPrepared {
            memory: self.memory.cancel(),
            context: self.context,
            rollback: self.rollback,
            reservation: self._reservation,
            _state: PhantomData,
        }
    }
}

/// Internal DTM event whose pinned graph has crossed the hardware-head edge.
///
/// Only the scheduler lifecycle can create this owner by pairing the prepared
/// event with its exact affine PAC publication. It intentionally has no
/// cancellation path or mutable access to controller-owned storage.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct DtmHeadPublishedEvent<Role> {
    memory: DtmMemoryGraphHeadPublished,
    context: DtmEventContext,
    _reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Role> DtmHeadPublishedEvent<Role> {
    pub(crate) const fn role(&self) -> DtmRole {
        match &self.context {
            DtmEventContext::Transmitter(_) => DtmRole::Transmitter,
            DtmEventContext::Receiver(_) => DtmRole::Receiver,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_running(
        self,
        run: &BluetoothSchedulerHardwareRunCommandPublished,
    ) -> DtmRunningEvent<Role> {
        let Self {
            memory,
            context,
            _reservation,
            _role: _,
        } = self;
        DtmRunningEvent {
            memory: memory.into_running(run),
            context,
            _reservation,
            _role: PhantomData,
        }
    }
}

/// Internal DTM event admitted through the complete scheduler RUN suffix.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct DtmRunningEvent<Role> {
    memory: DtmMemoryGraphRunning,
    context: DtmEventContext,
    _reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Role> DtmRunningEvent<Role> {
    pub(crate) const fn role(&self) -> DtmRole {
        match &self.context {
            DtmEventContext::Transmitter(_) => DtmRole::Transmitter,
            DtmEventContext::Receiver(_) => DtmRole::Receiver,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> DtmRunningEventCompletionObservation<Role> {
        let Self {
            memory,
            context,
            _reservation: reservation,
            _role: _,
        } = self;
        match memory.observe_completion(observed) {
            DtmMemoryGraphCompletionObservation::ListMismatch { owner, observed } => {
                DtmRunningEventCompletionObservation::ListMismatch {
                    item: Self {
                        memory: owner,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                    observed,
                }
            }
            DtmMemoryGraphCompletionObservation::StillInFlight(memory) => {
                DtmRunningEventCompletionObservation::StillInFlight(Self {
                    memory,
                    context,
                    _reservation: reservation,
                    _role: PhantomData,
                })
            }
            DtmMemoryGraphCompletionObservation::CompletionObserved(memory) => {
                DtmRunningEventCompletionObservation::CompletionObserved(
                    DtmCompletionObservedEvent {
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
pub(crate) enum DtmRunningEventCompletionObservation<Role> {
    ListMismatch {
        item: DtmRunningEvent<Role>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(DtmRunningEvent<Role>),
    CompletionObserved(DtmCompletionObservedEvent<Role>),
}

/// Internal event retaining every owner after a non-sentinel status read.
#[cfg(target_arch = "riscv32")]
pub(crate) struct DtmCompletionObservedEvent<Role> {
    memory: DtmMemoryGraphCompletionObserved,
    context: DtmEventContext,
    _reservation: DtmSchedulerReservation<SchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

#[cfg(target_arch = "riscv32")]
impl<Role> DtmCompletionObservedEvent<Role> {
    pub(crate) const fn role(&self) -> DtmRole {
        match &self.context {
            DtmEventContext::Transmitter(_) => DtmRole::Transmitter,
            DtmEventContext::Receiver(_) => DtmRole::Receiver,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) const fn status(&self) -> DtmSchedulerItemCompletionStatus {
        self.memory.status()
    }

    #[expect(
        clippy::result_large_err,
        reason = "lossless recycle failure retains the affine graph and removal owner"
    )]
    pub(crate) fn recycle<const CAPACITY: usize>(
        self,
        timeline: &mut crate::scheduler::timeline::SchedulerTimeline<CAPACITY>,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<DtmRecycleTimelineReleasedEvent<Role>, DtmCompletionRecycleFailure<Role>> {
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
                return Err(DtmCompletionRecycleFailure {
                    error: DtmCompletionRecycleError::MemoryIdentity(error),
                    item: DtmCompletionObservedEvent {
                        memory,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                    removal,
                });
            }
        };
        let (window, event, epoch) = reservation.into_parts();
        let release = match timeline.prepare_release(window) {
            Ok(release) => release,
            Err(failure) => {
                let reservation =
                    DtmSchedulerReservation::new(failure.into_reservation(), event, epoch);
                let (memory, removal) = prepared.into_parts();
                return Err(DtmCompletionRecycleFailure {
                    error: DtmCompletionRecycleError::ReservationIdentityMismatch,
                    item: DtmCompletionObservedEvent {
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
        Ok(DtmRecycleTimelineReleasedEvent {
            memory,
            context,
            _role: PhantomData,
        })
    }
}

#[cfg(target_arch = "riscv32")]
impl DtmCompletionObservedEvent<DtmReceiverEvent> {
    #[expect(
        clippy::result_large_err,
        reason = "lossless RX recycle failure retains the affine graph, session and removal owner"
    )]
    pub(crate) fn recycle_receiver_success<const CAPACITY: usize>(
        self,
        timeline: &mut crate::scheduler::timeline::SchedulerTimeline<CAPACITY>,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<
        DtmRxSuccessRecycleTimelineReleasedEvent,
        DtmCompletionRecycleFailure<DtmReceiverEvent>,
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
                return Err(DtmCompletionRecycleFailure {
                    error: DtmCompletionRecycleError::MemoryIdentity(error),
                    item: DtmCompletionObservedEvent {
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
                return Err(DtmCompletionRecycleFailure {
                    error: DtmCompletionRecycleError::ReceiverSuccessMemory(error),
                    item: DtmCompletionObservedEvent {
                        memory,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                    removal,
                });
            }
        };
        let (window, event, epoch) = reservation.into_parts();
        let release = match timeline.prepare_release(window) {
            Ok(release) => release,
            Err(failure) => {
                let reservation =
                    DtmSchedulerReservation::new(failure.into_reservation(), event, epoch);
                let (memory, removal) = rx_prepared.into_recycle_prepared().into_parts();
                return Err(DtmCompletionRecycleFailure {
                    error: DtmCompletionRecycleError::ReservationIdentityMismatch,
                    item: DtmCompletionObservedEvent {
                        memory,
                        context,
                        _reservation: reservation,
                        _role: PhantomData,
                    },
                    removal,
                });
            }
        };
        let DtmEventContext::Receiver(mut context) = context else {
            unreachable!()
        };
        let (memory, outcome) = rx_prepared.observe().consume_then_commit(|projection| {
            projection.map_or(
                crate::le::dtm::DtmRxCompletionOutcome::NoReturnedPacket,
                |result| context.session.account_projection(result),
            )
        });
        release.commit();
        Ok(DtmRxSuccessRecycleTimelineReleasedEvent {
            memory,
            facts: context.facts,
            session: context.session,
            last_committed_window: context.event_window,
            outcome,
        })
    }
}

/// Internal reason the complete DTM recycle transaction rejected ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub(crate) enum DtmCompletionRecycleError {
    MemoryIdentity(DtmMemoryGraphRecycleError),
    ReceiverSuccessMemory(DtmMemoryGraphRxSuccessRecycleError),
    ReservationIdentityMismatch,
}

/// Lossless rejection before either memory or timeline ownership changed.
#[cfg(target_arch = "riscv32")]
pub(crate) struct DtmCompletionRecycleFailure<Role> {
    error: DtmCompletionRecycleError,
    item: DtmCompletionObservedEvent<Role>,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

#[cfg(target_arch = "riscv32")]
impl<Role> DtmCompletionRecycleFailure<Role> {
    pub(crate) fn into_parts(
        self,
    ) -> (
        DtmCompletionRecycleError,
        DtmCompletionObservedEvent<Role>,
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
pub(crate) struct DtmRecycleTimelineReleasedEvent<Role> {
    memory: DtmMemoryGraphRecycleCleaned,
    context: DtmEventContext,
    _role: PhantomData<Role>,
}

#[cfg(target_arch = "riscv32")]
impl<Role> DtmRecycleTimelineReleasedEvent<Role> {
    pub(crate) fn finish_source_list_release(self) -> DtmRecycledEvent<Role> {
        let (memory, status) = self.memory.into_cpu_owned().into_parts();
        DtmRecycledEvent {
            memory,
            context: self.context,
            status,
            _role: PhantomData,
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct DtmRxSuccessRecycleTimelineReleasedEvent {
    memory: DtmMemoryGraphRecycleCleaned,
    facts: DtmReceiverCommandFacts,
    session: DtmReceiverSession,
    last_committed_window: DtmRxCommittedWindow,
    outcome: crate::le::dtm::DtmRxCompletionOutcome,
}

#[cfg(target_arch = "riscv32")]
impl DtmRxSuccessRecycleTimelineReleasedEvent {
    pub(crate) fn finish_source_list_release(self) -> DtmRxRearmedEvent {
        let (memory, _) = self.memory.into_cpu_owned().into_parts();
        DtmRxRearmedEvent {
            memory,
            facts: self.facts,
            session: self.session,
            last_committed_window: self.last_committed_window,
            outcome: self.outcome,
        }
    }
}

/// CPU-owned graph after one exact completion/removal/recycle transaction.
#[must_use = "the recycled DTM graph must be retained by the role owner"]
#[cfg(any(target_arch = "riscv32", test))]
pub struct DtmRecycledEvent<Role> {
    memory: DtmMemoryGraphCpuOwned,
    context: DtmEventContext,
    status: DtmSchedulerItemCompletionStatus,
    _role: PhantomData<Role>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Role> DtmRecycledEvent<Role> {
    /// Role retained by the recycled event.
    pub const fn role(&self) -> DtmRole {
        match &self.context {
            DtmEventContext::Transmitter(_) => DtmRole::Transmitter,
            DtmEventContext::Receiver(_) => DtmRole::Receiver,
        }
    }

    /// Completion status retained across recycle.
    pub const fn status(&self) -> DtmSchedulerItemCompletionStatus {
        self.status
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmRecycledEvent<DtmTransmitterEvent> {
    /// Transmitter packet pattern retained by the recycled event.
    pub const fn packet_pattern(&self) -> DtmPayloadPattern {
        match &self.context {
            DtmEventContext::Transmitter(context) => context.facts.pattern,
            DtmEventContext::Receiver(_) => unreachable!(),
        }
    }

    /// Transmitter payload length retained by the recycled event.
    pub const fn packet_length(&self) -> DtmPayloadLength {
        match &self.context {
            DtmEventContext::Transmitter(context) => context.facts.length,
            DtmEventContext::Receiver(_) => unreachable!(),
        }
    }

    /// Consume this recycle result into the fail-closed active TX owner.
    pub fn into_next(self) -> DtmActiveTransmitterCpuOwned {
        let DtmEventContext::Transmitter(context) = self.context else {
            unreachable!()
        };
        DtmActiveTransmitterCpuOwned {
            memory: self.memory,
            facts: context.facts,
            last_committed_window: context.event_window,
            status: self.status,
        }
    }
}

/// CPU-owned transmitter graph belonging to an already active DTM session.
///
/// It cannot be converted back to a fresh graph or passed to the first-event
/// admission API. Immutable command identity and the last committed phase
/// anchor remain inseparable from packet readiness and graph ownership.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the active transmitter graph must recur or enter a proven Test End path"]
pub struct DtmActiveTransmitterCpuOwned {
    memory: DtmMemoryGraphCpuOwned,
    facts: DtmTransmitterCommandFacts,
    last_committed_window: DtmTxEventWindow,
    status: DtmSchedulerItemCompletionStatus,
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmActiveTransmitterCpuOwned {
    pub(crate) const fn link_state(&self) -> DtmLinkStateReset {
        self.facts.link_state
    }

    pub(crate) const fn channel(&self) -> DtmChannel {
        self.facts.channel
    }

    pub(crate) const fn phy(&self) -> DtmPhy {
        self.facts.phy
    }

    pub(crate) const fn timing(&self) -> DtmTxSchedulerTiming {
        self.facts.timing
    }

    #[cfg(test)]
    pub(crate) const fn margin(&self) -> u32 {
        self.facts.margin
    }

    pub(crate) const fn last_committed_window(&self) -> DtmTxEventWindow {
        self.last_committed_window
    }

    /// Packet pattern retained by the completed active test.
    pub const fn packet_pattern(&self) -> DtmPayloadPattern {
        self.facts.pattern
    }

    /// Payload length retained by the completed active test.
    pub const fn packet_length(&self) -> DtmPayloadLength {
        self.facts.length
    }

    /// Scheduler completion status that returned this graph to CPU ownership.
    pub const fn status(&self) -> DtmSchedulerItemCompletionStatus {
        self.status
    }

    /// Finish this transmitter test at its fully recycled CPU-owned boundary.
    ///
    /// LE Test End reports zero packets for a transmitter test. Any separate
    /// vendor diagnostic event count is deliberately not projected into this
    /// standardized HCI result.
    pub fn into_test_ended(self) -> DtmTestEndedCpuOwned {
        DtmTestEndedCpuOwned {
            quiesced: DtmQuiescedCpuOwned {
                memory: self.memory.into_reclaimed(),
            },
            report: DtmTestEndReport::Transmitter,
        }
    }

    /// End active hardware ownership without attaching HCI terminal policy.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_quiesced(self) -> DtmQuiescedCpuOwned {
        DtmQuiescedCpuOwned {
            memory: self.memory.into_reclaimed(),
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmRecycledEvent<DtmReceiverEvent> {
    /// Recover the unchanged RX session after a non-success scheduler event.
    pub fn into_next(self) -> DtmActiveReceiverCpuOwned {
        let DtmEventContext::Receiver(context) = self.context else {
            unreachable!()
        };
        DtmActiveReceiverCpuOwned {
            memory: self.memory,
            facts: context.facts,
            session: context.session,
            last_committed_window: context.event_window,
        }
    }
}

/// RX graph/session after one successful bounded drain and re-arm.
#[must_use = "the re-armed receiver session must continue or finish explicitly"]
#[cfg(target_arch = "riscv32")]
pub struct DtmRxRearmedEvent {
    memory: DtmMemoryGraphCpuOwned,
    facts: DtmReceiverCommandFacts,
    session: DtmReceiverSession,
    last_committed_window: DtmRxCommittedWindow,
    outcome: crate::le::dtm::DtmRxCompletionOutcome,
}

#[cfg(target_arch = "riscv32")]
impl DtmRxRearmedEvent {
    /// Semantic result of this event's bounded returned-buffer drain.
    pub const fn outcome(&self) -> crate::le::dtm::DtmRxCompletionOutcome {
        self.outcome
    }

    /// Accumulated packet count retained for LE Test End.
    pub const fn received_packet_count(&self) -> u16 {
        self.session.received_packet_count()
    }

    /// Consume this re-arm proof into the sole next-event aggregate.
    pub fn into_next(self) -> DtmActiveReceiverCpuOwned {
        DtmActiveReceiverCpuOwned {
            memory: self.memory,
            facts: self.facts,
            session: self.session,
            last_committed_window: self.last_committed_window,
        }
    }
}

#[cfg(test)]
impl<Phase> DtmSchedulerBookkeepingPrepared<DtmTransmitterEvent, Phase>
where
    Phase: DtmSchedulerItemPhase<DtmTransmitterEvent>,
{
    /// Return the exact standard pattern retained through bookkeeping.
    pub const fn packet_pattern(&self) -> DtmPayloadPattern {
        match &self.context {
            DtmEventContext::Transmitter(context) => context.facts.pattern,
            DtmEventContext::Receiver(_) => unreachable!(),
        }
    }

    /// Return the exact payload length retained through bookkeeping.
    pub const fn packet_length(&self) -> DtmPayloadLength {
        match &self.context {
            DtmEventContext::Transmitter(context) => context.facts.length,
            DtmEventContext::Receiver(_) => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests;
