//! Peripheral-connection completion, recycle and LL classification.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use super::{
    PeripheralConnectionFirstEventRunning, PeripheralConnectionFirstEventRxPublished,
    PeripheralConnectionFirstWindow, PeripheralConnectionPacketStartTiming,
    PeripheralConnectionRecurringPhase,
};
#[cfg(target_arch = "riscv32")]
use crate::scheduler::SchedulerRawWindow;
#[cfg(any(target_arch = "riscv32", test))]
use oer_bluetooth_ll::connection::LePeripheralConnectionEventPeerActivity;
#[cfg(target_arch = "riscv32")]
use oer_bluetooth_ll::connection::{
    LePeripheralConnectionEventCompleted, LePeripheralConnectionEventInFlight,
    LePeripheralConnectionEventPrepared,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    BLUETOOTH_NON_SCANNING_RX_NODE_COUNT, LeReceivedBatch, LeRxError,
    PeripheralConnectionCapturedAnchorAvailability, PeripheralConnectionCapturedAnchorTime,
    PeripheralConnectionMemoryGraphActiveCpuOwned,
    PeripheralConnectionMemoryGraphCompletionObserved, PeripheralConnectionMemoryGraphRecycleError,
    PeripheralConnectionMemoryGraphRecyclePrepared, PeripheralConnectionMemoryGraphRxExtracted,
    PeripheralConnectionMemoryGraphRxPublished, PeripheralConnectionSchedulerItemCompletionStatus,
};
#[cfg(target_arch = "riscv32")]
use {
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerFinishedHardwareListObserved,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerSoftwareListRemovalReady,
    oer_esp32s31_hal::types::BluetoothControllerSramAddress,
};

#[cfg(target_arch = "riscv32")]
pub(crate) enum PeripheralConnectionFirstEventCompletionObservation {
    ListMismatch {
        running: PeripheralConnectionFirstEventRunning,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(PeripheralConnectionFirstEventRunning),
    CompletionObserved(PeripheralConnectionFirstEventCompletionObserved),
}

/// First connection event after one fenced non-sentinel status observation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed connection event must advance through scheduler unlink"]
pub(crate) struct PeripheralConnectionFirstEventCompletionObserved {
    graph: PeripheralConnectionMemoryGraphCompletionObserved,
    event: LePeripheralConnectionEventInFlight,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
    recurring_phase: PeripheralConnectionRecurringPhase,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionFirstEventCompletionObserved {
    pub(super) const fn new(
        graph: PeripheralConnectionMemoryGraphCompletionObserved,
        event: LePeripheralConnectionEventInFlight,
        first_window: PeripheralConnectionFirstWindow,
        requested_window: SchedulerRawWindow,
        resolved_window: SchedulerRawWindow,
        recurring_phase: PeripheralConnectionRecurringPhase,
    ) -> Self {
        Self {
            graph,
            event,
            first_window,
            requested_window,
            resolved_window,
            recurring_phase,
        }
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_item_address()
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure retains the completed event and removal proof"
    )]
    pub(crate) fn prepare_recycle(
        self,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<
        PeripheralConnectionCompletionRecyclePrepared,
        PeripheralConnectionCompletionRecycleFailure,
    > {
        let Self {
            graph,
            event,
            first_window,
            requested_window,
            resolved_window,
            recurring_phase,
        } = self;
        match graph.prepare_recycle_after_software_list_removal(removal) {
            Ok(graph) => Ok(PeripheralConnectionCompletionRecyclePrepared {
                graph,
                event,
                first_window,
                requested_window,
                resolved_window,
                recurring_phase,
            }),
            Err(failure) => {
                let error = failure.error();
                let (graph, removal) = failure.into_parts();
                Err(PeripheralConnectionCompletionRecycleFailure {
                    error,
                    completed: Self {
                        graph,
                        event,
                        first_window,
                        requested_window,
                        resolved_window,
                        recurring_phase,
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
pub(crate) struct PeripheralConnectionCompletionRecyclePrepared {
    graph: PeripheralConnectionMemoryGraphRecyclePrepared,
    event: LePeripheralConnectionEventInFlight,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
    recurring_phase: PeripheralConnectionRecurringPhase,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionCompletionRecyclePrepared {
    pub(crate) fn into_parts(
        self,
    ) -> (
        PeripheralConnectionFirstEventCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        let (graph, removal) = self.graph.into_parts();
        (
            PeripheralConnectionFirstEventCompletionObserved {
                graph,
                event: self.event,
                first_window: self.first_window,
                requested_window: self.requested_window,
                resolved_window: self.resolved_window,
                recurring_phase: self.recurring_phase,
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
        PeripheralConnectionCompletionRxExtracted,
        PeripheralConnectionCompletionRxExtractionFailure,
    > {
        let Self {
            graph,
            event,
            first_window,
            requested_window,
            resolved_window,
            recurring_phase,
        } = self;
        match graph.extract_received() {
            Ok(graph) => Ok(PeripheralConnectionCompletionRxExtracted {
                graph,
                event,
                first_window,
                requested_window,
                resolved_window,
                recurring_phase,
            }),
            Err(failure) => Err(PeripheralConnectionCompletionRxExtractionFailure {
                error: failure.error(),
                prepared: Self {
                    graph: failure.into_prepared(),
                    event,
                    first_window,
                    requested_window,
                    resolved_window,
                    recurring_phase,
                },
            }),
        }
    }
}

/// Lower recycle mismatch retaining the exact completion and removal proof.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed connection event and removal proof remain owned"]
pub(crate) struct PeripheralConnectionCompletionRecycleFailure {
    error: PeripheralConnectionMemoryGraphRecycleError,
    completed: PeripheralConnectionFirstEventCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionCompletionRecycleFailure {
    pub(crate) const fn error(&self) -> PeripheralConnectionMemoryGraphRecycleError {
        self.error
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        PeripheralConnectionFirstEventCompletionObserved,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.completed, self.removal)
    }
}

/// Malformed RX result retaining the entire uncommitted recycle transaction.
#[cfg(target_arch = "riscv32")]
#[must_use = "the failed RX transaction must enter fail-stop handling or be retained"]
pub(crate) struct PeripheralConnectionCompletionRxExtractionFailure {
    error: LeRxError,
    prepared: PeripheralConnectionCompletionRecyclePrepared,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionCompletionRxExtractionFailure {
    pub(crate) const fn error(&self) -> LeRxError {
        self.error
    }

    pub(crate) fn into_prepared(self) -> PeripheralConnectionCompletionRecyclePrepared {
        self.prepared
    }
}

/// Copied RX result joined to the uncommitted connection recycle owner.
#[cfg(target_arch = "riscv32")]
#[must_use = "the event resources must be committed before connection recurrence"]
pub(crate) struct PeripheralConnectionCompletionRxExtracted {
    graph: PeripheralConnectionMemoryGraphRxExtracted,
    event: LePeripheralConnectionEventInFlight,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
    recurring_phase: PeripheralConnectionRecurringPhase,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionCompletionRxExtracted {
    pub(crate) fn into_prepared(self) -> PeripheralConnectionCompletionRecyclePrepared {
        PeripheralConnectionCompletionRecyclePrepared {
            graph: self.graph.into_prepared(),
            event: self.event,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
            recurring_phase: self.recurring_phase,
        }
    }

    pub(crate) fn commit(self) -> PeripheralConnectionRecycledEvent {
        let (graph, batch, status, capture) = self.graph.commit().into_parts();
        PeripheralConnectionRecycledEvent {
            graph,
            event: self.event,
            batch,
            status,
            capture,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
            recurring_phase: self.recurring_phase,
        }
    }
}

/// CPU-owned active connection after event-local SRAM reclamation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the active connection must classify completion before LL advance"]
pub(crate) struct PeripheralConnectionRecycledEvent {
    graph: PeripheralConnectionMemoryGraphActiveCpuOwned,
    event: LePeripheralConnectionEventInFlight,
    batch: LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
    status: PeripheralConnectionSchedulerItemCompletionStatus,
    capture: PeripheralConnectionCapturedAnchorAvailability,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
    recurring_phase: PeripheralConnectionRecurringPhase,
}

#[cfg(any(target_arch = "riscv32", test))]
enum PeripheralConnectionCaptureCompletion<T> {
    Complete {
        activity: LePeripheralConnectionEventPeerActivity,
        packet_start: Option<T>,
    },
    NormalizationUnavailable,
}

#[cfg(any(target_arch = "riscv32", test))]
fn classify_peripheral_connection_capture<C, T>(
    capture: Option<C>,
    received_packet: bool,
    normalize: impl FnOnce(C) -> Option<T>,
) -> PeripheralConnectionCaptureCompletion<T> {
    let Some(captured) = capture else {
        return PeripheralConnectionCaptureCompletion::Complete {
            activity: if received_packet {
                LePeripheralConnectionEventPeerActivity::Observed
            } else {
                LePeripheralConnectionEventPeerActivity::Missed
            },
            packet_start: None,
        };
    };
    match normalize(captured) {
        Some(packet_start) => PeripheralConnectionCaptureCompletion::Complete {
            activity: LePeripheralConnectionEventPeerActivity::Observed,
            packet_start: Some(packet_start),
        },
        None => PeripheralConnectionCaptureCompletion::NormalizationUnavailable,
    }
}

/// Result of classifying one recycled connection event.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed owner or unchanged retry owner must be retained"]
pub(crate) enum PeripheralConnectionCompletionClassification {
    NormalizationUnavailable(PeripheralConnectionRecycledEvent),
    Completed(PeripheralConnectionCompletedEvent),
}

/// Closed portable event retaining every reclaimed S31 connection resource.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed connection owner must enter recurrence or teardown"]
pub(crate) struct PeripheralConnectionCompletedEvent {
    graph: PeripheralConnectionMemoryGraphActiveCpuOwned,
    event: LePeripheralConnectionEventCompleted,
    batch: LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
    status: PeripheralConnectionSchedulerItemCompletionStatus,
    packet_start: Option<PeripheralConnectionPacketStartTiming>,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
    recurring_phase: PeripheralConnectionRecurringPhase,
}

/// Opaque observations retained while recurrence provisionally owns the active
/// memory graph and portable completion.
///
/// Keeping this remainder in the completion module makes it impossible for the
/// recurring preparation path to reconstruct only a subset of the completed
/// event after cancellation.
#[cfg(target_arch = "riscv32")]
pub(crate) struct PeripheralConnectionCompletedEventRecurringRemainder {
    batch: LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
    status: PeripheralConnectionSchedulerItemCompletionStatus,
    packet_start: Option<PeripheralConnectionPacketStartTiming>,
    first_window: PeripheralConnectionFirstWindow,
    requested_window: SchedulerRawWindow,
    resolved_window: SchedulerRawWindow,
    recurring_phase: PeripheralConnectionRecurringPhase,
}

/// Closed-event components temporarily held by one combined recurrence owner.
#[cfg(target_arch = "riscv32")]
pub(crate) struct PeripheralConnectionCompletedEventRecurringParts {
    pub(crate) graph: PeripheralConnectionMemoryGraphActiveCpuOwned,
    pub(crate) event: LePeripheralConnectionEventCompleted,
    pub(crate) remainder: PeripheralConnectionCompletedEventRecurringRemainder,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionCompletedEventRecurringRemainder {
    pub(crate) const fn packet_start(&self) -> Option<&PeripheralConnectionPacketStartTiming> {
        self.packet_start.as_ref()
    }

    pub(crate) fn join_recurring_rx_publication(
        self,
        graph: PeripheralConnectionMemoryGraphRxPublished,
        event: LePeripheralConnectionEventPrepared,
        recurring_phase: PeripheralConnectionRecurringPhase,
    ) -> PeripheralConnectionFirstEventRxPublished {
        PeripheralConnectionFirstEventRxPublished {
            graph,
            event,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
            recurring_phase,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionCompletedEvent {
    pub(crate) fn receive_time(
        &self,
    ) -> oer_esp32s31_bluetooth_memory::PeripheralConnectionReceiveTime {
        self.graph.receive_time()
    }

    pub(crate) fn retire(
        self,
        runtime: &mut super::PeripheralConnectionRuntimeResources,
    ) -> core::ops::ControlFlow<Self> {
        match runtime.retire_active(self.graph) {
            Ok(()) => core::ops::ControlFlow::Continue(()),
            Err(graph) => core::ops::ControlFlow::Break(Self { graph, ..self }),
        }
    }

    pub(crate) fn process_control(
        &mut self,
        control: &mut oer_bluetooth_ll::control::LePeripheralControl,
        encryption: &mut oer_bluetooth_ll::security::LePeripheralEncryptionProcedure,
        acl: &mut super::super::active::acl::PeripheralConnectionAcl,
        version: Option<oer_bluetooth_ll::control::LeVersionInformation>,
        random: &mut dyn super::super::PeripheralEncryptionRandomSource,
    ) -> Result<bool, oer_bluetooth_ll::control::LePeripheralControlError> {
        let acknowledged = self.graph.reclaim_transmission();
        control.observe_transmission_completion(acknowledged);
        encryption.observe_transmission_completion(acknowledged);
        acl.observe_transmission_completion(acknowledged);
        #[cfg(feature = "dtm-diagnostics")]
        super::super::diagnostics::record_received(
            &self.batch,
            acknowledged,
            self.event.channel_map_updated(),
        );
        #[cfg(not(feature = "dtm-diagnostics"))]
        let _ = acknowledged;
        for index in 0..self.batch.len() {
            let packet = *self.batch.packet(index).expect("bounded RX batch");
            let pdu = packet.as_bytes();
            let header = pdu[0];
            let payload = &pdu[2..];
            use oer_bluetooth_ll::security::{
                LePeripheralEncryptedReceive as EncryptedReceive,
                LePeripheralEncryptionReceiveMode as ReceiveMode,
            };
            match encryption.receive_mode() {
                ReceiveMode::Plaintext => {
                    if header & 0x03 == 0x03 && payload.first() == Some(&0x03) {
                        if payload.len() != 23
                            || encryption
                                .begin(payload, random.next_encryption_random())
                                .is_err()
                        {
                            encryption.fail_unexpected_physical_channel_pdu();
                        }
                    } else {
                        self.dispatch_plaintext(control, acl, pdu, version)?;
                    }
                }
                ReceiveMode::EncryptedStartResponse => {
                    let mut encrypted = [0; u8::MAX as usize];
                    encrypted[..payload.len()].copy_from_slice(payload);
                    if encryption
                        .receive_encrypted_start_response(header, &mut encrypted[..payload.len()])
                        .is_err()
                    {
                        encryption.fail_unexpected_physical_channel_pdu();
                    }
                }
                ReceiveMode::Encrypted => {
                    let mut plaintext = [0; u8::MAX as usize];
                    plaintext[..payload.len()].copy_from_slice(payload);
                    match encryption.receive_active_packet(header, &mut plaintext[..payload.len()])
                    {
                        Ok(EncryptedReceive::PauseRequest) => {}
                        Ok(EncryptedReceive::Plaintext { length }) => {
                            let mut decoded = [0; u8::MAX as usize + 2];
                            decoded[0] = header;
                            decoded[1] = length as u8;
                            decoded[2..2 + length].copy_from_slice(&plaintext[..length]);
                            self.dispatch_plaintext(control, acl, &decoded[..2 + length], version)?;
                        }
                        Err(_) => encryption.fail_unexpected_physical_channel_pdu(),
                    }
                }
                ReceiveMode::UnencryptedPauseResponse => {
                    if header & 0x03 != 0x03
                        || encryption
                            .receive_unencrypted_pause_response(payload)
                            .is_err()
                    {
                        encryption.fail_unexpected_physical_channel_pdu();
                    }
                }
                ReceiveMode::RestartEncryptionRequest => {
                    let valid = header & 0x03 == 0x03
                        && payload.len() == 23
                        && payload.first() == Some(&0x03);
                    if !valid
                        || encryption
                            .begin(payload, random.next_encryption_random())
                            .is_err()
                    {
                        encryption.fail_unexpected_physical_channel_pdu();
                    }
                }
                ReceiveMode::Blocked => encryption.fail_unexpected_physical_channel_pdu(),
            }
        }
        Ok(self.enqueue_transmission(control, encryption, acl))
    }

    fn dispatch_plaintext(
        &mut self,
        control: &mut oer_bluetooth_ll::control::LePeripheralControl,
        acl: &mut super::super::active::acl::PeripheralConnectionAcl,
        pdu: &[u8],
        version: Option<oer_bluetooth_ll::control::LeVersionInformation>,
    ) -> Result<(), oer_bluetooth_ll::control::LePeripheralControlError> {
        match control.receive(pdu, version)? {
            oer_bluetooth_ll::control::LePeripheralReceive::Data(fragment) => {
                acl.accept_controller_fragment(fragment);
            }
            oer_bluetooth_ll::control::LePeripheralReceive::ChannelMapUpdate(update) => {
                self.event
                    .schedule_channel_map_update(update.channel_map(), update.instant())
                    .map_err(
                        oer_bluetooth_ll::control::LePeripheralControlError::ChannelMapUpdate,
                    )?;
            }
            oer_bluetooth_ll::control::LePeripheralReceive::ConnectionUpdate(update) => {
                self.event
                    .schedule_connection_update(update.timing(), update.instant())
                    .map_err(
                        oer_bluetooth_ll::control::LePeripheralControlError::ConnectionUpdate,
                    )?;
            }
            oer_bluetooth_ll::control::LePeripheralReceive::Control => {}
        }
        Ok(())
    }

    /// Fill the reclaimed TX allocation from the current control/ACL priority.
    ///
    /// This is separate from RX dispatch so a command accepted while the next
    /// RUN is held behind its receive reservation can still enter the exact
    /// CPU-owned graph once, under a freshly sampled procedure deadline.
    pub(crate) fn enqueue_transmission(
        &mut self,
        control: &mut oer_bluetooth_ll::control::LePeripheralControl,
        encryption: &mut oer_bluetooth_ll::security::LePeripheralEncryptionProcedure,
        acl: &mut super::super::active::acl::PeripheralConnectionAcl,
    ) -> bool {
        if !self.graph.can_enqueue_transmission() {
            return false;
        }
        if let Some(response) = encryption.pending_response()
            && self
                .graph
                .enqueue_control_transmission(response.as_bytes())
                .expect("an encryption control response fits the connection TX allocation")
        {
            encryption
                .response_enqueued()
                .expect("the retained response matches the encryption procedure state");
            return true;
        }
        if encryption.blocks_unrelated_transmission() {
            return false;
        }
        if let Some(response) = control.pending_response() {
            let mut encrypted = [0; 27];
            let payload = if let Some(cipher) = encryption.active_encryption() {
                let plaintext_len = response.as_bytes().len();
                encrypted[..plaintext_len].copy_from_slice(response.as_bytes());
                let encrypted_len = cipher
                    .encrypt_new_packet(0x03, &mut encrypted, plaintext_len)
                    .expect("legacy control responses fit encrypted packet storage");
                &encrypted[..encrypted_len]
            } else {
                response.as_bytes()
            };
            if self
                .graph
                .enqueue_control_transmission(payload)
                .expect("portable control response fits the connection TX allocation")
            {
                control.response_enqueued();
                #[cfg(feature = "dtm-diagnostics")]
                super::super::diagnostics::record_enqueued();
                return true;
            }
        } else {
            let maximum = if encryption.is_active() {
                oer_bluetooth_ll::security::LE_LEGACY_ENCRYPTED_PLAINTEXT_BYTES
            } else {
                super::super::active::acl::LEGACY_LE_DATA_PAYLOAD_CAPACITY
            };
            if let Some(fragment) = acl.next_fragment(maximum) {
                let length = fragment.payload().len();
                let continuing = fragment.is_continuing();
                let mut encrypted = [0; 27];
                let payload = if let Some(cipher) = encryption.active_encryption() {
                    encrypted[..length].copy_from_slice(fragment.payload());
                    let header = if continuing { 0x01 } else { 0x02 };
                    let encrypted_len = cipher
                        .encrypt_new_packet(header, &mut encrypted, length)
                        .expect("a legacy ACL fragment fits encrypted packet storage");
                    &encrypted[..encrypted_len]
                } else {
                    fragment.payload()
                };
                if self
                    .graph
                    .enqueue_acl_transmission(continuing, payload)
                    .expect("a legacy ACL fragment fits the connection TX allocation")
                {
                    acl.fragment_enqueued(length);
                    return true;
                }
            } else if let Some(cipher) = encryption.active_encryption() {
                let mut encrypted = [0; oer_bluetooth_ll::security::LE_ACL_MIC_BYTES];
                let encrypted_len = cipher
                    .encrypt_new_packet(0x01, &mut encrypted, 0)
                    .expect("an encrypted empty Data PDU is exactly one MIC");
                return self
                    .graph
                    .enqueue_acl_transmission(true, &encrypted[..encrypted_len])
                    .expect("an encrypted empty Data PDU fits the connection TX allocation");
            }
        }
        false
    }

    pub(crate) const fn link_layer_completion(&self) -> &LePeripheralConnectionEventCompleted {
        &self.event
    }

    pub(crate) const fn status(&self) -> PeripheralConnectionSchedulerItemCompletionStatus {
        self.status
    }

    pub(crate) const fn received(&self) -> &LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT> {
        &self.batch
    }

    /// Host ACL slots required before this completed batch can be consumed.
    /// LL control, malformed input, and empty acknowledgements require no slot
    /// and must remain processable while Controller ACL output is backpressured.
    pub(crate) fn required_controller_acl_slots(&self) -> usize {
        (0..self.batch.len())
            .filter(|&index| {
                self.batch.packet(index).is_some_and(|packet| {
                    oer_bluetooth_ll::control::LePeripheralControl::receive_requires_acl_slot(
                        packet.as_bytes(),
                    )
                    .unwrap_or(false)
                })
            })
            .count()
    }

    pub(crate) const fn packet_start(&self) -> Option<&PeripheralConnectionPacketStartTiming> {
        self.packet_start.as_ref()
    }

    /// Nominal phase committed by the event which just completed.
    pub(crate) const fn recurring_phase(&self) -> PeripheralConnectionRecurringPhase {
        self.recurring_phase
    }

    /// Split only for the combined recurring preparation transaction.
    pub(crate) fn into_recurring_parts(self) -> PeripheralConnectionCompletedEventRecurringParts {
        PeripheralConnectionCompletedEventRecurringParts {
            graph: self.graph,
            event: self.event,
            remainder: PeripheralConnectionCompletedEventRecurringRemainder {
                batch: self.batch,
                status: self.status,
                packet_start: self.packet_start,
                first_window: self.first_window,
                requested_window: self.requested_window,
                resolved_window: self.resolved_window,
                recurring_phase: self.recurring_phase,
            },
        }
    }

    /// Rejoin the exact components returned by [`Self::into_recurring_parts`].
    pub(crate) fn from_recurring_parts(
        parts: PeripheralConnectionCompletedEventRecurringParts,
    ) -> Self {
        let PeripheralConnectionCompletedEventRecurringParts {
            graph,
            event,
            remainder,
        } = parts;
        let PeripheralConnectionCompletedEventRecurringRemainder {
            batch,
            status,
            packet_start,
            first_window,
            requested_window,
            resolved_window,
            recurring_phase,
        } = remainder;
        Self {
            graph,
            event,
            batch,
            status,
            packet_start,
            first_window,
            requested_window,
            resolved_window,
            recurring_phase,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionRecycledEvent {
    fn complete(
        self,
        activity: LePeripheralConnectionEventPeerActivity,
        packet_start: Option<PeripheralConnectionPacketStartTiming>,
    ) -> PeripheralConnectionCompletedEvent {
        PeripheralConnectionCompletedEvent {
            graph: self.graph,
            event: self.event.complete(activity),
            batch: self.batch,
            status: self.status,
            packet_start,
            first_window: self.first_window,
            requested_window: self.requested_window,
            resolved_window: self.resolved_window,
            recurring_phase: self.recurring_phase,
        }
    }

    pub(crate) const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub(crate) const fn status(&self) -> PeripheralConnectionSchedulerItemCompletionStatus {
        self.status
    }

    pub(crate) const fn received(&self) -> &LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT> {
        &self.batch
    }

    pub(crate) fn classify_completion(
        self,
        normalize: impl FnOnce(
            PeripheralConnectionCapturedAnchorTime,
        ) -> Option<PeripheralConnectionPacketStartTiming>,
    ) -> PeripheralConnectionCompletionClassification {
        let capture = match self.capture {
            PeripheralConnectionCapturedAnchorAvailability::Absent => None,
            PeripheralConnectionCapturedAnchorAvailability::Available(captured) => Some(captured),
        };
        match classify_peripheral_connection_capture(capture, !self.batch.is_empty(), normalize) {
            PeripheralConnectionCaptureCompletion::Complete {
                activity,
                packet_start,
            } => PeripheralConnectionCompletionClassification::Completed(
                self.complete(activity, packet_start),
            ),
            PeripheralConnectionCaptureCompletion::NormalizationUnavailable => {
                PeripheralConnectionCompletionClassification::NormalizationUnavailable(self)
            }
        }
    }
}

#[cfg(test)]
mod tests;
