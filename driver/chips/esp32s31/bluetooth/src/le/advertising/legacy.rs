#![forbid(unsafe_code)]

//! ESP32-S31 hardware ownership for restricted legacy advertising.
//!
//! This boundary lowers one portable `ADV_NONCONN_IND` event into a bounded PDU
//! and a reviewed 1--3 item S31 descriptor chain. The complete selected-channel
//! event advances through scheduler bookkeeping, `HEAD`/interrupt/event/`RUN`
//! publication, fenced completion, hardware-head retirement, software unlink
//! and CPU recycle. Only that complete lower proof may advance the portable LL
//! owner. Returned per-item statuses remain diagnostic and make no claim about
//! observability on air.

#[cfg(target_arch = "riscv32")]
pub(crate) mod active;
#[cfg(target_arch = "riscv32")]
pub(crate) mod completion;
#[cfg(target_arch = "riscv32")]
pub(crate) mod recurring;
#[cfg(target_arch = "riscv32")]
pub(crate) mod runner;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod timing;

#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_ll::advertiser::{
    LegacyAdvertiserEventComplete, LegacyAdvertiserEventInFlight, LegacyAdvertiserScheduled,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_ll::advertising::AdvertisingDelay;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_bluetooth_ll::advertising::PrimaryAdvertisingChannel;
use open_esp_radio_bluetooth_ll::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertiser::{LegacyAdvertiserEnabled, LegacyAdvertiserEventPrepared, LegacyAdvertiserStandby},
    advertising::{
        AdvertisingInterval, LEGACY_ADVERTISING_PDU_CAPACITY, LegacyAdvertisingData,
        LegacyAdvertisingDataError, LegacyAdvertisingEncodeError,
        LegacyNonconnectableAdvertisement, LegacyNonconnectableAdvertisingSet,
        PrimaryAdvertisingChannelMap, PrimaryAdvertisingChannelMapError,
    },
    advertising_lifecycle::LegacyAdvertisingEventIdentity,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingMemoryGraphIdentity;
#[cfg(not(target_arch = "riscv32"))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingMemoryGraphModelAddress;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLeTxPacketPrepareError, BluetoothLegacyAdvertisingMemoryGraphBindFailure,
    BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    BluetoothLegacyAdvertisingMemoryGraphLinkStateReset,
    BluetoothLegacyAdvertisingMemoryGraphPacketPrepared,
    BluetoothLegacyAdvertisingMemoryGraphStorage, BluetoothLegacyAdvertisingPduError,
};

/// Why an accepted HCI snapshot could not become the portable LL role.
///
/// Every variant is a defensive cross-layer invariant: the HCI decoder has
/// already checked these domains. Keeping the failure typed prevents future
/// command expansion from silently reaching descriptor preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingSetError {
    Data(LegacyAdvertisingDataError),
    Channels(PrimaryAdvertisingChannelMapError),
    Interval,
}

/// Project one immutable HCI Enable snapshot into the portable LL lifecycle.
///
/// The S31 policy selects the Host's minimum interval, which is always inside
/// the accepted range and minimizes first-event latency. Advertising data is
/// copied into a self-contained LL owner so the async actor is not
/// self-referential and does not borrow the HCI configuration store.
pub fn prepare_legacy_advertising_set(
    request: open_esp_radio_bluetooth_hci::LeLegacyNonconnectableAdvertisingEnableRequest,
) -> Result<LegacyNonconnectableAdvertisingSet<'static>, BluetoothLegacyAdvertisingSetError> {
    let parameters = request.parameters();
    let advertiser = request.advertiser();
    let address_kind = match advertiser {
        open_esp_radio_bluetooth_hci::LeLegacyAdvertisingAddress::Public(_) => {
            LeDeviceAddressKind::Public
        }
        open_esp_radio_bluetooth_hci::LeLegacyAdvertisingAddress::Random(_) => {
            LeDeviceAddressKind::Random
        }
    };
    let wire_address = advertiser.wire_address();
    let mut wire_bytes = [0; 6];
    wire_bytes.copy_from_slice(wire_address.raw());
    let advertisement = LegacyNonconnectableAdvertisement::new(
        LeDeviceAddress::from_wire_bytes(wire_bytes, address_kind),
        LegacyAdvertisingData::new_owned(request.data().as_bytes())
            .map_err(BluetoothLegacyAdvertisingSetError::Data)?,
    );
    let channels = PrimaryAdvertisingChannelMap::new(
        parameters.channels().channel_37(),
        parameters.channels().channel_38(),
        parameters.channels().channel_39(),
    )
    .map_err(BluetoothLegacyAdvertisingSetError::Channels)?;
    let interval =
        AdvertisingInterval::new(u32::from(parameters.interval().minimum_units_625_us()))
            .map_err(|_| BluetoothLegacyAdvertisingSetError::Interval)?;
    Ok(LegacyNonconnectableAdvertisingSet::new(
        advertisement,
        channels,
        interval,
    ))
}
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLegacyAdvertisingEventCompletionStatuses,
    BluetoothLegacyAdvertisingMemoryGraphCompletionObservation,
    BluetoothLegacyAdvertisingMemoryGraphCompletionObserved,
    BluetoothLegacyAdvertisingMemoryGraphHeadPublished,
    BluetoothLegacyAdvertisingMemoryGraphRecycleError,
    BluetoothLegacyAdvertisingMemoryGraphRecyclePrepared,
    BluetoothLegacyAdvertisingMemoryGraphRecycled, BluetoothLegacyAdvertisingMemoryGraphRunning,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLegacyAdvertisingMemoryGraphEmptyListLinkPrepared,
    BluetoothLegacyAdvertisingMemoryGraphEventPrepareError,
    BluetoothLegacyAdvertisingMemoryGraphEventPrepared,
    BluetoothLegacyAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    BluetoothLegacyAdvertisingPrimaryChannelPlan,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareRunCommandPublished, BluetoothSchedulerSoftwareListRemovalReady,
};

#[cfg(target_arch = "riscv32")]
use crate::BluetoothLegacyAdvertisingRecurringTimingObservation;
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    BluetoothLegacyAdvertisingEventWindow, BluetoothLegacyAdvertisingTimingObservation,
    BluetoothSchedulerRawWindow, BluetoothSchedulerSoftwareConfig,
};

/// Requested default transmit power for legacy advertising.
///
/// The physical dBm request remains semantic at this boundary. Its private S31
/// descriptor encoding is selected only by the controller-memory codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLegacyAdvertisingDefaultTxPowerDbm(i8);

impl BluetoothLegacyAdvertisingDefaultTxPowerDbm {
    pub const fn new(dbm: i8) -> Self {
        Self(dbm)
    }

    pub const fn dbm(self) -> i8 {
        self.0
    }
}

/// A second advertising epoch cannot check out the sole controller graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingRuntimeBeginError {
    EventActive,
    GenerationExhausted,
    Preparation(BluetoothLegacyAdvertisingPreparationErrorKind),
}

/// Composition-owned graph and physical power policy for legacy advertising.
///
/// An empty slot means that the exact graph is retained by an affine event
/// typestate. Dropping that event cannot recreate availability.
#[must_use = "the advertising runtime retains the sole production graph"]
pub struct BluetoothLegacyAdvertisingRuntimeResources {
    default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    #[cfg(any(target_arch = "riscv32", test))]
    graph_identity: BluetoothLegacyAdvertisingMemoryGraphIdentity,
    standby: Option<LegacyAdvertiserStandby>,
    idle: Option<BluetoothLegacyAdvertisingMemoryGraphCpuOwned>,
}

/// Result of returning one cancelled event to its exact runtime.
#[must_use = "reuse Restored or retain the rejected affine event owner"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) enum BluetoothLegacyAdvertisingCancelledRestoreOutcome<'a> {
    /// Both the portable generation and SRAM graph are idle in this runtime.
    Restored,
    /// The runtime was occupied or the graph identity did not match.
    Rejected(BluetoothLegacyAdvertisingCancelled<'a>),
}

/// One runtime-owned event with its physical policy retained atomically.
#[must_use = "prepare the checked-out advertising event or return it to its runtime"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothLegacyAdvertisingRuntimeEvent {
    prepared: BluetoothLegacyAdvertisingPrepared<'static>,
    default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyAdvertisingRuntimeEvent {
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothLegacyAdvertisingPrepared<'static>,
        BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    ) {
        (self.prepared, self.default_tx_power_dbm)
    }
}

impl BluetoothLegacyAdvertisingRuntimeResources {
    fn from_claimed_graph(
        default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
        graph: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    ) -> Self {
        #[cfg(any(target_arch = "riscv32", test))]
        let graph_identity = graph.binding().identity();
        Self {
            default_tx_power_dbm,
            #[cfg(any(target_arch = "riscv32", test))]
            graph_identity,
            standby: Some(LegacyAdvertiserStandby::new()),
            idle: Some(graph),
        }
    }

    /// Bind one real statically placed advertising graph.
    #[cfg(target_arch = "riscv32")]
    pub fn claim_static(
        storage: &'static mut BluetoothLegacyAdvertisingMemoryGraphStorage,
        default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<Self, BluetoothLegacyAdvertisingMemoryGraphBindFailure> {
        let graph = BluetoothLegacyAdvertisingMemoryGraphStorage::pin_static(storage)?;
        Ok(Self::from_claimed_graph(default_tx_power_dbm, graph))
    }

    /// Bind one native model graph at a deterministic controller address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_static_model(
        storage: &'static mut BluetoothLegacyAdvertisingMemoryGraphStorage,
        base: BluetoothLegacyAdvertisingMemoryGraphModelAddress,
        default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<Self, BluetoothLegacyAdvertisingMemoryGraphBindFailure> {
        let graph = BluetoothLegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)?;
        Ok(Self::from_claimed_graph(default_tx_power_dbm, graph))
    }

    /// Physical default-power request retained with this exact graph.
    pub const fn default_tx_power_dbm(&self) -> BluetoothLegacyAdvertisingDefaultTxPowerDbm {
        self.default_tx_power_dbm
    }

    /// Whether no advertising event currently owns the graph.
    pub const fn event_is_idle(&self) -> bool {
        self.standby.is_some() && self.idle.is_some()
    }

    /// Begin one event from the retained generation and unique SRAM graph.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn begin_event(
        &mut self,
        set: LegacyNonconnectableAdvertisingSet<'static>,
    ) -> Result<BluetoothLegacyAdvertisingRuntimeEvent, BluetoothLegacyAdvertisingRuntimeBeginError>
    {
        if self.standby.is_none() || self.idle.is_none() {
            return Err(BluetoothLegacyAdvertisingRuntimeBeginError::EventActive);
        }
        let standby = self
            .standby
            .take()
            .expect("the complete idle runtime retains its advertiser generation");
        let memory = self
            .idle
            .take()
            .expect("the complete idle runtime retains its SRAM graph");
        let enabled = match standby.configure(set).enable() {
            Ok(enabled) => enabled,
            Err(failure) => {
                self.standby = Some(failure.into_configured().into_standby());
                self.idle = Some(memory);
                return Err(BluetoothLegacyAdvertisingRuntimeBeginError::GenerationExhausted);
            }
        };
        match BluetoothLegacyAdvertisingPrepared::prepare(enabled, memory) {
            Ok(prepared) => Ok(BluetoothLegacyAdvertisingRuntimeEvent {
                prepared,
                default_tx_power_dbm: self.default_tx_power_dbm,
            }),
            Err(failure) => {
                let (enabled, memory, error) = failure.into_parts();
                self.standby = Some(enabled.disable().into_standby());
                self.idle = Some(memory);
                Err(BluetoothLegacyAdvertisingRuntimeBeginError::Preparation(
                    error,
                ))
            }
        }
    }

    /// Return one pre-publication cancellation to this exact runtime.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn restore_cancelled(
        &mut self,
        cancelled: BluetoothLegacyAdvertisingCancelled<'static>,
    ) -> BluetoothLegacyAdvertisingCancelledRestoreOutcome<'static> {
        let (enabled, memory) = cancelled.into_parts();
        if self.standby.is_some()
            || self.idle.is_some()
            || memory.binding().identity() != self.graph_identity
        {
            return BluetoothLegacyAdvertisingCancelledRestoreOutcome::Rejected(
                BluetoothLegacyAdvertisingCancelled { enabled, memory },
            );
        }
        self.standby = Some(enabled.disable().into_standby());
        self.idle = Some(memory);
        BluetoothLegacyAdvertisingCancelledRestoreOutcome::Restored
    }

    /// Disable a completed Link Layer event and return its exact graph to this runtime.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection returns the complete affine event owner"
    )]
    pub(crate) fn restore_completed_disabled(
        &mut self,
        completed: BluetoothLegacyAdvertisingEventCompleted<'static>,
    ) -> Result<(), BluetoothLegacyAdvertisingEventCompleted<'static>> {
        let BluetoothLegacyAdvertisingEventCompleted {
            complete,
            memory,
            statuses,
            phase,
        } = completed;
        if self.standby.is_some()
            || self.idle.is_some()
            || memory.binding().identity() != self.graph_identity
        {
            return Err(BluetoothLegacyAdvertisingEventCompleted {
                complete,
                memory,
                statuses,
                phase,
            });
        }
        self.standby = Some(complete.disable().into_standby());
        self.idle = Some(memory);
        Ok(())
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn controller_channel_plan(
    selected: PrimaryAdvertisingChannelMap,
) -> BluetoothLegacyAdvertisingPrimaryChannelPlan {
    BluetoothLegacyAdvertisingPrimaryChannelPlan::new(
        selected.contains(PrimaryAdvertisingChannel::Channel37),
        selected.contains(PrimaryAdvertisingChannel::Channel38),
        selected.contains(PrimaryAdvertisingChannel::Channel39),
    )
    .expect("the portable advertising channel map is non-empty")
}

/// One fully encoded S31 legacy-advertising transmission before hardware admission.
///
/// The portable continuation remains private, so code cannot claim that the
/// transmission is in flight without first adding the missing sealed S31
/// hardware ticket at this boundary.
#[must_use = "admit through a reviewed hardware ticket, cancel, or retain the prepared owner"]
pub struct BluetoothLegacyAdvertisingPrepared<'a> {
    prepared: LegacyAdvertiserEventPrepared<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphPacketPrepared,
}

/// Result of applying the restricted S31 legacy-advertising link-state reset.
#[must_use = "advance Reset or retain the exact rejected prepared owner"]
pub enum BluetoothLegacyAdvertisingLinkStateResetOutcome<'a> {
    /// The portable event and SRAM graph advanced together.
    Reset(BluetoothLegacyAdvertisingLinkStateReset<'a>),
    /// The packet did not satisfy the restricted reset contract.
    Rejected {
        prepared: BluetoothLegacyAdvertisingPrepared<'a>,
        error: BluetoothLegacyAdvertisingPduError,
    },
}

impl<'a> BluetoothLegacyAdvertisingPrepared<'a> {
    /// Encode one complete portable event into bounded chip-owned storage.
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc failure must return both exact affine owners"
        )
    )]
    pub fn prepare(
        enabled: LegacyAdvertiserEnabled<'a>,
        memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    ) -> Result<Self, BluetoothLegacyAdvertisingPreparationError<'a>> {
        let prepared = enabled.prepare_event();
        let mut encoded = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
        let pdu_len = match prepared.encode(&mut encoded) {
            Ok(length) => length,
            Err(error) => {
                return Err(BluetoothLegacyAdvertisingPreparationError {
                    enabled: prepared.cancel(),
                    memory,
                    error: BluetoothLegacyAdvertisingPreparationErrorKind::PduEncoding(error),
                });
            }
        };
        let memory = match memory.prepare_packet(&encoded[..pdu_len]) {
            Ok(memory) => memory,
            Err(failure) => {
                let (memory, error) = failure.into_parts();
                return Err(BluetoothLegacyAdvertisingPreparationError {
                    enabled: prepared.cancel(),
                    memory,
                    error: BluetoothLegacyAdvertisingPreparationErrorKind::ControllerPacket(error),
                });
            }
        };
        Ok(Self { prepared, memory })
    }

    /// Exact portable generation/event identity retained by this owner.
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.prepared.identity()
    }

    /// Complete selected primary-channel map for this event.
    pub const fn channels(&self) -> PrimaryAdvertisingChannelMap {
        self.prepared.channels()
    }

    /// Complete encoded Link Layer PDU, excluding preamble, Access Address, CRC and whitening.
    pub fn pdu(&self) -> &[u8] {
        self.memory.pdu()
    }

    /// Apply the reviewed no-RX/no-CTE/no-privacy LE 1M link-state reset.
    pub fn reset_link_state(
        self,
        default_tx_power: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    ) -> BluetoothLegacyAdvertisingLinkStateResetOutcome<'a> {
        let Self { prepared, memory } = self;
        match memory.reset_link_state(default_tx_power.dbm()) {
            Ok(memory) => BluetoothLegacyAdvertisingLinkStateResetOutcome::Reset(
                BluetoothLegacyAdvertisingLinkStateReset { prepared, memory },
            ),
            Err(failure) => {
                let (memory, error) = failure.into_parts();
                BluetoothLegacyAdvertisingLinkStateResetOutcome::Rejected {
                    prepared: Self { prepared, memory },
                    error,
                }
            }
        }
    }

    /// Cancel before hardware admission and recover both affine owners.
    pub fn cancel(self) -> BluetoothLegacyAdvertisingCancelled<'a> {
        BluetoothLegacyAdvertisingCancelled {
            enabled: self.prepared.cancel(),
            memory: self.memory.cancel(),
        }
    }
}

/// One reset advertising graph which still lacks scheduler event timing.
#[must_use = "advance to event scheduling, cancel, or retain the reset owner"]
pub struct BluetoothLegacyAdvertisingLinkStateReset<'a> {
    prepared: LegacyAdvertiserEventPrepared<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphLinkStateReset,
}

/// Result of projecting one first-event window from a live timing observation.
#[must_use = "schedule Candidate or retain the reset owner after TimingRejected"]
#[cfg(any(target_arch = "riscv32", test))]
pub enum BluetoothLegacyAdvertisingFirstEventCandidateOutcome<'a> {
    /// The complete event window is representable in the retained epoch.
    Candidate(BluetoothLegacyAdvertisingFirstEventCandidate<'a>),
    /// Projection failed without modifying SRAM or timeline ownership.
    TimingRejected(BluetoothLegacyAdvertisingLinkStateReset<'a>),
}

impl<'a> BluetoothLegacyAdvertisingLinkStateReset<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.prepared.identity()
    }

    pub fn pdu(&self) -> &[u8] {
        self.memory.pdu()
    }

    /// Form the first scheduler candidate from one ordered live timing proof.
    ///
    /// No SRAM event field or timeline slot changes here. The projected raw
    /// window remains inseparable from the reset graph until the common
    /// scheduler accepts or cancels it.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn form_first_event_candidate(
        self,
        timing: BluetoothLegacyAdvertisingTimingObservation,
        config: BluetoothSchedulerSoftwareConfig,
    ) -> BluetoothLegacyAdvertisingFirstEventCandidateOutcome<'a> {
        let payload_length = self.memory.payload_length();
        let Some((scheduler_window, raw_window, raw_item_duration)) = timing.first_le_1m_window(
            config,
            payload_length,
            self.prepared.channels().channel_count(),
        ) else {
            return BluetoothLegacyAdvertisingFirstEventCandidateOutcome::TimingRejected(self);
        };
        BluetoothLegacyAdvertisingFirstEventCandidateOutcome::Candidate(
            BluetoothLegacyAdvertisingFirstEventCandidate {
                reset: self,
                scheduler_window,
                raw_window,
                raw_item_duration,
            },
        )
    }

    pub fn cancel(self) -> BluetoothLegacyAdvertisingCancelled<'a> {
        BluetoothLegacyAdvertisingCancelled {
            enabled: self.prepared.cancel(),
            memory: self.memory.cancel(),
        }
    }
}

/// First advertising event with live timing but no timeline reservation.
#[must_use = "the candidate must enter common scheduling, be cancelled, or retained"]
#[cfg(any(target_arch = "riscv32", test))]
pub struct BluetoothLegacyAdvertisingFirstEventCandidate<'a> {
    reset: BluetoothLegacyAdvertisingLinkStateReset<'a>,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
    raw_window: BluetoothSchedulerRawWindow,
    raw_item_duration: u32,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> BluetoothLegacyAdvertisingFirstEventCandidate<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.reset.identity()
    }

    pub fn pdu(&self) -> &[u8] {
        self.reset.pdu()
    }

    /// Duration of the projected scheduler reservation including preparation.
    pub const fn projected_window_duration(&self) -> u32 {
        self.raw_window.duration()
    }

    pub(crate) const fn raw_window(&self) -> BluetoothSchedulerRawWindow {
        self.raw_window
    }

    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub(crate) fn prepare_resolved_event_image(
        self,
        resolved_window: BluetoothSchedulerRawWindow,
    ) -> Result<
        BluetoothLegacyAdvertisingEventImagePrepared<'a>,
        BluetoothLegacyAdvertisingFirstEventImagePrepareFailure<'a>,
    > {
        let Self {
            reset,
            scheduler_window,
            raw_window,
            raw_item_duration,
        } = self;
        let BluetoothLegacyAdvertisingLinkStateReset { prepared, memory } = reset;
        let channels = controller_channel_plan(prepared.channels());
        match memory.prepare_event(channels, resolved_window.start(), raw_item_duration) {
            Ok(memory) => Ok(BluetoothLegacyAdvertisingEventImagePrepared {
                prepared,
                memory,
                scheduler_window,
            }),
            Err(failure) => {
                let (memory, error) = failure.into_parts();
                Err(BluetoothLegacyAdvertisingFirstEventImagePrepareFailure {
                    candidate: Self {
                        reset: BluetoothLegacyAdvertisingLinkStateReset { prepared, memory },
                        scheduler_window,
                        raw_window,
                        raw_item_duration,
                    },
                    error,
                })
            }
        }
    }

    /// Cancel before timeline admission and recover both ordinary owners.
    pub fn cancel(self) -> BluetoothLegacyAdvertisingCancelled<'a> {
        self.reset.cancel()
    }
}

/// Portable work paired with a complete CPU-owned first-event SRAM image.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the event image must remain paired with its scheduler reservation"]
pub(crate) struct BluetoothLegacyAdvertisingEventImagePrepared<'a> {
    prepared: LegacyAdvertiserEventPrepared<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphEventPrepared,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> BluetoothLegacyAdvertisingEventImagePrepared<'a> {
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.prepared.identity()
    }

    pub(crate) fn pdu(&self) -> &[u8] {
        self.memory.pdu()
    }

    pub(crate) const fn phase(&self) -> crate::BluetoothLegacyAdvertisingEventPhase {
        self.scheduler_window.phase()
    }

    pub(crate) fn prepare_scheduler_bookkeeping(
        self,
    ) -> BluetoothLegacyAdvertisingSchedulerBookkeepingPrepared<'a> {
        BluetoothLegacyAdvertisingSchedulerBookkeepingPrepared {
            prepared: self.prepared,
            memory: self.memory.prepare_scheduler_bookkeeping(),
            scheduler_window: self.scheduler_window,
        }
    }

    pub(crate) fn cancel(self) -> BluetoothLegacyAdvertisingCancelled<'a> {
        BluetoothLegacyAdvertisingCancelled {
            enabled: self.prepared.cancel(),
            memory: self.memory.cancel(),
        }
    }
}

/// Portable event paired with common scheduler bookkeeping.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothLegacyAdvertisingSchedulerBookkeepingPrepared<'a> {
    prepared: LegacyAdvertiserEventPrepared<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> BluetoothLegacyAdvertisingSchedulerBookkeepingPrepared<'a> {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) fn prepare_empty_list_link(
        self,
    ) -> BluetoothLegacyAdvertisingEmptyListLinkPrepared<'a> {
        BluetoothLegacyAdvertisingEmptyListLinkPrepared {
            prepared: self.prepared,
            memory: self.memory.prepare_empty_list_link(),
            scheduler_window: self.scheduler_window,
        }
    }

    pub(crate) fn cancel(self) -> BluetoothLegacyAdvertisingEventImagePrepared<'a> {
        BluetoothLegacyAdvertisingEventImagePrepared {
            prepared: self.prepared,
            memory: self.memory.cancel(),
            scheduler_window: self.scheduler_window,
        }
    }
}

/// CPU-owned advertising event joined to an independently proven empty list.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothLegacyAdvertisingEmptyListLinkPrepared<'a> {
    prepared: LegacyAdvertiserEventPrepared<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphEmptyListLinkPrepared,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> BluetoothLegacyAdvertisingEmptyListLinkPrepared<'a> {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_head_published(
        self,
        publication: &BluetoothSchedulerHardwareListHeadPublished,
    ) -> BluetoothLegacyAdvertisingHeadPublishedEvent<'a> {
        BluetoothLegacyAdvertisingHeadPublishedEvent {
            in_flight: self.prepared.into_submitted(),
            memory: self.memory.into_head_published(publication),
            scheduler_window: self.scheduler_window,
        }
    }

    pub(crate) fn cancel(self) -> BluetoothLegacyAdvertisingSchedulerBookkeepingPrepared<'a> {
        BluetoothLegacyAdvertisingSchedulerBookkeepingPrepared {
            prepared: self.prepared,
            memory: self.memory.cancel(),
            scheduler_window: self.scheduler_window,
        }
    }
}

/// Hardware-visible advertising event after exact scheduler-head publication.
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothLegacyAdvertisingHeadPublishedEvent<'a> {
    in_flight: LegacyAdvertiserEventInFlight<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphHeadPublished,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingHeadPublishedEvent<'a> {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) fn into_running(
        self,
        run: &BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothLegacyAdvertisingRunningEvent<'a> {
        BluetoothLegacyAdvertisingRunningEvent {
            in_flight: self.in_flight,
            memory: self.memory.into_running(run),
            scheduler_window: self.scheduler_window,
        }
    }
}

/// Hardware-owned advertising event admitted through the complete RUN suffix.
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothLegacyAdvertisingRunningEvent<'a> {
    in_flight: LegacyAdvertiserEventInFlight<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphRunning,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothLegacyAdvertisingRunningEvent<'_> {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingRunningEvent<'a> {
    pub(crate) fn observe_completion(
        self,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> BluetoothLegacyAdvertisingRunningEventCompletionObservation<'a> {
        let Self {
            in_flight,
            memory,
            scheduler_window,
        } = self;
        match memory.observe_completion(observed) {
            BluetoothLegacyAdvertisingMemoryGraphCompletionObservation::ListMismatch {
                owner,
                observed,
            } => BluetoothLegacyAdvertisingRunningEventCompletionObservation::ListMismatch {
                item: Self {
                    in_flight,
                    memory: owner,
                    scheduler_window,
                },
                observed,
            },
            BluetoothLegacyAdvertisingMemoryGraphCompletionObservation::StillInFlight(memory) => {
                BluetoothLegacyAdvertisingRunningEventCompletionObservation::StillInFlight(Self {
                    in_flight,
                    memory,
                    scheduler_window,
                })
            }
            BluetoothLegacyAdvertisingMemoryGraphCompletionObservation::CompletionObserved(
                memory,
            ) => BluetoothLegacyAdvertisingRunningEventCompletionObservation::CompletionObserved(
                BluetoothLegacyAdvertisingCompletionObservedEvent {
                    _in_flight: in_flight,
                    memory,
                    _scheduler_window: scheduler_window,
                },
            ),
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothLegacyAdvertisingRunningEventCompletionObservation<'a> {
    ListMismatch {
        item: BluetoothLegacyAdvertisingRunningEvent<'a>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(BluetoothLegacyAdvertisingRunningEvent<'a>),
    CompletionObserved(BluetoothLegacyAdvertisingCompletionObservedEvent<'a>),
}

/// Hardware-owned advertising event after a non-sentinel completion status.
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothLegacyAdvertisingCompletionObservedEvent<'a> {
    _in_flight: LegacyAdvertiserEventInFlight<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphCompletionObserved,
    _scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothLegacyAdvertisingCompletionObservedEvent<'_> {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingCompletionObservedEvent<'a> {
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure retains the exact completed event and removal proof"
    )]
    pub(crate) fn prepare_recycle(
        self,
        removal: BluetoothSchedulerSoftwareListRemovalReady,
    ) -> Result<
        BluetoothLegacyAdvertisingCompletionRecyclePrepared<'a>,
        BluetoothLegacyAdvertisingCompletionRecycleFailure<'a>,
    > {
        let Self {
            _in_flight: in_flight,
            memory,
            _scheduler_window: scheduler_window,
        } = self;
        match memory.prepare_recycle_after_software_list_removal(removal) {
            Ok(memory) => Ok(BluetoothLegacyAdvertisingCompletionRecyclePrepared {
                in_flight,
                memory,
                scheduler_window,
            }),
            Err(failure) => {
                let error = failure.error();
                let (memory, removal) = failure.into_parts();
                Err(BluetoothLegacyAdvertisingCompletionRecycleFailure {
                    error,
                    item: Self {
                        _in_flight: in_flight,
                        memory,
                        _scheduler_window: scheduler_window,
                    },
                    removal,
                })
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothLegacyAdvertisingCompletionRecyclePrepared<'a> {
    in_flight: LegacyAdvertiserEventInFlight<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphRecyclePrepared,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingCompletionRecyclePrepared<'a> {
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothLegacyAdvertisingCompletionObservedEvent<'a>,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        let (memory, removal) = self.memory.into_parts();
        (
            BluetoothLegacyAdvertisingCompletionObservedEvent {
                _in_flight: self.in_flight,
                memory,
                _scheduler_window: self.scheduler_window,
            },
            removal,
        )
    }

    pub(crate) fn commit(self) -> BluetoothLegacyAdvertisingRecycledEvent<'a> {
        let phase = self.scheduler_window.phase();
        let memory = self.memory.commit();
        BluetoothLegacyAdvertisingRecycledEvent::new(self.in_flight, memory, phase)
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothLegacyAdvertisingCompletionRecycleFailure<'a> {
    error: BluetoothLegacyAdvertisingMemoryGraphRecycleError,
    item: BluetoothLegacyAdvertisingCompletionObservedEvent<'a>,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingCompletionRecycleFailure<'a> {
    pub(crate) const fn error(&self) -> BluetoothLegacyAdvertisingMemoryGraphRecycleError {
        self.error
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothLegacyAdvertisingCompletionObservedEvent<'a>,
        BluetoothSchedulerSoftwareListRemovalReady,
    ) {
        (self.item, self.removal)
    }
}

/// CPU-owned graph after one scheduler-completed advertising event.
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothLegacyAdvertisingRecycledEvent<'a> {
    in_flight: LegacyAdvertiserEventInFlight<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    statuses: BluetoothLegacyAdvertisingEventCompletionStatuses,
    phase: crate::BluetoothLegacyAdvertisingEventPhase,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingRecycledEvent<'a> {
    fn new(
        in_flight: LegacyAdvertiserEventInFlight<'a>,
        memory: BluetoothLegacyAdvertisingMemoryGraphRecycled,
        phase: crate::BluetoothLegacyAdvertisingEventPhase,
    ) -> Self {
        let (memory, statuses) = memory.into_parts();
        Self {
            in_flight,
            memory,
            statuses,
            phase,
        }
    }

    /// Consume the exact completed-event proof and advance portable LL state.
    pub(crate) fn complete_event(self) -> BluetoothLegacyAdvertisingEventCompleted<'a> {
        let Self {
            in_flight,
            memory,
            statuses,
            phase,
        } = self;
        BluetoothLegacyAdvertisingEventCompleted {
            complete: in_flight.complete_exact(),
            memory,
            statuses,
            phase,
        }
    }
}

/// Portable LL continuation paired with the released S31 SRAM graph.
///
/// Statuses are diagnostic: reviewed vendor recycling consumes every scheduled
/// primary-channel item for zero and nonzero non-sentinel values. This type
/// therefore makes no claim that a packet was observable on air.
#[cfg(target_arch = "riscv32")]
#[must_use = "schedule the next advertising event or retain all returned owners"]
pub struct BluetoothLegacyAdvertisingEventCompleted<'a> {
    complete: LegacyAdvertiserEventComplete<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    statuses: BluetoothLegacyAdvertisingEventCompletionStatuses,
    phase: crate::BluetoothLegacyAdvertisingEventPhase,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingEventCompleted<'a> {
    /// Per-channel scheduler values retained for diagnostics and research.
    pub const fn statuses(&self) -> BluetoothLegacyAdvertisingEventCompletionStatuses {
        self.statuses
    }

    /// Nominal first-event phase retained across hardware completion.
    pub const fn phase(&self) -> crate::BluetoothLegacyAdvertisingEventPhase {
        self.phase
    }

    /// Attach one fresh Link Layer delay to the next event without losing the graph.
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn schedule_next(
        self,
        delay: AdvertisingDelay,
    ) -> Result<
        BluetoothLegacyAdvertisingNextEventScheduled<'a>,
        BluetoothLegacyAdvertisingEventScheduleFailure<'a>,
    > {
        let Self {
            complete,
            memory,
            statuses,
            phase,
        } = self;
        match complete.schedule_next(delay) {
            Ok(scheduled) => Ok(BluetoothLegacyAdvertisingNextEventScheduled {
                scheduled,
                memory,
                previous_statuses: statuses,
                previous_phase: phase,
            }),
            Err(exhausted) => Err(BluetoothLegacyAdvertisingEventScheduleFailure {
                completed: Self {
                    complete: exhausted.into_complete(),
                    memory,
                    statuses,
                    phase,
                },
            }),
        }
    }

    /// Recover the protocol continuation, CPU graph and diagnostic status.
    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertiserEventComplete<'a>,
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingEventCompletionStatuses,
        crate::BluetoothLegacyAdvertisingEventPhase,
    ) {
        (self.complete, self.memory, self.statuses, self.phase)
    }
}

/// Next portable event plus the exact reusable S31 graph and previous phase.
#[cfg(target_arch = "riscv32")]
#[must_use = "prepare the recurring event, disable it, or retain every owner"]
pub struct BluetoothLegacyAdvertisingNextEventScheduled<'a> {
    scheduled: LegacyAdvertiserScheduled<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    previous_statuses: BluetoothLegacyAdvertisingEventCompletionStatuses,
    previous_phase: crate::BluetoothLegacyAdvertisingEventPhase,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingNextEventScheduled<'a> {
    pub const fn start_offset_micros(&self) -> u64 {
        self.scheduled.start_offset_micros()
    }

    pub const fn previous_statuses(&self) -> BluetoothLegacyAdvertisingEventCompletionStatuses {
        self.previous_statuses
    }

    /// Rebuild the reusable graph and project its exact recurring event window.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure retains every affine recurrence input"
    )]
    pub fn prepare_candidate(
        self,
        default_tx_power: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
        timing: BluetoothLegacyAdvertisingRecurringTimingObservation,
        config: BluetoothSchedulerSoftwareConfig,
    ) -> Result<
        BluetoothLegacyAdvertisingRecurringEventCandidate<'a>,
        BluetoothLegacyAdvertisingRecurringPreparationFailure<'a>,
    > {
        let Self {
            scheduled,
            memory,
            previous_statuses,
            previous_phase,
        } = self;
        let start_offset_micros = scheduled.start_offset_micros();
        prepare_recurring_candidate(
            scheduled.into_event(),
            memory,
            previous_statuses,
            previous_phase,
            start_offset_micros,
            default_tx_power,
            timing,
            config,
        )
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertiserScheduled<'a>,
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingEventCompletionStatuses,
        crate::BluetoothLegacyAdvertisingEventPhase,
    ) {
        (
            self.scheduled,
            self.memory,
            self.previous_statuses,
            self.previous_phase,
        )
    }

    /// Disable the not-yet-prepared successor and recover its exact graph.
    pub fn cancel(self) -> BluetoothLegacyAdvertisingCancelled<'a> {
        let Self {
            scheduled,
            memory,
            previous_statuses: _,
            previous_phase: _,
        } = self;
        BluetoothLegacyAdvertisingCancelled {
            enabled: scheduled.into_event(),
            memory,
        }
    }
}

#[cfg(target_arch = "riscv32")]
#[expect(
    clippy::too_many_arguments,
    reason = "each affine recurrence input is independently owned and semantically distinct"
)]
#[expect(
    clippy::result_large_err,
    reason = "the no-alloc failure retains every affine recurrence input"
)]
fn prepare_recurring_candidate<'a>(
    enabled: LegacyAdvertiserEnabled<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    previous_statuses: BluetoothLegacyAdvertisingEventCompletionStatuses,
    previous_phase: crate::BluetoothLegacyAdvertisingEventPhase,
    start_offset_micros: u64,
    default_tx_power: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    timing: BluetoothLegacyAdvertisingRecurringTimingObservation,
    config: BluetoothSchedulerSoftwareConfig,
) -> Result<
    BluetoothLegacyAdvertisingRecurringEventCandidate<'a>,
    BluetoothLegacyAdvertisingRecurringPreparationFailure<'a>,
> {
    let prepared = match BluetoothLegacyAdvertisingPrepared::prepare(enabled, memory) {
        Ok(prepared) => prepared,
        Err(failure) => {
            let (enabled, memory, error) = failure.into_parts();
            return Err(BluetoothLegacyAdvertisingRecurringPreparationFailure {
                enabled,
                memory,
                previous_statuses,
                previous_phase,
                start_offset_micros,
                error: BluetoothLegacyAdvertisingRecurringPreparationError::Packet(error),
            });
        }
    };
    let reset = match prepared.reset_link_state(default_tx_power) {
        BluetoothLegacyAdvertisingLinkStateResetOutcome::Reset(reset) => reset,
        BluetoothLegacyAdvertisingLinkStateResetOutcome::Rejected { prepared, error } => {
            let cancelled = prepared.cancel();
            let (enabled, memory) = cancelled.into_parts();
            return Err(BluetoothLegacyAdvertisingRecurringPreparationFailure {
                enabled,
                memory,
                previous_statuses,
                previous_phase,
                start_offset_micros,
                error: BluetoothLegacyAdvertisingRecurringPreparationError::LinkState(error),
            });
        }
    };
    let payload_length = reset.memory.payload_length();
    let Some((scheduler_window, raw_window, raw_item_duration)) = timing.recurring_le_1m_window(
        previous_phase,
        start_offset_micros,
        config,
        payload_length,
        reset.prepared.channels().channel_count(),
    ) else {
        let cancelled = reset.cancel();
        let (enabled, memory) = cancelled.into_parts();
        return Err(BluetoothLegacyAdvertisingRecurringPreparationFailure {
            enabled,
            memory,
            previous_statuses,
            previous_phase,
            start_offset_micros,
            error: BluetoothLegacyAdvertisingRecurringPreparationError::Timing,
        });
    };
    Ok(BluetoothLegacyAdvertisingRecurringEventCandidate {
        reset,
        scheduler_window,
        raw_window,
        raw_item_duration,
        previous_statuses,
        previous_phase,
        start_offset_micros,
    })
}

/// Recurring event rebuilt in CPU-owned memory but not admitted to the timeline.
#[cfg(target_arch = "riscv32")]
#[must_use = "admit the recurring event, cancel it, or retain every owner"]
pub struct BluetoothLegacyAdvertisingRecurringEventCandidate<'a> {
    reset: BluetoothLegacyAdvertisingLinkStateReset<'a>,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
    raw_window: BluetoothSchedulerRawWindow,
    raw_item_duration: u32,
    previous_statuses: BluetoothLegacyAdvertisingEventCompletionStatuses,
    previous_phase: crate::BluetoothLegacyAdvertisingEventPhase,
    start_offset_micros: u64,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingRecurringEventCandidate<'a> {
    pub const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.reset.identity()
    }

    pub fn pdu(&self) -> &[u8] {
        self.reset.pdu()
    }

    pub const fn previous_statuses(&self) -> BluetoothLegacyAdvertisingEventCompletionStatuses {
        self.previous_statuses
    }

    pub const fn projected_window_duration(&self) -> u32 {
        self.raw_window.duration()
    }

    pub(crate) const fn raw_window(&self) -> BluetoothSchedulerRawWindow {
        self.raw_window
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure returns the exact recurring candidate"
    )]
    pub(crate) fn prepare_resolved_event_image(
        self,
        resolved_window: BluetoothSchedulerRawWindow,
    ) -> Result<
        BluetoothLegacyAdvertisingRecurringEventImagePrepared<'a>,
        BluetoothLegacyAdvertisingRecurringEventImagePrepareFailure<'a>,
    > {
        let Self {
            reset,
            scheduler_window,
            raw_window,
            raw_item_duration,
            previous_statuses,
            previous_phase,
            start_offset_micros,
        } = self;
        let BluetoothLegacyAdvertisingLinkStateReset { prepared, memory } = reset;
        let channels = controller_channel_plan(prepared.channels());
        match memory.prepare_event(channels, resolved_window.start(), raw_item_duration) {
            Ok(memory) => Ok(BluetoothLegacyAdvertisingRecurringEventImagePrepared {
                image: BluetoothLegacyAdvertisingEventImagePrepared {
                    prepared,
                    memory,
                    scheduler_window,
                },
                previous_statuses,
                previous_phase,
                start_offset_micros,
            }),
            Err(failure) => {
                let (memory, error) = failure.into_parts();
                Err(
                    BluetoothLegacyAdvertisingRecurringEventImagePrepareFailure {
                        candidate: Self {
                            reset: BluetoothLegacyAdvertisingLinkStateReset { prepared, memory },
                            scheduler_window,
                            raw_window,
                            raw_item_duration,
                            previous_statuses,
                            previous_phase,
                            start_offset_micros,
                        },
                        error,
                    },
                )
            }
        }
    }

    pub fn cancel(self) -> BluetoothLegacyAdvertisingRecurringCancelled<'a> {
        BluetoothLegacyAdvertisingRecurringCancelled {
            cancelled: self.reset.cancel(),
            previous_statuses: self.previous_statuses,
            previous_phase: self.previous_phase,
            start_offset_micros: self.start_offset_micros,
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothLegacyAdvertisingRecurringEventImagePrepared<'a> {
    image: BluetoothLegacyAdvertisingEventImagePrepared<'a>,
    previous_statuses: BluetoothLegacyAdvertisingEventCompletionStatuses,
    previous_phase: crate::BluetoothLegacyAdvertisingEventPhase,
    start_offset_micros: u64,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingRecurringEventImagePrepared<'a> {
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothLegacyAdvertisingEventImagePrepared<'a>,
        BluetoothLegacyAdvertisingEventCompletionStatuses,
        crate::BluetoothLegacyAdvertisingEventPhase,
        u64,
    ) {
        (
            self.image,
            self.previous_statuses,
            self.previous_phase,
            self.start_offset_micros,
        )
    }
}

/// Failed recurring packet/reset/time preparation retaining every input owner.
#[cfg(target_arch = "riscv32")]
#[must_use = "retry, disable, or recover every recurrence input"]
pub struct BluetoothLegacyAdvertisingRecurringPreparationFailure<'a> {
    enabled: LegacyAdvertiserEnabled<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    previous_statuses: BluetoothLegacyAdvertisingEventCompletionStatuses,
    previous_phase: crate::BluetoothLegacyAdvertisingEventPhase,
    start_offset_micros: u64,
    error: BluetoothLegacyAdvertisingRecurringPreparationError,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingRecurringPreparationFailure<'a> {
    pub const fn error(&self) -> BluetoothLegacyAdvertisingRecurringPreparationError {
        self.error
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc retry retains every affine recurrence input"
    )]
    pub fn retry(
        self,
        default_tx_power: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
        timing: BluetoothLegacyAdvertisingRecurringTimingObservation,
        config: BluetoothSchedulerSoftwareConfig,
    ) -> Result<
        BluetoothLegacyAdvertisingRecurringEventCandidate<'a>,
        BluetoothLegacyAdvertisingRecurringPreparationFailure<'a>,
    > {
        prepare_recurring_candidate(
            self.enabled,
            self.memory,
            self.previous_statuses,
            self.previous_phase,
            self.start_offset_micros,
            default_tx_power,
            timing,
            config,
        )
    }

    /// Disable the rejected unpublished successor and recover its exact graph.
    pub fn cancel(self) -> BluetoothLegacyAdvertisingCancelled<'a> {
        BluetoothLegacyAdvertisingCancelled {
            enabled: self.enabled,
            memory: self.memory,
        }
    }
}

/// Finite pre-admission failure while rebuilding one recurring event.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingRecurringPreparationError {
    Packet(BluetoothLegacyAdvertisingPreparationErrorKind),
    LinkState(BluetoothLegacyAdvertisingPduError),
    Timing,
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothLegacyAdvertisingRecurringEventImagePrepareFailure<'a> {
    candidate: BluetoothLegacyAdvertisingRecurringEventCandidate<'a>,
    error: BluetoothLegacyAdvertisingMemoryGraphEventPrepareError,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingRecurringEventImagePrepareFailure<'a> {
    pub(crate) const fn error(&self) -> BluetoothLegacyAdvertisingMemoryGraphEventPrepareError {
        self.error
    }

    pub(crate) fn into_candidate(self) -> BluetoothLegacyAdvertisingRecurringEventCandidate<'a> {
        self.candidate
    }
}

/// Lossless cancellation after the next event was rebuilt but not published.
#[cfg(target_arch = "riscv32")]
#[must_use = "recover the enabled event, graph and previous diagnostics"]
pub struct BluetoothLegacyAdvertisingRecurringCancelled<'a> {
    cancelled: BluetoothLegacyAdvertisingCancelled<'a>,
    previous_statuses: BluetoothLegacyAdvertisingEventCompletionStatuses,
    previous_phase: crate::BluetoothLegacyAdvertisingEventPhase,
    start_offset_micros: u64,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingRecurringCancelled<'a> {
    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyAdvertisingCancelled<'a>,
        BluetoothLegacyAdvertisingEventCompletionStatuses,
        crate::BluetoothLegacyAdvertisingEventPhase,
        u64,
    ) {
        (
            self.cancelled,
            self.previous_statuses,
            self.previous_phase,
            self.start_offset_micros,
        )
    }
}

/// Event-sequence exhaustion retaining the complete post-recycle owner.
#[cfg(target_arch = "riscv32")]
#[must_use = "recover the completed event and its SRAM graph"]
pub struct BluetoothLegacyAdvertisingEventScheduleFailure<'a> {
    completed: BluetoothLegacyAdvertisingEventCompleted<'a>,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingEventScheduleFailure<'a> {
    pub fn into_completed(self) -> BluetoothLegacyAdvertisingEventCompleted<'a> {
        self.completed
    }
}

/// Failed private event encoding retaining the pre-admission candidate.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothLegacyAdvertisingFirstEventImagePrepareFailure<'a> {
    candidate: BluetoothLegacyAdvertisingFirstEventCandidate<'a>,
    error: BluetoothLegacyAdvertisingMemoryGraphEventPrepareError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> BluetoothLegacyAdvertisingFirstEventImagePrepareFailure<'a> {
    pub(crate) const fn error(&self) -> BluetoothLegacyAdvertisingMemoryGraphEventPrepareError {
        self.error
    }

    pub(crate) fn into_candidate(self) -> BluetoothLegacyAdvertisingFirstEventCandidate<'a> {
        self.candidate
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl core::fmt::Debug for BluetoothLegacyAdvertisingFirstEventCandidate<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingFirstEventCandidate")
            .field("identity", &self.identity())
            .field("scheduler_window", &self.scheduler_window)
            .field("raw_window", &self.raw_window)
            .finish_non_exhaustive()
    }
}

/// Lossless cancellation before any advertising descriptor is publishable.
#[must_use = "both the portable advertiser and bound SRAM graph must be retained"]
pub struct BluetoothLegacyAdvertisingCancelled<'a> {
    enabled: LegacyAdvertiserEnabled<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
}

impl<'a> BluetoothLegacyAdvertisingCancelled<'a> {
    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertiserEnabled<'a>,
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    ) {
        (self.enabled, self.memory)
    }
}

/// Bounded PDU encoding failed before any S31 hardware ownership changed.
#[must_use = "the unchanged advertiser and SRAM graph remain recoverable"]
pub struct BluetoothLegacyAdvertisingPreparationError<'a> {
    enabled: LegacyAdvertiserEnabled<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    error: BluetoothLegacyAdvertisingPreparationErrorKind,
}

impl<'a> BluetoothLegacyAdvertisingPreparationError<'a> {
    pub const fn error(&self) -> BluetoothLegacyAdvertisingPreparationErrorKind {
        self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyAdvertiserEnabled<'a>,
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingPreparationErrorKind,
    ) {
        (self.enabled, self.memory, self.error)
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingPreparationError<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingPreparationError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Finite CPU-side reason why one advertising transmission was not prepared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingPreparationErrorKind {
    /// The portable Link Layer PDU producer rejected its bounded destination.
    PduEncoding(LegacyAdvertisingEncodeError),
    /// The complete PDU did not satisfy the common controller TX allocation.
    ControllerPacket(BluetoothLeTxPacketPrepareError),
}

#[cfg(test)]
mod tests;
