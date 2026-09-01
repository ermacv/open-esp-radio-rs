//! ESP32-S31 hardware ownership for restricted legacy advertising.
//!
//! This boundary lowers one portable `ADV_NONCONN_IND` event into a bounded PDU
//! and a reviewed 1--3 item S31 descriptor chain. The complete selected-channel
//! event advances through scheduler bookkeeping, `HEAD`/interrupt/event/`RUN`
//! publication, fenced completion, hardware-head retirement, software unlink
//! and CPU recycle. Only that complete lower proof may advance the portable LL
//! owner. Returned per-item statuses remain diagnostic and make no claim about
//! observability on air.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_ll::advertiser::{
    LegacyAdvertiserEventComplete, LegacyAdvertiserEventInFlight, LegacyAdvertiserScheduled,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_ll::advertising::AdvertisingDelay;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_bluetooth_ll::advertising::PrimaryAdvertisingChannel;
use open_esp_radio_bluetooth_ll::{
    advertiser::{
        LegacyAdvertiserEnabled, LegacyAdvertiserEventPrepared, LegacyAdvertisingEventIdentity,
    },
    advertising::{
        LEGACY_ADVERTISING_PDU_CAPACITY, LegacyAdvertisingEncodeError, PrimaryAdvertisingChannelMap,
    },
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLeTxPacketPrepareError, BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
    BluetoothLegacyAdvertisingMemoryGraphLinkStateReset,
    BluetoothLegacyAdvertisingMemoryGraphPacketPrepared, BluetoothLegacyAdvertisingPduError,
};
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
    BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareError,
    BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepared,
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
    ) -> Result<
        BluetoothLegacyAdvertisingLinkStateReset<'a>,
        BluetoothLegacyAdvertisingLinkStateResetError<'a>,
    > {
        let Self { prepared, memory } = self;
        match memory.reset_link_state(default_tx_power.dbm()) {
            Ok(memory) => Ok(BluetoothLegacyAdvertisingLinkStateReset { prepared, memory }),
            Err(failure) => {
                let (memory, error) = failure.into_parts();
                Err(BluetoothLegacyAdvertisingLinkStateResetError {
                    prepared: Self { prepared, memory },
                    error,
                })
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
    ) -> Result<
        BluetoothLegacyAdvertisingFirstEventCandidate<'a>,
        BluetoothLegacyAdvertisingFirstEventTimingFailure<'a>,
    > {
        let payload_length = self.memory.payload_length();
        let Some((scheduler_window, raw_window, raw_item_duration)) = timing.first_le_1m_window(
            config,
            payload_length,
            self.prepared.channels().channel_count(),
        ) else {
            return Err(BluetoothLegacyAdvertisingFirstEventTimingFailure { reset: self });
        };
        Ok(BluetoothLegacyAdvertisingFirstEventCandidate {
            reset: self,
            scheduler_window,
            raw_window,
            raw_item_duration,
        })
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

    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc failure returns the exact affine candidate"
        )
    )]
    pub(crate) fn prepare_resolved_event_image(
        self,
        resolved_window: BluetoothSchedulerRawWindow,
    ) -> Result<
        BluetoothLegacyAdvertisingFirstEventImagePrepared<'a>,
        BluetoothLegacyAdvertisingFirstEventImagePrepareFailure<'a>,
    > {
        let Self {
            reset,
            scheduler_window,
            raw_window,
            raw_item_duration,
        } = self;
        let BluetoothLegacyAdvertisingLinkStateReset { prepared, memory } = reset;
        let selected = prepared.channels();
        let channels = BluetoothLegacyAdvertisingPrimaryChannelPlan::new(
            selected.contains(PrimaryAdvertisingChannel::Channel37),
            selected.contains(PrimaryAdvertisingChannel::Channel38),
            selected.contains(PrimaryAdvertisingChannel::Channel39),
        )
        .expect("the portable advertising channel map is non-empty");
        match memory.prepare_first_event(channels, resolved_window.start(), raw_item_duration) {
            Ok(memory) => Ok(BluetoothLegacyAdvertisingFirstEventImagePrepared {
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
pub(crate) struct BluetoothLegacyAdvertisingFirstEventImagePrepared<'a> {
    prepared: LegacyAdvertiserEventPrepared<'a>,
    memory: BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepared,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> BluetoothLegacyAdvertisingFirstEventImagePrepared<'a> {
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

    pub(crate) fn cancel(self) -> BluetoothLegacyAdvertisingFirstEventImagePrepared<'a> {
        BluetoothLegacyAdvertisingFirstEventImagePrepared {
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

    pub(crate) const fn statuses(&self) -> BluetoothLegacyAdvertisingEventCompletionStatuses {
        self.memory.statuses()
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

    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.in_flight.identity()
    }

    pub(crate) const fn statuses(&self) -> BluetoothLegacyAdvertisingEventCompletionStatuses {
        self.statuses
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
    error: BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> BluetoothLegacyAdvertisingFirstEventImagePrepareFailure<'a> {
    pub(crate) const fn error(
        &self,
    ) -> BluetoothLegacyAdvertisingMemoryGraphFirstEventPrepareError {
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

/// A live scheduler window could not be represented in one forward raw epoch.
#[must_use = "the reset advertising owner remains recoverable"]
#[cfg(any(target_arch = "riscv32", test))]
pub struct BluetoothLegacyAdvertisingFirstEventTimingFailure<'a> {
    reset: BluetoothLegacyAdvertisingLinkStateReset<'a>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> BluetoothLegacyAdvertisingFirstEventTimingFailure<'a> {
    pub fn into_reset(self) -> BluetoothLegacyAdvertisingLinkStateReset<'a> {
        self.reset
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl core::fmt::Debug for BluetoothLegacyAdvertisingFirstEventTimingFailure<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingFirstEventTimingFailure")
            .finish_non_exhaustive()
    }
}

/// A packet prepared outside the portable producer did not match the
/// restricted advertising reset contract.
#[must_use = "the packet-prepared owner remains recoverable"]
pub struct BluetoothLegacyAdvertisingLinkStateResetError<'a> {
    prepared: BluetoothLegacyAdvertisingPrepared<'a>,
    error: BluetoothLegacyAdvertisingPduError,
}

impl<'a> BluetoothLegacyAdvertisingLinkStateResetError<'a> {
    pub const fn error(&self) -> BluetoothLegacyAdvertisingPduError {
        self.error
    }

    pub fn into_prepared(self) -> BluetoothLegacyAdvertisingPrepared<'a> {
        self.prepared
    }
}

impl core::fmt::Debug for BluetoothLegacyAdvertisingLinkStateResetError<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingLinkStateResetError")
            .field("error", &self.error)
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
mod tests {
    use open_esp_radio_bluetooth_ll::{
        LeDeviceAddress, LeDeviceAddressKind,
        advertiser::LegacyAdvertiserStandby,
        advertising::{
            AdvertisingInterval, LegacyAdvertisingData, LegacyNonconnectableAdvertisement,
            LegacyNonconnectableAdvertisingSet, PrimaryAdvertisingChannelMap,
        },
    };
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingMemoryGraphModelAddress,
        BluetoothLegacyAdvertisingMemoryGraphStorage,
    };
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::{BluetoothLegacyAdvertisingDefaultTxPowerDbm, BluetoothLegacyAdvertisingPrepared};
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
        BluetoothLegacyAdvertisingTimingObservation, BluetoothSchedulerInstant,
        BluetoothSchedulerSoftwareConfig,
    };

    fn enabled(
        channels: PrimaryAdvertisingChannelMap,
    ) -> open_esp_radio_bluetooth_ll::advertiser::LegacyAdvertiserEnabled<'static> {
        let advertisement = LegacyNonconnectableAdvertisement::new(
            LeDeviceAddress::from_wire_bytes([6, 5, 4, 3, 2, 1], LeDeviceAddressKind::Public),
            LegacyAdvertisingData::new(&[2, 1, 6]).expect("the fixed data fits legacy advertising"),
        );
        LegacyAdvertiserStandby::new()
            .configure(LegacyNonconnectableAdvertisingSet::new(
                advertisement,
                channels,
                AdvertisingInterval::new(AdvertisingInterval::MIN_UNITS)
                    .expect("the minimum interval is valid"),
            ))
            .enable()
            .expect("the first generation is available")
    }

    fn memory() -> BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothLegacyAdvertisingMemoryGraphStorage::new(),
        ));
        let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("the model base uses controller SRAM syntax");
        BluetoothLegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
            .expect("the advertising graph fits physical controller SRAM")
    }

    #[test]
    fn preparation_retains_identity_and_cancel_restores_the_same_event() {
        let prepared = BluetoothLegacyAdvertisingPrepared::prepare(
            enabled(PrimaryAdvertisingChannelMap::all()),
            memory(),
        )
        .expect("bounded validated advertising data always fits the chip PDU");
        let identity = prepared.identity();

        assert_eq!(prepared.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);
        assert_eq!(prepared.channels(), PrimaryAdvertisingChannelMap::all());
        let (enabled, _memory) = prepared.cancel().into_parts();
        assert_eq!(enabled.prepare_event().identity(), identity);
    }

    #[test]
    fn portable_primary_channel_plan_survives_chip_preparation() {
        for channels in [
            PrimaryAdvertisingChannelMap::new(true, false, false).unwrap(),
            PrimaryAdvertisingChannelMap::new(false, true, true).unwrap(),
            PrimaryAdvertisingChannelMap::all(),
        ] {
            let prepared = BluetoothLegacyAdvertisingPrepared::prepare(enabled(channels), memory())
                .expect("bounded validated advertising data always fits the chip PDU");
            assert_eq!(prepared.channels(), channels);
        }
    }

    #[test]
    fn reset_retains_the_exact_protocol_work_and_remains_cancellable() {
        let prepared = BluetoothLegacyAdvertisingPrepared::prepare(
            enabled(PrimaryAdvertisingChannelMap::all()),
            memory(),
        )
        .expect("bounded validated advertising data always fits the chip PDU");
        let identity = prepared.identity();
        let reset = prepared
            .reset_link_state(BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(0))
            .expect("the portable producer emits the restricted PDU form");

        assert_eq!(reset.identity(), identity);
        assert_eq!(reset.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);
        let (enabled, memory) = reset.cancel().into_parts();
        assert_eq!(enabled.prepare_event().identity(), identity);
        assert!(memory.prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6]).is_ok());
    }

    #[test]
    fn sealed_live_timing_forms_a_cancellable_first_event_candidate() {
        let reset = BluetoothLegacyAdvertisingPrepared::prepare(
            enabled(PrimaryAdvertisingChannelMap::all()),
            memory(),
        )
        .expect("bounded validated advertising data always fits the chip PDU")
        .reset_link_state(BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(0))
        .expect("the portable producer emits the restricted PDU form");
        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let timing = BluetoothLegacyAdvertisingTimingObservation {
            current: BluetoothSchedulerInstant::from_image(10_000),
            radio_ready: BluetoothSchedulerInstant::from_image(11_999),
            epoch: BluetoothControllerSchedulerEpoch::new(
                BluetoothControllerTimeSample::for_validation(100),
                1_000,
                scale,
            ),
        };
        let candidate = reset
            .form_first_event_candidate(
                timing,
                BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            )
            .expect("the reviewed timing window projects into one raw epoch");

        assert_eq!(candidate.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);
        assert_eq!(candidate.projected_window_duration(), 192);
        let (enabled, _) = candidate.cancel().into_parts();
        assert_eq!(
            enabled.prepare_event().channels(),
            PrimaryAdvertisingChannelMap::all()
        );
    }
}
