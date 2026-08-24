//! Linear pure actor and non-executable transaction intents.

use core::fmt;

use open_esp_radio_esp32s31_ieee802154_dma::{
    RxArm, RxDelivered, RxDmaAddress, RxStubDelivered, TxArmed, TxCompleted, TxDmaAddress,
};
use open_esp_radio_esp32s31_ieee802154_irq::{
    Ieee802154Event, Ieee802154RxAbortReason, Ieee802154TxAbortReason,
};

use crate::batch::{MacCcaSample, MacEnergySample, MacEventBatch, MacMeasurementSample};

/// How a transmit intent reaches the channel.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacTransmitAccess {
    /// Request `TX_START` without a preceding hardware CCA.
    Direct,
    /// Request the combined CCA-then-transmit path.
    ClearChannelAssessment,
}

/// Whether a transmit intent continues into the reviewed `RX_ACK` phase.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacTransmitAcknowledgement {
    /// `TX_DONE` is terminal for this logical request.
    None,
    /// `TX_DONE` advances to an acknowledgement receive phase.
    Expected,
}

/// Bounded hardware subset of the public energy-detection duration input.
///
/// The public API accepts `uint32_t`, then its LL boundary narrows to
/// `uint16_t`. This type rejects values that would truncate instead of
/// reproducing that implicit conversion. Units and on-air accuracy remain
/// outside this pure planner.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacEnergyDetectionDuration(u16);

impl MacEnergyDetectionDuration {
    /// Preserve one already bounded hardware-field value.
    pub const fn from_hardware_units(units: u16) -> Self {
        Self(units)
    }

    /// Fail closed when the public input would narrow at the LL boundary.
    pub const fn try_from_public_units(
        units: u32,
    ) -> Result<Self, MacEnergyDetectionDurationError> {
        if units <= u16::MAX as u32 {
            Ok(Self(units as u16))
        } else {
            Err(MacEnergyDetectionDurationError::OutOfHardwareSubset { units })
        }
    }

    /// Return the unqualified bounded hardware value.
    pub const fn hardware_units(self) -> u16 {
        self.0
    }
}

/// Public ED duration cannot be represented without LL truncation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacEnergyDetectionDurationError {
    /// The `uint32_t` public input exceeds the open driver's bounded subset.
    OutOfHardwareSubset {
        /// Complete rejected public input.
        units: u32,
    },
}

/// Observable phase of one pure actor chain.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacActivePhase {
    /// One armed RX buffer is retained for a receive intent.
    Receive,
    /// One armed TX buffer is retained before `TX_DONE`.
    Transmit {
        /// Direct or CCA-gated transmit selection.
        access: MacTransmitAccess,
        /// Whether this request will continue to `RX_ACK`.
        acknowledgement: MacTransmitAcknowledgement,
    },
    /// `TX_DONE` was processed and the paired RX buffer remains retained.
    AwaitingAcknowledgement {
        /// Access selection used for the preceding transmit phase.
        access: MacTransmitAccess,
    },
    /// A standalone clear-channel assessment is pending.
    ClearChannelAssessment,
    /// A standalone energy-detection request is pending.
    EnergyDetection {
        /// Exact public duration input carried by the intent.
        duration: MacEnergyDetectionDuration,
    },
}

/// One command name carried by a non-executable start plan.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacCommandIntent {
    /// Receive-start intent.
    Receive,
    /// Direct transmit-start intent.
    Transmit,
    /// CCA-gated transmit-start intent.
    TransmitWithClearChannelAssessment,
    /// Standalone clear-channel-assessment intent.
    ClearChannelAssessment,
    /// Standalone energy-detection intent.
    EnergyDetection,
}

/// DMA address publication retained by one active plan.
///
/// Address tokens borrow the corresponding armed resources. They cannot
/// outlive the active actor from which the plan was obtained.
#[derive(Clone, Copy)]
pub enum MacDmaPublication<'active> {
    /// No frame address belongs to this request.
    None,
    /// One receive address belongs to this request.
    Receive(RxDmaAddress<'active>),
    /// One transmit address belongs to this request.
    Transmit(TxDmaAddress<'active>),
    /// A transmit plus ACK-receive pair belongs to this request.
    TransmitWithAcknowledgement {
        /// Address of the hardware-owned TX image.
        transmit: TxDmaAddress<'active>,
        /// Address of the hardware-owned ACK receive image.
        acknowledgement_receive: RxDmaAddress<'active>,
    },
}

/// One ordered step in a non-executable transaction intent.
///
/// No variant performs the named action. A future hardware owner must prove
/// every step, including state-specific quiescence, before interpreting it.
#[derive(Clone, Copy)]
pub enum MacIntentStep<'active> {
    /// Require state-specific quiescence and event reconciliation externally.
    RequireStateSpecificQuiescence,
    /// Require the already reviewed static-policy refresh externally.
    RefreshStaticPolicy,
    /// Publish one typed TX address through a future audited owner.
    PublishTransmitAddress(TxDmaAddress<'active>),
    /// Publish one typed RX address through a future audited owner.
    PublishReceiveAddress(RxDmaAddress<'active>),
    /// Configure the public ED-duration input through a future audited owner.
    ConfigureEnergyDetectionDuration(u16),
    /// Request one command through a future audited owner.
    RequestCommand(MacCommandIntent),
}

/// Borrowed deterministic, deliberately partial intent for one operation.
///
/// The first step is a requirement, not a claim that `STOP` is synchronous or
/// sufficient. RF/PHY acquisition, event-path readiness, PTI/coexistence, and
/// command execution are external prerequisites deliberately absent here. The
/// plan has no execute method and no PAC/MMIO capability.
#[derive(Clone, Copy)]
pub struct MacStartPlan<'active> {
    publication: MacDmaPublication<'active>,
    measurement_duration: Option<u16>,
    command: MacCommandIntent,
}

impl<'active> MacStartPlan<'active> {
    /// Return the exact number of ordered intent steps.
    pub const fn step_count(self) -> usize {
        2 + publication_step_count(self.publication)
            + if self.measurement_duration.is_some() {
                1
            } else {
                0
            }
            + 1
    }

    /// Return one ordered intent step, or `None` beyond the plan.
    pub const fn step(self, index: usize) -> Option<MacIntentStep<'active>> {
        if index == 0 {
            return Some(MacIntentStep::RequireStateSpecificQuiescence);
        }
        if index == 1 {
            return Some(MacIntentStep::RefreshStaticPolicy);
        }

        let publication_index = index - 2;
        if let Some(step) = publication_step(self.publication, publication_index) {
            return Some(step);
        }

        let after_publication = 2 + publication_step_count(self.publication);
        if let Some(duration) = self.measurement_duration {
            if index == after_publication {
                return Some(MacIntentStep::ConfigureEnergyDetectionDuration(duration));
            }
            if index == after_publication + 1 {
                return Some(MacIntentStep::RequestCommand(self.command));
            }
        } else if index == after_publication {
            return Some(MacIntentStep::RequestCommand(self.command));
        }
        None
    }

    /// Return the borrowed DMA publication set.
    pub const fn dma_publication(self) -> MacDmaPublication<'active> {
        self.publication
    }

    /// Return the final command intent.
    pub const fn command(self) -> MacCommandIntent {
        self.command
    }
}

const fn publication_step_count(publication: MacDmaPublication<'_>) -> usize {
    match publication {
        MacDmaPublication::None => 0,
        MacDmaPublication::Receive(_) | MacDmaPublication::Transmit(_) => 1,
        MacDmaPublication::TransmitWithAcknowledgement { .. } => 2,
    }
}

const fn publication_step(
    publication: MacDmaPublication<'_>,
    index: usize,
) -> Option<MacIntentStep<'_>> {
    match (publication, index) {
        (MacDmaPublication::Receive(address), 0) => {
            Some(MacIntentStep::PublishReceiveAddress(address))
        }
        (MacDmaPublication::Transmit(address), 0) => {
            Some(MacIntentStep::PublishTransmitAddress(address))
        }
        (MacDmaPublication::TransmitWithAcknowledgement { transmit, .. }, 0) => {
            Some(MacIntentStep::PublishTransmitAddress(transmit))
        }
        (
            MacDmaPublication::TransmitWithAcknowledgement {
                acknowledgement_receive,
                ..
            },
            1,
        ) => Some(MacIntentStep::PublishReceiveAddress(
            acknowledgement_receive,
        )),
        _ => None,
    }
}

/// Explicit no-DMA resource retained by CCA and ED requests.
///
/// The constructor is private; callers receive this value only after resolving
/// a terminal request.
#[derive(Debug, Eq, PartialEq)]
pub struct MacNoDmaResources {
    _private: (),
}

/// Paired DMA ownership for a transmit request that expects an ACK.
pub struct MacTxWithAckResources<'tx, 'rx, const COUNT: usize> {
    transmit: TxArmed<'tx>,
    acknowledgement_receive: RxArm<'rx, COUNT>,
}

/// Host-model RX ownership after a terminal DMA transition.
pub enum MacModelResolvedRx<'pool, const COUNT: usize> {
    /// One ordinary receive slot is now CPU-owned.
    Frame(RxDelivered<'pool, COUNT>),
    /// The drop stub completed and remains intentionally opaque.
    Stub(RxStubDelivered<'pool, COUNT>),
}

/// Host-model TX-with-ACK ownership after both DMA transitions complete.
pub struct MacModelResolvedTxWithAck<'tx, 'rx, const COUNT: usize> {
    transmit: TxCompleted<'tx>,
    acknowledgement_receive: MacModelResolvedRx<'rx, COUNT>,
}

impl<'tx, 'rx, const COUNT: usize> MacModelResolvedTxWithAck<'tx, 'rx, COUNT> {
    /// Split the CPU-owned TX result from the completed ACK receive result.
    pub fn into_parts(self) -> (TxCompleted<'tx>, MacModelResolvedRx<'rx, COUNT>) {
        (self.transmit, self.acknowledgement_receive)
    }
}

impl<'tx, 'rx, const COUNT: usize> MacTxWithAckResources<'tx, 'rx, COUNT> {
    /// Split the pair after terminal resolution transfers it to a reclaim
    /// callback.
    pub fn into_parts(self) -> (TxArmed<'tx>, RxArm<'rx, COUNT>) {
        (self.transmit, self.acknowledgement_receive)
    }
}

/// Unique ready state for one pure actor chain.
///
/// A target constructor is intentionally absent. Native construction exists
/// for deterministic model tests only; a future operational owner must add a
/// separately reviewed binding after IRQ acknowledgement and command gates are
/// proved.
pub struct MacReady {
    _private: (),
}

impl MacReady {
    /// Construct a native, non-operational model chain.
    #[cfg(not(target_arch = "riscv32"))]
    pub const fn new_model() -> Self {
        Self { _private: () }
    }

    /// Retain one RX DMA token in a receive request with automatic ACK
    /// explicitly disabled by the external static-policy owner.
    ///
    /// Automatic and enhanced ACK receive paths are intentionally absent:
    /// their `RX_DONE` is not terminal until later ACK processing, so treating
    /// them as this bounded request would permit premature DMA reclamation.
    ///
    /// ```compile_fail
    /// use open_esp_radio_esp32s31_ieee802154_dma::{
    ///     DMA_LOW, DmaFrameAddress, RxArm, RxPoolStorage,
    /// };
    /// use open_esp_radio_esp32s31_ieee802154_mac::MacReady;
    ///
    /// let storage = Box::leak(Box::new(RxPoolStorage::<1>::new()));
    /// let pool = RxPoolStorage::pin_static_model(
    ///     storage,
    ///     DmaFrameAddress::try_new(DMA_LOW).unwrap(),
    /// ).unwrap();
    /// let armed = pool.arm_next().unwrap();
    /// let ready = MacReady::new_model();
    /// let _receive = ready.request_receive_without_auto_ack(armed);
    /// let _second = ready.request_clear_channel_assessment();
    /// ```
    pub fn request_receive_without_auto_ack<const COUNT: usize>(
        self,
        armed: RxArm<'_, COUNT>,
    ) -> MacActive<RxArm<'_, COUNT>> {
        MacActive {
            ready: self,
            resources: armed,
            phase: MacActivePhase::Receive,
        }
    }

    /// Retain one TX DMA token in a no-ACK transmit request.
    ///
    /// The caller chooses the logical access path explicitly. This pure actor
    /// does not parse the frame-control field or execute either command.
    ///
    /// ```compile_fail
    /// use open_esp_radio_esp32s31_ieee802154_dma::{
    ///     DMA_LOW, DmaFrameAddress, TxStorage,
    /// };
    /// use open_esp_radio_esp32s31_ieee802154_mac::{MacReady, MacTransmitAccess};
    ///
    /// let storage = Box::leak(Box::new(TxStorage::new()));
    /// let mut owner = TxStorage::pin_static_model(
    ///     storage,
    ///     DmaFrameAddress::try_new(DMA_LOW).unwrap(),
    /// );
    /// let armed = owner.prepare(&[0x01]).unwrap().arm();
    /// let _active = MacReady::new_model().request_transmit_without_ack(
    ///     armed,
    ///     MacTransmitAccess::Direct,
    /// );
    /// let _completed = armed.complete_model();
    /// ```
    pub fn request_transmit_without_ack(
        self,
        armed: TxArmed<'_>,
        access: MacTransmitAccess,
    ) -> MacActive<TxArmed<'_>> {
        MacActive {
            ready: self,
            resources: armed,
            phase: MacActivePhase::Transmit {
                access,
                acknowledgement: MacTransmitAcknowledgement::None,
            },
        }
    }

    /// Retain one TX and one ACK-RX token in a transmit-with-ACK request.
    pub fn request_transmit_with_ack<'tx, 'rx, const COUNT: usize>(
        self,
        transmit: TxArmed<'tx>,
        acknowledgement_receive: RxArm<'rx, COUNT>,
        access: MacTransmitAccess,
    ) -> MacActive<MacTxWithAckResources<'tx, 'rx, COUNT>> {
        MacActive {
            ready: self,
            resources: MacTxWithAckResources {
                transmit,
                acknowledgement_receive,
            },
            phase: MacActivePhase::Transmit {
                access,
                acknowledgement: MacTransmitAcknowledgement::Expected,
            },
        }
    }

    /// Create a standalone clear-channel-assessment request.
    pub fn request_clear_channel_assessment(self) -> MacActive<MacNoDmaResources> {
        MacActive {
            ready: self,
            resources: MacNoDmaResources { _private: () },
            phase: MacActivePhase::ClearChannelAssessment,
        }
    }

    /// Create a standalone energy-detection request.
    pub fn request_energy_detection(
        self,
        duration: MacEnergyDetectionDuration,
    ) -> MacActive<MacNoDmaResources> {
        MacActive {
            ready: self,
            resources: MacNoDmaResources { _private: () },
            phase: MacActivePhase::EnergyDetection { duration },
        }
    }
}

/// Exactly one active logical operation plus its retained resources.
///
/// This type is neither `Clone` nor `Copy`. Processing consumes it, and a
/// rejected batch returns the exact value unchanged.
pub struct MacActive<R> {
    ready: MacReady,
    resources: R,
    phase: MacActivePhase,
}

impl<R> MacActive<R> {
    /// Return the current logical phase.
    pub const fn phase(&self) -> MacActivePhase {
        self.phase
    }

    /// Process one complete sampled batch transactionally.
    ///
    /// Impossible event/reason combinations return the exact active actor.
    /// A terminal callback creates a deferred state only after the whole batch
    /// has been validated, matching the reviewed ISR's single end-of-batch
    /// `next_operation` decision.
    pub fn process_batch(
        self,
        batch: MacEventBatch,
    ) -> Result<MacBatchOutcome<R>, MacBatchRejected<R>> {
        match evaluate_batch(self.phase, batch) {
            Ok(Evaluation::Pending(next_phase)) => Ok(MacBatchOutcome::Pending(Self {
                ready: self.ready,
                resources: self.resources,
                phase: next_phase,
            })),
            Ok(Evaluation::Terminal(completion)) => Ok(MacBatchOutcome::Deferred(MacDeferred {
                ready: self.ready,
                resources: self.resources,
                completion,
            })),
            Err(reason) => Err(MacBatchRejected {
                active: self,
                batch,
                reason,
            }),
        }
    }

    fn plan<'active>(
        &self,
        publication: MacDmaPublication<'active>,
    ) -> Option<MacStartPlan<'active>> {
        let (measurement_duration, command) = match self.phase {
            MacActivePhase::Receive => (None, MacCommandIntent::Receive),
            MacActivePhase::Transmit { access, .. } => match access {
                MacTransmitAccess::Direct => (None, MacCommandIntent::Transmit),
                MacTransmitAccess::ClearChannelAssessment => (
                    Some(8),
                    MacCommandIntent::TransmitWithClearChannelAssessment,
                ),
            },
            MacActivePhase::AwaitingAcknowledgement { .. } => return None,
            MacActivePhase::ClearChannelAssessment => {
                (Some(8), MacCommandIntent::ClearChannelAssessment)
            }
            MacActivePhase::EnergyDetection { duration } => (
                Some(duration.hardware_units()),
                MacCommandIntent::EnergyDetection,
            ),
        };
        Some(MacStartPlan {
            publication,
            measurement_duration,
            command,
        })
    }
}

impl<'pool, const COUNT: usize> MacActive<RxArm<'pool, COUNT>> {
    /// Borrow the receive start intent and its lifetime-bound address token.
    pub fn start_plan(&self) -> Option<MacStartPlan<'_>> {
        let address = match &self.resources {
            RxArm::Buffer(armed) => armed.dma_address(),
            RxArm::Stub(armed) => armed.dma_address(),
        };
        self.plan(MacDmaPublication::Receive(address))
    }
}

impl MacActive<TxArmed<'_>> {
    /// Borrow the transmit start intent and its lifetime-bound address token.
    pub fn start_plan(&self) -> Option<MacStartPlan<'_>> {
        self.plan(MacDmaPublication::Transmit(self.resources.dma_address()))
    }
}

impl<'tx, 'rx, const COUNT: usize> MacActive<MacTxWithAckResources<'tx, 'rx, COUNT>> {
    /// Borrow the TX-plus-ACK-RX intent and both lifetime-bound addresses.
    pub fn start_plan(&self) -> Option<MacStartPlan<'_>> {
        let acknowledgement_receive = match &self.resources.acknowledgement_receive {
            RxArm::Buffer(armed) => armed.dma_address(),
            RxArm::Stub(armed) => armed.dma_address(),
        };
        self.plan(MacDmaPublication::TransmitWithAcknowledgement {
            transmit: self.resources.transmit.dma_address(),
            acknowledgement_receive,
        })
    }
}

impl MacActive<MacNoDmaResources> {
    /// Borrow the CCA or ED intent, which contains no DMA publication.
    pub fn start_plan(&self) -> Option<MacStartPlan<'_>> {
        self.plan(MacDmaPublication::None)
    }
}

/// Successful completion of one supported logical request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacCompletion {
    /// A receive DMA operation reached `RX_DONE`.
    ReceiveFrame,
    /// A receive DMA operation reached a supported terminal abort.
    ReceiveAborted(Ieee802154RxAbortReason),
    /// A no-ACK transmit reached `TX_DONE`.
    TransmitComplete,
    /// A transmit-with-ACK reached `ACK_RX_DONE`.
    TransmitAcknowledged,
    /// A transmit reached a supported terminal abort.
    TransmitAborted(Ieee802154TxAbortReason),
    /// ACK receive terminated through the reviewed TX-abort timeout reason.
    AcknowledgementTimedOutByAbort,
    /// ACK receive terminated through timer-zero overflow.
    AcknowledgementTimedOutByTimer,
    /// Standalone CCA reached `ED_DONE` with its sampled status.
    ClearChannelAssessment(MacCcaSample),
    /// Standalone CCA reached a supported ED-path abort.
    ClearChannelAssessmentAborted(Ieee802154RxAbortReason),
    /// Standalone ED reached `ED_DONE` with its uncalibrated sample.
    EnergyDetection(MacEnergySample),
    /// Standalone ED reached a supported ED-path abort.
    EnergyDetectionAborted(Ieee802154RxAbortReason),
}

/// Result of one accepted non-empty batch.
pub enum MacBatchOutcome<R> {
    /// No terminal callback ran; the exact operation remains active.
    Pending(MacActive<R>),
    /// A terminal callback ran; one deferred-next decision is now required.
    Deferred(MacDeferred<R>),
}

impl<R> fmt::Debug for MacBatchOutcome<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending(active) => formatter
                .debug_tuple("Pending")
                .field(&active.phase)
                .finish(),
            Self::Deferred(deferred) => formatter
                .debug_tuple("Deferred")
                .field(&deferred.completion)
                .finish(),
        }
    }
}

/// Why one internally consistent batch is impossible or unsupported in the
/// current logical phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacBatchRejectReason {
    /// An empty batch cannot advance an active operation.
    Empty,
    /// At least one reviewed event is not allowed in the current phase.
    UnexpectedEventBits {
        /// Complete event subset rejected for the current phase.
        bits: u16,
    },
    /// More than one mutually exclusive terminal edge was sampled.
    ConflictingTerminalEvents {
        /// Complete conflicting terminal subset.
        bits: u16,
    },
    /// The receive-abort reason has no supported transition in this phase.
    UnexpectedRxAbortReason(Ieee802154RxAbortReason),
    /// The transmit-abort reason has no supported transition in this phase.
    UnexpectedTxAbortReason(Ieee802154TxAbortReason),
    /// `ED_DONE` carried the other operation's measurement kind.
    UnexpectedMeasurement(MacMeasurementSample),
}

/// Transactional rejection retaining the exact active actor and input batch.
pub struct MacBatchRejected<R> {
    active: MacActive<R>,
    batch: MacEventBatch,
    reason: MacBatchRejectReason,
}

impl<R> fmt::Debug for MacBatchRejected<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacBatchRejected")
            .field("phase", &self.active.phase)
            .field("batch", &self.batch)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

impl<R> MacBatchRejected<R> {
    /// Return the deterministic rejection reason.
    pub const fn reason(&self) -> MacBatchRejectReason {
        self.reason
    }

    /// Return the rejected immutable batch.
    pub const fn batch(&self) -> MacEventBatch {
        self.batch
    }

    /// Recover the exact active actor for inspection or retry.
    pub fn into_active(self) -> MacActive<R> {
        self.active
    }
}

/// The single next-operation choice made after an entire terminal batch.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacDeferredNext {
    /// Yield to the future owner's idle/sleep policy.
    IdlePolicy,
    /// Request a new receive transaction after resources are reclaimed.
    ReceiveWhenIdle,
}

/// Terminal actor that withholds the ready state until one deferred decision
/// and a type-specific resource transition have both completed.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154Event;
/// use open_esp_radio_esp32s31_ieee802154_mac::{MacBatchOutcome, MacEventBatch, MacReady};
///
/// let active = MacReady::new_model().request_clear_channel_assessment();
/// let batch = MacEventBatch::new(
///     Ieee802154Event::RxAbort.mask(),
///     Some(open_esp_radio_esp32s31_ieee802154_irq::Ieee802154RxAbortReason::EdAbort),
///     None,
///     None,
/// ).unwrap();
/// let MacBatchOutcome::Deferred(deferred) = active.process_batch(batch).unwrap() else {
///     panic!();
/// };
/// let _second = deferred.request_clear_channel_assessment();
/// ```
///
/// A generic identity-closure escape is deliberately absent:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154Event;
/// use open_esp_radio_esp32s31_ieee802154_mac::{
///     MacBatchOutcome, MacDeferredNext, MacEventBatch, MacReady,
/// };
///
/// let active = MacReady::new_model().request_clear_channel_assessment();
/// let batch = MacEventBatch::new(
///     Ieee802154Event::RxAbort.mask(),
///     Some(open_esp_radio_esp32s31_ieee802154_irq::Ieee802154RxAbortReason::EdAbort),
///     None,
///     None,
/// ).unwrap();
/// let MacBatchOutcome::Deferred(deferred) = active.process_batch(batch).unwrap() else {
///     panic!();
/// };
/// let _ = deferred.resolve_with(MacDeferredNext::IdlePolicy, |armed| armed);
/// ```
pub struct MacDeferred<R> {
    ready: MacReady,
    resources: R,
    completion: MacCompletion,
}

impl<R> MacDeferred<R> {
    /// Return the terminal logical completion without exposing resources.
    pub const fn completion(&self) -> MacCompletion {
        self.completion
    }
}

impl MacDeferred<MacNoDmaResources> {
    /// Resolve a terminal CCA/ED request, which has no DMA ownership to prove.
    pub fn resolve(self, next: MacDeferredNext) -> MacResolved<MacNoDmaResources> {
        MacResolved {
            ready: self.ready,
            reclaimed: self.resources,
            completion: self.completion,
            next,
        }
    }
}

#[cfg(not(target_arch = "riscv32"))]
impl<'pool, const COUNT: usize> MacDeferred<RxArm<'pool, COUNT>> {
    /// Complete the retained RX token in a native model, then restore ready.
    pub fn resolve_model(
        self,
        next: MacDeferredNext,
    ) -> Result<
        MacResolved<MacModelResolvedRx<'pool, COUNT>>,
        open_esp_radio_esp32s31_ieee802154_dma::RxPoolError,
    > {
        let reclaimed = match self.resources {
            RxArm::Buffer(armed) => MacModelResolvedRx::Frame(armed.complete_model()?),
            RxArm::Stub(armed) => MacModelResolvedRx::Stub(armed.complete_model()?),
        };
        Ok(MacResolved {
            ready: self.ready,
            reclaimed,
            completion: self.completion,
            next,
        })
    }
}

#[cfg(not(target_arch = "riscv32"))]
impl<'owner> MacDeferred<TxArmed<'owner>> {
    /// Complete the retained TX token in a native model, then restore ready.
    pub fn resolve_model(self, next: MacDeferredNext) -> MacResolved<TxCompleted<'owner>> {
        MacResolved {
            ready: self.ready,
            reclaimed: self.resources.complete_model(),
            completion: self.completion,
            next,
        }
    }
}

#[cfg(not(target_arch = "riscv32"))]
impl<'tx, 'rx, const COUNT: usize> MacDeferred<MacTxWithAckResources<'tx, 'rx, COUNT>> {
    /// Complete both retained tokens in a native model, then restore ready.
    ///
    /// An RX state mismatch quarantines the complete actor chain: no ready
    /// state or still-armed token is returned from the error path.
    pub fn resolve_model(
        self,
        next: MacDeferredNext,
    ) -> Result<
        MacResolved<MacModelResolvedTxWithAck<'tx, 'rx, COUNT>>,
        open_esp_radio_esp32s31_ieee802154_dma::RxPoolError,
    > {
        let acknowledgement_receive = match self.resources.acknowledgement_receive {
            RxArm::Buffer(armed) => MacModelResolvedRx::Frame(armed.complete_model()?),
            RxArm::Stub(armed) => MacModelResolvedRx::Stub(armed.complete_model()?),
        };
        let reclaimed = MacModelResolvedTxWithAck {
            transmit: self.resources.transmit.complete_model(),
            acknowledgement_receive,
        };
        Ok(MacResolved {
            ready: self.ready,
            reclaimed,
            completion: self.completion,
            next,
        })
    }
}

/// Resolved terminal batch with the unique ready state and reclaimed value.
pub struct MacResolved<C> {
    ready: MacReady,
    reclaimed: C,
    completion: MacCompletion,
    next: MacDeferredNext,
}

impl<C> MacResolved<C> {
    /// Split the resolved transaction into its unique ready state, reclaimed
    /// value, completion, and deferred-next choice.
    pub fn into_parts(self) -> (MacReady, C, MacCompletion, MacDeferredNext) {
        (self.ready, self.reclaimed, self.completion, self.next)
    }
}

enum Evaluation {
    Pending(MacActivePhase),
    Terminal(MacCompletion),
}

fn evaluate_batch(
    phase: MacActivePhase,
    batch: MacEventBatch,
) -> Result<Evaluation, MacBatchRejectReason> {
    if batch.events().bits() == 0 {
        return Err(MacBatchRejectReason::Empty);
    }
    match phase {
        MacActivePhase::Receive => evaluate_receive(batch),
        MacActivePhase::Transmit {
            access,
            acknowledgement,
        } => evaluate_transmit(batch, access, acknowledgement),
        MacActivePhase::AwaitingAcknowledgement { access } => {
            evaluate_acknowledgement(batch, access)
        }
        MacActivePhase::ClearChannelAssessment => evaluate_cca(batch),
        MacActivePhase::EnergyDetection { .. } => evaluate_energy_detection(batch),
    }
}

fn evaluate_receive(batch: MacEventBatch) -> Result<Evaluation, MacBatchRejectReason> {
    const ALLOWED: u16 = Ieee802154Event::RxSfdDone.bit()
        | Ieee802154Event::RxDone.bit()
        | Ieee802154Event::RxAbort.bit();
    reject_unexpected_events(batch, ALLOWED)?;
    reject_conflicting(
        batch,
        Ieee802154Event::RxDone.bit() | Ieee802154Event::RxAbort.bit(),
    )?;

    if let Some(reason) = batch.rx_abort_reason() {
        if !is_terminal_receive_abort(reason) {
            return Err(MacBatchRejectReason::UnexpectedRxAbortReason(reason));
        }
        return Ok(Evaluation::Terminal(MacCompletion::ReceiveAborted(reason)));
    }
    if batch.events().contains(Ieee802154Event::RxDone) {
        return Ok(Evaluation::Terminal(MacCompletion::ReceiveFrame));
    }
    Ok(Evaluation::Pending(MacActivePhase::Receive))
}

fn evaluate_transmit(
    batch: MacEventBatch,
    access: MacTransmitAccess,
    acknowledgement: MacTransmitAcknowledgement,
) -> Result<Evaluation, MacBatchRejectReason> {
    let mut allowed = Ieee802154Event::RxSfdDone.bit()
        | Ieee802154Event::TxSfdDone.bit()
        | Ieee802154Event::TxDone.bit()
        | Ieee802154Event::TxAbort.bit();
    if acknowledgement == MacTransmitAcknowledgement::Expected {
        allowed |= Ieee802154Event::AckRxDone.bit() | Ieee802154Event::Timer0Overflow.bit();
    }
    reject_unexpected_events(batch, allowed)?;

    let tx_done = batch.events().contains(Ieee802154Event::TxDone);
    let ack_done = batch.events().contains(Ieee802154Event::AckRxDone);
    let timer_done = batch.events().contains(Ieee802154Event::Timer0Overflow);
    let mut completion = None;
    let mut phase = MacActivePhase::Transmit {
        access,
        acknowledgement,
    };

    // Reviewed order is TX_DONE, ACK_RX_DONE, TX_ABORT, TIMER0. The vendor
    // deferred flag uses assignment rather than OR.
    if tx_done {
        match acknowledgement {
            MacTransmitAcknowledgement::None => {
                completion = Some(MacCompletion::TransmitComplete);
            }
            MacTransmitAcknowledgement::Expected => {
                phase = MacActivePhase::AwaitingAcknowledgement { access };
            }
        }
    }

    if ack_done {
        if completion.is_some() {
            return Err(conflicting_terminal_batch(batch));
        }
        completion = Some(MacCompletion::TransmitAcknowledged);
    }

    if let Some(reason) = batch.tx_abort_reason() {
        match phase {
            MacActivePhase::Transmit { .. } => {
                if !is_terminal_transmit_abort(reason, access) {
                    return Err(MacBatchRejectReason::UnexpectedTxAbortReason(reason));
                }
                if completion.is_some() {
                    return Err(conflicting_terminal_batch(batch));
                }
                completion = Some(MacCompletion::TransmitAborted(reason));
            }
            MacActivePhase::AwaitingAcknowledgement { .. } => {
                if reason == Ieee802154TxAbortReason::RxAckTimeout {
                    if completion.is_some() {
                        return Err(conflicting_terminal_batch(batch));
                    }
                    completion = Some(MacCompletion::AcknowledgementTimedOutByAbort);
                } else {
                    // Invalid-ACK handlers stop timer zero before assigning
                    // NEEDS_NEXT_OPT(false). This bounded actor has no
                    // timeout-disarmed RX_ACK phase, so accepting one would
                    // make a later timer event falsely terminal.
                    return Err(MacBatchRejectReason::UnexpectedTxAbortReason(reason));
                }
            }
            _ => unreachable!("transmit evaluation has only TX and RX_ACK phases"),
        }
    }

    if timer_done {
        if !matches!(phase, MacActivePhase::AwaitingAcknowledgement { .. }) {
            return Err(MacBatchRejectReason::UnexpectedEventBits {
                bits: Ieee802154Event::Timer0Overflow.bit(),
            });
        }
        if completion.is_some() {
            return Err(conflicting_terminal_batch(batch));
        }
        completion = Some(MacCompletion::AcknowledgementTimedOutByTimer);
    }

    match completion {
        Some(completion) => Ok(Evaluation::Terminal(completion)),
        None => Ok(Evaluation::Pending(phase)),
    }
}

fn evaluate_acknowledgement(
    batch: MacEventBatch,
    access: MacTransmitAccess,
) -> Result<Evaluation, MacBatchRejectReason> {
    const ALLOWED: u16 = Ieee802154Event::RxSfdDone.bit()
        | Ieee802154Event::AckRxDone.bit()
        | Ieee802154Event::TxAbort.bit()
        | Ieee802154Event::Timer0Overflow.bit();
    reject_unexpected_events(batch, ALLOWED)?;
    reject_conflicting(
        batch,
        Ieee802154Event::AckRxDone.bit()
            | Ieee802154Event::TxAbort.bit()
            | Ieee802154Event::Timer0Overflow.bit(),
    )?;

    if let Some(reason) = batch.tx_abort_reason() {
        if reason == Ieee802154TxAbortReason::RxAckTimeout {
            return Ok(Evaluation::Terminal(
                MacCompletion::AcknowledgementTimedOutByAbort,
            ));
        }
        // See the sequential fold above: all non-timeout ACK aborts require a
        // timeout-disarmed phase deliberately absent from this iteration.
        return Err(MacBatchRejectReason::UnexpectedTxAbortReason(reason));
    }
    if batch.events().contains(Ieee802154Event::AckRxDone) {
        return Ok(Evaluation::Terminal(MacCompletion::TransmitAcknowledged));
    }
    if batch.events().contains(Ieee802154Event::Timer0Overflow) {
        return Ok(Evaluation::Terminal(
            MacCompletion::AcknowledgementTimedOutByTimer,
        ));
    }
    Ok(Evaluation::Pending(
        MacActivePhase::AwaitingAcknowledgement { access },
    ))
}

fn evaluate_cca(batch: MacEventBatch) -> Result<Evaluation, MacBatchRejectReason> {
    const ALLOWED: u16 = Ieee802154Event::EdDone.bit() | Ieee802154Event::RxAbort.bit();
    reject_unexpected_events(batch, ALLOWED)?;
    reject_conflicting(batch, ALLOWED)?;

    if let Some(reason) = batch.rx_abort_reason() {
        if !is_terminal_measurement_abort(reason) {
            return Err(MacBatchRejectReason::UnexpectedRxAbortReason(reason));
        }
        return Ok(Evaluation::Terminal(
            MacCompletion::ClearChannelAssessmentAborted(reason),
        ));
    }
    match batch.measurement() {
        Some(MacMeasurementSample::ClearChannel(sample)) => Ok(Evaluation::Terminal(
            MacCompletion::ClearChannelAssessment(sample),
        )),
        Some(measurement) => Err(MacBatchRejectReason::UnexpectedMeasurement(measurement)),
        None => unreachable!("a validated ED_DONE batch always contains a measurement"),
    }
}

fn evaluate_energy_detection(batch: MacEventBatch) -> Result<Evaluation, MacBatchRejectReason> {
    const ALLOWED: u16 = Ieee802154Event::EdDone.bit() | Ieee802154Event::RxAbort.bit();
    reject_unexpected_events(batch, ALLOWED)?;
    reject_conflicting(batch, ALLOWED)?;

    if let Some(reason) = batch.rx_abort_reason() {
        if !is_terminal_measurement_abort(reason) {
            return Err(MacBatchRejectReason::UnexpectedRxAbortReason(reason));
        }
        return Ok(Evaluation::Terminal(MacCompletion::EnergyDetectionAborted(
            reason,
        )));
    }
    match batch.measurement() {
        Some(MacMeasurementSample::Energy(sample)) => {
            Ok(Evaluation::Terminal(MacCompletion::EnergyDetection(sample)))
        }
        Some(measurement) => Err(MacBatchRejectReason::UnexpectedMeasurement(measurement)),
        None => unreachable!("a validated ED_DONE batch always contains a measurement"),
    }
}

fn reject_unexpected_events(
    batch: MacEventBatch,
    allowed: u16,
) -> Result<(), MacBatchRejectReason> {
    let unexpected = batch.events().bits() & !allowed;
    if unexpected == 0 {
        Ok(())
    } else {
        Err(MacBatchRejectReason::UnexpectedEventBits { bits: unexpected })
    }
}

fn reject_conflicting(batch: MacEventBatch, terminal: u16) -> Result<(), MacBatchRejectReason> {
    let conflicting = batch.events().bits() & terminal;
    if conflicting.count_ones() <= 1 {
        Ok(())
    } else {
        Err(MacBatchRejectReason::ConflictingTerminalEvents { bits: conflicting })
    }
}

fn conflicting_terminal_batch(batch: MacEventBatch) -> MacBatchRejectReason {
    MacBatchRejectReason::ConflictingTerminalEvents {
        bits: batch.events().bits()
            & (Ieee802154Event::TxDone.bit()
                | Ieee802154Event::AckRxDone.bit()
                | Ieee802154Event::TxAbort.bit()
                | Ieee802154Event::Timer0Overflow.bit()),
    }
}

const fn is_terminal_receive_abort(reason: Ieee802154RxAbortReason) -> bool {
    matches!(
        reason,
        Ieee802154RxAbortReason::SfdTimeout
            | Ieee802154RxAbortReason::CrcError
            | Ieee802154RxAbortReason::InvalidLength
            | Ieee802154RxAbortReason::FilterFail
            | Ieee802154RxAbortReason::NoRss
            | Ieee802154RxAbortReason::CoexistenceBreak
            | Ieee802154RxAbortReason::UnexpectedAck
            | Ieee802154RxAbortReason::RxRestart
    )
}

const fn is_terminal_transmit_abort(
    reason: Ieee802154TxAbortReason,
    access: MacTransmitAccess,
) -> bool {
    match reason {
        Ieee802154TxAbortReason::TxCoexistenceBreak | Ieee802154TxAbortReason::TxSecurityError => {
            true
        }
        Ieee802154TxAbortReason::CcaFailed | Ieee802154TxAbortReason::CcaBusy => {
            matches!(access, MacTransmitAccess::ClearChannelAssessment)
        }
        _ => false,
    }
}

const fn is_terminal_measurement_abort(reason: Ieee802154RxAbortReason) -> bool {
    matches!(
        reason,
        Ieee802154RxAbortReason::EdAbort | Ieee802154RxAbortReason::EdCoexistenceReject
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_ieee802154_dma::{
        DMA_LOW, DmaFrameAddress, PinnedRxPool, PinnedTxBuffer, RxPoolStorage, TxStorage,
    };
    use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154EventMask;

    fn tx_owner(address: u32) -> PinnedTxBuffer {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(TxStorage::new()));
        TxStorage::pin_static_model(storage, DmaFrameAddress::try_new(address).unwrap())
    }

    fn rx_pool<const COUNT: usize>(address: u32) -> PinnedRxPool<COUNT> {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(RxPoolStorage::new()));
        RxPoolStorage::pin_static_model(storage, DmaFrameAddress::try_new(address).unwrap())
            .unwrap()
    }

    fn batch(events: Ieee802154EventMask) -> MacEventBatch {
        MacEventBatch::new(events, None, None, None).unwrap()
    }

    fn rx_abort(reason: Ieee802154RxAbortReason) -> MacEventBatch {
        MacEventBatch::new(Ieee802154Event::RxAbort.mask(), Some(reason), None, None).unwrap()
    }

    fn tx_abort(reason: Ieee802154TxAbortReason) -> MacEventBatch {
        MacEventBatch::new(Ieee802154Event::TxAbort.mask(), None, Some(reason), None).unwrap()
    }

    fn cca_done(sample: MacCcaSample) -> MacEventBatch {
        MacEventBatch::new(
            Ieee802154Event::EdDone.mask(),
            None,
            None,
            Some(MacMeasurementSample::ClearChannel(sample)),
        )
        .unwrap()
    }

    fn ed_done(sample: i8) -> MacEventBatch {
        MacEventBatch::new(
            Ieee802154Event::EdDone.mask(),
            None,
            None,
            Some(MacMeasurementSample::Energy(
                MacEnergySample::from_raw_code(sample),
            )),
        )
        .unwrap()
    }

    fn deferred<R>(outcome: MacBatchOutcome<R>) -> MacDeferred<R> {
        match outcome {
            MacBatchOutcome::Deferred(deferred) => deferred,
            MacBatchOutcome::Pending(_) => panic!("expected terminal batch"),
        }
    }

    fn pending<R>(outcome: MacBatchOutcome<R>) -> MacActive<R> {
        match outcome {
            MacBatchOutcome::Pending(active) => active,
            MacBatchOutcome::Deferred(_) => panic!("expected progress batch"),
        }
    }

    #[test]
    fn partial_start_plans_have_deterministic_order_and_bound_addresses() {
        let pool = rx_pool::<1>(DMA_LOW);
        let rx = pool.arm_next().unwrap();
        let active = MacReady::new_model().request_receive_without_auto_ack(rx);
        let plan = active.start_plan().unwrap();
        assert_eq!(plan.step_count(), 4);
        assert!(matches!(
            plan.step(0),
            Some(MacIntentStep::RequireStateSpecificQuiescence)
        ));
        assert!(matches!(
            plan.step(1),
            Some(MacIntentStep::RefreshStaticPolicy)
        ));
        let Some(MacIntentStep::PublishReceiveAddress(address)) = plan.step(2) else {
            panic!();
        };
        assert_eq!(address.as_u32(), DMA_LOW);
        assert!(matches!(
            plan.step(3),
            Some(MacIntentStep::RequestCommand(MacCommandIntent::Receive))
        ));
        assert!(plan.step(4).is_none());

        let mut tx_owner = tx_owner(DMA_LOW + 128);
        let tx = tx_owner.prepare(&[1]).unwrap().arm();
        let tx_active = MacReady::new_model()
            .request_transmit_without_ack(tx, MacTransmitAccess::ClearChannelAssessment);
        let tx_plan = tx_active.start_plan().unwrap();
        assert_eq!(tx_plan.step_count(), 5);
        assert!(matches!(
            tx_plan.step(3),
            Some(MacIntentStep::ConfigureEnergyDetectionDuration(8))
        ));
        assert!(matches!(
            tx_plan.step(4),
            Some(MacIntentStep::RequestCommand(
                MacCommandIntent::TransmitWithClearChannelAssessment
            ))
        ));
    }

    #[test]
    fn public_ed_duration_narrowing_fails_closed() {
        for units in [0_u32, 1, u16::MAX as u32] {
            let duration = MacEnergyDetectionDuration::try_from_public_units(units).unwrap();
            assert_eq!(u32::from(duration.hardware_units()), units);
        }
        for units in [u16::MAX as u32 + 1, u32::MAX] {
            assert_eq!(
                MacEnergyDetectionDuration::try_from_public_units(units),
                Err(MacEnergyDetectionDurationError::OutOfHardwareSubset { units })
            );
        }
    }

    #[test]
    fn transmit_with_ack_plan_publishes_tx_before_rx() {
        let mut tx_owner = tx_owner(DMA_LOW);
        let tx = tx_owner.prepare(&[1]).unwrap().arm();
        let pool = rx_pool::<1>(DMA_LOW + 128);
        let rx = pool.arm_next().unwrap();
        let active =
            MacReady::new_model().request_transmit_with_ack(tx, rx, MacTransmitAccess::Direct);
        let plan = active.start_plan().unwrap();
        assert_eq!(plan.step_count(), 5);
        let Some(MacIntentStep::PublishTransmitAddress(tx_address)) = plan.step(2) else {
            panic!();
        };
        let Some(MacIntentStep::PublishReceiveAddress(rx_address)) = plan.step(3) else {
            panic!();
        };
        assert_eq!(tx_address.as_u32(), DMA_LOW);
        assert_eq!(rx_address.as_u32(), DMA_LOW + 128);
    }

    #[test]
    fn receive_progress_terminal_rejection_and_reclaim_are_linear() {
        let pool = rx_pool::<1>(DMA_LOW);
        let rx = pool.arm_next().unwrap();
        let active = MacReady::new_model().request_receive_without_auto_ack(rx);
        let active = pending(
            active
                .process_batch(batch(Ieee802154Event::RxSfdDone.mask()))
                .unwrap(),
        );
        assert_eq!(active.phase(), MacActivePhase::Receive);

        let rejected = active
            .process_batch(batch(Ieee802154Event::TxDone.mask()))
            .expect_err("TX_DONE cannot complete RX");
        assert_eq!(
            rejected.reason(),
            MacBatchRejectReason::UnexpectedEventBits {
                bits: Ieee802154Event::TxDone.bit()
            }
        );
        let active = rejected.into_active();
        let completion = deferred(
            active
                .process_batch(batch(Ieee802154Event::RxDone.mask()))
                .unwrap(),
        );
        assert_eq!(completion.completion(), MacCompletion::ReceiveFrame);
        let resolved = completion
            .resolve_model(MacDeferredNext::ReceiveWhenIdle)
            .unwrap();
        let (_ready, reclaimed, result, next) = resolved.into_parts();
        match reclaimed {
            MacModelResolvedRx::Frame(frame) => frame.release().unwrap(),
            MacModelResolvedRx::Stub(stub) => stub.discard().unwrap(),
        }
        assert_eq!(result, MacCompletion::ReceiveFrame);
        assert_eq!(next, MacDeferredNext::ReceiveWhenIdle);
    }

    #[test]
    fn no_ack_transmit_has_one_terminal_edge() {
        for access in [
            MacTransmitAccess::Direct,
            MacTransmitAccess::ClearChannelAssessment,
        ] {
            let mut owner = tx_owner(DMA_LOW);
            let armed = owner.prepare(&[1]).unwrap().arm();
            let active = MacReady::new_model().request_transmit_without_ack(armed, access);
            let completion = deferred(
                active
                    .process_batch(batch(
                        Ieee802154Event::TxSfdDone
                            .mask()
                            .union(Ieee802154Event::TxDone.mask()),
                    ))
                    .unwrap(),
            );
            assert_eq!(completion.completion(), MacCompletion::TransmitComplete);
            let resolved = completion.resolve_model(MacDeferredNext::IdlePolicy);
            let (_ready, completed, _, _) = resolved.into_parts();
            completed.release();
        }
    }

    #[test]
    fn ack_transmit_advances_then_defers_once_for_terminal_batch() {
        let mut owner = tx_owner(DMA_LOW);
        let tx = owner.prepare(&[1]).unwrap().arm();
        let pool = rx_pool::<1>(DMA_LOW + 128);
        let rx = pool.arm_next().unwrap();
        let active =
            MacReady::new_model().request_transmit_with_ack(tx, rx, MacTransmitAccess::Direct);
        let active = pending(
            active
                .process_batch(batch(Ieee802154Event::TxDone.mask()))
                .unwrap(),
        );
        assert_eq!(
            active.phase(),
            MacActivePhase::AwaitingAcknowledgement {
                access: MacTransmitAccess::Direct
            }
        );
        assert!(
            active.start_plan().is_none(),
            "RX_ACK must not expose a second transmit start intent"
        );
        let completion = deferred(
            active
                .process_batch(batch(
                    Ieee802154Event::RxSfdDone
                        .mask()
                        .union(Ieee802154Event::AckRxDone.mask()),
                ))
                .unwrap(),
        );
        assert_eq!(completion.completion(), MacCompletion::TransmitAcknowledged);
        let resolved = completion
            .resolve_model(MacDeferredNext::IdlePolicy)
            .unwrap();
        let (_ready, reclaimed, _, _) = resolved.into_parts();
        let (tx, rx) = reclaimed.into_parts();
        tx.release();
        match rx {
            MacModelResolvedRx::Frame(frame) => frame.release().unwrap(),
            MacModelResolvedRx::Stub(stub) => stub.discard().unwrap(),
        }
    }

    #[test]
    fn tx_done_and_ack_done_can_share_one_reviewed_order_batch() {
        let mut owner = tx_owner(DMA_LOW);
        let tx = owner.prepare(&[1]).unwrap().arm();
        let pool = rx_pool::<1>(DMA_LOW + 128);
        let rx = pool.arm_next().unwrap();
        let active = MacReady::new_model().request_transmit_with_ack(
            tx,
            rx,
            MacTransmitAccess::ClearChannelAssessment,
        );
        let events = Ieee802154Event::TxDone
            .mask()
            .union(Ieee802154Event::AckRxDone.mask());
        let completion = deferred(active.process_batch(batch(events)).unwrap());
        assert_eq!(completion.completion(), MacCompletion::TransmitAcknowledged);
    }

    #[test]
    fn delayed_ack_is_legal_in_tx_but_multiple_terminals_fail_closed() {
        let mut owner = tx_owner(DMA_LOW);
        let tx = owner.prepare(&[1]).unwrap().arm();
        let pool = rx_pool::<1>(DMA_LOW + 128);
        let rx = pool.arm_next().unwrap();
        let active =
            MacReady::new_model().request_transmit_with_ack(tx, rx, MacTransmitAccess::Direct);
        let completion = deferred(
            active
                .process_batch(batch(Ieee802154Event::AckRxDone.mask()))
                .unwrap(),
        );
        assert_eq!(completion.completion(), MacCompletion::TransmitAcknowledged);

        let mut owner = tx_owner(DMA_LOW + 256);
        let tx = owner.prepare(&[1]).unwrap().arm();
        let pool = rx_pool::<1>(DMA_LOW + 384);
        let active = MacReady::new_model().request_transmit_with_ack(
            tx,
            pool.arm_next().unwrap(),
            MacTransmitAccess::Direct,
        );
        let conflicting = MacEventBatch::new(
            Ieee802154Event::AckRxDone
                .mask()
                .union(Ieee802154Event::TxAbort.mask()),
            None,
            Some(Ieee802154TxAbortReason::TxSecurityError),
            None,
        )
        .unwrap();
        let rejected = active
            .process_batch(conflicting)
            .expect_err("success plus abort is ambiguous");
        assert!(matches!(
            rejected.reason(),
            MacBatchRejectReason::ConflictingTerminalEvents { .. }
        ));
    }

    #[test]
    fn ack_timeout_sources_are_terminal_only_after_tx_done() {
        for by_timer in [false, true] {
            let mut owner = tx_owner(DMA_LOW);
            let tx = owner.prepare(&[1]).unwrap().arm();
            let pool = rx_pool::<1>(DMA_LOW + 128);
            let rx = pool.arm_next().unwrap();
            let active =
                MacReady::new_model().request_transmit_with_ack(tx, rx, MacTransmitAccess::Direct);
            let active = pending(
                active
                    .process_batch(batch(Ieee802154Event::TxDone.mask()))
                    .unwrap(),
            );
            let terminal = if by_timer {
                batch(Ieee802154Event::Timer0Overflow.mask())
            } else {
                tx_abort(Ieee802154TxAbortReason::RxAckTimeout)
            };
            let completion = deferred(active.process_batch(terminal).unwrap());
            assert_eq!(
                completion.completion(),
                if by_timer {
                    MacCompletion::AcknowledgementTimedOutByTimer
                } else {
                    MacCompletion::AcknowledgementTimedOutByAbort
                }
            );
        }
    }

    #[test]
    fn invalid_ack_abort_is_rejected_and_timer_requires_valid_fold() {
        let mut owner = tx_owner(DMA_LOW);
        let tx = owner.prepare(&[1]).unwrap().arm();
        let pool = rx_pool::<1>(DMA_LOW + 128);
        let active = MacReady::new_model().request_transmit_with_ack(
            tx,
            pool.arm_next().unwrap(),
            MacTransmitAccess::Direct,
        );
        let invalid_ack = MacEventBatch::new(
            Ieee802154Event::TxDone
                .mask()
                .union(Ieee802154Event::TxAbort.mask()),
            None,
            Some(Ieee802154TxAbortReason::RxAckCrcError),
            None,
        )
        .unwrap();
        let rejected = active
            .process_batch(invalid_ack)
            .expect_err("invalid-ACK abort disarms the opaque vendor timer callback");
        assert_eq!(
            rejected.reason(),
            MacBatchRejectReason::UnexpectedTxAbortReason(Ieee802154TxAbortReason::RxAckCrcError)
        );
        let active = rejected.into_active();
        assert_eq!(
            active.phase(),
            MacActivePhase::Transmit {
                access: MacTransmitAccess::Direct,
                acknowledgement: MacTransmitAcknowledgement::Expected,
            }
        );

        let ordered_timeout = batch(
            Ieee802154Event::TxDone
                .mask()
                .union(Ieee802154Event::Timer0Overflow.mask()),
        );
        let terminal = deferred(active.process_batch(ordered_timeout).unwrap());
        assert_eq!(
            terminal.completion(),
            MacCompletion::AcknowledgementTimedOutByTimer
        );
    }

    #[test]
    fn transmit_abort_reasons_are_access_specific() {
        for reason in [
            Ieee802154TxAbortReason::TxCoexistenceBreak,
            Ieee802154TxAbortReason::TxSecurityError,
        ] {
            let mut owner = tx_owner(DMA_LOW);
            let armed = owner.prepare(&[1]).unwrap().arm();
            let completion = deferred(
                MacReady::new_model()
                    .request_transmit_without_ack(armed, MacTransmitAccess::Direct)
                    .process_batch(tx_abort(reason))
                    .unwrap(),
            );
            assert_eq!(
                completion.completion(),
                MacCompletion::TransmitAborted(reason)
            );
        }

        let mut owner = tx_owner(DMA_LOW);
        let armed = owner.prepare(&[1]).unwrap().arm();
        let active =
            MacReady::new_model().request_transmit_without_ack(armed, MacTransmitAccess::Direct);
        let rejected = active
            .process_batch(tx_abort(Ieee802154TxAbortReason::CcaBusy))
            .expect_err("CCA_BUSY requires the combined TX_CCA state");
        assert_eq!(
            rejected.reason(),
            MacBatchRejectReason::UnexpectedTxAbortReason(Ieee802154TxAbortReason::CcaBusy)
        );
        let active = rejected.into_active();
        assert_eq!(
            active.phase(),
            MacActivePhase::Transmit {
                access: MacTransmitAccess::Direct,
                acknowledgement: MacTransmitAcknowledgement::None
            }
        );
    }

    #[test]
    fn standalone_cca_and_ed_require_their_own_sample_kind() {
        for sample in [MacCcaSample::Clear, MacCcaSample::Busy] {
            let completion = deferred(
                MacReady::new_model()
                    .request_clear_channel_assessment()
                    .process_batch(cca_done(sample))
                    .unwrap(),
            );
            assert_eq!(
                completion.completion(),
                MacCompletion::ClearChannelAssessment(sample)
            );
            let resolved = completion.resolve(MacDeferredNext::IdlePolicy);
            let (_ready, no_dma, result, next) = resolved.into_parts();
            assert_eq!(no_dma, MacNoDmaResources { _private: () });
            assert_eq!(result, MacCompletion::ClearChannelAssessment(sample));
            assert_eq!(next, MacDeferredNext::IdlePolicy);
        }

        for sample in [i8::MIN, -1, 0, i8::MAX] {
            let completion = deferred(
                MacReady::new_model()
                    .request_energy_detection(MacEnergyDetectionDuration::from_hardware_units(99))
                    .process_batch(ed_done(sample))
                    .unwrap(),
            );
            assert_eq!(
                completion.completion(),
                MacCompletion::EnergyDetection(MacEnergySample::from_raw_code(sample))
            );
        }

        let rejected = MacReady::new_model()
            .request_clear_channel_assessment()
            .process_batch(ed_done(-42))
            .expect_err("energy sample cannot complete CCA");
        assert!(matches!(
            rejected.reason(),
            MacBatchRejectReason::UnexpectedMeasurement(MacMeasurementSample::Energy(_))
        ));
    }

    #[test]
    fn only_source_terminal_measurement_aborts_are_accepted() {
        for reason in [
            Ieee802154RxAbortReason::EdAbort,
            Ieee802154RxAbortReason::EdCoexistenceReject,
        ] {
            let cca = deferred(
                MacReady::new_model()
                    .request_clear_channel_assessment()
                    .process_batch(rx_abort(reason))
                    .unwrap(),
            );
            assert_eq!(
                cca.completion(),
                MacCompletion::ClearChannelAssessmentAborted(reason)
            );
            let ed = deferred(
                MacReady::new_model()
                    .request_energy_detection(MacEnergyDetectionDuration::from_hardware_units(1))
                    .process_batch(rx_abort(reason))
                    .unwrap(),
            );
            assert_eq!(
                ed.completion(),
                MacCompletion::EnergyDetectionAborted(reason)
            );
        }
        let rejected = MacReady::new_model()
            .request_clear_channel_assessment()
            .process_batch(rx_abort(Ieee802154RxAbortReason::EdStop))
            .expect_err("vendor EdStop does not request deferred next operation");
        assert_eq!(
            rejected.reason(),
            MacBatchRejectReason::UnexpectedRxAbortReason(Ieee802154RxAbortReason::EdStop)
        );
    }

    #[test]
    fn receive_abort_reason_domain_is_exhaustive() {
        const REASONS: [Ieee802154RxAbortReason; 16] = [
            Ieee802154RxAbortReason::RxStop,
            Ieee802154RxAbortReason::SfdTimeout,
            Ieee802154RxAbortReason::CrcError,
            Ieee802154RxAbortReason::InvalidLength,
            Ieee802154RxAbortReason::FilterFail,
            Ieee802154RxAbortReason::NoRss,
            Ieee802154RxAbortReason::CoexistenceBreak,
            Ieee802154RxAbortReason::UnexpectedAck,
            Ieee802154RxAbortReason::RxRestart,
            Ieee802154RxAbortReason::TxAckTimeout,
            Ieee802154RxAbortReason::TxAckStop,
            Ieee802154RxAbortReason::TxAckCoexistenceBreak,
            Ieee802154RxAbortReason::EnhancedAckSecurityError,
            Ieee802154RxAbortReason::EdAbort,
            Ieee802154RxAbortReason::EdStop,
            Ieee802154RxAbortReason::EdCoexistenceReject,
        ];
        for reason in REASONS {
            let pool = rx_pool::<1>(DMA_LOW);
            let active =
                MacReady::new_model().request_receive_without_auto_ack(pool.arm_next().unwrap());
            let result = active.process_batch(rx_abort(reason));
            assert_eq!(result.is_ok(), is_terminal_receive_abort(reason));
        }
    }

    #[test]
    fn every_named_event_subset_is_deterministic_for_receive_and_cca() {
        for raw in 0_u16..=0x3fff {
            let Ok(events) = Ieee802154EventMask::from_named_bits(raw) else {
                continue;
            };
            if events.contains(Ieee802154Event::ClockCountMatch) {
                continue;
            }
            let rx_reason = events
                .contains(Ieee802154Event::RxAbort)
                .then_some(Ieee802154RxAbortReason::CrcError);
            let tx_reason = events
                .contains(Ieee802154Event::TxAbort)
                .then_some(Ieee802154TxAbortReason::TxSecurityError);
            let measurement = events
                .contains(Ieee802154Event::EdDone)
                .then_some(MacMeasurementSample::ClearChannel(MacCcaSample::Clear));
            let event_batch =
                MacEventBatch::new(events, rx_reason, tx_reason, measurement).unwrap();

            let pool = rx_pool::<1>(DMA_LOW);
            let receive =
                MacReady::new_model().request_receive_without_auto_ack(pool.arm_next().unwrap());
            let first = receive.process_batch(event_batch);
            let receive_again = match first {
                Ok(MacBatchOutcome::Pending(active)) => active.process_batch(event_batch).is_ok(),
                Ok(MacBatchOutcome::Deferred(_)) => true,
                Err(rejected) => rejected.into_active().process_batch(event_batch).is_err(),
            };
            assert!(receive_again, "RX mask {raw:#06x} was nondeterministic");

            let cca = MacReady::new_model().request_clear_channel_assessment();
            let first = cca.process_batch(event_batch);
            let cca_again = match first {
                Ok(MacBatchOutcome::Pending(active)) => active.process_batch(event_batch).is_ok(),
                Ok(MacBatchOutcome::Deferred(_)) => true,
                Err(rejected) => rejected.into_active().process_batch(event_batch).is_err(),
            };
            assert!(cca_again, "CCA mask {raw:#06x} was nondeterministic");
        }
    }
}
