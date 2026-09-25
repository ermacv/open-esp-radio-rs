//! Passive scan preparation, publication and recycling over the hardware scheduler.

use crate::runtime_resources::ControllerPoweredTaskRuntime;
#[allow(unused_imports)]
use crate::scheduler::core::*;
#[cfg(any(target_arch = "riscv32", test))]
use crate::scheduler::timeline::{SchedulerInitialAdmissionResolved, SchedulerWindowReservation};
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    ControllerTimeSample,
    scheduler::{
        SchedulerReservationError, SchedulerSequenceAuthorizationError, SchedulerSequenceReady,
        SchedulerTimingPolicy,
    },
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    LeReceivedBatch, LeRxError, PassiveScanMemoryGraphCommandPublished,
    PassiveScanMemoryGraphCompletionObserved, PassiveScanMemoryGraphPublicationError,
    PassiveScanMemoryGraphPublicationMismatch, PassiveScanMemoryGraphRecycleError,
    PassiveScanMemoryGraphRecycled, PassiveScanSchedulerItemCompletionStatus,
};
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_bluetooth_memory::{
    PassiveScanMemoryGraphCpuOwned, PassiveScanMemoryGraphEventPrepared,
    PassiveScanMemoryGraphSchedulerAdmissionPrepared, PassiveScanPrimaryChannel,
    PassiveScanSchedulerWindow, PassiveScanStartSelection,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerSoftwareListRemovalReady,
};
use {
    oer_esp32s31_hal::bluetooth::BluetoothControllerLatchedTime,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex,
    oer_esp32s31_hal::types::BluetoothControllerSramAddress,
};

/// Fresh initial-admission sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh scanner admission observation must be consumed or retained"]
pub struct PassiveScanAdmissionObservation {
    pub(crate) sample: ControllerTimeSample,
}

/// Fresh post-overlap sequence sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh scanner sequence observation must be consumed or retained"]
pub struct PassiveScanSequenceObservation {
    pub(crate) sample: ControllerTimeSample,
}

/// CPU-owned scanner graph with a requested window not yet admitted to the
/// common scheduler timeline.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the scanner candidate must enter common scheduling or be returned"]
pub struct PassiveScanFirstEventCandidate {
    graph: PassiveScanMemoryGraphCpuOwned,
    channel: PassiveScanPrimaryChannel,
    requested_window: crate::scheduler::SchedulerRawWindow,
    controller_time: BluetoothControllerLatchedTime,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PassiveScanFirstEventCandidate {
    pub(crate) const fn new(
        graph: PassiveScanMemoryGraphCpuOwned,
        channel: PassiveScanPrimaryChannel,
        requested_window: crate::scheduler::SchedulerRawWindow,
        controller_time: BluetoothControllerLatchedTime,
    ) -> Self {
        Self {
            graph,
            channel,
            requested_window,
            controller_time,
        }
    }

    pub const fn requested_window(&self) -> crate::scheduler::SchedulerRawWindow {
        self.requested_window
    }

    pub fn cancel(self) -> PassiveScanMemoryGraphCpuOwned {
        self.graph
    }

    fn prepare_resolved_event(
        self,
        resolved_window: crate::scheduler::SchedulerRawWindow,
    ) -> PassiveScanMemoryGraphEventPrepared {
        let window = PassiveScanSchedulerWindow::from_controller_ticks(
            resolved_window.start(),
            resolved_window.end(),
        )
        .expect("a timeline reservation retains a non-empty forward window");
        let selection = if resolved_window.start() == self.requested_window.start() {
            PassiveScanStartSelection::Requested
        } else {
            PassiveScanStartSelection::EarliestAvailable
        };
        self.graph
            .prepare_first_event(self.channel, window, selection, self.controller_time)
    }
}

/// First scanner event after common timeline admission and before the second
/// sequence deadline.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the admitted scanner event must pass sequence authorization or be cancelled"]
pub struct PassiveScanFirstPreSequence {
    candidate: PassiveScanFirstEventCandidate,
    reservation: SchedulerWindowReservation<SchedulerInitialAdmissionResolved>,
}

/// Scanner event image paired with the exact timeline interval encoded into it.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the prepared scanner event must be merged, cancelled, or retained"]
pub struct PassiveScanEventPrepared {
    graph: PassiveScanMemoryGraphEventPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PassiveScanEventPrepared {
    pub const fn channel(&self) -> PassiveScanPrimaryChannel {
        self.graph.channel()
    }

    pub const fn window(&self) -> PassiveScanSchedulerWindow {
        self.graph.window()
    }
}

/// Finite scanner preparation rejection before any MMIO publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub enum PassiveScanFirstEventPreparationError {
    Timeline(SchedulerReservationError),
    Sequence(SchedulerSequenceAuthorizationError),
}

/// Lossless first-scanner-event preparation rejection.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged scanner graph must be retried, cancelled, or retained"]
pub struct PassiveScanFirstEventPreparationFailure {
    candidate: PassiveScanFirstEventCandidate,
    error: PassiveScanFirstEventPreparationError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PassiveScanFirstEventPreparationFailure {
    pub const fn error(&self) -> PassiveScanFirstEventPreparationError {
        self.error
    }

    pub fn into_candidate(self) -> PassiveScanFirstEventCandidate {
        self.candidate
    }
}

/// Lossless rejection while joining one detached scanner item to the empty list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged detached scanner event remains CPU-owned"]
pub struct PassiveScanEmptySchedulerMergeFailure {
    error: SchedulerEmptyListMergeError,
    prepared: PassiveScanEventPrepared,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PassiveScanEmptySchedulerMergeFailure {
    /// Exact reason the exclusive scheduler epoch rejected the scanner item.
    pub const fn error(&self) -> SchedulerEmptyListMergeError {
        self.error
    }

    /// Recover the unchanged detached scanner graph.
    pub fn into_prepared(self) -> PassiveScanEventPrepared {
        self.prepared
    }
}

/// Detached scanner item joined to the source-owned empty scheduler list.
///
/// No scanner register, RX-list head, scheduler head or RUN command is visible
/// to hardware in this state. Cancellation restores both the common list epoch
/// and the scanner's private three-item free chain.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the scanner merge must be published through the same scheduler or cancelled"]
pub struct PassiveScanEmptySchedulerMergePrepared {
    graph: PassiveScanMemoryGraphSchedulerAdmissionPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PassiveScanEmptySchedulerMergePrepared {
    /// Exact detached item selected as the common scheduler head.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    /// Hardware list assigned to the first standalone passive scanner.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        BluetoothSchedulerHardwareListIndex::ZERO
    }
}

#[cfg(target_arch = "riscv32")]
enum PassiveScanSchedulerHeadPublicationFailureOwner {
    PrePublication(PassiveScanEmptySchedulerMergePrepared),
    RxPublication {
        _mismatch: PassiveScanMemoryGraphPublicationMismatch,
        _reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
        _head: BluetoothSchedulerHardwareListHead,
    },
}

/// Lossless scanner-head publication failure.
///
/// A scheduler-head validation error is retryable because it occurs before
/// MMIO. An RX publication mismatch follows the first MMIO write and is sealed
/// fail-stop while retaining the graph, HAL publication, reservation and
/// validated scheduler head.
#[cfg(target_arch = "riscv32")]
#[must_use = "inspect whether the exact retained owner is retryable or fail-stop"]
pub struct PassiveScanSchedulerHeadPublicationFailure {
    head_error: Option<SchedulerHeadPublicationError>,
    rx_publication_error: Option<PassiveScanMemoryGraphPublicationError>,
    owner: PassiveScanSchedulerHeadPublicationFailureOwner,
}

#[cfg(target_arch = "riscv32")]
impl PassiveScanSchedulerHeadPublicationFailure {
    /// Exact reason the common scheduler head could not be prepared.
    pub const fn head_error(&self) -> Option<SchedulerHeadPublicationError> {
        self.head_error
    }

    /// Return the typed RX publication mismatch after the first MMIO write.
    pub const fn rx_publication_error(&self) -> Option<PassiveScanMemoryGraphPublicationError> {
        self.rx_publication_error
    }

    /// Recover the unchanged merge only for a pre-MMIO validation failure.
    pub fn into_retryable_merged(self) -> Result<PassiveScanEmptySchedulerMergePrepared, Self> {
        match self {
            Self {
                owner: PassiveScanSchedulerHeadPublicationFailureOwner::PrePublication(merged),
                ..
            } => Ok(merged),
            failure @ Self {
                owner: PassiveScanSchedulerHeadPublicationFailureOwner::RxPublication { .. },
                ..
            } => Err(failure),
        }
    }
}

/// Scanner graph whose RX list, command and scheduler head are hardware-visible.
///
/// The publication transaction validates the common list identity before its
/// first irreversible write, then publishes RX memory, the restricted scanner
/// command and the exact scheduler head in that order. Dynamic interrupts and
/// RUN remain absent.
#[cfg(target_arch = "riscv32")]
#[must_use = "the scanner head must advance through the common RUN suffix"]
pub struct PassiveScanSchedulerHeadPublished {
    graph: PassiveScanMemoryGraphCommandPublished,
    publication: BluetoothSchedulerHardwareListHeadPublished,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl PassiveScanSchedulerHeadPublished {
    /// Exact scanner item retained by the graph and hardware head token.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    /// Hardware list containing the first scanner item.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.publication.index()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        PassiveScanMemoryGraphCommandPublished,
        BluetoothSchedulerHardwareListHeadPublished,
        SchedulerWindowReservation<SchedulerSequenceReady>,
    ) {
        (self.graph, self.publication, self.reservation)
    }
}

/// CPU-owned scanner graph and copied receive results.
#[cfg(target_arch = "riscv32")]
#[must_use = "return the graph and received packets to the scanner role owner"]
pub(crate) struct PassiveScanSchedulerRecycled {
    graph: PassiveScanMemoryGraphRecycled,
}

#[cfg(target_arch = "riscv32")]
impl PassiveScanSchedulerRecycled {
    pub fn into_parts(
        self,
    ) -> (
        oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned,
        LeReceivedBatch,
        PassiveScanSchedulerItemCompletionStatus,
    ) {
        self.graph.into_parts()
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct PassiveScanSchedulerRecycleReady {
    graph: PassiveScanMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl PassiveScanSchedulerRecycleReady {
    const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_item_address()
    }

    const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.removal.index()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "failure retains the scanner graph; success returns CPU ownership"]
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc role-tail outcomes retain the exact scanner graph on every branch"
)]
pub(crate) enum PassiveScanSchedulerRecycleStep {
    SchedulerIdentityMismatch {
        _ready: PassiveScanSchedulerRecycleReady,
    },
    FinishedListDrainStillActive {
        _ready: PassiveScanSchedulerRecycleReady,
    },
    MemoryIdentityMismatch {
        _ready: PassiveScanSchedulerRecycleReady,
        _error: PassiveScanMemoryGraphRecycleError,
    },
    ReceiveInvalid {
        _ready: PassiveScanSchedulerRecycleReady,
        _error: LeRxError,
    },
    ReservationIdentityMismatch {
        _ready: PassiveScanSchedulerRecycleReady,
    },
    Recycled(PassiveScanSchedulerRecycled),
}

#[cfg(any(target_arch = "riscv32", test))]
/// Role scheduler operations composed from the powered runtime's primitives.
pub(crate) trait PassiveScanScheduling<const SCHEDULER_CAPACITY: usize> {
    /// Admit one requested passive-scanner window into the common timeline.
    #[cfg(any(target_arch = "riscv32", test))]
    fn admit_passive_scan_first_event(
        &mut self,
        candidate: PassiveScanFirstEventCandidate,
        admission: PassiveScanAdmissionObservation,
    ) -> Result<PassiveScanFirstPreSequence, PassiveScanFirstEventPreparationFailure>;

    /// Authorize the second deadline and only then encode the
    /// overlap-resolved scanner window into private SRAM.
    #[cfg(any(target_arch = "riscv32", test))]
    fn prepare_passive_scan_first_event(
        &mut self,
        admitted: PassiveScanFirstPreSequence,
        sequence: PassiveScanSequenceObservation,
    ) -> Result<PassiveScanEventPrepared, PassiveScanFirstEventPreparationFailure>;

    /// Release one unpublished scanner event and its exact timeline slot.
    #[cfg(any(target_arch = "riscv32", test))]
    fn cancel_passive_scan_first_event(
        &mut self,
        prepared: PassiveScanEventPrepared,
    ) -> PassiveScanMemoryGraphCpuOwned;

    /// Release an admitted scanner candidate before its sequence sample arrives.
    #[cfg(any(target_arch = "riscv32", test))]
    fn cancel_passive_scan_first_pre_sequence(
        &mut self,
        admitted: PassiveScanFirstPreSequence,
    ) -> PassiveScanMemoryGraphCpuOwned;

    /// Join the detached first scanner item to this epoch's empty scheduler list.
    ///
    /// The private scanner graph has already removed the item from its free
    /// chain. This transition atomically reserves the same address in the
    /// source-owned common list without publishing MMIO.
    #[cfg(any(target_arch = "riscv32", test))]
    fn prepare_passive_scan_empty_list_merge(
        &mut self,
        prepared: PassiveScanEventPrepared,
    ) -> Result<PassiveScanEmptySchedulerMergePrepared, PassiveScanEmptySchedulerMergeFailure>;

    /// Restore an unpublished scanner merge through the same scheduler epoch.
    ///
    /// Success restores both the common empty-list proof and the selected
    /// scanner item's position in the private three-item free chain.
    #[cfg(any(target_arch = "riscv32", test))]
    #[cfg_attr(
        all(target_arch = "riscv32", not(test)),
        expect(
            dead_code,
            reason = "the scanner lifecycle publishes every merge; host tests exercise the rollback"
        )
    )]
    fn cancel_passive_scan_empty_list_merge(
        &mut self,
        merged: PassiveScanEmptySchedulerMergePrepared,
    ) -> Result<PassiveScanEventPrepared, PassiveScanEmptySchedulerMergePrepared>;

    /// Publish the complete lower passive-scanner transaction.
    ///
    /// The common list identity and scheduler-head encoding are checked before
    /// MMIO. An RX publication mismatch seals every owner after that first
    /// write; only a matching RX proof continues to the restricted scanner
    /// command and scheduler head in the reviewed hardware order.
    #[cfg(target_arch = "riscv32")]
    fn publish_passive_scan_scheduler_head(
        &mut self,
        merged: PassiveScanEmptySchedulerMergePrepared,
    ) -> Result<PassiveScanSchedulerHeadPublished, PassiveScanSchedulerHeadPublicationFailure>;

    /// Extract RX packets and release the scanner memory and common-list owners.
    #[cfg(target_arch = "riscv32")]
    fn recycle_passive_scan_completed(
        &mut self,
        ready: SingleItemSchedulerSoftwareListRemovalReady<
            crate::le::scanning::passive::active::PassiveScanCompletionRole,
        >,
    ) -> PassiveScanSchedulerRecycleStep;
}

#[cfg(any(target_arch = "riscv32", test))]
impl<const SCHEDULER_CAPACITY: usize> PassiveScanScheduling<SCHEDULER_CAPACITY>
    for ControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>
{
    #[cfg(any(target_arch = "riscv32", test))]
    fn admit_passive_scan_first_event(
        &mut self,
        candidate: PassiveScanFirstEventCandidate,
        admission: PassiveScanAdmissionObservation,
    ) -> Result<PassiveScanFirstPreSequence, PassiveScanFirstEventPreparationFailure> {
        let requested = candidate.requested_window();
        let timing_policy = SchedulerTimingPolicy::from_scheduler_config(
            self.scheduler_config(),
            self.controller_time_scale(),
        );
        match self
            .scheduler_timeline_mut()
            .reserve_phase_locked_initial_window(
                requested.start(),
                requested.end(),
                timing_policy,
                admission.sample,
            ) {
            Ok(reservation) => Ok(PassiveScanFirstPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(PassiveScanFirstEventPreparationFailure {
                candidate,
                error: PassiveScanFirstEventPreparationError::Timeline(error),
            }),
        }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn prepare_passive_scan_first_event(
        &mut self,
        admitted: PassiveScanFirstPreSequence,
        sequence: PassiveScanSequenceObservation,
    ) -> Result<PassiveScanEventPrepared, PassiveScanFirstEventPreparationFailure> {
        let PassiveScanFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return Err(PassiveScanFirstEventPreparationFailure {
                    candidate,
                    error: PassiveScanFirstEventPreparationError::Sequence(error),
                });
            }
        };
        let graph = candidate.prepare_resolved_event(reservation.window());
        Ok(PassiveScanEventPrepared { graph, reservation })
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn cancel_passive_scan_first_event(
        &mut self,
        prepared: PassiveScanEventPrepared,
    ) -> PassiveScanMemoryGraphCpuOwned {
        let PassiveScanEventPrepared { graph, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        graph.into_cpu_owned()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn cancel_passive_scan_first_pre_sequence(
        &mut self,
        admitted: PassiveScanFirstPreSequence,
    ) -> PassiveScanMemoryGraphCpuOwned {
        let PassiveScanFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        self.release_scheduler_reservation(reservation);
        candidate.cancel()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn prepare_passive_scan_empty_list_merge(
        &mut self,
        prepared: PassiveScanEventPrepared,
    ) -> Result<PassiveScanEmptySchedulerMergePrepared, PassiveScanEmptySchedulerMergeFailure> {
        let PassiveScanEventPrepared { graph, reservation } = prepared;
        let graph = graph.prepare_scheduler_admission();
        let address = graph.scheduler_head();
        if let Err(error) = self.scheduler_list_mut().prepare_first_item(address) {
            return Err(PassiveScanEmptySchedulerMergeFailure {
                error,
                prepared: PassiveScanEventPrepared {
                    graph: graph.cancel(),
                    reservation,
                },
            });
        }
        Ok(PassiveScanEmptySchedulerMergePrepared { graph, reservation })
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn cancel_passive_scan_empty_list_merge(
        &mut self,
        merged: PassiveScanEmptySchedulerMergePrepared,
    ) -> Result<PassiveScanEventPrepared, PassiveScanEmptySchedulerMergePrepared> {
        if !self
            .scheduler_list_mut()
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        let PassiveScanEmptySchedulerMergePrepared { graph, reservation } = merged;
        Ok(PassiveScanEventPrepared {
            graph: graph.cancel(),
            reservation,
        })
    }

    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the powered task owner and exact scanner graph jointly retain every PAC publication prerequisite"
    )]
    fn publish_passive_scan_scheduler_head(
        &mut self,
        merged: PassiveScanEmptySchedulerMergePrepared,
    ) -> Result<PassiveScanSchedulerHeadPublished, PassiveScanSchedulerHeadPublicationFailure> {
        let address = merged.scheduler_item_address();
        let index = merged.hardware_list_index();
        let head = match self.validate_first_scheduler_item_head(address) {
            Ok(head) => head,
            Err(error) => {
                return Err(PassiveScanSchedulerHeadPublicationFailure {
                    head_error: Some(error),
                    rx_publication_error: None,
                    owner: PassiveScanSchedulerHeadPublicationFailureOwner::PrePublication(merged),
                });
            }
        };
        let PassiveScanEmptySchedulerMergePrepared { graph, reservation } = merged;
        let graph = graph.prepare_publication();
        let graph = match self.publish_passive_scan_rx_memory(graph) {
            Ok(graph) => graph,
            Err(mismatch) => {
                let error = mismatch.error();
                return Err(PassiveScanSchedulerHeadPublicationFailure {
                    head_error: None,
                    rx_publication_error: Some(error),
                    owner: PassiveScanSchedulerHeadPublicationFailureOwner::RxPublication {
                        _mismatch: mismatch,
                        _reservation: reservation,
                        _head: head,
                    },
                });
            }
        };
        let graph = self.publish_passive_scan_command(graph);
        let publication = self.publish_validated_first_scheduler_item_head(address, index, head);
        Ok(PassiveScanSchedulerHeadPublished {
            graph,
            publication,
            reservation,
        })
    }

    #[cfg(target_arch = "riscv32")]
    fn recycle_passive_scan_completed(
        &mut self,
        ready: SingleItemSchedulerSoftwareListRemovalReady<
            crate::le::scanning::passive::active::PassiveScanCompletionRole,
        >,
    ) -> PassiveScanSchedulerRecycleStep {
        let (graph, removal, reservation) = ready.into_parts();
        let ready = PassiveScanSchedulerRecycleReady {
            graph,
            removal,
            reservation,
        };
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                .scheduler_list_mut()
                .retains_software_list_removal_ready_first_item(address)
        {
            return PassiveScanSchedulerRecycleStep::SchedulerIdentityMismatch { _ready: ready };
        }
        if self.scheduler_finished_lists_mut().is_active() {
            return PassiveScanSchedulerRecycleStep::FinishedListDrainStillActive { _ready: ready };
        }
        let PassiveScanSchedulerRecycleReady {
            graph,
            removal,
            reservation,
        } = ready;
        let prepared = match graph.prepare_recycle_after_software_list_removal(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (graph, removal) = failure.into_parts();
                return PassiveScanSchedulerRecycleStep::MemoryIdentityMismatch {
                    _ready: PassiveScanSchedulerRecycleReady {
                        graph,
                        removal,
                        reservation,
                    },
                    _error: error,
                };
            }
        };
        let extracted = match prepared.extract_received() {
            Ok(extracted) => extracted,
            Err(failure) => {
                let error = failure.error();
                let (graph, removal) = failure.into_prepared().into_parts();
                return PassiveScanSchedulerRecycleStep::ReceiveInvalid {
                    _ready: PassiveScanSchedulerRecycleReady {
                        graph,
                        removal,
                        reservation,
                    },
                    _error: error,
                };
            }
        };
        let release = match self.scheduler_timeline_mut().prepare_release(reservation) {
            Ok(release) => release,
            Err(failure) => {
                let reservation = failure.into_reservation();
                let (graph, removal) = extracted.into_prepared().into_parts();
                return PassiveScanSchedulerRecycleStep::ReservationIdentityMismatch {
                    _ready: PassiveScanSchedulerRecycleReady {
                        graph,
                        removal,
                        reservation,
                    },
                };
            }
        };
        let graph = extracted.commit();
        release.commit();
        self.scheduler_list_mut().commit_recycled_first_item();
        PassiveScanSchedulerRecycleStep::Recycled(PassiveScanSchedulerRecycled { graph })
    }
}

#[cfg(test)]
mod tests;
