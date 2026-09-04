//! Peripheral-connection completion, recycle and LL classification.

#![forbid(unsafe_code)]

#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_bluetooth_ll::connection::LePeripheralConnectionEventPeerActivity;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_ll::connection::{
    LePeripheralConnectionEventCompleted, LePeripheralConnectionEventInFlight,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BLUETOOTH_NON_SCANNING_RX_NODE_COUNT, BluetoothLeReceivedBatch, BluetoothLeRxError,
    BluetoothPeripheralConnectionCapturedAnchorAvailability,
    BluetoothPeripheralConnectionCapturedAnchorTime,
    BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned,
    BluetoothPeripheralConnectionMemoryGraphCompletionObserved,
    BluetoothPeripheralConnectionMemoryGraphRecycleError,
    BluetoothPeripheralConnectionMemoryGraphRecyclePrepared,
    BluetoothPeripheralConnectionMemoryGraphRxExtracted,
    BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerSoftwareListRemovalReady,
};

#[cfg(target_arch = "riscv32")]
use super::{
    BluetoothPeripheralConnectionFirstEventRunning, BluetoothPeripheralConnectionFirstWindow,
    BluetoothPeripheralConnectionPacketStartTiming,
};
#[cfg(target_arch = "riscv32")]
use crate::BluetoothSchedulerRawWindow;

#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothPeripheralConnectionFirstEventCompletionObservation {
    ListMismatch {
        running: BluetoothPeripheralConnectionFirstEventRunning,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(BluetoothPeripheralConnectionFirstEventRunning),
    CompletionObserved(BluetoothPeripheralConnectionFirstEventCompletionObserved),
}

/// First connection event after one fenced non-sentinel status observation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed connection event must advance through scheduler unlink"]
pub(crate) struct BluetoothPeripheralConnectionFirstEventCompletionObserved {
    graph: BluetoothPeripheralConnectionMemoryGraphCompletionObserved,
    event: LePeripheralConnectionEventInFlight,
    first_window: BluetoothPeripheralConnectionFirstWindow,
    requested_window: BluetoothSchedulerRawWindow,
    resolved_window: BluetoothSchedulerRawWindow,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionFirstEventCompletionObserved {
    pub(super) const fn new(
        graph: BluetoothPeripheralConnectionMemoryGraphCompletionObserved,
        event: LePeripheralConnectionEventInFlight,
        first_window: BluetoothPeripheralConnectionFirstWindow,
        requested_window: BluetoothSchedulerRawWindow,
        resolved_window: BluetoothSchedulerRawWindow,
    ) -> Self {
        Self {
            graph,
            event,
            first_window,
            requested_window,
            resolved_window,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_item_address()
    }

    pub(crate) const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub(crate) const fn status(
        &self,
    ) -> BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
        self.graph.status()
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure retains the completed event and removal proof"
    )]
    pub(crate) fn prepare_recycle(
        self,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<
        BluetoothPeripheralConnectionCompletionRecyclePrepared,
        BluetoothPeripheralConnectionCompletionRecycleFailure,
    > {
        let Self {
            graph,
            event,
            first_window,
            requested_window,
            resolved_window,
        } = self;
        match graph.prepare_recycle_after_software_list_removal(removal) {
            Ok(graph) => Ok(BluetoothPeripheralConnectionCompletionRecyclePrepared {
                graph,
                event,
                first_window,
                requested_window,
                resolved_window,
            }),
            Err(failure) => {
                let error = failure.error();
                let (graph, removal) = failure.into_parts();
                Err(BluetoothPeripheralConnectionCompletionRecycleFailure {
                    error,
                    completed: Self {
                        graph,
                        event,
                        first_window,
                        requested_window,
                        resolved_window,
                    },
                    removal,
                })
            }
        }
    }
}

/// Completed connection event authorized for lower RX extraction.
#[cfg(target_arch = "riscv32")]
#[must_use = "the recycle transaction must be extracted or returned unchanged"]
pub(crate) struct BluetoothPeripheralConnectionCompletionRecyclePrepared {
    graph: BluetoothPeripheralConnectionMemoryGraphRecyclePrepared,
    event: LePeripheralConnectionEventInFlight,
    first_window: BluetoothPeripheralConnectionFirstWindow,
    requested_window: BluetoothSchedulerRawWindow,
    resolved_window: BluetoothSchedulerRawWindow,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionCompletionRecyclePrepared {
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothPeripheralConnectionFirstEventCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        let (graph, removal) = self.graph.into_parts();
        (
            BluetoothPeripheralConnectionFirstEventCompletionObserved {
                graph,
                event: self.event,
                first_window: self.first_window,
                requested_window: self.requested_window,
                resolved_window: self.resolved_window,
            },
            removal,
        )
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure retains the complete recycle transaction"
    )]
    pub(crate) fn extract_received(
        self,
    ) -> Result<
        BluetoothPeripheralConnectionCompletionRxExtracted,
        BluetoothPeripheralConnectionCompletionRxExtractionFailure,
    > {
        let Self {
            graph,
            event,
            first_window,
            requested_window,
            resolved_window,
        } = self;
        match graph.extract_received() {
            Ok(graph) => Ok(BluetoothPeripheralConnectionCompletionRxExtracted {
                graph,
                event,
                first_window,
                requested_window,
                resolved_window,
            }),
            Err(failure) => Err(BluetoothPeripheralConnectionCompletionRxExtractionFailure {
                error: failure.error(),
                prepared: Self {
                    graph: failure.into_prepared(),
                    event,
                    first_window,
                    requested_window,
                    resolved_window,
                },
            }),
        }
    }
}

/// Lower recycle mismatch retaining the exact completion and removal proof.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed connection event and removal proof remain owned"]
pub(crate) struct BluetoothPeripheralConnectionCompletionRecycleFailure {
    error: BluetoothPeripheralConnectionMemoryGraphRecycleError,
    completed: BluetoothPeripheralConnectionFirstEventCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionCompletionRecycleFailure {
    pub(crate) const fn error(&self) -> BluetoothPeripheralConnectionMemoryGraphRecycleError {
        self.error
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothPeripheralConnectionFirstEventCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }
}

/// Malformed RX result retaining the entire uncommitted recycle transaction.
#[cfg(target_arch = "riscv32")]
#[must_use = "the failed RX transaction must enter fail-stop handling or be retained"]
pub(crate) struct BluetoothPeripheralConnectionCompletionRxExtractionFailure {
    error: BluetoothLeRxError,
    prepared: BluetoothPeripheralConnectionCompletionRecyclePrepared,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionCompletionRxExtractionFailure {
    pub(crate) const fn error(&self) -> BluetoothLeRxError {
        self.error
    }

    pub(crate) fn into_prepared(self) -> BluetoothPeripheralConnectionCompletionRecyclePrepared {
        self.prepared
    }
}

/// Copied RX result joined to the uncommitted connection recycle owner.
#[cfg(target_arch = "riscv32")]
#[must_use = "the event resources must be committed before connection recurrence"]
pub(crate) struct BluetoothPeripheralConnectionCompletionRxExtracted {
    graph: BluetoothPeripheralConnectionMemoryGraphRxExtracted,
    event: LePeripheralConnectionEventInFlight,
    first_window: BluetoothPeripheralConnectionFirstWindow,
    requested_window: BluetoothSchedulerRawWindow,
    resolved_window: BluetoothSchedulerRawWindow,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionCompletionRxExtracted {
    pub(crate) fn into_prepared(self) -> BluetoothPeripheralConnectionCompletionRecyclePrepared {
        BluetoothPeripheralConnectionCompletionRecyclePrepared {
            graph: self.graph.into_prepared(),
            event: self.event,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
        }
    }

    pub(crate) fn commit(self) -> BluetoothPeripheralConnectionRecycledEvent {
        let (graph, batch, status, capture) = self.graph.commit().into_parts();
        BluetoothPeripheralConnectionRecycledEvent {
            graph,
            event: self.event,
            batch,
            status,
            capture,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
        }
    }
}

/// CPU-owned active connection after event-local SRAM reclamation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the active connection must classify completion before LL advance"]
#[allow(
    dead_code,
    reason = "the next recurring-event transition consumes the preserved active graph and phase"
)]
pub(crate) struct BluetoothPeripheralConnectionRecycledEvent {
    graph: BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned,
    event: LePeripheralConnectionEventInFlight,
    batch: BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
    status: BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
    capture: BluetoothPeripheralConnectionCapturedAnchorAvailability,
    first_window: BluetoothPeripheralConnectionFirstWindow,
    requested_window: BluetoothSchedulerRawWindow,
    resolved_window: BluetoothSchedulerRawWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
enum BluetoothPeripheralConnectionCaptureCompletion<T> {
    Complete {
        activity: LePeripheralConnectionEventPeerActivity,
        packet_start: Option<T>,
    },
    NormalizationUnavailable,
}

#[cfg(any(target_arch = "riscv32", test))]
fn classify_peripheral_connection_capture<C, T>(
    capture: Option<C>,
    normalize: impl FnOnce(C) -> Option<T>,
) -> BluetoothPeripheralConnectionCaptureCompletion<T> {
    let Some(captured) = capture else {
        return BluetoothPeripheralConnectionCaptureCompletion::Complete {
            activity: LePeripheralConnectionEventPeerActivity::Missed,
            packet_start: None,
        };
    };
    match normalize(captured) {
        Some(packet_start) => BluetoothPeripheralConnectionCaptureCompletion::Complete {
            activity: LePeripheralConnectionEventPeerActivity::Observed,
            packet_start: Some(packet_start),
        },
        None => BluetoothPeripheralConnectionCaptureCompletion::NormalizationUnavailable,
    }
}

/// Result of classifying one recycled connection event.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed owner or unchanged retry owner must be retained"]
pub(crate) enum BluetoothPeripheralConnectionCompletionClassification {
    NormalizationUnavailable(BluetoothPeripheralConnectionRecycledEvent),
    Completed(BluetoothPeripheralConnectionCompletedEvent),
}

/// Closed portable event retaining every reclaimed S31 connection resource.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed connection owner must enter recurrence or teardown"]
#[allow(
    dead_code,
    reason = "recurrence consumes the retained active graph and scheduling phase in the next edge"
)]
pub(crate) struct BluetoothPeripheralConnectionCompletedEvent {
    graph: BluetoothPeripheralConnectionMemoryGraphActiveCpuOwned,
    event: LePeripheralConnectionEventCompleted,
    batch: BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
    status: BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
    packet_start: Option<BluetoothPeripheralConnectionPacketStartTiming>,
    first_window: BluetoothPeripheralConnectionFirstWindow,
    requested_window: BluetoothSchedulerRawWindow,
    resolved_window: BluetoothSchedulerRawWindow,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionCompletedEvent {
    pub(crate) const fn link_layer_completion(&self) -> &LePeripheralConnectionEventCompleted {
        &self.event
    }

    pub(crate) const fn status(
        &self,
    ) -> BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
        self.status
    }

    pub(crate) const fn received(
        &self,
    ) -> BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT> {
        self.batch
    }

    pub(crate) const fn packet_start(
        &self,
    ) -> Option<&BluetoothPeripheralConnectionPacketStartTiming> {
        self.packet_start.as_ref()
    }
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionRecycledEvent {
    fn complete(
        self,
        activity: LePeripheralConnectionEventPeerActivity,
        packet_start: Option<BluetoothPeripheralConnectionPacketStartTiming>,
    ) -> BluetoothPeripheralConnectionCompletedEvent {
        BluetoothPeripheralConnectionCompletedEvent {
            graph: self.graph,
            event: self.event.complete(activity),
            batch: self.batch,
            status: self.status,
            packet_start,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
        }
    }

    pub(crate) const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub(crate) const fn status(
        &self,
    ) -> BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
        self.status
    }

    pub(crate) const fn received(
        &self,
    ) -> BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT> {
        self.batch
    }

    pub(crate) fn classify_completion(
        self,
        normalize: impl FnOnce(
            BluetoothPeripheralConnectionCapturedAnchorTime,
        ) -> Option<BluetoothPeripheralConnectionPacketStartTiming>,
    ) -> BluetoothPeripheralConnectionCompletionClassification {
        let capture = match self.capture {
            BluetoothPeripheralConnectionCapturedAnchorAvailability::Absent => None,
            BluetoothPeripheralConnectionCapturedAnchorAvailability::Available(captured) => {
                Some(captured)
            }
        };
        match classify_peripheral_connection_capture(capture, normalize) {
            BluetoothPeripheralConnectionCaptureCompletion::Complete {
                activity,
                packet_start,
            } => BluetoothPeripheralConnectionCompletionClassification::Completed(
                self.complete(activity, packet_start),
            ),
            BluetoothPeripheralConnectionCaptureCompletion::NormalizationUnavailable => {
                BluetoothPeripheralConnectionCompletionClassification::NormalizationUnavailable(
                    self,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use open_esp_radio_bluetooth_ll::connection::LePeripheralConnectionEventPeerActivity;

    use super::{
        BluetoothPeripheralConnectionCaptureCompletion, classify_peripheral_connection_capture,
    };

    #[test]
    fn absent_connection_capture_is_a_missed_event_without_normalization() {
        let called = Cell::new(false);

        let result = classify_peripheral_connection_capture(None::<()>, |_| {
            called.set(true);
            Some(())
        });

        let BluetoothPeripheralConnectionCaptureCompletion::Complete {
            activity,
            packet_start,
        } = result
        else {
            panic!("an absent capture completes without timestamp normalization");
        };
        assert_eq!(activity, LePeripheralConnectionEventPeerActivity::Missed);
        assert_eq!(packet_start, None);
        assert!(!called.get());
    }

    #[test]
    fn available_connection_capture_is_observed_after_one_normalization() {
        let calls = Cell::new(0);

        let result = classify_peripheral_connection_capture(Some(()), |_| {
            calls.set(calls.get() + 1);
            Some(37_u32)
        });

        let BluetoothPeripheralConnectionCaptureCompletion::Complete {
            activity,
            packet_start,
        } = result
        else {
            panic!("a normalized available capture completes as observed");
        };
        assert_eq!(activity, LePeripheralConnectionEventPeerActivity::Observed);
        assert_eq!(packet_start, Some(37));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn available_connection_capture_without_normalization_remains_uncompleted() {
        let calls = Cell::new(0);

        let result = classify_peripheral_connection_capture(Some(()), |_| {
            calls.set(calls.get() + 1);
            None::<()>
        });

        assert!(matches!(
            result,
            BluetoothPeripheralConnectionCaptureCompletion::NormalizationUnavailable
        ));
        assert_eq!(calls.get(), 1);
    }
}
