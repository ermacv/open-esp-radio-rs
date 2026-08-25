//! Finite polled IEEE 802.15.4 energy-detection and CCA transactions.
//!
//! This closed engine owns no DMA buffer and creates no CPU interrupt route. A
//! transaction may temporarily enable exactly `ED_DONE` and `RX_ABORT` only
//! while its semantic backend proves that the CPU route is detached. A stale
//! event-status image fails closed before `ED_START`.
//!
//! Recovery samples and acknowledges the complete pending W1C image in one
//! backend transaction. Reuse is possible only when the image actually
//! consumed by that transaction is exactly lone `ED_DONE`. Abort, timeout,
//! conflicting status, acknowledgement drift, and backend failures mask the
//! window for containment but can never return a reusable owner.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), allow(dead_code))]

use core::convert::Infallible;

use crate::{ieee802154_lifecycle::Ieee802154Channel, ieee802154_policy::Ieee802154CcaMode};

/// Source-confirmed ED duration used by the standalone CCA entry path.
pub(crate) const IEEE802154_CCA_ED_DURATION: u16 = 8;

/// One finite operation supported by the interrupt-detached polling boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PolledOperation {
    /// Sample one uncalibrated signed ED RSS code.
    EnergyDetection {
        /// Checked 2.4 GHz channel selected before the command.
        channel: Ieee802154Channel,
        /// Complete source-level `uint16_t` ED-duration subset.
        duration: u16,
    },
    /// Sample the source-confirmed `CCA_BUSY` result.
    ClearChannelAssessment {
        /// Checked 2.4 GHz channel selected before the command.
        channel: Ieee802154Channel,
        /// Existing reviewed static CCA mode.
        mode: Ieee802154CcaMode,
        /// Signed source-level CCA threshold code.
        threshold_code: i8,
    },
}

impl Ieee802154PolledOperation {
    /// Construct one standalone ED request.
    pub(crate) const fn energy_detection(channel: Ieee802154Channel, duration: u16) -> Self {
        Self::EnergyDetection { channel, duration }
    }

    /// Construct one standalone CCA request.
    pub(crate) const fn clear_channel_assessment(
        channel: Ieee802154Channel,
        mode: Ieee802154CcaMode,
        threshold_code: i8,
    ) -> Self {
        Self::ClearChannelAssessment {
            channel,
            mode,
            threshold_code,
        }
    }

    /// Return the checked 2.4 GHz channel selected by this operation.
    pub const fn channel(self) -> Ieee802154Channel {
        match self {
            Self::EnergyDetection { channel, .. }
            | Self::ClearChannelAssessment { channel, .. } => channel,
        }
    }

    /// Return the configured ED duration, including the fixed reviewed value
    /// used by standalone CCA.
    pub const fn duration(self) -> u16 {
        match self {
            Self::EnergyDetection { duration, .. } => duration,
            Self::ClearChannelAssessment { .. } => IEEE802154_CCA_ED_DURATION,
        }
    }
}

/// Nonzero number of status samples permitted for one command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154OperationPollBudget(u32);

impl Ieee802154OperationPollBudget {
    /// Reject a zero-sample transaction before it owns the backend.
    pub const fn new(samples: u32) -> Option<Self> {
        if samples == 0 {
            None
        } else {
            Some(Self(samples))
        }
    }

    /// Return the exact maximum number of status samples.
    pub const fn samples(self) -> u32 {
        self.0
    }
}

/// Semantic readback of event-enable state, without a raw mask image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154OperationEventMaskState {
    /// Every MAC event remains masked.
    AllMasked,
    /// Exactly `ED_DONE` and `RX_ABORT` are enabled.
    EdDoneAndRxAbortOnly,
    /// At least one other event is enabled or one required event is missing.
    Unexpected,
}

/// Semantic readback of receive-abort-enable state, without a raw mask image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154OperationRxAbortMaskState {
    /// Every receive-abort reason remains masked.
    AllMasked,
    /// Exactly ED abort, ED stop, and ED coexistence reject are enabled.
    ///
    /// The source reason codes are 24, 25, and 26. This semantic type does not
    /// expose or accept their register-positioned bit image.
    EdOperationReasonsOnly,
    /// At least one other reason is enabled or one required reason is missing.
    Unexpected,
}

/// Complete non-acknowledging `EVENT_STATUS` observation.
///
/// Keeping the full register image is intentional: accepting only two booleans
/// would hide an unrelated latched event and could incorrectly permit reuse.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154OperationEventObservation(u16);

impl Ieee802154OperationEventObservation {
    const RX_ABORT: u16 = 0x0010;
    const ED_DONE: u16 = 0x0040;

    /// Preserve the complete source-confirmed 14-bit status image.
    pub(crate) const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    /// Return the complete retained status image.
    pub const fn raw(self) -> u16 {
        self.0
    }

    /// Return whether no event is latched.
    pub const fn is_clear(self) -> bool {
        self.0 == 0
    }

    /// Return whether this is exactly the one recoverable terminal status.
    const fn is_ed_done_only(self) -> bool {
        self.0 == Self::ED_DONE
    }

    /// Return whether `RX_ABORT` is present in the full observation.
    const fn has_rx_abort(self) -> bool {
        self.0 & Self::RX_ABORT != 0
    }

    /// Return whether `ED_DONE` is present in the full observation.
    const fn has_ed_done(self) -> bool {
        self.0 & Self::ED_DONE != 0
    }
}

/// Closed semantic backend for one serialized polled operation.
///
/// Implementations must retain exclusive ownership of the event-enable fields,
/// command leaf, ED result fields, and CPU-route attachment decision for the
/// complete transaction. Only `acknowledge_pending_events` may acknowledge
/// event status. It must sample the complete pending image once, consume that
/// exact non-replayable W1C snapshot, and return its semantic observation.
pub(crate) trait Ieee802154PolledOperationBackend: Sized {
    /// Backend-specific failure retained in a terminal quarantine owner.
    type Error;

    /// Select the checked channel through the existing channel policy.
    fn set_channel(&mut self, channel: Ieee802154Channel) -> Result<(), Self::Error>;

    /// Select the reviewed CCA mode.
    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) -> Result<(), Self::Error>;

    /// Select the signed CCA threshold code.
    fn set_cca_threshold_code(&mut self, threshold: i8) -> Result<(), Self::Error>;

    /// Replace the source-level ED duration subset.
    fn set_ed_duration(&mut self, duration: u16) -> Result<(), Self::Error>;

    /// Observe whether the MAC CPU interrupt route is detached.
    fn cpu_interrupt_route_is_detached(&mut self) -> Result<bool, Self::Error>;

    /// Sample the semantic event-enable state.
    fn operation_event_mask_state(
        &mut self,
    ) -> Result<Ieee802154OperationEventMaskState, Self::Error>;

    /// Sample the semantic receive-abort-enable state.
    fn operation_rx_abort_mask_state(
        &mut self,
    ) -> Result<Ieee802154OperationRxAbortMaskState, Self::Error>;

    /// Enable exactly `ED_DONE` and `RX_ABORT`, preserving every other event as masked.
    fn enable_ed_done_and_rx_abort(&mut self) -> Result<(), Self::Error>;

    /// Enable exactly ED abort, ED stop, and ED coexistence reject.
    fn enable_ed_operation_rx_abort_reasons(&mut self) -> Result<(), Self::Error>;

    /// Mask `ED_DONE` and `RX_ABORT` without touching or acknowledging status.
    fn mask_ed_done_and_rx_abort(&mut self) -> Result<(), Self::Error>;

    /// Mask the three ED-operation receive-abort reasons.
    fn mask_ed_operation_rx_abort_reasons(&mut self) -> Result<(), Self::Error>;

    /// Order policy and event-enable changes at the device boundary.
    fn order_device_accesses(&mut self) -> Result<(), Self::Error>;

    /// Issue the source-confirmed `ED_START` command.
    fn request_ed_start(&mut self) -> Result<(), Self::Error>;

    /// Sample the complete event status without acknowledging any bit.
    fn sample_event_status(&mut self) -> Result<Ieee802154OperationEventObservation, Self::Error>;

    /// Sample and acknowledge the complete pending W1C event image.
    fn acknowledge_pending_events(
        &mut self,
    ) -> Result<Ieee802154OperationEventObservation, Self::Error>;

    /// Retain the complete receive-abort status after `RX_ABORT`.
    fn sample_rx_abort_status(&mut self) -> Result<u32, Self::Error>;

    /// Sample the signed `ED_RSS` result after `ED_DONE`.
    fn sample_ed_rss_code(&mut self) -> Result<i8, Self::Error>;

    /// Sample `CCA_BUSY` after `ED_DONE`.
    fn sample_cca_busy(&mut self) -> Result<bool, Self::Error>;
}

/// Observable stage at which a one-shot owner entered quarantine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154OperationStage {
    /// Preconditions and static request fields.
    Prepare,
    /// Detached-route event-enable window.
    StartEventWindow,
    /// `ED_START` command publication.
    StartCommand,
    /// Non-acknowledging status poll.
    Poll,
    /// Terminal result sampling.
    TerminalSample,
    /// Snapshot-consuming W1C acknowledgement of the terminal event.
    AcknowledgeTerminalEvent,
    /// Terminal or timeout event remasking.
    Cleanup,
}

/// Why a transaction could not preserve the closed one-shot contract.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Ieee802154OperationQuarantineReason<Error> {
    /// One semantic backend operation failed.
    Backend {
        /// Stage containing the failed operation.
        stage: Ieee802154OperationStage,
        /// Backend-specific failure.
        error: Error,
    },
    /// The CPU route was not detached at a required checkpoint.
    CpuInterruptRouteAttached {
        /// Stage whose detached-route proof failed.
        stage: Ieee802154OperationStage,
    },
    /// Event enables did not match the exact expected state.
    UnexpectedEventMask {
        /// Stage whose event-mask proof failed.
        stage: Ieee802154OperationStage,
        /// Semantic mask state that was observed.
        observed: Ieee802154OperationEventMaskState,
    },
    /// Receive-abort enables did not match the exact expected state.
    UnexpectedRxAbortMask {
        /// Stage whose receive-abort-mask proof failed.
        stage: Ieee802154OperationStage,
        /// Semantic receive-abort mask state that was observed.
        observed: Ieee802154OperationRxAbortMaskState,
    },
    /// A nonzero event status was already latched before `ED_START`.
    StaleEventStatus {
        /// Complete status retained at the pre-command gate.
        observed: Ieee802154OperationEventObservation,
    },
    /// Nonzero status contained neither `RX_ABORT` nor exactly lone `ED_DONE`.
    UnexpectedTerminalStatus {
        /// Complete terminal status retained for diagnosis.
        observed: Ieee802154OperationEventObservation,
    },
    /// The W1C snapshot actually consumed was not exactly lone `ED_DONE`.
    UnexpectedAcknowledgedEvents {
        /// Complete acknowledged event image retained for diagnosis.
        observed: Ieee802154OperationEventObservation,
    },
    /// `ED_DONE` and `RX_ABORT` were observed together.
    ConflictingTerminalEvents {
        /// Complete conflicting status retained for diagnosis.
        observed: Ieee802154OperationEventObservation,
    },
}

/// Terminal quarantine retaining a backend that must not be reused.
#[must_use = "a quarantined IEEE 802.15.4 operation retains its one-shot backend"]
pub(crate) struct Ieee802154OperationQuarantined<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    backend: Backend,
    reason: Ieee802154OperationQuarantineReason<Backend::Error>,
}

impl<Backend> Ieee802154OperationQuarantined<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// Borrow the reason without exposing the retained backend.
    pub(crate) const fn reason(&self) -> &Ieee802154OperationQuarantineReason<Backend::Error> {
        &self.reason
    }
}

/// Serialized backend before any ED/CCA request has been prepared.
pub(crate) struct Ieee802154PolledOperationOwner<Backend> {
    backend: Backend,
}

impl<Backend> Ieee802154PolledOperationOwner<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// Bind a HAL-owned semantic backend without changing hardware.
    ///
    /// This crate-private hook is not an operational public constructor. The
    /// prepare transition still proves a detached CPU route and all events
    /// masked before any request field is changed.
    pub(crate) const fn from_semantic_backend(backend: Backend) -> Self {
        Self { backend }
    }

    /// Prepare request fields while all events remain masked.
    pub(crate) fn prepare(
        self,
        operation: Ieee802154PolledOperation,
        budget: Ieee802154OperationPollBudget,
    ) -> Result<Ieee802154PolledOperationPrepared<Backend>, Ieee802154OperationQuarantined<Backend>>
    {
        let mut backend = self.backend;
        if let Err(reason) = require_detached_route(&mut backend, Ieee802154OperationStage::Prepare)
        {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::Prepare,
            Ieee802154OperationEventMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::Prepare,
            Ieee802154OperationRxAbortMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }

        if let Err(error) = backend.set_channel(operation.channel()) {
            return Err(quarantined(
                backend,
                backend_error(Ieee802154OperationStage::Prepare, error),
            ));
        }
        if let Ieee802154PolledOperation::ClearChannelAssessment {
            mode,
            threshold_code,
            ..
        } = operation
        {
            if let Err(error) = backend.set_cca_mode(mode) {
                return Err(quarantined(
                    backend,
                    backend_error(Ieee802154OperationStage::Prepare, error),
                ));
            }
            if let Err(error) = backend.set_cca_threshold_code(threshold_code) {
                return Err(quarantined(
                    backend,
                    backend_error(Ieee802154OperationStage::Prepare, error),
                ));
            }
        }
        if let Err(error) = backend.set_ed_duration(operation.duration()) {
            return Err(quarantined(
                backend,
                backend_error(Ieee802154OperationStage::Prepare, error),
            ));
        }
        if let Err(error) = backend.order_device_accesses() {
            return Err(quarantined(
                backend,
                backend_error(Ieee802154OperationStage::Prepare, error),
            ));
        }
        if let Err(reason) = require_detached_route(&mut backend, Ieee802154OperationStage::Prepare)
        {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::Prepare,
            Ieee802154OperationEventMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::Prepare,
            Ieee802154OperationRxAbortMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }

        Ok(Ieee802154PolledOperationPrepared {
            backend,
            operation,
            budget,
        })
    }
}

/// Prepared one-shot operation whose events are still completely masked.
pub(crate) struct Ieee802154PolledOperationPrepared<Backend> {
    backend: Backend,
    operation: Ieee802154PolledOperation,
    budget: Ieee802154OperationPollBudget,
}

impl<Backend> Ieee802154PolledOperationPrepared<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// Open the exact detached-route event window and issue `ED_START`.
    pub(crate) fn start(
        self,
    ) -> Result<Ieee802154PolledOperationActive<Backend>, Ieee802154OperationQuarantined<Backend>>
    {
        let mut backend = self.backend;
        if let Err(reason) =
            require_detached_route(&mut backend, Ieee802154OperationStage::StartEventWindow)
        {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::StartEventWindow,
            Ieee802154OperationEventMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::StartEventWindow,
            Ieee802154OperationRxAbortMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }
        if let Err(error) = backend.enable_ed_done_and_rx_abort() {
            return Err(quarantined_after_cleanup(
                backend,
                backend_error(Ieee802154OperationStage::StartEventWindow, error),
            ));
        }
        if let Err(error) = backend.enable_ed_operation_rx_abort_reasons() {
            return Err(quarantined_after_cleanup(
                backend,
                backend_error(Ieee802154OperationStage::StartEventWindow, error),
            ));
        }
        if let Err(error) = backend.order_device_accesses() {
            return Err(quarantined_after_cleanup(
                backend,
                backend_error(Ieee802154OperationStage::StartEventWindow, error),
            ));
        }
        if let Err(reason) =
            require_detached_route(&mut backend, Ieee802154OperationStage::StartEventWindow)
        {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::StartEventWindow,
            Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly,
        ) {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::StartEventWindow,
            Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly,
        ) {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        let status = match backend.sample_event_status() {
            Ok(status) => status,
            Err(error) => {
                return Err(quarantined_after_cleanup(
                    backend,
                    backend_error(Ieee802154OperationStage::StartEventWindow, error),
                ));
            }
        };
        if !status.is_clear() {
            return Err(quarantined_after_cleanup(
                backend,
                Ieee802154OperationQuarantineReason::StaleEventStatus { observed: status },
            ));
        }
        if let Err(error) = backend.request_ed_start() {
            return Err(quarantined_after_cleanup(
                backend,
                backend_error(Ieee802154OperationStage::StartCommand, error),
            ));
        }

        Ok(Ieee802154PolledOperationActive {
            backend,
            operation: self.operation,
            remaining_polls: self.budget.samples(),
            polls: 0,
        })
    }
}

/// Active detached-route polled command retaining the only backend owner.
pub(crate) struct Ieee802154PolledOperationActive<Backend> {
    backend: Backend,
    operation: Ieee802154PolledOperation,
    remaining_polls: u32,
    polls: u32,
}

/// One accepted result sampled after a lone `ED_DONE`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PolledOperationResult {
    /// Uncalibrated signed `ED_RSS` code.
    EnergyDetection { rss_code: i8 },
    /// Source-confirmed `CCA_BUSY` classification.
    ClearChannelAssessment { busy: bool },
}

/// Evidence preserved for one successfully completed operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PolledOperationEvidence {
    operation: Ieee802154PolledOperation,
    result: Ieee802154PolledOperationResult,
    polls: u32,
}

impl Ieee802154PolledOperationEvidence {
    /// Return the exact prepared operation.
    pub const fn operation(self) -> Ieee802154PolledOperation {
        self.operation
    }

    /// Return the sampled terminal result.
    pub const fn result(self) -> Ieee802154PolledOperationResult {
        self.result
    }

    /// Return the number of non-acknowledging event samples consumed.
    pub const fn polls(self) -> u32 {
        self.polls
    }
}

/// Successful result owner while the detached event window remains active.
#[must_use = "a completed operation must be recovered before its backend can be reused"]
pub(crate) struct Ieee802154PolledOperationCompleted<Backend> {
    backend: Backend,
    evidence: Ieee802154PolledOperationEvidence,
}

impl<Backend> Ieee802154PolledOperationCompleted<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// Borrow the successful evidence before recovery.
    pub(crate) const fn evidence(&self) -> &Ieee802154PolledOperationEvidence {
        &self.evidence
    }

    /// Consume the complete pending W1C snapshot, require exactly `ED_DONE`,
    /// and close the event window before returning a reusable owner.
    pub(crate) fn recover(
        self,
    ) -> Result<Ieee802154PolledOperationRecovered<Backend>, Ieee802154OperationQuarantined<Backend>>
    {
        let mut backend = self.backend;
        if let Err(reason) = require_detached_route(
            &mut backend,
            Ieee802154OperationStage::AcknowledgeTerminalEvent,
        ) {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::AcknowledgeTerminalEvent,
            Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly,
        ) {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::AcknowledgeTerminalEvent,
            Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly,
        ) {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        let acknowledged = match backend.acknowledge_pending_events() {
            Ok(acknowledged) => acknowledged,
            Err(error) => {
                return Err(quarantined_after_cleanup(
                    backend,
                    backend_error(Ieee802154OperationStage::AcknowledgeTerminalEvent, error),
                ));
            }
        };
        if !acknowledged.is_ed_done_only() {
            return Err(quarantined_after_cleanup(
                backend,
                Ieee802154OperationQuarantineReason::UnexpectedAcknowledgedEvents {
                    observed: acknowledged,
                },
            ));
        }

        let backend = cleanup(backend)?;
        Ok(Ieee802154PolledOperationRecovered {
            owner: Ieee802154PolledOperationOwner { backend },
            evidence: self.evidence,
        })
    }
}

/// Successful evidence paired with the only reusable operation owner.
#[must_use = "a recovered operation contains the reusable IEEE 802.15.4 owner"]
pub(crate) struct Ieee802154PolledOperationRecovered<Backend> {
    owner: Ieee802154PolledOperationOwner<Backend>,
    evidence: Ieee802154PolledOperationEvidence,
}

impl<Backend> Ieee802154PolledOperationRecovered<Backend> {
    /// Borrow the successful evidence retained across recovery.
    pub(crate) const fn evidence(&self) -> &Ieee802154PolledOperationEvidence {
        &self.evidence
    }

    /// Consume successful evidence and return the reusable serialized owner.
    pub(crate) fn into_owner(self) -> Ieee802154PolledOperationOwner<Backend> {
        self.owner
    }
}

/// Evidence retained when the operation's `RX_ABORT` event was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PolledOperationAbortEvidence {
    operation: Ieee802154PolledOperation,
    event_status: Ieee802154OperationEventObservation,
    rx_abort_status: u32,
    polls: u32,
}

impl Ieee802154PolledOperationAbortEvidence {
    /// Return the operation that aborted.
    pub const fn operation(self) -> Ieee802154PolledOperation {
        self.operation
    }

    /// Return the complete event status observed at abort.
    pub const fn event_status(self) -> Ieee802154OperationEventObservation {
        self.event_status
    }

    /// Return the complete receive-abort status sampled before containment.
    pub const fn rx_abort_status(self) -> u32 {
        self.rx_abort_status
    }

    /// Return the number of completed event-status samples.
    pub const fn polls(self) -> u32 {
        self.polls
    }
}

/// Aborted owner after event and abort masks were closed for containment.
///
/// No event acknowledgement or `STOP` was issued, and no recovery transition
/// is exposed.
#[must_use = "an aborted operation retains a non-reusable backend"]
pub(crate) struct Ieee802154PolledOperationAborted<Backend> {
    backend: Backend,
    evidence: Ieee802154PolledOperationAbortEvidence,
}

impl<Backend> Ieee802154PolledOperationAborted<Backend> {
    /// Borrow the complete abort evidence.
    pub(crate) const fn evidence(&self) -> &Ieee802154PolledOperationAbortEvidence {
        &self.evidence
    }
}

/// Timed-out one-shot owner after operation events were remasked.
///
/// No `STOP` or hardware-idle claim is made, so the backend is intentionally
/// retained without a recovery transition.
#[must_use = "a timed-out operation may still be active in hardware"]
pub(crate) struct Ieee802154PolledOperationTimeout<Backend> {
    backend: Backend,
    operation: Ieee802154PolledOperation,
    polls: u32,
}

impl<Backend> Ieee802154PolledOperationTimeout<Backend> {
    /// Return the operation that exhausted its poll budget.
    pub(crate) const fn operation(&self) -> Ieee802154PolledOperation {
        self.operation
    }

    /// Return the exact number of completed status samples.
    pub(crate) const fn polls(&self) -> u32 {
        self.polls
    }
}

/// Result of one finite status poll.
pub(crate) enum Ieee802154PolledOperationPoll<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// No terminal event was observed and budget remains.
    Pending(Ieee802154PolledOperationActive<Backend>),
    /// Lone `ED_DONE` was observed and result fields were sampled.
    Completed(Ieee802154PolledOperationCompleted<Backend>),
    /// `RX_ABORT` and its complete diagnostic evidence were observed.
    Aborted(Ieee802154PolledOperationAborted<Backend>),
    /// The exact sample budget was exhausted without a terminal event.
    Timeout(Ieee802154PolledOperationTimeout<Backend>),
    /// An invariant or backend operation failed.
    Quarantined(Ieee802154OperationQuarantined<Backend>),
}

/// Terminal reason why a production polled operation cannot return a reusable
/// IEEE 802.15.4 owner.
///
/// Abort and timeout deliberately remain terminal: no source proves that a
/// generic `STOP` makes an in-flight MAC operation synchronously reusable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PolledOperationFailure {
    /// `RX_ABORT` completed the operation; full status evidence is retained.
    Aborted(Ieee802154PolledOperationAbortEvidence),
    /// The exact status-sample budget expired without a terminal event.
    Timeout {
        /// Operation which may still be active in hardware.
        operation: Ieee802154PolledOperation,
        /// Exact number of completed status observations.
        polls: u32,
    },
    /// A CPU interrupt route was not completely at reset.
    CpuInterruptRouteAttached {
        /// Checkpoint which detected the attached or unexpected route image.
        stage: Ieee802154OperationStage,
    },
    /// Event enables differed from the exact state required at a checkpoint.
    UnexpectedEventMask {
        /// Checkpoint whose proof failed.
        stage: Ieee802154OperationStage,
        /// Complete semantic classification observed there.
        observed: Ieee802154OperationEventMaskState,
    },
    /// Receive-abort enables differed from the exact required state.
    UnexpectedRxAbortMask {
        /// Checkpoint whose proof failed.
        stage: Ieee802154OperationStage,
        /// Complete semantic classification observed there.
        observed: Ieee802154OperationRxAbortMaskState,
    },
    /// A nonzero event was already pending before `ED_START`.
    StaleEventStatus {
        /// Complete fourteen-bit pre-command status.
        observed: Ieee802154OperationEventObservation,
    },
    /// A nonzero status was neither `RX_ABORT` nor lone `ED_DONE`.
    UnexpectedTerminalStatus {
        /// Complete fourteen-bit terminal status.
        observed: Ieee802154OperationEventObservation,
    },
    /// The W1C snapshot actually consumed was not exactly lone `ED_DONE`.
    UnexpectedAcknowledgedEvents {
        /// Complete fourteen-bit acknowledged status.
        observed: Ieee802154OperationEventObservation,
    },
    /// `ED_DONE` and `RX_ABORT` were observed in the same status sample.
    ConflictingTerminalEvents {
        /// Complete fourteen-bit conflicting status.
        observed: Ieee802154OperationEventObservation,
    },
}

/// Run one infallible semantic backend through prepare, start, finite polling,
/// and exact recovery.
///
/// Concrete MMIO backends are infallible at the Rust call boundary; all
/// hardware divergence is represented by readback-based failure variants.
pub(crate) fn run_ieee802154_polled_operation<Backend>(
    backend: Backend,
    operation: Ieee802154PolledOperation,
    budget: Ieee802154OperationPollBudget,
) -> Result<Ieee802154PolledOperationEvidence, Ieee802154PolledOperationFailure>
where
    Backend: Ieee802154PolledOperationBackend<Error = Infallible>,
{
    let prepared = Ieee802154PolledOperationOwner::from_semantic_backend(backend)
        .prepare(operation, budget)
        .map_err(public_failure_from_quarantine)?;
    let mut active = prepared.start().map_err(public_failure_from_quarantine)?;

    loop {
        match active.poll() {
            Ieee802154PolledOperationPoll::Pending(next) => active = next,
            Ieee802154PolledOperationPoll::Completed(completed) => {
                let recovered = completed
                    .recover()
                    .map_err(public_failure_from_quarantine)?;
                let evidence = *recovered.evidence();
                drop(recovered.into_owner());
                return Ok(evidence);
            }
            Ieee802154PolledOperationPoll::Aborted(aborted) => {
                return Err(Ieee802154PolledOperationFailure::Aborted(
                    *aborted.evidence(),
                ));
            }
            Ieee802154PolledOperationPoll::Timeout(timeout) => {
                return Err(Ieee802154PolledOperationFailure::Timeout {
                    operation: timeout.operation(),
                    polls: timeout.polls(),
                });
            }
            Ieee802154PolledOperationPoll::Quarantined(quarantine) => {
                return Err(public_failure_from_quarantine(quarantine));
            }
        }
    }
}

fn public_failure_from_quarantine<Backend>(
    quarantine: Ieee802154OperationQuarantined<Backend>,
) -> Ieee802154PolledOperationFailure
where
    Backend: Ieee802154PolledOperationBackend<Error = Infallible>,
{
    let Ieee802154OperationQuarantined { backend: _, reason } = quarantine;
    match reason {
        Ieee802154OperationQuarantineReason::Backend { error, .. } => match error {},
        Ieee802154OperationQuarantineReason::CpuInterruptRouteAttached { stage } => {
            Ieee802154PolledOperationFailure::CpuInterruptRouteAttached { stage }
        }
        Ieee802154OperationQuarantineReason::UnexpectedEventMask { stage, observed } => {
            Ieee802154PolledOperationFailure::UnexpectedEventMask { stage, observed }
        }
        Ieee802154OperationQuarantineReason::UnexpectedRxAbortMask { stage, observed } => {
            Ieee802154PolledOperationFailure::UnexpectedRxAbortMask { stage, observed }
        }
        Ieee802154OperationQuarantineReason::StaleEventStatus { observed } => {
            Ieee802154PolledOperationFailure::StaleEventStatus { observed }
        }
        Ieee802154OperationQuarantineReason::UnexpectedTerminalStatus { observed } => {
            Ieee802154PolledOperationFailure::UnexpectedTerminalStatus { observed }
        }
        Ieee802154OperationQuarantineReason::UnexpectedAcknowledgedEvents { observed } => {
            Ieee802154PolledOperationFailure::UnexpectedAcknowledgedEvents { observed }
        }
        Ieee802154OperationQuarantineReason::ConflictingTerminalEvents { observed } => {
            Ieee802154PolledOperationFailure::ConflictingTerminalEvents { observed }
        }
    }
}

impl<Backend> Ieee802154PolledOperationActive<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// Sample one finite status observation without acknowledging events.
    pub(crate) fn poll(self) -> Ieee802154PolledOperationPoll<Backend> {
        let mut backend = self.backend;
        if let Err(reason) = require_detached_route(&mut backend, Ieee802154OperationStage::Poll) {
            return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                backend, reason,
            ));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::Poll,
            Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly,
        ) {
            return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                backend, reason,
            ));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::Poll,
            Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly,
        ) {
            return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                backend, reason,
            ));
        }
        let status = match backend.sample_event_status() {
            Ok(status) => status,
            Err(error) => {
                return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                    backend,
                    backend_error(Ieee802154OperationStage::Poll, error),
                ));
            }
        };
        let polls = self.polls + 1;
        let remaining_polls = self.remaining_polls - 1;

        if status.has_ed_done() && status.has_rx_abort() {
            return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                backend,
                Ieee802154OperationQuarantineReason::ConflictingTerminalEvents { observed: status },
            ));
        }

        if status.has_rx_abort() {
            let rx_abort_status = match backend.sample_rx_abort_status() {
                Ok(status) => status,
                Err(error) => {
                    return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                        backend,
                        backend_error(Ieee802154OperationStage::TerminalSample, error),
                    ));
                }
            };
            return match cleanup(backend) {
                Ok(backend) => {
                    Ieee802154PolledOperationPoll::Aborted(Ieee802154PolledOperationAborted {
                        backend,
                        evidence: Ieee802154PolledOperationAbortEvidence {
                            operation: self.operation,
                            event_status: status,
                            rx_abort_status,
                            polls,
                        },
                    })
                }
                Err(quarantine) => Ieee802154PolledOperationPoll::Quarantined(quarantine),
            };
        }

        if !status.is_clear() && !status.is_ed_done_only() {
            return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                backend,
                Ieee802154OperationQuarantineReason::UnexpectedTerminalStatus { observed: status },
            ));
        }

        if status.is_ed_done_only() {
            let result = match self.operation {
                Ieee802154PolledOperation::EnergyDetection { .. } => {
                    match backend.sample_ed_rss_code() {
                        Ok(rss_code) => {
                            Ieee802154PolledOperationResult::EnergyDetection { rss_code }
                        }
                        Err(error) => {
                            return Ieee802154PolledOperationPoll::Quarantined(
                                quarantined_after_cleanup(
                                    backend,
                                    backend_error(Ieee802154OperationStage::TerminalSample, error),
                                ),
                            );
                        }
                    }
                }
                Ieee802154PolledOperation::ClearChannelAssessment { .. } => {
                    match backend.sample_cca_busy() {
                        Ok(busy) => {
                            Ieee802154PolledOperationResult::ClearChannelAssessment { busy }
                        }
                        Err(error) => {
                            return Ieee802154PolledOperationPoll::Quarantined(
                                quarantined_after_cleanup(
                                    backend,
                                    backend_error(Ieee802154OperationStage::TerminalSample, error),
                                ),
                            );
                        }
                    }
                }
            };
            return Ieee802154PolledOperationPoll::Completed(Ieee802154PolledOperationCompleted {
                backend,
                evidence: Ieee802154PolledOperationEvidence {
                    operation: self.operation,
                    result,
                    polls,
                },
            });
        }

        if remaining_polls == 0 {
            return match cleanup(backend) {
                Ok(backend) => {
                    Ieee802154PolledOperationPoll::Timeout(Ieee802154PolledOperationTimeout {
                        backend,
                        operation: self.operation,
                        polls,
                    })
                }
                Err(quarantine) => Ieee802154PolledOperationPoll::Quarantined(quarantine),
            };
        }

        Ieee802154PolledOperationPoll::Pending(Ieee802154PolledOperationActive {
            backend,
            operation: self.operation,
            remaining_polls,
            polls,
        })
    }
}

fn backend_error<Error>(
    stage: Ieee802154OperationStage,
    error: Error,
) -> Ieee802154OperationQuarantineReason<Error> {
    Ieee802154OperationQuarantineReason::Backend { stage, error }
}

fn quarantined<Backend>(
    backend: Backend,
    reason: Ieee802154OperationQuarantineReason<Backend::Error>,
) -> Ieee802154OperationQuarantined<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    Ieee802154OperationQuarantined { backend, reason }
}

fn require_detached_route<Backend>(
    backend: &mut Backend,
    stage: Ieee802154OperationStage,
) -> Result<(), Ieee802154OperationQuarantineReason<Backend::Error>>
where
    Backend: Ieee802154PolledOperationBackend,
{
    match backend.cpu_interrupt_route_is_detached() {
        Ok(true) => Ok(()),
        Ok(false) => Err(Ieee802154OperationQuarantineReason::CpuInterruptRouteAttached { stage }),
        Err(error) => Err(backend_error(stage, error)),
    }
}

fn require_event_mask<Backend>(
    backend: &mut Backend,
    stage: Ieee802154OperationStage,
    expected: Ieee802154OperationEventMaskState,
) -> Result<(), Ieee802154OperationQuarantineReason<Backend::Error>>
where
    Backend: Ieee802154PolledOperationBackend,
{
    match backend.operation_event_mask_state() {
        Ok(observed) if observed == expected => Ok(()),
        Ok(observed) => {
            Err(Ieee802154OperationQuarantineReason::UnexpectedEventMask { stage, observed })
        }
        Err(error) => Err(backend_error(stage, error)),
    }
}

fn require_rx_abort_mask<Backend>(
    backend: &mut Backend,
    stage: Ieee802154OperationStage,
    expected: Ieee802154OperationRxAbortMaskState,
) -> Result<(), Ieee802154OperationQuarantineReason<Backend::Error>>
where
    Backend: Ieee802154PolledOperationBackend,
{
    match backend.operation_rx_abort_mask_state() {
        Ok(observed) if observed == expected => Ok(()),
        Ok(observed) => {
            Err(Ieee802154OperationQuarantineReason::UnexpectedRxAbortMask { stage, observed })
        }
        Err(error) => Err(backend_error(stage, error)),
    }
}

fn quarantined_after_cleanup<Backend>(
    backend: Backend,
    reason: Ieee802154OperationQuarantineReason<Backend::Error>,
) -> Ieee802154OperationQuarantined<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    match cleanup(backend) {
        Ok(backend) => quarantined(backend, reason),
        Err(cleanup_failure) => cleanup_failure,
    }
}

fn cleanup<Backend>(
    mut backend: Backend,
) -> Result<Backend, Ieee802154OperationQuarantined<Backend>>
where
    Backend: Ieee802154PolledOperationBackend,
{
    if let Err(error) = backend.mask_ed_done_and_rx_abort() {
        return Err(quarantined(
            backend,
            backend_error(Ieee802154OperationStage::Cleanup, error),
        ));
    }
    if let Err(error) = backend.mask_ed_operation_rx_abort_reasons() {
        return Err(quarantined(
            backend,
            backend_error(Ieee802154OperationStage::Cleanup, error),
        ));
    }
    if let Err(error) = backend.order_device_accesses() {
        return Err(quarantined(
            backend,
            backend_error(Ieee802154OperationStage::Cleanup, error),
        ));
    }
    if let Err(reason) = require_event_mask(
        &mut backend,
        Ieee802154OperationStage::Cleanup,
        Ieee802154OperationEventMaskState::AllMasked,
    ) {
        return Err(quarantined(backend, reason));
    }
    if let Err(reason) = require_rx_abort_mask(
        &mut backend,
        Ieee802154OperationStage::Cleanup,
        Ieee802154OperationRxAbortMaskState::AllMasked,
    ) {
        return Err(quarantined(backend, reason));
    }
    if let Err(reason) = require_detached_route(&mut backend, Ieee802154OperationStage::Cleanup) {
        return Err(quarantined(backend, reason));
    }
    Ok(backend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, vec::Vec};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FakeError {
        Injected,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        RouteRead,
        MaskRead,
        RxAbortMaskRead,
        Channel(u8),
        CcaMode(Ieee802154CcaMode),
        CcaThreshold(i8),
        Duration(u16),
        EnableOperationEvents,
        EnableOperationRxAborts,
        MaskOperationEvents,
        MaskOperationRxAborts,
        Fence,
        EdStart,
        EventStatus(Ieee802154OperationEventObservation),
        AcknowledgePendingEvents(Ieee802154OperationEventObservation),
        RxAbortStatus(u32),
        EdRss(i8),
        CcaBusy(bool),
    }

    struct FakeBackend {
        operations: Vec<Operation>,
        route_detached: bool,
        mask: Ieee802154OperationEventMaskState,
        rx_abort_mask: Ieee802154OperationRxAbortMaskState,
        event_status: Ieee802154OperationEventObservation,
        pending_at_acknowledge: Option<Ieee802154OperationEventObservation>,
        samples: VecDeque<Ieee802154OperationEventObservation>,
        rx_abort_status: u32,
        polling: bool,
        rss_code: i8,
        cca_busy: bool,
        fail_on_call: Option<usize>,
        calls: usize,
    }

    impl FakeBackend {
        fn new(samples: impl IntoIterator<Item = Ieee802154OperationEventObservation>) -> Self {
            Self {
                operations: Vec::new(),
                route_detached: true,
                mask: Ieee802154OperationEventMaskState::AllMasked,
                rx_abort_mask: Ieee802154OperationRxAbortMaskState::AllMasked,
                event_status: Ieee802154OperationEventObservation::default(),
                pending_at_acknowledge: None,
                samples: samples.into_iter().collect(),
                rx_abort_status: 0x0300_0000,
                polling: false,
                rss_code: -42,
                cca_busy: false,
                fail_on_call: None,
                calls: 0,
            }
        }

        fn before_call(&mut self) -> Result<(), FakeError> {
            let call = self.calls;
            self.calls += 1;
            if self.fail_on_call == Some(call) {
                Err(FakeError::Injected)
            } else {
                Ok(())
            }
        }
    }

    impl Ieee802154PolledOperationBackend for FakeBackend {
        type Error = FakeError;

        fn set_channel(&mut self, channel: Ieee802154Channel) -> Result<(), Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::Channel(channel.number()));
            Ok(())
        }

        fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) -> Result<(), Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::CcaMode(mode));
            Ok(())
        }

        fn set_cca_threshold_code(&mut self, threshold: i8) -> Result<(), Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::CcaThreshold(threshold));
            Ok(())
        }

        fn set_ed_duration(&mut self, duration: u16) -> Result<(), Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::Duration(duration));
            Ok(())
        }

        fn cpu_interrupt_route_is_detached(&mut self) -> Result<bool, Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::RouteRead);
            Ok(self.route_detached)
        }

        fn operation_event_mask_state(
            &mut self,
        ) -> Result<Ieee802154OperationEventMaskState, Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::MaskRead);
            Ok(self.mask)
        }

        fn operation_rx_abort_mask_state(
            &mut self,
        ) -> Result<Ieee802154OperationRxAbortMaskState, Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::RxAbortMaskRead);
            Ok(self.rx_abort_mask)
        }

        fn enable_ed_done_and_rx_abort(&mut self) -> Result<(), Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::EnableOperationEvents);
            self.mask = Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly;
            Ok(())
        }

        fn enable_ed_operation_rx_abort_reasons(&mut self) -> Result<(), Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::EnableOperationRxAborts);
            self.rx_abort_mask = Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly;
            Ok(())
        }

        fn mask_ed_done_and_rx_abort(&mut self) -> Result<(), Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::MaskOperationEvents);
            self.mask = Ieee802154OperationEventMaskState::AllMasked;
            self.polling = false;
            Ok(())
        }

        fn mask_ed_operation_rx_abort_reasons(&mut self) -> Result<(), Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::MaskOperationRxAborts);
            self.rx_abort_mask = Ieee802154OperationRxAbortMaskState::AllMasked;
            Ok(())
        }

        fn order_device_accesses(&mut self) -> Result<(), Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::Fence);
            Ok(())
        }

        fn request_ed_start(&mut self) -> Result<(), Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::EdStart);
            self.polling = true;
            Ok(())
        }

        fn sample_event_status(
            &mut self,
        ) -> Result<Ieee802154OperationEventObservation, Self::Error> {
            self.before_call()?;
            if self.polling && self.event_status.is_clear() {
                self.event_status = self.samples.pop_front().unwrap_or_default();
            }
            self.operations
                .push(Operation::EventStatus(self.event_status));
            Ok(self.event_status)
        }

        fn acknowledge_pending_events(
            &mut self,
        ) -> Result<Ieee802154OperationEventObservation, Self::Error> {
            self.before_call()?;
            let acknowledged = self.pending_at_acknowledge.unwrap_or(self.event_status);
            self.operations
                .push(Operation::AcknowledgePendingEvents(acknowledged));
            self.event_status = Ieee802154OperationEventObservation::default();
            self.polling = false;
            Ok(acknowledged)
        }

        fn sample_rx_abort_status(&mut self) -> Result<u32, Self::Error> {
            self.before_call()?;
            self.operations
                .push(Operation::RxAbortStatus(self.rx_abort_status));
            Ok(self.rx_abort_status)
        }

        fn sample_ed_rss_code(&mut self) -> Result<i8, Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::EdRss(self.rss_code));
            Ok(self.rss_code)
        }

        fn sample_cca_busy(&mut self) -> Result<bool, Self::Error> {
            self.before_call()?;
            self.operations.push(Operation::CcaBusy(self.cca_busy));
            Ok(self.cca_busy)
        }
    }

    fn channel() -> Ieee802154Channel {
        Ieee802154Channel::new(20).unwrap()
    }

    fn budget(samples: u32) -> Ieee802154OperationPollBudget {
        Ieee802154OperationPollBudget::new(samples).unwrap()
    }

    fn owner(backend: FakeBackend) -> Ieee802154PolledOperationOwner<FakeBackend> {
        Ieee802154PolledOperationOwner::from_semantic_backend(backend)
    }

    fn start(
        backend: FakeBackend,
        operation: Ieee802154PolledOperation,
        samples: u32,
    ) -> Ieee802154PolledOperationActive<FakeBackend> {
        owner(backend)
            .prepare(operation, budget(samples))
            .unwrap_or_else(|_| panic!("prepare must succeed"))
            .start()
            .unwrap_or_else(|_| panic!("start must succeed"))
    }

    #[test]
    fn zero_poll_budget_is_rejected_before_ownership() {
        assert_eq!(Ieee802154OperationPollBudget::new(0), None);
        assert_eq!(budget(3).samples(), 3);
    }

    #[test]
    fn cca_prepare_and_start_use_duration_eight_and_exact_detached_window() {
        let operation = Ieee802154PolledOperation::clear_channel_assessment(
            channel(),
            Ieee802154CcaMode::CarrierOrEnergyDetection,
            -71,
        );
        let active = start(FakeBackend::new([]), operation, 2);

        assert_eq!(
            active.backend.operations,
            [
                Operation::RouteRead,
                Operation::MaskRead,
                Operation::RxAbortMaskRead,
                Operation::Channel(20),
                Operation::CcaMode(Ieee802154CcaMode::CarrierOrEnergyDetection),
                Operation::CcaThreshold(-71),
                Operation::Duration(IEEE802154_CCA_ED_DURATION),
                Operation::Fence,
                Operation::RouteRead,
                Operation::MaskRead,
                Operation::RxAbortMaskRead,
                Operation::RouteRead,
                Operation::MaskRead,
                Operation::RxAbortMaskRead,
                Operation::EnableOperationEvents,
                Operation::EnableOperationRxAborts,
                Operation::Fence,
                Operation::RouteRead,
                Operation::MaskRead,
                Operation::RxAbortMaskRead,
                Operation::EventStatus(Ieee802154OperationEventObservation::from_raw(0)),
                Operation::EdStart,
            ]
        );
        assert_eq!(
            active.backend.mask,
            Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly
        );
        assert_eq!(
            active.backend.rx_abort_mask,
            Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly
        );
    }

    #[test]
    fn energy_detection_completion_recovers_in_proved_order() {
        let operation = Ieee802154PolledOperation::energy_detection(channel(), 27);
        let active = start(
            FakeBackend::new([Ieee802154OperationEventObservation::from_raw(0x0040)]),
            operation,
            2,
        );
        let completed = match active.poll() {
            Ieee802154PolledOperationPoll::Completed(completed) => completed,
            _ => panic!("lone ED_DONE must complete"),
        };

        assert_eq!(completed.evidence().operation(), operation);
        assert_eq!(
            completed.evidence().result(),
            Ieee802154PolledOperationResult::EnergyDetection { rss_code: -42 }
        );
        assert_eq!(completed.evidence().polls(), 1);
        assert_eq!(
            completed.backend.mask,
            Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly
        );
        assert_eq!(
            completed.backend.rx_abort_mask,
            Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly
        );

        let recovered = completed
            .recover()
            .unwrap_or_else(|_| panic!("exact ED_DONE acknowledgement must recover"));
        assert_eq!(
            recovered.evidence().result(),
            Ieee802154PolledOperationResult::EnergyDetection { rss_code: -42 }
        );
        assert_eq!(
            recovered.owner.backend.mask,
            Ieee802154OperationEventMaskState::AllMasked
        );
        assert_eq!(
            recovered.owner.backend.rx_abort_mask,
            Ieee802154OperationRxAbortMaskState::AllMasked
        );
        assert!(recovered.owner.backend.operations.ends_with(&[
            Operation::EventStatus(Ieee802154OperationEventObservation::from_raw(0x0040)),
            Operation::EdRss(-42),
            Operation::RouteRead,
            Operation::MaskRead,
            Operation::RxAbortMaskRead,
            Operation::AcknowledgePendingEvents(Ieee802154OperationEventObservation::from_raw(
                0x0040
            ),),
            Operation::MaskOperationEvents,
            Operation::MaskOperationRxAborts,
            Operation::Fence,
            Operation::MaskRead,
            Operation::RxAbortMaskRead,
            Operation::RouteRead,
        ]));
    }

    #[test]
    fn cca_completion_samples_busy_without_reading_rss() {
        let mut backend = FakeBackend::new([Ieee802154OperationEventObservation::from_raw(0x0040)]);
        backend.cca_busy = true;
        let active = start(
            backend,
            Ieee802154PolledOperation::clear_channel_assessment(
                channel(),
                Ieee802154CcaMode::EnergyDetection,
                -64,
            ),
            1,
        );
        let completed = match active.poll() {
            Ieee802154PolledOperationPoll::Completed(completed) => completed,
            _ => panic!("ED_DONE must produce CCA evidence"),
        };

        assert_eq!(
            completed.evidence().result(),
            Ieee802154PolledOperationResult::ClearChannelAssessment { busy: true }
        );
        assert!(
            completed
                .backend
                .operations
                .contains(&Operation::CcaBusy(true))
        );
        assert!(
            !completed
                .backend
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::EdRss(_)))
        );
    }

    #[test]
    fn recovered_owner_supports_back_to_back_serialized_ed() {
        let operation = Ieee802154PolledOperation::energy_detection(channel(), 8);
        let backend = FakeBackend::new([
            Ieee802154OperationEventObservation::from_raw(0x0040),
            Ieee802154OperationEventObservation::from_raw(0x0040),
        ]);
        let completed = match start(backend, operation, 1).poll() {
            Ieee802154PolledOperationPoll::Completed(completed) => completed,
            _ => panic!("first operation must complete"),
        };
        let recovered = completed
            .recover()
            .unwrap_or_else(|_| panic!("first operation must recover"));
        let active = recovered
            .into_owner()
            .prepare(operation, budget(1))
            .unwrap_or_else(|_| panic!("recovered owner must prepare again"))
            .start()
            .unwrap_or_else(|_| panic!("recovered owner must start again"));
        let completed = match active.poll() {
            Ieee802154PolledOperationPoll::Completed(completed) => completed,
            _ => panic!("second operation must complete"),
        };
        let recovered = completed
            .recover()
            .unwrap_or_else(|_| panic!("second operation must recover"));

        assert_eq!(
            recovered
                .owner
                .backend
                .operations
                .iter()
                .filter(|operation| matches!(operation, Operation::EdStart))
                .count(),
            2
        );
        assert_eq!(
            recovered
                .owner
                .backend
                .operations
                .iter()
                .filter(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
                .count(),
            2
        );
    }

    #[test]
    fn pending_then_timeout_uses_exact_budget_and_remasks_without_stop_claim() {
        let operation = Ieee802154PolledOperation::energy_detection(channel(), 9);
        let active = start(FakeBackend::new([]), operation, 2);
        let active = match active.poll() {
            Ieee802154PolledOperationPoll::Pending(active) => active,
            _ => panic!("first empty sample must remain pending"),
        };
        let timeout = match active.poll() {
            Ieee802154PolledOperationPoll::Timeout(timeout) => timeout,
            _ => panic!("second empty sample must exhaust the budget"),
        };

        assert_eq!(timeout.operation(), operation);
        assert_eq!(timeout.polls(), 2);
        assert_eq!(
            timeout.backend.mask,
            Ieee802154OperationEventMaskState::AllMasked
        );
        assert_eq!(
            timeout.backend.rx_abort_mask,
            Ieee802154OperationRxAbortMaskState::AllMasked
        );
        assert_eq!(
            timeout
                .backend
                .operations
                .iter()
                .filter(|operation| matches!(operation, Operation::EdStart))
                .count(),
            1
        );
        assert!(
            !timeout
                .backend
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
        );
    }

    #[test]
    fn stale_status_is_contained_before_ed_start_without_acknowledgement() {
        let mut backend = FakeBackend::new([]);
        backend.event_status = Ieee802154OperationEventObservation::from_raw(0x0100);
        let prepared = owner(backend)
            .prepare(
                Ieee802154PolledOperation::energy_detection(channel(), 1),
                budget(1),
            )
            .unwrap_or_else(|_| panic!("request fields may prepare before status gate"));
        let quarantine = match prepared.start() {
            Err(quarantine) => quarantine,
            Ok(_) => panic!("stale status must block ED_START"),
        };

        assert_eq!(
            quarantine.reason(),
            &Ieee802154OperationQuarantineReason::StaleEventStatus {
                observed: Ieee802154OperationEventObservation::from_raw(0x0100),
            }
        );
        assert!(!quarantine.backend.operations.contains(&Operation::EdStart));
        assert!(
            !quarantine
                .backend
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
        );
        assert_eq!(
            quarantine.backend.mask,
            Ieee802154OperationEventMaskState::AllMasked
        );
    }

    #[test]
    fn attached_route_is_rejected_before_any_request_write() {
        let mut backend = FakeBackend::new([]);
        backend.route_detached = false;
        let quarantine = match owner(backend).prepare(
            Ieee802154PolledOperation::energy_detection(channel(), 1),
            budget(1),
        ) {
            Err(quarantine) => quarantine,
            Ok(_) => panic!("attached route must fail closed"),
        };

        assert_eq!(
            quarantine.reason(),
            &Ieee802154OperationQuarantineReason::CpuInterruptRouteAttached {
                stage: Ieee802154OperationStage::Prepare,
            }
        );
        assert_eq!(quarantine.backend.operations, [Operation::RouteRead]);
    }

    #[test]
    fn unexpected_event_or_abort_enable_is_rejected_before_request_writes() {
        let operation = Ieee802154PolledOperation::energy_detection(channel(), 1);
        let mut backend = FakeBackend::new([]);
        backend.mask = Ieee802154OperationEventMaskState::Unexpected;
        let quarantine = match owner(backend).prepare(operation, budget(1)) {
            Err(quarantine) => quarantine,
            Ok(_) => panic!("unexpected event enable must fail closed"),
        };
        assert_eq!(
            quarantine.reason(),
            &Ieee802154OperationQuarantineReason::UnexpectedEventMask {
                stage: Ieee802154OperationStage::Prepare,
                observed: Ieee802154OperationEventMaskState::Unexpected,
            }
        );
        assert_eq!(
            quarantine.backend.operations,
            [Operation::RouteRead, Operation::MaskRead]
        );

        let mut backend = FakeBackend::new([]);
        backend.rx_abort_mask = Ieee802154OperationRxAbortMaskState::Unexpected;
        let quarantine = match owner(backend).prepare(operation, budget(1)) {
            Err(quarantine) => quarantine,
            Ok(_) => panic!("unexpected receive-abort enable must fail closed"),
        };
        assert_eq!(
            quarantine.reason(),
            &Ieee802154OperationQuarantineReason::UnexpectedRxAbortMask {
                stage: Ieee802154OperationStage::Prepare,
                observed: Ieee802154OperationRxAbortMaskState::Unexpected,
            }
        );
        assert_eq!(
            quarantine.backend.operations,
            [
                Operation::RouteRead,
                Operation::MaskRead,
                Operation::RxAbortMaskRead,
            ]
        );
    }

    #[test]
    fn conflicting_terminal_events_are_remasked_and_quarantined() {
        let active = start(
            FakeBackend::new([Ieee802154OperationEventObservation::from_raw(0x0050)]),
            Ieee802154PolledOperation::energy_detection(channel(), 1),
            1,
        );
        let quarantine = match active.poll() {
            Ieee802154PolledOperationPoll::Quarantined(quarantine) => quarantine,
            _ => panic!("conflicting terminal sample must quarantine"),
        };

        assert_eq!(
            quarantine.reason(),
            &Ieee802154OperationQuarantineReason::ConflictingTerminalEvents {
                observed: Ieee802154OperationEventObservation::from_raw(0x0050),
            }
        );
        assert_eq!(
            quarantine.backend.mask,
            Ieee802154OperationEventMaskState::AllMasked
        );
        assert_eq!(
            quarantine.backend.rx_abort_mask,
            Ieee802154OperationRxAbortMaskState::AllMasked
        );
        assert!(
            !quarantine
                .backend
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
        );
    }

    #[test]
    fn rx_abort_retains_full_evidence_and_never_acknowledges() {
        let mut backend = FakeBackend::new([Ieee802154OperationEventObservation::from_raw(0x0110)]);
        backend.rx_abort_status = 0x0380_0000;
        let active = start(
            backend,
            Ieee802154PolledOperation::energy_detection(channel(), 1),
            1,
        );
        let aborted = match active.poll() {
            Ieee802154PolledOperationPoll::Aborted(aborted) => aborted,
            _ => panic!("RX_ABORT must end the polled request"),
        };

        assert_eq!(
            aborted.evidence().operation(),
            Ieee802154PolledOperation::energy_detection(channel(), 1)
        );
        assert_eq!(aborted.evidence().event_status().raw(), 0x0110);
        assert_eq!(aborted.evidence().rx_abort_status(), 0x0380_0000);
        assert_eq!(aborted.evidence().polls(), 1);
        assert_eq!(
            aborted.backend.mask,
            Ieee802154OperationEventMaskState::AllMasked
        );
        assert!(
            !aborted
                .backend
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
        );
        assert!(
            aborted
                .backend
                .operations
                .windows(2)
                .any(|operations| operations
                    == [
                        Operation::EventStatus(Ieee802154OperationEventObservation::from_raw(
                            0x0110
                        )),
                        Operation::RxAbortStatus(0x0380_0000),
                    ])
        );
    }

    #[test]
    fn unrelated_terminal_status_is_contained_without_acknowledgement() {
        let active = start(
            FakeBackend::new([Ieee802154OperationEventObservation::from_raw(0x0100)]),
            Ieee802154PolledOperation::energy_detection(channel(), 1),
            1,
        );
        let quarantine = match active.poll() {
            Ieee802154PolledOperationPoll::Quarantined(quarantine) => quarantine,
            _ => panic!("unrelated terminal status must quarantine"),
        };

        assert_eq!(
            quarantine.reason(),
            &Ieee802154OperationQuarantineReason::UnexpectedTerminalStatus {
                observed: Ieee802154OperationEventObservation::from_raw(0x0100),
            }
        );
        assert!(
            !quarantine
                .backend
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
        );
    }

    #[test]
    fn acknowledged_snapshot_must_be_exactly_lone_ed_done() {
        for acknowledged_bits in [0, 0x0100, 0x0140, 0x0050] {
            let acknowledged = Ieee802154OperationEventObservation::from_raw(acknowledged_bits);
            let mut backend =
                FakeBackend::new([Ieee802154OperationEventObservation::from_raw(0x0040)]);
            backend.pending_at_acknowledge = Some(acknowledged);
            let completed = match start(
                backend,
                Ieee802154PolledOperation::energy_detection(channel(), 1),
                1,
            )
            .poll()
            {
                Ieee802154PolledOperationPoll::Completed(completed) => completed,
                _ => panic!("lone ED_DONE poll sample must complete before recovery"),
            };
            let quarantine = match completed.recover() {
                Err(quarantine) => quarantine,
                Ok(_) => panic!("non-ED_DONE acknowledgement must block reuse"),
            };

            assert_eq!(
                quarantine.reason(),
                &Ieee802154OperationQuarantineReason::UnexpectedAcknowledgedEvents {
                    observed: acknowledged,
                }
            );
            assert!(quarantine.backend.operations.windows(2).any(|operations| {
                operations
                    == [
                        Operation::AcknowledgePendingEvents(acknowledged),
                        Operation::MaskOperationEvents,
                    ]
            }));
            assert_eq!(
                quarantine.backend.mask,
                Ieee802154OperationEventMaskState::AllMasked
            );
        }
    }

    #[test]
    fn recovery_rechecks_active_window_before_acknowledgement() {
        let active = start(
            FakeBackend::new([Ieee802154OperationEventObservation::from_raw(0x0040)]),
            Ieee802154PolledOperation::energy_detection(channel(), 1),
            1,
        );
        let mut completed = match active.poll() {
            Ieee802154PolledOperationPoll::Completed(completed) => completed,
            _ => panic!("lone ED_DONE must complete"),
        };
        completed.backend.mask = Ieee802154OperationEventMaskState::Unexpected;
        let quarantine = match completed.recover() {
            Err(quarantine) => quarantine,
            Ok(_) => panic!("changed event window must block recovery"),
        };

        assert_eq!(
            quarantine.reason(),
            &Ieee802154OperationQuarantineReason::UnexpectedEventMask {
                stage: Ieee802154OperationStage::AcknowledgeTerminalEvent,
                observed: Ieee802154OperationEventMaskState::Unexpected,
            }
        );
        assert!(
            !quarantine
                .backend
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
        );
        assert_eq!(
            quarantine.backend.mask,
            Ieee802154OperationEventMaskState::AllMasked
        );
    }

    #[test]
    fn acknowledgement_backend_failure_contains_and_never_recovers() {
        let active = start(
            FakeBackend::new([Ieee802154OperationEventObservation::from_raw(0x0040)]),
            Ieee802154PolledOperation::energy_detection(channel(), 1),
            1,
        );
        let mut completed = match active.poll() {
            Ieee802154PolledOperationPoll::Completed(completed) => completed,
            _ => panic!("lone ED_DONE must complete"),
        };
        completed.backend.fail_on_call = Some(completed.backend.calls + 3);
        let quarantine = match completed.recover() {
            Err(quarantine) => quarantine,
            Ok(_) => panic!("failed acknowledgement must not recover"),
        };

        assert!(matches!(
            quarantine.reason(),
            Ieee802154OperationQuarantineReason::Backend {
                stage: Ieee802154OperationStage::AcknowledgeTerminalEvent,
                error: FakeError::Injected,
            }
        ));
        assert_eq!(
            quarantine.backend.mask,
            Ieee802154OperationEventMaskState::AllMasked
        );
        assert_eq!(
            quarantine.backend.rx_abort_mask,
            Ieee802154OperationRxAbortMaskState::AllMasked
        );
    }

    #[test]
    fn backend_failure_during_start_is_cleaned_and_quarantined() {
        let operation = Ieee802154PolledOperation::energy_detection(channel(), 1);
        let prepared = owner(FakeBackend::new([]))
            .prepare(operation, budget(1))
            .unwrap_or_else(|_| panic!("prepare must succeed"));
        let mut prepared = prepared;
        prepared.backend.fail_on_call = Some(prepared.backend.calls + 10);
        let quarantine = match prepared.start() {
            Err(quarantine) => quarantine,
            Ok(_) => panic!("injected command failure must quarantine"),
        };

        assert!(matches!(
            quarantine.reason(),
            Ieee802154OperationQuarantineReason::Backend {
                stage: Ieee802154OperationStage::StartCommand,
                error: FakeError::Injected,
            }
        ));
        assert_eq!(
            quarantine.backend.mask,
            Ieee802154OperationEventMaskState::AllMasked
        );
        assert_eq!(
            quarantine.backend.rx_abort_mask,
            Ieee802154OperationRxAbortMaskState::AllMasked
        );
    }
}
