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
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCompletionObservation, BluetoothDtmMemoryGraphCompletionObserved,
    BluetoothDtmMemoryGraphRecycleCleaned, BluetoothDtmMemoryGraphRecycleError,
    BluetoothDtmMemoryGraphRxSuccessRecycleError,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphPositionalEventPrepared,
    BluetoothDtmMemoryGraphPrepareError, BluetoothDtmMemoryGraphPrepareFailure,
    BluetoothDtmMemoryGraphReclaimed, BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared,
    BluetoothDtmPositionalEventWords, BluetoothDtmSchedulerItemCompletionStatus,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphEmptyListLinkPrepared, BluetoothDtmMemoryGraphHeadPublished,
    BluetoothDtmMemoryGraphRunning,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareRunCommandPublished, BluetoothSchedulerSoftwareListRemovalReady,
};

use crate::{
    BluetoothDtmChannel, BluetoothDtmInitialSchedulerItemPhase, BluetoothDtmLinkStateReset,
    BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern, BluetoothDtmPhy,
    BluetoothDtmPreparedTxGraph, BluetoothDtmRecurringSchedulerItemPhase, BluetoothDtmRole,
    BluetoothDtmRxInitialEventWindow, BluetoothDtmRxRecurringEventWindow,
    BluetoothDtmSchedulerMargin, BluetoothDtmTxEventWindow, BluetoothDtmTxSchedulerTiming,
    BluetoothSchedulerReservation, BluetoothSchedulerSequenceReady,
    dtm_rx_completion::BluetoothDtmReceiverSession,
    dtm_scheduler_item::apply_overlap_insertion_power,
};

/// Type marker for a transmitter event with a prepared packet prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmTransmitterEvent {}

/// Type marker for a receiver event without a transmitter packet prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmReceiverEvent {}

/// Immutable command identity retained across every transmitter event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothDtmTransmitterCommandFacts {
    link_state: BluetoothDtmLinkStateReset,
    channel: BluetoothDtmChannel,
    phy: BluetoothDtmPhy,
    timing: BluetoothDtmTxSchedulerTiming,
    margin: BluetoothDtmSchedulerMargin,
    pattern: BluetoothDtmPayloadPattern,
    length: BluetoothDtmPayloadLength,
}

/// Immutable command identity retained across every receiver event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothDtmReceiverCommandFacts {
    link_state: BluetoothDtmLinkStateReset,
    channel: BluetoothDtmChannel,
    phy: BluetoothDtmPhy,
    margin: BluetoothDtmSchedulerMargin,
}

/// Exact receiver window most recently committed by the scheduler lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmRxCommittedWindow {
    /// Initial-window identity committed by the first completed RX event.
    Initial(BluetoothDtmRxInitialEventWindow),
    /// Recurring-window identity committed by a later completed RX event.
    Recurring(BluetoothDtmRxRecurringEventWindow),
}

/// Semantic LE Test End result retained by the affine command owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub enum BluetoothDtmTestEndReport {
    /// A transmitter test reports zero packets through HCI.
    Transmitter,
    /// A receiver test reports its accumulated accepted-packet count.
    Receiver { received_packets: u16 },
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmTestEndReport {
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
pub(crate) struct BluetoothDtmQuiescedCpuOwned {
    memory: BluetoothDtmMemoryGraphReclaimed,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmQuiescedCpuOwned {
    pub(crate) fn into_reclaimed_graph(self) -> BluetoothDtmMemoryGraphReclaimed {
        self.memory
    }

    #[cfg(test)]
    pub(crate) fn from_cpu_owned_for_test(memory: BluetoothDtmMemoryGraphCpuOwned) -> Self {
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
pub struct BluetoothDtmTestEndedCpuOwned {
    quiesced: BluetoothDtmQuiescedCpuOwned,
    report: BluetoothDtmTestEndReport,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmTestEndedCpuOwned {
    /// Borrow the role-specific Test End report without releasing the graph.
    pub const fn report(&self) -> BluetoothDtmTestEndReport {
        self.report
    }

    /// Release the pinned graph to the idle session after response handoff.
    pub fn into_reclaimed_graph(self) -> BluetoothDtmMemoryGraphReclaimed {
        self.quiesced.into_reclaimed_graph()
    }
}

/// Why two validated DTM transforms cannot describe one event plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothDtmReviewedEventWordsPlanError {
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
pub(crate) struct BluetoothDtmReviewedEventWordsPlanFailure {
    error: BluetoothDtmReviewedEventWordsPlanError,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
}

impl BluetoothDtmReviewedEventWordsPlanFailure {
    /// Borrow the finite composition failure reason.
    #[cfg(test)]
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
/// Construction consumes an affine reservation that already passed its
/// phase-specific pre-sequence policy and fresh Controller-time sequence gate.
/// Initial insertion includes admission and bounded overlap displacement;
/// recurring insertion retains its exact collision-free window. Sequence timing
/// can therefore only be formed from the window retained by that reservation.
pub(crate) struct BluetoothDtmReviewedEventWordsPlan<Role> {
    link_state: BluetoothDtmLinkStateReset,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

impl<Role> BluetoothDtmReviewedEventWordsPlan<Role> {
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_reservation(
        self,
    ) -> BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady> {
        self.reservation
    }

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
        let retained_window = self.reservation.window();
        let link_state = self
            .link_state
            .with_private_links(seed.tx_head(), seed.rx_tail())
            .apply(current.link_state())
            .apply_event_context(self.link_state.role(), epoch.raw_time_for_scheduler_time(0));
        let scheduler_item = event.apply_raw_window(
            current.scheduler_item(),
            retained_window.start(),
            retained_window.end(),
        );
        let scheduler_item = apply_overlap_insertion_power(scheduler_item, link_state)
            .apply_sequence_timing(self.reservation.timing_policy().sequence_lead_raw_delta());
        BluetoothDtmPositionalEventWords::new(link_state, scheduler_item)
    }
}

/// Failed graph preparation retaining the sequence-ready scheduler plan.
pub(crate) struct BluetoothDtmReviewedEventPrepareFailure<Role> {
    memory: BluetoothDtmMemoryGraphPrepareFailure,
    _plan: BluetoothDtmReviewedEventWordsPlan<Role>,
    _pattern: BluetoothDtmPayloadPattern,
    _length: BluetoothDtmPayloadLength,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmReviewedEventPrepareFailure<BluetoothDtmTransmitterEvent> {
    /// Recover the byte-unchanged graph, complete TX program and reusable plan.
    ///
    /// Keeping the pattern and length here is required for a failed admission
    /// to rebuild the consumed packet-readiness proof without losing the DTM
    /// command that produced it.
    pub fn into_retry(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothDtmMemoryGraphPrepareError,
        BluetoothDtmPayloadPattern,
        BluetoothDtmPayloadLength,
        BluetoothDtmReviewedEventWordsPlan<BluetoothDtmTransmitterEvent>,
    ) {
        let (memory, error) = self.memory.into_parts();
        (memory, error, self._pattern, self._length, self._plan)
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
    pub(crate) fn new_transmitter(
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
    pub(crate) fn prepare_first(
        self,
        owner: BluetoothDtmPreparedTxGraph,
        channel: BluetoothDtmChannel,
        phy: BluetoothDtmPhy,
        timing: BluetoothDtmTxSchedulerTiming,
        margin: BluetoothDtmSchedulerMargin,
        window: BluetoothDtmTxEventWindow,
    ) -> Result<
        BluetoothDtmReviewedEventWordsPrepared<
            BluetoothDtmTransmitterEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
        BluetoothDtmReviewedEventPrepareFailure<BluetoothDtmTransmitterEvent>,
    > {
        let plan = self;
        let (memory, pattern, length) = owner.into_parts();
        let facts = BluetoothDtmTransmitterCommandFacts {
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
                return Err(BluetoothDtmReviewedEventPrepareFailure {
                    memory,
                    _plan: plan,
                    _pattern: pattern,
                    _length: length,
                });
            }
        };

        Ok(BluetoothDtmReviewedEventWordsPrepared {
            memory: prepared,
            context: BluetoothDtmEventContext::Transmitter(BluetoothDtmTransmitterEventContext {
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
        owner: BluetoothDtmActiveTransmitterCpuOwned,
        window: BluetoothDtmTxEventWindow,
    ) -> Result<
        BluetoothDtmReviewedEventWordsPrepared<
            BluetoothDtmTransmitterEvent,
            BluetoothDtmRecurringSchedulerItemPhase,
        >,
        BluetoothDtmRecurringTransmitterEventPrepareFailure,
    > {
        let plan = self;
        let BluetoothDtmActiveTransmitterCpuOwned {
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
                return Err(BluetoothDtmRecurringTransmitterEventPrepareFailure {
                    memory,
                    plan,
                    facts,
                    last_committed_window,
                    status,
                });
            }
        };

        Ok(BluetoothDtmReviewedEventWordsPrepared {
            memory: prepared,
            context: BluetoothDtmEventContext::Transmitter(BluetoothDtmTransmitterEventContext {
                facts,
                event_window: window,
            }),
            rollback: (last_committed_window, status),
            reservation: plan.reservation,
            _state: PhantomData,
        })
    }
}

impl BluetoothDtmReviewedEventWordsPlan<BluetoothDtmReceiverEvent> {
    /// Pair a receiver reset with its sequence-ready scheduler reservation.
    pub(crate) fn new_receiver(
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
    pub(crate) fn prepare_first(
        self,
        owner: BluetoothDtmReceiverCpuOwned,
        channel: BluetoothDtmChannel,
        phy: BluetoothDtmPhy,
        margin: BluetoothDtmSchedulerMargin,
        window: BluetoothDtmRxInitialEventWindow,
    ) -> Result<
        BluetoothDtmReviewedEventWordsPrepared<
            BluetoothDtmReceiverEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
        BluetoothDtmReceiverEventPrepareFailure,
    > {
        let plan = self;
        let BluetoothDtmReceiverCpuOwned { memory, session } = owner;
        let facts = BluetoothDtmReceiverCommandFacts {
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
                return Err(BluetoothDtmReceiverEventPrepareFailure {
                    memory,
                    _plan: plan,
                    _session: session,
                });
            }
        };

        Ok(BluetoothDtmReviewedEventWordsPrepared {
            memory: prepared,
            context: BluetoothDtmEventContext::Receiver(BluetoothDtmReceiverEventContext {
                facts,
                session,
                event_window: BluetoothDtmRxCommittedWindow::Initial(window),
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
        owner: BluetoothDtmActiveReceiverCpuOwned,
        window: BluetoothDtmRxRecurringEventWindow,
    ) -> Result<
        BluetoothDtmReviewedEventWordsPrepared<
            BluetoothDtmReceiverEvent,
            BluetoothDtmRecurringSchedulerItemPhase,
        >,
        BluetoothDtmRecurringReceiverEventPrepareFailure,
    > {
        let plan = self;
        let BluetoothDtmActiveReceiverCpuOwned {
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
                return Err(BluetoothDtmRecurringReceiverEventPrepareFailure {
                    memory,
                    plan,
                    facts,
                    session,
                    last_committed_window,
                });
            }
        };

        Ok(BluetoothDtmReviewedEventWordsPrepared {
            memory: prepared,
            context: BluetoothDtmEventContext::Receiver(BluetoothDtmReceiverEventContext {
                facts,
                session,
                event_window: BluetoothDtmRxCommittedWindow::Recurring(window),
            }),
            rollback: last_committed_window,
            reservation: plan.reservation,
            _state: PhantomData,
        })
    }
}

/// Fresh CPU-owned receiver graph before the first DTM event.
///
/// Recycled events return [`BluetoothDtmActiveReceiverCpuOwned`] instead, so
/// an active session cannot re-enter the initial descriptor path.
#[must_use = "the fresh receiver graph and its test state must stay together"]
pub struct BluetoothDtmReceiverCpuOwned {
    memory: BluetoothDtmMemoryGraphCpuOwned,
    session: BluetoothDtmReceiverSession,
}

/// CPU-owned receiver graph belonging to an already active DTM session.
///
/// This type deliberately has no conversion back to
/// [`BluetoothDtmReceiverCpuOwned`]. The immutable command and last committed
/// window travel with the graph, so only the recurring Controller operation or
/// a proven Test End path can consume it.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the active receiver graph must recur or enter a proven Test End path"]
pub struct BluetoothDtmActiveReceiverCpuOwned {
    memory: BluetoothDtmMemoryGraphCpuOwned,
    facts: BluetoothDtmReceiverCommandFacts,
    session: BluetoothDtmReceiverSession,
    last_committed_window: BluetoothDtmRxCommittedWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmActiveReceiverCpuOwned {
    pub(crate) const fn link_state(&self) -> BluetoothDtmLinkStateReset {
        self.facts.link_state
    }

    pub(crate) const fn channel(&self) -> BluetoothDtmChannel {
        self.facts.channel
    }

    pub(crate) const fn phy(&self) -> BluetoothDtmPhy {
        self.facts.phy
    }

    #[cfg(test)]
    pub(crate) const fn margin(&self) -> BluetoothDtmSchedulerMargin {
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
    pub fn into_test_ended(self) -> BluetoothDtmTestEndedCpuOwned {
        let report = BluetoothDtmTestEndReport::Receiver {
            received_packets: self.session.received_packet_count(),
        };
        BluetoothDtmTestEndedCpuOwned {
            quiesced: BluetoothDtmQuiescedCpuOwned {
                memory: self.memory.into_reclaimed(),
            },
            report,
        }
    }

    /// End active hardware ownership without attaching HCI terminal policy.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_quiesced(self) -> BluetoothDtmQuiescedCpuOwned {
        BluetoothDtmQuiescedCpuOwned {
            memory: self.memory.into_reclaimed(),
        }
    }
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
pub(crate) struct BluetoothDtmReceiverEventPrepareFailure {
    memory: BluetoothDtmMemoryGraphPrepareFailure,
    _plan: BluetoothDtmReviewedEventWordsPlan<BluetoothDtmReceiverEvent>,
    _session: BluetoothDtmReceiverSession,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmReceiverEventPrepareFailure {
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
                session: self._session,
            },
            error,
            self._plan,
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
pub(crate) struct BluetoothDtmRecurringTransmitterEventPrepareFailure {
    memory: BluetoothDtmMemoryGraphPrepareFailure,
    plan: BluetoothDtmReviewedEventWordsPlan<BluetoothDtmTransmitterEvent>,
    facts: BluetoothDtmTransmitterCommandFacts,
    last_committed_window: BluetoothDtmTxEventWindow,
    status: BluetoothDtmSchedulerItemCompletionStatus,
}

#[cfg(any(target_arch = "riscv32", test))]
#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "target-only scheduler consumes recurring recovery on production builds"
    )
)]
impl BluetoothDtmRecurringTransmitterEventPrepareFailure {
    pub(crate) fn into_retry(
        self,
    ) -> (
        BluetoothDtmActiveTransmitterCpuOwned,
        BluetoothDtmMemoryGraphPrepareError,
        BluetoothDtmReviewedEventWordsPlan<BluetoothDtmTransmitterEvent>,
    ) {
        let (memory, error) = self.memory.into_parts();
        (
            BluetoothDtmActiveTransmitterCpuOwned {
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
impl core::fmt::Debug for BluetoothDtmRecurringTransmitterEventPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmRecurringTransmitterEventPrepareFailure")
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
pub(crate) struct BluetoothDtmRecurringReceiverEventPrepareFailure {
    memory: BluetoothDtmMemoryGraphPrepareFailure,
    plan: BluetoothDtmReviewedEventWordsPlan<BluetoothDtmReceiverEvent>,
    facts: BluetoothDtmReceiverCommandFacts,
    session: BluetoothDtmReceiverSession,
    last_committed_window: BluetoothDtmRxCommittedWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "target-only scheduler consumes recurring recovery on production builds"
    )
)]
impl BluetoothDtmRecurringReceiverEventPrepareFailure {
    pub(crate) fn into_retry(
        self,
    ) -> (
        BluetoothDtmActiveReceiverCpuOwned,
        BluetoothDtmMemoryGraphPrepareError,
        BluetoothDtmReviewedEventWordsPlan<BluetoothDtmReceiverEvent>,
    ) {
        let (memory, error) = self.memory.into_parts();
        (
            BluetoothDtmActiveReceiverCpuOwned {
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
impl core::fmt::Debug for BluetoothDtmRecurringReceiverEventPrepareFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmRecurringReceiverEventPrepareFailure")
            .field("error", self.memory.error())
            .finish_non_exhaustive()
    }
}

struct BluetoothDtmTransmitterEventContext {
    facts: BluetoothDtmTransmitterCommandFacts,
    event_window: BluetoothDtmTxEventWindow,
}

struct BluetoothDtmReceiverEventContext {
    facts: BluetoothDtmReceiverCommandFacts,
    session: BluetoothDtmReceiverSession,
    event_window: BluetoothDtmRxCommittedWindow,
}

enum BluetoothDtmEventContext {
    Transmitter(BluetoothDtmTransmitterEventContext),
    Receiver(BluetoothDtmReceiverEventContext),
}

mod phase_sealed {
    pub trait Sealed<Role> {}
}

impl phase_sealed::Sealed<BluetoothDtmTransmitterEvent> for BluetoothDtmInitialSchedulerItemPhase {}

impl phase_sealed::Sealed<BluetoothDtmTransmitterEvent>
    for BluetoothDtmRecurringSchedulerItemPhase
{
}

impl phase_sealed::Sealed<BluetoothDtmReceiverEvent> for BluetoothDtmInitialSchedulerItemPhase {}

impl phase_sealed::Sealed<BluetoothDtmReceiverEvent> for BluetoothDtmRecurringSchedulerItemPhase {}

/// Valid relation between a DTM event role and its preparation phase.
///
/// Initial preparation has no prior active owner. Recurring preparation must
/// retain the committed window (and TX completion status) until publication,
/// so cancellation can reconstruct that owner without inspecting runtime
/// phase data. This trait is sealed: only the four supported TX/RX and
/// initial/recurring relations can be formed.
pub trait BluetoothDtmSchedulerItemPhase<Role>: phase_sealed::Sealed<Role> {
    #[doc(hidden)]
    type Rollback;
}

impl BluetoothDtmSchedulerItemPhase<BluetoothDtmTransmitterEvent>
    for BluetoothDtmInitialSchedulerItemPhase
{
    type Rollback = ();
}

impl BluetoothDtmSchedulerItemPhase<BluetoothDtmTransmitterEvent>
    for BluetoothDtmRecurringSchedulerItemPhase
{
    type Rollback = (
        BluetoothDtmTxEventWindow,
        BluetoothDtmSchedulerItemCompletionStatus,
    );
}

impl BluetoothDtmSchedulerItemPhase<BluetoothDtmReceiverEvent>
    for BluetoothDtmInitialSchedulerItemPhase
{
    type Rollback = ();
}

impl BluetoothDtmSchedulerItemPhase<BluetoothDtmReceiverEvent>
    for BluetoothDtmRecurringSchedulerItemPhase
{
    type Rollback = BluetoothDtmRxCommittedWindow;
}

/// CPU-owned bound graph containing one role-consistent reviewed word image.
///
/// The role marker preserves whether TX packet readiness was consumed into the
/// event. This type exposes no packet mutation or publication operation.
pub(crate) struct BluetoothDtmReviewedEventWordsPrepared<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    memory: BluetoothDtmMemoryGraphPositionalEventPrepared,
    context: BluetoothDtmEventContext,
    rollback: Phase::Rollback,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _state: PhantomData<(Role, Phase)>,
}

impl<Role, Phase> BluetoothDtmReviewedEventWordsPrepared<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    /// Install the common scheduler bookkeeping prefix for this exact graph.
    ///
    /// The resulting state remains CPU-owned and cancellable. Only that later
    /// state may form a scheduler request; this prevents event words without
    /// the in-flight sentinel and cleared completion link from being admitted.
    pub fn prepare_scheduler_bookkeeping(
        self,
    ) -> BluetoothDtmSchedulerBookkeepingPrepared<Role, Phase> {
        BluetoothDtmSchedulerBookkeepingPrepared {
            memory: self.memory.prepare_scheduler_bookkeeping(),
            context: self.context,
            rollback: self.rollback,
            reservation: self.reservation,
            _state: PhantomData,
        }
    }
}

impl
    BluetoothDtmReviewedEventWordsPrepared<
        BluetoothDtmTransmitterEvent,
        BluetoothDtmInitialSchedulerItemPhase,
    >
{
    /// Cancel a first event before publication and recover ordinary ownership.
    pub(crate) fn cancel_first(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    ) {
        let BluetoothDtmEventContext::Transmitter(_) = self.context else {
            unreachable!()
        };
        (self.memory.cancel(), self.reservation)
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl
    BluetoothDtmReviewedEventWordsPrepared<
        BluetoothDtmTransmitterEvent,
        BluetoothDtmRecurringSchedulerItemPhase,
    >
{
    /// Cancel a recurring event and reconstruct the exact prior active owner.
    pub(crate) fn cancel_recurring(
        self,
    ) -> (
        BluetoothDtmActiveTransmitterCpuOwned,
        BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    ) {
        let BluetoothDtmEventContext::Transmitter(context) = self.context else {
            unreachable!()
        };
        let (last_committed_window, status) = self.rollback;
        (
            BluetoothDtmActiveTransmitterCpuOwned {
                memory: self.memory.cancel(),
                facts: context.facts,
                last_committed_window,
                status,
            },
            self.reservation,
        )
    }
}

impl
    BluetoothDtmReviewedEventWordsPrepared<
        BluetoothDtmReceiverEvent,
        BluetoothDtmInitialSchedulerItemPhase,
    >
{
    /// Cancel a first event without detaching the RX session from memory.
    pub(crate) fn cancel_first(
        self,
    ) -> (
        BluetoothDtmReceiverCpuOwned,
        BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    ) {
        let BluetoothDtmEventContext::Receiver(context) = self.context else {
            unreachable!()
        };
        (
            BluetoothDtmReceiverCpuOwned {
                memory: self.memory.cancel(),
                session: context.session,
            },
            self.reservation,
        )
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl
    BluetoothDtmReviewedEventWordsPrepared<
        BluetoothDtmReceiverEvent,
        BluetoothDtmRecurringSchedulerItemPhase,
    >
{
    /// Cancel a recurring event and reconstruct the exact prior active owner.
    pub(crate) fn cancel_recurring(
        self,
    ) -> (
        BluetoothDtmActiveReceiverCpuOwned,
        BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    ) {
        let BluetoothDtmEventContext::Receiver(context) = self.context else {
            unreachable!()
        };
        (
            BluetoothDtmActiveReceiverCpuOwned {
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
pub(crate) struct BluetoothDtmSchedulerBookkeepingPrepared<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    memory: BluetoothDtmMemoryGraphSchedulerBookkeepingPrepared,
    context: BluetoothDtmEventContext,
    rollback: Phase::Rollback,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _state: PhantomData<(Role, Phase)>,
}

impl<Role, Phase> BluetoothDtmSchedulerBookkeepingPrepared<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    /// Return the typed controller-SRAM identity of the retained item.
    #[cfg(target_arch = "riscv32")]
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn prepare_empty_list_link(self) -> BluetoothDtmEmptyListLinkPrepared<Role, Phase> {
        BluetoothDtmEmptyListLinkPrepared {
            memory: self.memory.prepare_empty_list_link(),
            context: self.context,
            rollback: self.rollback,
            _reservation: self.reservation,
            _state: PhantomData,
        }
    }

    /// Cancel before publication and recover the prepared event words.
    pub fn cancel(self) -> BluetoothDtmReviewedEventWordsPrepared<Role, Phase> {
        BluetoothDtmReviewedEventWordsPrepared {
            memory: self.memory.cancel(),
            context: self.context,
            rollback: self.rollback,
            reservation: self.reservation,
            _state: PhantomData,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<Phase> BluetoothDtmSchedulerBookkeepingPrepared<BluetoothDtmTransmitterEvent, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<BluetoothDtmTransmitterEvent>,
{
    /// Return the exact standard pattern retained from packet preparation.
    pub(crate) const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        match &self.context {
            BluetoothDtmEventContext::Transmitter(context) => context.facts.pattern,
            BluetoothDtmEventContext::Receiver(_) => unreachable!(),
        }
    }

    /// Return the exact payload length retained from packet preparation.
    pub(crate) const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        match &self.context {
            BluetoothDtmEventContext::Transmitter(context) => context.facts.length,
            BluetoothDtmEventContext::Receiver(_) => unreachable!(),
        }
    }
}

/// Internal join candidate after the item-side empty-list transform.
///
/// Only the scheduler module can combine this memory owner with its affine
/// exclusive empty-list epoch. Keeping this type crate-private prevents a
/// memory-only transition from being mistaken for list ownership.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothDtmEmptyListLinkPrepared<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    memory: BluetoothDtmMemoryGraphEmptyListLinkPrepared,
    context: BluetoothDtmEventContext,
    rollback: Phase::Rollback,
    _reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _state: PhantomData<(Role, Phase)>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Role, Phase> BluetoothDtmEmptyListLinkPrepared<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    pub(crate) const fn role(&self) -> BluetoothDtmRole {
        match &self.context {
            BluetoothDtmEventContext::Transmitter(_) => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver(_) => BluetoothDtmRole::Receiver,
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
    ) -> BluetoothDtmHeadPublishedEvent<Role> {
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
        BluetoothDtmHeadPublishedEvent {
            memory: memory.into_head_published(publication),
            context,
            _reservation,
            _role: PhantomData,
        }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel(self) -> BluetoothDtmSchedulerBookkeepingPrepared<Role, Phase> {
        BluetoothDtmSchedulerBookkeepingPrepared {
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
pub(crate) struct BluetoothDtmHeadPublishedEvent<Role> {
    memory: BluetoothDtmMemoryGraphHeadPublished,
    context: BluetoothDtmEventContext,
    _reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Role> BluetoothDtmHeadPublishedEvent<Role> {
    pub(crate) const fn role(&self) -> BluetoothDtmRole {
        match &self.context {
            BluetoothDtmEventContext::Transmitter(_) => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver(_) => BluetoothDtmRole::Receiver,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_running(
        self,
        run: &BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothDtmRunningEvent<Role> {
        let Self {
            memory,
            context,
            _reservation,
            _role: _,
        } = self;
        BluetoothDtmRunningEvent {
            memory: memory.into_running(run),
            context,
            _reservation,
            _role: PhantomData,
        }
    }
}

/// Internal DTM event admitted through the complete scheduler RUN suffix.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothDtmRunningEvent<Role> {
    memory: BluetoothDtmMemoryGraphRunning,
    context: BluetoothDtmEventContext,
    _reservation: BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
    _role: PhantomData<Role>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Role> BluetoothDtmRunningEvent<Role> {
    pub(crate) const fn role(&self) -> BluetoothDtmRole {
        match &self.context {
            BluetoothDtmEventContext::Transmitter(_) => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver(_) => BluetoothDtmRole::Receiver,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> BluetoothDtmRunningEventCompletionObservation<Role> {
        let Self {
            memory,
            context,
            _reservation: reservation,
            _role: _,
        } = self;
        match memory.observe_completion(observed) {
            BluetoothDtmMemoryGraphCompletionObservation::ListMismatch { owner, observed } => {
                BluetoothDtmRunningEventCompletionObservation::ListMismatch {
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
                BluetoothDtmRunningEventCompletionObservation::StillInFlight(Self {
                    memory,
                    context,
                    _reservation: reservation,
                    _role: PhantomData,
                })
            }
            BluetoothDtmMemoryGraphCompletionObservation::CompletionObserved(memory) => {
                BluetoothDtmRunningEventCompletionObservation::CompletionObserved(
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
pub(crate) enum BluetoothDtmRunningEventCompletionObservation<Role> {
    ListMismatch {
        item: BluetoothDtmRunningEvent<Role>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(BluetoothDtmRunningEvent<Role>),
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
            BluetoothDtmEventContext::Transmitter(_) => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver(_) => BluetoothDtmRole::Receiver,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
        self.memory.status()
    }

    #[expect(
        clippy::result_large_err,
        reason = "lossless recycle failure retains the affine graph and removal owner"
    )]
    pub(crate) fn recycle<const CAPACITY: usize>(
        self,
        timeline: &mut crate::scheduler_timeline::BluetoothSchedulerTimeline<CAPACITY>,
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
    #[expect(
        clippy::result_large_err,
        reason = "lossless RX recycle failure retains the affine graph, session and removal owner"
    )]
    pub(crate) fn recycle_receiver_success<const CAPACITY: usize>(
        self,
        timeline: &mut crate::scheduler_timeline::BluetoothSchedulerTimeline<CAPACITY>,
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
        let BluetoothDtmEventContext::Receiver(mut context) = context else {
            unreachable!()
        };
        let (memory, outcome) = rx_prepared.observe().consume_then_commit(|projection| {
            projection.map_or(
                crate::BluetoothDtmRxCompletionOutcome::NoReturnedPacket,
                |result| context.session.account_projection(result),
            )
        });
        release.commit();
        Ok(BluetoothDtmRxSuccessRecycleTimelineReleasedEvent {
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
    facts: BluetoothDtmReceiverCommandFacts,
    session: BluetoothDtmReceiverSession,
    last_committed_window: BluetoothDtmRxCommittedWindow,
    outcome: crate::BluetoothDtmRxCompletionOutcome,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmRxSuccessRecycleTimelineReleasedEvent {
    pub(crate) fn finish_source_list_release(self) -> BluetoothDtmRxRearmedEvent {
        let (memory, _) = self.memory.into_cpu_owned().into_parts();
        BluetoothDtmRxRearmedEvent {
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
pub struct BluetoothDtmRecycledEvent<Role> {
    memory: BluetoothDtmMemoryGraphCpuOwned,
    context: BluetoothDtmEventContext,
    status: BluetoothDtmSchedulerItemCompletionStatus,
    _role: PhantomData<Role>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Role> BluetoothDtmRecycledEvent<Role> {
    /// Role retained by the recycled event.
    pub const fn role(&self) -> BluetoothDtmRole {
        match &self.context {
            BluetoothDtmEventContext::Transmitter(_) => BluetoothDtmRole::Transmitter,
            BluetoothDtmEventContext::Receiver(_) => BluetoothDtmRole::Receiver,
        }
    }

    /// Completion status retained across recycle.
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
        self.status
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmRecycledEvent<BluetoothDtmTransmitterEvent> {
    /// Transmitter packet pattern retained by the recycled event.
    pub const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        match &self.context {
            BluetoothDtmEventContext::Transmitter(context) => context.facts.pattern,
            BluetoothDtmEventContext::Receiver(_) => unreachable!(),
        }
    }

    /// Transmitter payload length retained by the recycled event.
    pub const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        match &self.context {
            BluetoothDtmEventContext::Transmitter(context) => context.facts.length,
            BluetoothDtmEventContext::Receiver(_) => unreachable!(),
        }
    }

    /// Consume this recycle result into the fail-closed active TX owner.
    pub fn into_next(self) -> BluetoothDtmActiveTransmitterCpuOwned {
        let BluetoothDtmEventContext::Transmitter(context) = self.context else {
            unreachable!()
        };
        BluetoothDtmActiveTransmitterCpuOwned {
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
pub struct BluetoothDtmActiveTransmitterCpuOwned {
    memory: BluetoothDtmMemoryGraphCpuOwned,
    facts: BluetoothDtmTransmitterCommandFacts,
    last_committed_window: BluetoothDtmTxEventWindow,
    status: BluetoothDtmSchedulerItemCompletionStatus,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmActiveTransmitterCpuOwned {
    pub(crate) const fn link_state(&self) -> BluetoothDtmLinkStateReset {
        self.facts.link_state
    }

    pub(crate) const fn channel(&self) -> BluetoothDtmChannel {
        self.facts.channel
    }

    pub(crate) const fn phy(&self) -> BluetoothDtmPhy {
        self.facts.phy
    }

    pub(crate) const fn timing(&self) -> BluetoothDtmTxSchedulerTiming {
        self.facts.timing
    }

    #[cfg(test)]
    pub(crate) const fn margin(&self) -> BluetoothDtmSchedulerMargin {
        self.facts.margin
    }

    pub(crate) const fn last_committed_window(&self) -> BluetoothDtmTxEventWindow {
        self.last_committed_window
    }

    /// Packet pattern retained by the completed active test.
    pub const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        self.facts.pattern
    }

    /// Payload length retained by the completed active test.
    pub const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        self.facts.length
    }

    /// Scheduler completion status that returned this graph to CPU ownership.
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
        self.status
    }

    /// Finish this transmitter test at its fully recycled CPU-owned boundary.
    ///
    /// LE Test End reports zero packets for a transmitter test. Any separate
    /// vendor diagnostic event count is deliberately not projected into this
    /// standardized HCI result.
    pub fn into_test_ended(self) -> BluetoothDtmTestEndedCpuOwned {
        BluetoothDtmTestEndedCpuOwned {
            quiesced: BluetoothDtmQuiescedCpuOwned {
                memory: self.memory.into_reclaimed(),
            },
            report: BluetoothDtmTestEndReport::Transmitter,
        }
    }

    /// End active hardware ownership without attaching HCI terminal policy.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_quiesced(self) -> BluetoothDtmQuiescedCpuOwned {
        BluetoothDtmQuiescedCpuOwned {
            memory: self.memory.into_reclaimed(),
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmRecycledEvent<BluetoothDtmReceiverEvent> {
    /// Recover the unchanged RX session after a non-success scheduler event.
    pub fn into_next(self) -> BluetoothDtmActiveReceiverCpuOwned {
        let BluetoothDtmEventContext::Receiver(context) = self.context else {
            unreachable!()
        };
        BluetoothDtmActiveReceiverCpuOwned {
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
pub struct BluetoothDtmRxRearmedEvent {
    memory: BluetoothDtmMemoryGraphCpuOwned,
    facts: BluetoothDtmReceiverCommandFacts,
    session: BluetoothDtmReceiverSession,
    last_committed_window: BluetoothDtmRxCommittedWindow,
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
    pub fn into_next(self) -> BluetoothDtmActiveReceiverCpuOwned {
        BluetoothDtmActiveReceiverCpuOwned {
            memory: self.memory,
            facts: self.facts,
            session: self.session,
            last_committed_window: self.last_committed_window,
        }
    }
}

#[cfg(test)]
impl<Phase> BluetoothDtmSchedulerBookkeepingPrepared<BluetoothDtmTransmitterEvent, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<BluetoothDtmTransmitterEvent>,
{
    /// Return the exact standard pattern retained through bookkeeping.
    pub const fn packet_pattern(&self) -> BluetoothDtmPayloadPattern {
        match &self.context {
            BluetoothDtmEventContext::Transmitter(context) => context.facts.pattern,
            BluetoothDtmEventContext::Receiver(_) => unreachable!(),
        }
    }

    /// Return the exact payload length retained through bookkeeping.
    pub const fn packet_length(&self) -> BluetoothDtmPayloadLength {
        match &self.context {
            BluetoothDtmEventContext::Transmitter(context) => context.facts.length,
            BluetoothDtmEventContext::Receiver(_) => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDtmBoundSramLinkAddress, BluetoothDtmMemoryGraphModelAddress,
        BluetoothDtmMemoryGraphStorage, BluetoothDtmRxResultProjection,
        BluetoothDtmSchedulerAllocationConfig,
    };
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::{
        BluetoothDtmActiveReceiverCpuOwned, BluetoothDtmActiveTransmitterCpuOwned,
        BluetoothDtmEventContext, BluetoothDtmReceiverCommandFacts, BluetoothDtmReceiverCpuOwned,
        BluetoothDtmReceiverEvent, BluetoothDtmReceiverEventContext, BluetoothDtmRecycledEvent,
        BluetoothDtmReviewedEventWordsPlan, BluetoothDtmReviewedEventWordsPlanError,
        BluetoothDtmRxCommittedWindow, BluetoothDtmTestEndReport,
        BluetoothDtmTransmitterCommandFacts, BluetoothDtmTransmitterEvent,
        BluetoothDtmTransmitterEventContext,
    };
    use crate::scheduler_timeline::BluetoothSchedulerTimeline;
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
        BluetoothDtmDefaultTxPowerDbm, BluetoothDtmLinkStateReset, BluetoothDtmPayloadLength,
        BluetoothDtmPayloadPattern, BluetoothDtmPhy, BluetoothDtmRole,
        BluetoothDtmRxRecurringEventWindow, BluetoothDtmSchedulerInstant,
        BluetoothDtmSchedulerItemEvent, BluetoothDtmSchedulerMargin,
        BluetoothDtmSchedulerTimingPolicy, BluetoothDtmTxGraphPrepare, BluetoothDtmTxTimingMicros,
        BluetoothSchedulerReservation, BluetoothSchedulerSequenceAuthorizationError,
        BluetoothSchedulerSequenceReady,
    };
    use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmSchedulerItemCompletionStatus;

    fn allocation_config() -> BluetoothDtmSchedulerAllocationConfig {
        BluetoothDtmSchedulerAllocationConfig::new(2, 3, 5, 4)
    }

    fn owner(base: u32) -> crate::BluetoothDtmMemoryGraphCpuOwned {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothDtmMemoryGraphStorage::new()));
        let base = BluetoothDtmMemoryGraphModelAddress::new(base)
            .expect("test base has valid compressed-pointer syntax");
        BluetoothDtmMemoryGraphStorage::pin_static_model(storage, base, allocation_config())
            .expect("test graph fits physical controller SRAM")
    }

    fn link_state(role: BluetoothDtmRole) -> BluetoothDtmLinkStateReset {
        BluetoothDtmLinkStateReset::new(None, None, BluetoothDtmDefaultTxPowerDbm::new(0), role)
    }

    fn epoch() -> BluetoothControllerSchedulerEpoch {
        BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
        )
    }

    fn channel() -> BluetoothDtmChannel {
        BluetoothDtmChannel::new(5).expect("channel five is valid")
    }

    fn margin() -> BluetoothDtmSchedulerMargin {
        crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone().dtm_scheduler_margin()
    }

    fn tx_timing() -> crate::BluetoothDtmTxSchedulerTiming {
        BluetoothDtmTxTimingMicros::new(
            BluetoothDtmPayloadLength::from_hci_image(3),
            BluetoothDtmPhy::Le2M,
            0,
        )
        .scheduler_timing()
    }

    fn tx_window() -> crate::BluetoothDtmTxEventWindow {
        tx_timing().initial_event_window(
            crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothDtmSchedulerInstant::from_image(64),
            BluetoothDtmSchedulerInstant::from_image(1_118),
        )
    }

    fn rx_initial_window() -> crate::BluetoothDtmRxInitialEventWindow {
        crate::BluetoothDtmRxInitialEventWindow::new(
            crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothDtmSchedulerInstant::from_image(64),
            BluetoothDtmSchedulerInstant::from_image(1_118),
        )
    }

    fn item(role: BluetoothDtmRole) -> BluetoothDtmSchedulerItemEvent {
        match role {
            BluetoothDtmRole::Transmitter => BluetoothDtmSchedulerItemEvent::new_transmitter(
                channel(),
                BluetoothDtmPhy::Le2M,
                tx_window(),
            ),
            BluetoothDtmRole::Receiver => BluetoothDtmSchedulerItemEvent::new_initial_receiver(
                channel(),
                BluetoothDtmPhy::LeCoded,
                rx_initial_window(),
            ),
        }
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
        initial_reservation_for_event(timeline, item(role))
    }

    fn initial_reservation_for_event<const CAPACITY: usize>(
        timeline: &mut BluetoothSchedulerTimeline<CAPACITY>,
        event: BluetoothDtmSchedulerItemEvent,
    ) -> BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady> {
        timeline
            .reserve_initial_dtm_event(event, epoch(), timing_policy(), admission_sample())
            .expect("the first guarded deadline is open")
            .authorize_sequence(admission_sample())
            .expect("the initial sequence deadline is open")
    }

    fn recurring_reservation_for_event<const CAPACITY: usize>(
        timeline: &mut BluetoothSchedulerTimeline<CAPACITY>,
        event: BluetoothDtmSchedulerItemEvent,
    ) -> BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady> {
        timeline
            .reserve_recurring_dtm_event(event, epoch(), timing_policy())
            .expect("the exact recurring window is collision-free")
            .authorize_sequence(admission_sample())
            .expect("the sole recurring sequence deadline is open")
    }

    #[test]
    fn tx_plan_requires_and_retains_the_prepared_packet_identity() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let stale = BluetoothDtmBoundSramLinkAddress::new(0x2f00_0400)
            .expect("stale model link remains syntactically valid");
        let reset = BluetoothDtmLinkStateReset::new(
            Some(stale),
            Some(stale),
            BluetoothDtmDefaultTxPowerDbm::new(20),
            BluetoothDtmRole::Transmitter,
        );
        let plan = BluetoothDtmReviewedEventWordsPlan::new_transmitter(
            reset,
            reservation(&mut timeline, BluetoothDtmRole::Transmitter),
        )
        .expect("both transforms encode TX");

        let packet = owner(0x2f07_0000).prepare_dtm_tx_packet(
            BluetoothDtmPayloadPattern::Repeated11110000,
            BluetoothDtmPayloadLength::from_hci_image(3),
        );

        let prepared = plan
            .prepare_first(
                packet,
                channel(),
                BluetoothDtmPhy::Le2M,
                tx_timing(),
                margin(),
                tx_window(),
            )
            .expect("fresh private links replace both stale plan links");
        let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
        assert_eq!(
            scheduler_prepared.packet_pattern(),
            BluetoothDtmPayloadPattern::Repeated11110000
        );
        assert_eq!(scheduler_prepared.packet_length().hci_image(), 3);
        let prepared = scheduler_prepared
            .prepare_empty_list_link()
            .cancel()
            .cancel();
        let (_owner, reservation) = prepared.cancel_first();
        assert!(timeline.release(reservation).is_ok());
        assert!(timeline.is_empty());
    }

    #[test]
    fn receiver_plan_cancellation_preserves_the_session_owner() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let reset = link_state(BluetoothDtmRole::Receiver);
        let plan = BluetoothDtmReviewedEventWordsPlan::new_receiver(
            reset,
            reservation(&mut timeline, BluetoothDtmRole::Receiver),
        )
        .expect("both transforms encode RX");

        let prepared = plan
            .prepare_first(
                BluetoothDtmReceiverCpuOwned::new(owner(0x2f00_0100)),
                channel(),
                BluetoothDtmPhy::LeCoded,
                margin(),
                rx_initial_window(),
            )
            .expect("the bound graph accepts the receiver plan");
        let scheduler_prepared = prepared.prepare_scheduler_bookkeeping();
        let (owner, reservation) = scheduler_prepared
            .prepare_empty_list_link()
            .cancel()
            .cancel()
            .cancel_first();

        assert_eq!(owner.received_packet_count(), 0);
        assert!(timeline.release(reservation).is_ok());
        assert!(timeline.is_empty());
    }

    #[test]
    fn recurring_tx_sequence_ready_reservation_enters_plan_and_cancels_losslessly() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let reset = link_state(BluetoothDtmRole::Transmitter);
        let pattern = BluetoothDtmPayloadPattern::Repeated11110000;
        let length = BluetoothDtmPayloadLength::from_hci_image(3);
        let memory = owner(0x2f06_0000)
            .prepare_dtm_tx_packet(pattern, length)
            .discard();
        let committed_window = tx_window();
        let candidate_window = tx_timing()
            .advance_event_window(
                crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
                committed_window,
                BluetoothDtmSchedulerInstant::from_image(1_100),
            )
            .window();
        let facts = BluetoothDtmTransmitterCommandFacts {
            link_state: reset,
            channel: channel(),
            phy: BluetoothDtmPhy::Le2M,
            timing: tx_timing(),
            margin: margin(),
            pattern,
            length,
        };
        let active = BluetoothDtmActiveTransmitterCpuOwned {
            memory,
            facts,
            last_committed_window: committed_window,
            status: BluetoothDtmSchedulerItemCompletionStatus::Zero,
        };
        let event = BluetoothDtmSchedulerItemEvent::new_transmitter(
            channel(),
            BluetoothDtmPhy::Le2M,
            candidate_window,
        )
        .expect("TX event accepts LE 2M");
        let plan = BluetoothDtmReviewedEventWordsPlan::new_transmitter(
            reset,
            recurring_reservation_for_event(&mut timeline, event),
        )
        .expect("both transforms encode TX");

        let prepared = plan
            .prepare_recurring(active, candidate_window)
            .expect("active TX graph accepts recurring preparation");
        let (active, reservation) = prepared
            .prepare_scheduler_bookkeeping()
            .prepare_empty_list_link()
            .cancel()
            .cancel()
            .cancel_recurring();

        assert_eq!(active.link_state(), reset);
        assert_eq!(active.channel(), channel());
        assert_eq!(active.phy(), BluetoothDtmPhy::Le2M);
        assert_eq!(active.timing(), tx_timing());
        assert_eq!(active.margin(), margin());
        assert_eq!(active.last_committed_window, committed_window);
        assert_eq!(
            active.status(),
            BluetoothDtmSchedulerItemCompletionStatus::Zero
        );
        assert!(timeline.release(reservation).is_ok());
    }

    #[test]
    fn recurring_rx_cancellation_restores_session_and_committed_window() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let reset = link_state(BluetoothDtmRole::Receiver);
        let committed_window = BluetoothDtmRxCommittedWindow::Initial(rx_initial_window());
        let candidate_window = BluetoothDtmRxRecurringEventWindow::new(
            crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothDtmSchedulerInstant::from_image(1_100),
            BluetoothDtmSchedulerInstant::from_image(1_120),
        );
        let facts = BluetoothDtmReceiverCommandFacts {
            link_state: reset,
            channel: channel(),
            phy: BluetoothDtmPhy::LeCoded,
            margin: margin(),
        };
        let active = BluetoothDtmActiveReceiverCpuOwned {
            memory: owner(0x2f05_0000),
            facts,
            session: crate::dtm_rx_completion::BluetoothDtmReceiverSession::new(),
            last_committed_window: committed_window,
        };
        let event = BluetoothDtmSchedulerItemEvent::new_recurring_receiver(
            channel(),
            BluetoothDtmPhy::LeCoded,
            candidate_window,
        )
        .expect("RX event accepts LE Coded");
        let plan = BluetoothDtmReviewedEventWordsPlan::new_receiver(
            reset,
            recurring_reservation_for_event(&mut timeline, event),
        )
        .expect("both transforms encode RX");

        let prepared = plan
            .prepare_recurring(active, candidate_window)
            .expect("active RX graph accepts recurring preparation");
        let (active, reservation) = prepared
            .prepare_scheduler_bookkeeping()
            .prepare_empty_list_link()
            .cancel()
            .cancel()
            .cancel_recurring();

        assert_eq!(active.link_state(), reset);
        assert_eq!(active.channel(), channel());
        assert_eq!(active.phy(), BluetoothDtmPhy::LeCoded);
        assert_eq!(active.margin(), margin());
        assert_eq!(active.last_committed_window, committed_window);
        assert_eq!(active.received_packet_count(), 0);
        assert!(timeline.release(reservation).is_ok());
    }

    #[test]
    fn completed_events_commit_only_the_candidate_window_into_active_owners() {
        let reset_tx = link_state(BluetoothDtmRole::Transmitter);
        let tx_candidate = tx_timing()
            .advance_event_window(
                crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
                tx_window(),
                BluetoothDtmSchedulerInstant::from_image(1_100),
            )
            .window();
        let tx_facts = BluetoothDtmTransmitterCommandFacts {
            link_state: reset_tx,
            channel: channel(),
            phy: BluetoothDtmPhy::Le2M,
            timing: tx_timing(),
            margin: margin(),
            pattern: BluetoothDtmPayloadPattern::Repeated11110000,
            length: BluetoothDtmPayloadLength::from_hci_image(3),
        };
        let recycled_tx = BluetoothDtmRecycledEvent::<BluetoothDtmTransmitterEvent> {
            memory: owner(0x2f04_0000),
            context: BluetoothDtmEventContext::Transmitter(BluetoothDtmTransmitterEventContext {
                facts: tx_facts,
                event_window: tx_candidate,
            }),
            status: BluetoothDtmSchedulerItemCompletionStatus::Zero,
            _role: core::marker::PhantomData,
        };
        assert_eq!(recycled_tx.packet_pattern(), tx_facts.pattern);
        assert_eq!(recycled_tx.packet_length(), tx_facts.length);
        let tx = recycled_tx.into_next();
        assert_eq!(tx.packet_pattern(), tx_facts.pattern);
        assert_eq!(tx.packet_length(), tx_facts.length);
        assert_eq!(tx.last_committed_window(), tx_candidate);
        assert_eq!(tx.status(), BluetoothDtmSchedulerItemCompletionStatus::Zero);

        let reset_rx = link_state(BluetoothDtmRole::Receiver);
        let rx_candidate = BluetoothDtmRxRecurringEventWindow::new(
            crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothDtmSchedulerInstant::from_image(1_100),
            BluetoothDtmSchedulerInstant::from_image(1_120),
        );
        let recycled_rx = BluetoothDtmRecycledEvent::<BluetoothDtmReceiverEvent> {
            memory: owner(0x2f03_0000),
            context: BluetoothDtmEventContext::Receiver(BluetoothDtmReceiverEventContext {
                facts: BluetoothDtmReceiverCommandFacts {
                    link_state: reset_rx,
                    channel: channel(),
                    phy: BluetoothDtmPhy::LeCoded,
                    margin: margin(),
                },
                session: crate::dtm_rx_completion::BluetoothDtmReceiverSession::new(),
                event_window: BluetoothDtmRxCommittedWindow::Recurring(rx_candidate),
            }),
            status: BluetoothDtmSchedulerItemCompletionStatus::Zero,
            _role: core::marker::PhantomData,
        };
        assert_eq!(recycled_rx.role(), BluetoothDtmRole::Receiver);
        assert_eq!(
            recycled_rx.status(),
            BluetoothDtmSchedulerItemCompletionStatus::Zero
        );
        let rx = recycled_rx.into_next();
        assert_eq!(
            rx.last_committed_window,
            BluetoothDtmRxCommittedWindow::Recurring(rx_candidate)
        );
        assert_eq!(rx.received_packet_count(), 0);
    }

    #[test]
    fn active_roles_hold_the_reclaimed_graph_through_test_end_handoff() {
        let reset_tx = link_state(BluetoothDtmRole::Transmitter);
        let tx = BluetoothDtmActiveTransmitterCpuOwned {
            memory: owner(0x2f02_0000),
            facts: BluetoothDtmTransmitterCommandFacts {
                link_state: reset_tx,
                channel: channel(),
                phy: BluetoothDtmPhy::Le2M,
                timing: tx_timing(),
                margin: margin(),
                pattern: BluetoothDtmPayloadPattern::Repeated11110000,
                length: BluetoothDtmPayloadLength::from_hci_image(3),
            },
            last_committed_window: tx_window(),
            status: BluetoothDtmSchedulerItemCompletionStatus::Zero,
        };
        let ended = tx.into_test_ended();
        let stopping = crate::BluetoothDtmSessionStopping::new(ended);
        assert_eq!(stopping.report(), BluetoothDtmTestEndReport::Transmitter);
        assert_eq!(stopping.report().reported_packet_count(), 0);
        let _next_graph = stopping.response_published().begin_epoch().into_graph();

        let mut session = crate::dtm_rx_completion::BluetoothDtmReceiverSession::new();
        assert!(matches!(
            session.account_projection(BluetoothDtmRxResultProjection::from_word(0)),
            crate::BluetoothDtmRxCompletionOutcome::Counted {
                received_packet_count: 1,
                ..
            }
        ));
        let reset_rx = link_state(BluetoothDtmRole::Receiver);
        let rx = BluetoothDtmActiveReceiverCpuOwned {
            memory: owner(0x2f01_0000),
            facts: BluetoothDtmReceiverCommandFacts {
                link_state: reset_rx,
                channel: channel(),
                phy: BluetoothDtmPhy::LeCoded,
                margin: margin(),
            },
            session,
            last_committed_window: BluetoothDtmRxCommittedWindow::Initial(rx_initial_window()),
        };
        let ended = rx.into_test_ended();
        let stopping = crate::BluetoothDtmSessionStopping::new(ended);
        assert_eq!(
            stopping.report(),
            BluetoothDtmTestEndReport::Receiver {
                received_packets: 1
            }
        );
        assert_eq!(stopping.report().reported_packet_count(), 1);
        let _next_graph = stopping.response_published().begin_epoch().into_graph();
    }

    #[test]
    fn plan_rejects_mixed_roles_before_it_can_consume_memory() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let reset = link_state(BluetoothDtmRole::Transmitter);
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
            .reserve_initial_dtm_event(
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
