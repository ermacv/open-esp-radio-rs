#![forbid(unsafe_code)]

//! Pre-publication ownership for one response-capable legacy advertisement.
//!
//! This boundary converts one typed HCI snapshot into portable Link Layer
//! `ADV_IND` and `SCAN_RSP` state, then atomically checks out the independent
//! response graph and the peripheral role's reusable RX pool. It performs only
//! controller-SRAM preparation. Scheduler admission, MMIO publication and any
//! claim of radio progress remain outside this module.

#[cfg(target_arch = "riscv32")]
pub(crate) mod active;
#[cfg(target_arch = "riscv32")]
pub(crate) mod completion;
#[cfg(target_arch = "riscv32")]
pub(crate) mod hci;
pub(crate) mod recurring;
#[cfg(target_arch = "riscv32")]
pub(crate) mod runner;

use oer_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertiserStandby;
#[cfg(not(target_arch = "riscv32"))]
use oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingMemoryGraphModelAddress;

use oer_esp32s31_bluetooth_memory::{
    LegacyConnectableAdvertisingMemoryGraphBindFailure,
    LegacyConnectableAdvertisingMemoryGraphCpuOwned,
    LegacyConnectableAdvertisingMemoryGraphIdentity,
    LegacyConnectableAdvertisingMemoryGraphStorage,
};

#[cfg(any(target_arch = "riscv32", test))]
use oer_bluetooth_ll::{
    LeDeviceAddressKind,
    advertising::{
        AdvertisingDelay, AdvertisingIntervalError, LegacyAdvertisingDataError,
        PrimaryAdvertisingChannel, PrimaryAdvertisingChannelMap, PrimaryAdvertisingChannelMapError,
    },
    advertising_lifecycle::LegacyAdvertisingEventIdentity,
    connectable_advertising::{
        LegacyConnectableAdvertiserConfigured, LegacyConnectableAdvertisingEvent,
        LegacyConnectableAdvertisingEventComplete, LegacyConnectableAdvertisingEventInFlight,
        LegacyConnectableAdvertisingSet, LegacyConnectableConnectionRequestAccepted,
        LegacyConnectableConnectionRequestAdmission, LegacyPreparedConnectableAdvertisingEvent,
    },
};
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_bluetooth_memory::{
    BLUETOOTH_NON_SCANNING_RX_NODE_COUNT, LeReceivedPdu, LegacyAdvertisingPrimaryChannel,
    LegacyConnectableAdvIndPacketInput,
    LegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared,
    LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
    LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared,
    LegacyConnectableAdvertisingMemoryGraphPrepareError,
    LegacyConnectableAdvertisingMemoryGraphPrepared,
    LegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    LegacyConnectableAdvertisingMemoryInput, LegacyConnectableAdvertisingOwnAddress,
    LegacyConnectableAdvertisingPduFitError,
    LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    LegacyConnectableScanResponsePacketInput,
};
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_hal::types::BluetoothControllerSramAddress;

#[cfg(target_arch = "riscv32")]
use oer_bluetooth_hci::{
    LeLegacyAdvertisingAddress, LeLegacyAdvertisingRole,
    LeLegacyConnectableAdvertisingEnableRequest,
};
#[cfg(target_arch = "riscv32")]
use oer_bluetooth_ll::{
    LeDeviceAddress,
    advertising::{AdvertisingInterval, LegacyAdvertisingData},
    connectable_advertising::{
        LeChannelSelectionAlgorithmTwoSupport, LegacyConnectableAdvertisement,
        LegacyScanResponseData,
    },
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    LeReceivedBatch, LegacyConnectableAdvertisingMemoryGraphCompletionObserved,
    LegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
    LegacyConnectableAdvertisingMemoryGraphRecycled,
    LegacyConnectableAdvertisingMemoryGraphRunning,
    LegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::ControllerRandomAddress;

use crate::le::advertising::LegacyAdvertisingDefaultTxPowerDbm;
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    LegacyAdvertisingEventWindow, LegacyAdvertisingRecurringTimingObservation,
    le::{
        advertising::{LegacyAdvertisingEventPhase, LegacyAdvertisingTimingObservation},
        peripheral::{
            PeripheralConnectionRuntimeBeginError, PeripheralConnectionRuntimeResources,
            connection::{
                PeripheralConnectionAcceptedRequest,
                PeripheralConnectionAcceptedResetCancellationError,
                PeripheralConnectionRuntimeAllocation,
                PeripheralConnectionRuntimeGraphRejoinFailure,
                PeripheralConnectionRuntimeGraphReserved,
            },
        },
    },
    scheduler::{SchedulerRawWindow, SchedulerSoftwareConfig},
};

/// Why one typed HCI snapshot cannot become the restricted portable role.
///
/// Every case is defensive: the HCI layer already validates all but the S31
/// one-channel policy. Keeping the projection fallible prevents later HCI
/// expansion from silently widening the hardware contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
#[cfg_attr(
    not(target_arch = "riscv32"),
    expect(
        dead_code,
        reason = "the target HCI enable projection constructs these validation errors; host CPU ownership tests refine an already validated portable set"
    )
)]
pub(crate) enum LegacyConnectableAdvertisingSetError {
    Role,
    AdvertisingData(LegacyAdvertisingDataError),
    ScanResponseData(LegacyAdvertisingDataError),
    Channels(PrimaryAdvertisingChannelMapError),
    Interval(AdvertisingIntervalError),
    MultiplePrimaryChannels { selected: usize },
}

/// Portable connectable set refined to the one-channel S31 memory contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the prepared connectable set must begin an event or remain retained"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct LegacyConnectableAdvertisingSetPrepared {
    set: LegacyConnectableAdvertisingSet<'static>,
    primary_channel: LegacyAdvertisingPrimaryChannel,
    own_address: LegacyConnectableAdvertisingOwnAddress,
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyConnectableAdvertisingSetPrepared {
    /// Portable configuration retained independently of controller memory.
    pub(crate) const fn set(self) -> LegacyConnectableAdvertisingSet<'static> {
        self.set
    }

    /// Random-address publication intent, without exposing a register image.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn random_address(self) -> Option<ControllerRandomAddress> {
        match self.own_address {
            LegacyConnectableAdvertisingOwnAddress::Public => None,
            LegacyConnectableAdvertisingOwnAddress::Random(wire) => {
                Some(ControllerRandomAddress::from_hci_wire_bytes(wire))
            }
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn selected_primary_channel(
    channels: PrimaryAdvertisingChannelMap,
) -> Result<LegacyAdvertisingPrimaryChannel, LegacyConnectableAdvertisingSetError> {
    let selected = channels.channel_count();
    if selected != 1 {
        return Err(LegacyConnectableAdvertisingSetError::MultiplePrimaryChannels { selected });
    }
    if channels.contains(PrimaryAdvertisingChannel::Channel37) {
        Ok(LegacyAdvertisingPrimaryChannel::Channel37)
    } else if channels.contains(PrimaryAdvertisingChannel::Channel38) {
        Ok(LegacyAdvertisingPrimaryChannel::Channel38)
    } else {
        Ok(LegacyAdvertisingPrimaryChannel::Channel39)
    }
}

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) fn refine_portable_set(
    set: LegacyConnectableAdvertisingSet<'static>,
) -> Result<LegacyConnectableAdvertisingSetPrepared, LegacyConnectableAdvertisingSetError> {
    let primary_channel = selected_primary_channel(set.channels())?;
    let advertiser = set.advertisement().advertiser();
    let own_address = match advertiser.kind() {
        LeDeviceAddressKind::Public => LegacyConnectableAdvertisingOwnAddress::Public,
        LeDeviceAddressKind::Random => {
            LegacyConnectableAdvertisingOwnAddress::Random(advertiser.wire_bytes())
        }
    };
    Ok(LegacyConnectableAdvertisingSetPrepared {
        set,
        primary_channel,
        own_address,
    })
}

/// Convert an accepted HCI `ADV_IND` snapshot into a self-contained LL set.
///
/// The S31 first slice accepts exactly one selected primary channel. This is
/// checked here, before either static runtime can be checked out.
#[cfg(target_arch = "riscv32")]
pub(crate) fn prepare_legacy_connectable_advertising_set(
    request: LeLegacyConnectableAdvertisingEnableRequest,
) -> Result<LegacyConnectableAdvertisingSetPrepared, LegacyConnectableAdvertisingSetError> {
    let parameters = request.parameters();
    if parameters.role() != LeLegacyAdvertisingRole::Connectable {
        return Err(LegacyConnectableAdvertisingSetError::Role);
    }
    let address_kind = match request.advertiser() {
        LeLegacyAdvertisingAddress::Public(_) => LeDeviceAddressKind::Public,
        LeLegacyAdvertisingAddress::Random(_) => LeDeviceAddressKind::Random,
    };
    let wire = request.advertiser().wire_address();
    let mut wire_bytes = [0; 6];
    wire_bytes.copy_from_slice(wire.raw());
    let advertiser = LeDeviceAddress::from_wire_bytes(wire_bytes, address_kind);
    let advertisement = LegacyConnectableAdvertisement::new(
        advertiser,
        LegacyAdvertisingData::new_owned(request.data().as_bytes())
            .map_err(LegacyConnectableAdvertisingSetError::AdvertisingData)?,
        LeChannelSelectionAlgorithmTwoSupport::Unsupported,
    );
    let scan_response = LegacyScanResponseData::new_owned(request.scan_response_data().as_bytes())
        .map_err(LegacyConnectableAdvertisingSetError::ScanResponseData)?;
    let selected = parameters.channels();
    let channels = PrimaryAdvertisingChannelMap::new(
        selected.channel_37(),
        selected.channel_38(),
        selected.channel_39(),
    )
    .map_err(LegacyConnectableAdvertisingSetError::Channels)?;
    let interval =
        AdvertisingInterval::new(u32::from(parameters.interval().minimum_units_625_us()))
            .map_err(LegacyConnectableAdvertisingSetError::Interval)?;
    refine_portable_set(LegacyConnectableAdvertisingSet::new(
        advertisement,
        scan_response,
        channels,
        interval,
    ))
}

/// Composition-owned response-capable graph and physical power policy.
///
/// The peripheral connection allocation remains a separate runtime because an
/// accepted `CONNECT_IND` returns this advertising graph while retaining that
/// allocation for the new connection.
#[must_use = "the connectable advertising runtime retains its sole graph"]
pub struct LegacyConnectableAdvertisingRuntimeResources {
    default_tx_power_dbm: LegacyAdvertisingDefaultTxPowerDbm,
    #[cfg_attr(
        not(any(target_arch = "riscv32", test)),
        expect(
            dead_code,
            reason = "retained runtime identity is consumed by target event transitions and host ownership tests"
        )
    )]
    graph_identity: LegacyConnectableAdvertisingMemoryGraphIdentity,
    #[cfg_attr(
        not(any(target_arch = "riscv32", test)),
        expect(
            dead_code,
            reason = "retained runtime identity is consumed by target event transitions and host ownership tests"
        )
    )]
    standby: Option<LegacyConnectableAdvertiserStandby>,
    idle: Option<LegacyConnectableAdvertisingMemoryGraphCpuOwned>,
}

#[cfg_attr(
    any(target_arch = "riscv32", test),
    expect(
        clippy::result_large_err,
        reason = "preparation and rollback return the exact affine graph and role allocation inline"
    )
)]
impl LegacyConnectableAdvertisingRuntimeResources {
    fn from_claimed_graph(
        graph: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        default_tx_power_dbm: LegacyAdvertisingDefaultTxPowerDbm,
    ) -> Self {
        Self {
            default_tx_power_dbm,
            graph_identity: graph.identity(),
            standby: Some(LegacyConnectableAdvertiserStandby::new()),
            idle: Some(graph),
        }
    }

    /// Bind one statically placed production response-capable graph.
    #[cfg(target_arch = "riscv32")]
    pub fn claim_static(
        storage: &'static mut LegacyConnectableAdvertisingMemoryGraphStorage,
        default_tx_power_dbm: LegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<Self, LegacyConnectableAdvertisingMemoryGraphBindFailure> {
        let graph = LegacyConnectableAdvertisingMemoryGraphStorage::pin_static(storage)?;
        Ok(Self::from_claimed_graph(graph, default_tx_power_dbm))
    }

    /// Bind one native model graph at a deterministic controller address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_static_model(
        storage: &'static mut LegacyConnectableAdvertisingMemoryGraphStorage,
        base: LegacyConnectableAdvertisingMemoryGraphModelAddress,
        default_tx_power_dbm: LegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<Self, LegacyConnectableAdvertisingMemoryGraphBindFailure> {
        let graph =
            LegacyConnectableAdvertisingMemoryGraphStorage::pin_static_model(storage, base)?;
        Ok(Self::from_claimed_graph(graph, default_tx_power_dbm))
    }

    /// Physical transmit-power request retained with the graph.
    pub const fn default_tx_power_dbm(&self) -> LegacyAdvertisingDefaultTxPowerDbm {
        self.default_tx_power_dbm
    }

    /// Whether the sole response-capable graph is available for a new event.
    pub const fn event_is_idle(&self) -> bool {
        self.idle.is_some()
    }

    /// Restore the portable generation owner after advertising stops between events.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn restore_disabled_advertiser(
        &mut self,
        configured: LegacyConnectableAdvertiserConfigured<'static>,
    ) -> Result<(), LegacyConnectableAdvertisingDisabledRestoreFailure> {
        if self.standby.is_some() || self.idle.is_none() {
            return Err(LegacyConnectableAdvertisingDisabledRestoreFailure {
                _configured: configured,
            });
        }
        self.standby = Some(configured.into_standby());
        Ok(())
    }

    /// Atomically prepare one portable event and loan the peripheral RX pool.
    ///
    /// All ordinary errors restore both runtime slots before returning. An
    /// impossible identity disagreement is retained as an opaque fail-stop
    /// owner rather than fabricating either CPU owner.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn begin_event(
        &mut self,
        definition: LegacyConnectableAdvertisingSetPrepared,
        peripheral: &mut PeripheralConnectionRuntimeResources,
    ) -> Result<LegacyConnectableAdvertisingPrepared, LegacyConnectableAdvertisingRuntimeBeginFailure>
    {
        if self.standby.is_none() || self.idle.is_none() {
            return Err(
                LegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
                    definition,
                },
            );
        }
        let Some(standby) = self.standby.take() else {
            return Err(
                LegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
                    definition,
                },
            );
        };
        let portable = match standby.configure(definition.set).enable() {
            Ok(event) => event.prepare(),
            Err(failure) => {
                self.standby = Some(failure.into_configured().into_standby());
                return Err(LegacyConnectableAdvertisingRuntimeBeginFailure::GenerationExhausted);
            }
        };
        self.begin_prepared_event(definition, portable, peripheral)
    }

    /// Rebuild a portable successor selected by `schedule_next` in the two
    /// exact static runtime slots.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn begin_scheduled_event(
        &mut self,
        definition: LegacyConnectableAdvertisingSetPrepared,
        event: LegacyConnectableAdvertisingEvent<'static>,
        peripheral: &mut PeripheralConnectionRuntimeResources,
    ) -> Result<LegacyConnectableAdvertisingPrepared, LegacyConnectableAdvertisingRuntimeBeginFailure>
    {
        self.begin_prepared_event(definition, event.prepare(), peripheral)
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn begin_prepared_event(
        &mut self,
        definition: LegacyConnectableAdvertisingSetPrepared,
        portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
        peripheral: &mut PeripheralConnectionRuntimeResources,
    ) -> Result<LegacyConnectableAdvertisingPrepared, LegacyConnectableAdvertisingRuntimeBeginFailure>
    {
        let adv_ind = portable.adv_ind_pdu();
        let scan_response = portable.scan_response_pdu();
        let adv_ind = match LegacyConnectableAdvIndPacketInput::try_from_encoded_extent(
            adv_ind.as_bytes(),
            adv_ind.payload_length(),
        ) {
            Ok(input) => input,
            Err(error) => {
                self.standby = Some(portable.disable().into_standby());
                return Err(LegacyConnectableAdvertisingRuntimeBeginFailure::PduFit {
                    definition,
                    error,
                });
            }
        };
        let scan_response = match LegacyConnectableScanResponsePacketInput::try_from_encoded_extent(
            scan_response.as_bytes(),
            scan_response.payload_length(),
        ) {
            Ok(input) => input,
            Err(error) => {
                self.standby = Some(portable.disable().into_standby());
                return Err(LegacyConnectableAdvertisingRuntimeBeginFailure::PduFit {
                    definition,
                    error,
                });
            }
        };

        let Some(graph) = self.idle.take() else {
            self.standby = Some(portable.disable().into_standby());
            return Err(
                LegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
                    definition,
                },
            );
        };
        let allocation = match peripheral.begin_event() {
            Ok(allocation) => allocation,
            Err(error) => {
                self.idle = Some(graph);
                self.standby = Some(portable.disable().into_standby());
                return Err(
                    LegacyConnectableAdvertisingRuntimeBeginFailure::PeripheralEventActive {
                        definition,
                        error,
                    },
                );
            }
        };
        let (reserved, pool) = allocation.reserve_graph();
        let input = LegacyConnectableAdvertisingMemoryInput::new(
            adv_ind,
            scan_response,
            definition.own_address,
            definition.primary_channel,
        );
        match graph.prepare_response_capable_event(input, pool, self.default_tx_power_dbm.dbm()) {
            Ok(memory) => Ok(LegacyConnectableAdvertisingPrepared {
                definition,
                portable,
                memory,
                reserved,
            }),
            Err(failure) => {
                let (graph, pool, error) = failure.into_parts();
                let configured = portable.disable();
                let allocation = match reserved.rejoin_receive_pool(pool) {
                    Ok(allocation) => allocation,
                    Err(rejoin) => {
                        return Err(
                            LegacyConnectableAdvertisingRuntimeBeginFailure::OwnershipInvariant {
                                _invariant:
                                    LegacyConnectableAdvertisingOwnershipInvariant::ReceivePoolRejoin {
                                        _definition: definition,
                                        _configured: configured,
                                        _graph: graph,
                                        _rejoin: rejoin,
                                    },
                            },
                        );
                    }
                };
                let cancelled = LegacyConnectableAdvertisingCancelled {
                    definition,
                    configured,
                    graph,
                    allocation,
                };
                match self.restore_cancelled(cancelled, peripheral) {
                    Ok(definition) => Err(
                        LegacyConnectableAdvertisingRuntimeBeginFailure::MemoryPreparation {
                            definition,
                            error,
                        },
                    ),
                    Err(cancelled) => Err(
                        LegacyConnectableAdvertisingRuntimeBeginFailure::OwnershipInvariant {
                            _invariant:
                                LegacyConnectableAdvertisingOwnershipInvariant::RuntimeRestore {
                                    _cancelled: cancelled,
                                },
                        },
                    ),
                }
            }
        }
    }

    /// Restore an unpublished cancellation only to both originating runtimes.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn restore_cancelled(
        &mut self,
        cancelled: LegacyConnectableAdvertisingCancelled,
        peripheral: &mut PeripheralConnectionRuntimeResources,
    ) -> Result<LegacyConnectableAdvertisingSetPrepared, LegacyConnectableAdvertisingCancelled>
    {
        if self.standby.is_some()
            || self.idle.is_some()
            || cancelled.graph.identity() != self.graph_identity
        {
            return Err(cancelled);
        }
        let LegacyConnectableAdvertisingCancelled {
            definition,
            configured,
            graph,
            allocation,
        } = cancelled;
        let allocation = match peripheral.restore_idle(allocation) {
            Ok(()) => {
                self.standby = Some(configured.into_standby());
                self.idle = Some(graph);
                return Ok(definition);
            }
            Err(allocation) => allocation,
        };
        Err(LegacyConnectableAdvertisingCancelled {
            definition,
            configured,
            graph,
            allocation,
        })
    }

    /// Atomically return an event with no accepted connection to both runtimes.
    ///
    /// The peripheral slot is restored before the advertising slot is changed.
    /// Every condition that could reject the advertising owner is checked first,
    /// so success cannot leave a partially restored pair.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn restore_no_connection(
        &mut self,
        outcome: LegacyConnectableAdvertisingNoConnection,
        peripheral: &mut PeripheralConnectionRuntimeResources,
    ) -> Result<
        LegacyConnectableAdvertisingNoConnectionRestored,
        LegacyConnectableAdvertisingNoConnection,
    > {
        if self.standby.is_some()
            || self.idle.is_some()
            || outcome.graph.identity() != self.graph_identity
        {
            return Err(outcome);
        }
        let LegacyConnectableAdvertisingNoConnection {
            definition,
            graph,
            allocation,
            complete,
            phase,
            scheduler_status,
            rejected_packets,
        } = outcome;
        let allocation = match peripheral.restore_idle(allocation) {
            Ok(()) => {
                self.idle = Some(graph);
                return Ok(LegacyConnectableAdvertisingNoConnectionRestored {
                    definition,
                    complete,
                    phase,
                    scheduler_status,
                    rejected_packets,
                });
            }
            Err(allocation) => allocation,
        };
        Err(LegacyConnectableAdvertisingNoConnection {
            definition,
            graph,
            allocation,
            complete,
            phase,
            scheduler_status,
            rejected_packets,
        })
    }

    /// Return only the advertising graph after a connection was accepted.
    ///
    /// The peripheral allocation deliberately remains checked out in the
    /// returned transfer owner and therefore cannot be reused by another role.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn restore_connection_accepted(
        &mut self,
        outcome: LegacyConnectableAdvertisingConnectionAccepted,
    ) -> Result<
        LegacyConnectableAdvertisingConnectionTransfer,
        LegacyConnectableAdvertisingConnectionAccepted,
    > {
        if self.idle.is_some() || outcome.graph.identity() != self.graph_identity {
            return Err(outcome);
        }
        let LegacyConnectableAdvertisingConnectionAccepted {
            graph,
            configured,
            identity,
            peripheral,
            phase,
            scheduler_status,
            rejected_packets,
        } = outcome;
        let advertising_set = configured.set();
        self.standby = Some(configured.into_standby());
        self.idle = Some(graph);
        Ok(LegacyConnectableAdvertisingConnectionTransfer {
            advertising_set,
            identity,
            peripheral,
            phase,
            scheduler_status,
            rejected_packets,
        })
    }
}

/// Pre-publication response graph retaining every portable and affine owner.
#[must_use = "the prepared response graph must advance, cancel, or remain retained"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct LegacyConnectableAdvertisingPrepared {
    definition: LegacyConnectableAdvertisingSetPrepared,
    portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
    memory: LegacyConnectableAdvertisingMemoryGraphPrepared,
    reserved: PeripheralConnectionRuntimeGraphReserved,
}

#[cfg(any(target_arch = "riscv32", test))]
#[expect(
    clippy::result_large_err,
    reason = "preparation and rollback return the exact affine graph, role allocation, or connection owner inline"
)]
impl LegacyConnectableAdvertisingPrepared {
    #[cfg(test)]
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.portable.identity()
    }

    /// Form the complete response-capable scheduler window without changing SRAM.
    pub(crate) fn form_first_event_candidate(
        self,
        timing: LegacyAdvertisingTimingObservation,
        config: SchedulerSoftwareConfig,
    ) -> Result<
        LegacyConnectableAdvertisingEventCandidate,
        LegacyConnectableAdvertisingEventTimingFailure,
    > {
        let Some((scheduler_window, raw_window)) =
            timing.first_connectable_window(config, self.memory.post_anchor_duration())
        else {
            return Err(LegacyConnectableAdvertisingEventTimingFailure { prepared: self });
        };
        Ok(LegacyConnectableAdvertisingEventCandidate {
            prepared: self,
            scheduler_window,
            raw_window,
        })
    }

    /// Project a phase-locked successor selected by the portable Link Layer.
    pub(crate) fn form_recurring_event_candidate(
        self,
        timing: LegacyAdvertisingRecurringTimingObservation,
        previous_phase: LegacyAdvertisingEventPhase,
        start_offset_micros: u64,
        config: SchedulerSoftwareConfig,
    ) -> Result<
        LegacyConnectableAdvertisingEventCandidate,
        LegacyConnectableAdvertisingEventTimingFailure,
    > {
        let Some((scheduler_window, raw_window)) = timing.recurring_connectable_window(
            previous_phase,
            start_offset_micros,
            config,
            self.memory.post_anchor_duration(),
        ) else {
            return Err(LegacyConnectableAdvertisingEventTimingFailure { prepared: self });
        };
        Ok(LegacyConnectableAdvertisingEventCandidate {
            prepared: self,
            scheduler_window,
            raw_window,
        })
    }

    /// Recover all ordinary CPU owners before any scheduler publication.
    pub(crate) fn cancel(
        self,
    ) -> Result<
        LegacyConnectableAdvertisingCancelled,
        LegacyConnectableAdvertisingCancellationInvariant,
    > {
        let Self {
            definition,
            portable,
            memory,
            reserved,
        } = self;
        let configured = portable.disable();
        let (graph, pool) = memory.cancel();
        match reserved.rejoin_receive_pool(pool) {
            Ok(allocation) => Ok(LegacyConnectableAdvertisingCancelled {
                definition,
                configured,
                graph,
                allocation,
            }),
            Err(rejoin) => Err(LegacyConnectableAdvertisingCancellationInvariant {
                _definition: definition,
                _configured: configured,
                _graph: graph,
                _rejoin: rejoin,
            }),
        }
    }
}

/// Timing projection failed without changing the prepared response graph.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged connectable advertising owner remains recoverable"]
pub(crate) struct LegacyConnectableAdvertisingEventTimingFailure {
    prepared: LegacyConnectableAdvertisingPrepared,
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyConnectableAdvertisingEventTimingFailure {
    pub(crate) fn into_prepared(self) -> LegacyConnectableAdvertisingPrepared {
        self.prepared
    }
}

/// Complete response graph with live timing but no common-timeline reservation.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the candidate must enter common scheduling, cancel, or remain retained"]
pub(crate) struct LegacyConnectableAdvertisingEventCandidate {
    prepared: LegacyConnectableAdvertisingPrepared,
    scheduler_window: LegacyAdvertisingEventWindow,
    raw_window: SchedulerRawWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[expect(
    clippy::result_large_err,
    reason = "preparation and rollback return the exact affine graph, role allocation, or connection owner inline"
)]
impl LegacyConnectableAdvertisingEventCandidate {
    pub(crate) const fn raw_window(&self) -> SchedulerRawWindow {
        self.raw_window
    }

    #[cfg(test)]
    pub(crate) const fn phase(&self) -> LegacyAdvertisingEventPhase {
        self.scheduler_window.phase()
    }

    pub(crate) fn prepare_resolved_event_image(
        self,
        resolved_window: SchedulerRawWindow,
    ) -> Result<
        LegacyConnectableAdvertisingEventImagePrepared,
        LegacyConnectableAdvertisingEventImagePrepareFailure,
    > {
        let Self {
            prepared,
            scheduler_window,
            raw_window,
        } = self;
        let LegacyConnectableAdvertisingPrepared {
            definition,
            portable,
            memory,
            reserved,
        } = prepared;
        match memory.prepare_event_fields(resolved_window.start(), resolved_window.end()) {
            Ok(memory) => Ok(LegacyConnectableAdvertisingEventImagePrepared {
                definition,
                portable,
                memory,
                reserved,
                scheduler_window,
            }),
            Err(failure) => {
                let (memory, error) = failure.into_parts();
                Err(LegacyConnectableAdvertisingEventImagePrepareFailure {
                    candidate: Self {
                        prepared: LegacyConnectableAdvertisingPrepared {
                            definition,
                            portable,
                            memory,
                            reserved,
                        },
                        scheduler_window,
                        raw_window,
                    },
                    error,
                })
            }
        }
    }

    pub(crate) fn cancel(
        self,
    ) -> Result<
        LegacyConnectableAdvertisingCancelled,
        LegacyConnectableAdvertisingCancellationInvariant,
    > {
        self.prepared.cancel()
    }
}

/// Event-field failure retaining the unchanged candidate and both memory owners.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged connectable advertising candidate remains recoverable"]
pub(crate) struct LegacyConnectableAdvertisingEventImagePrepareFailure {
    candidate: LegacyConnectableAdvertisingEventCandidate,
    error: LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyConnectableAdvertisingEventImagePrepareFailure {
    pub(crate) const fn error(
        &self,
    ) -> LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError {
        self.error
    }

    pub(crate) fn into_candidate(self) -> LegacyConnectableAdvertisingEventCandidate {
        self.candidate
    }
}

/// Complete event fields paired with every portable and affine owner.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the event image must remain paired with its scheduler reservation"]
pub(crate) struct LegacyConnectableAdvertisingEventImagePrepared {
    definition: LegacyConnectableAdvertisingSetPrepared,
    portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
    memory: LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared,
    reserved: PeripheralConnectionRuntimeGraphReserved,
    scheduler_window: LegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[expect(
    clippy::result_large_err,
    reason = "preparation and rollback return the exact affine graph, role allocation, or connection owner inline"
)]
impl LegacyConnectableAdvertisingEventImagePrepared {
    pub(crate) fn prepare_scheduler_bookkeeping(
        self,
    ) -> LegacyConnectableAdvertisingSchedulerBookkeepingPrepared {
        LegacyConnectableAdvertisingSchedulerBookkeepingPrepared {
            definition: self.definition,
            portable: self.portable,
            memory: self.memory.prepare_scheduler_bookkeeping(),
            reserved: self.reserved,
            scheduler_window: self.scheduler_window,
        }
    }

    pub(crate) fn cancel(
        self,
    ) -> Result<
        LegacyConnectableAdvertisingCancelled,
        LegacyConnectableAdvertisingCancellationInvariant,
    > {
        LegacyConnectableAdvertisingPrepared {
            definition: self.definition,
            portable: self.portable,
            memory: self.memory.cancel(),
            reserved: self.reserved,
        }
        .cancel()
    }
}

/// Connectable event with common scheduler bookkeeping but no list ownership.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct LegacyConnectableAdvertisingSchedulerBookkeepingPrepared {
    definition: LegacyConnectableAdvertisingSetPrepared,
    portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
    memory: LegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    reserved: PeripheralConnectionRuntimeGraphReserved,
    scheduler_window: LegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyConnectableAdvertisingSchedulerBookkeepingPrepared {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) fn prepare_empty_list_link(
        self,
    ) -> LegacyConnectableAdvertisingEmptyListLinkPrepared {
        LegacyConnectableAdvertisingEmptyListLinkPrepared {
            definition: self.definition,
            portable: self.portable,
            memory: self.memory.prepare_empty_list_link(),
            reserved: self.reserved,
            scheduler_window: self.scheduler_window,
        }
    }

    pub(crate) fn cancel(self) -> LegacyConnectableAdvertisingEventImagePrepared {
        LegacyConnectableAdvertisingEventImagePrepared {
            definition: self.definition,
            portable: self.portable,
            memory: self.memory.cancel(),
            reserved: self.reserved,
            scheduler_window: self.scheduler_window,
        }
    }
}

/// Response-capable event joined to the source-owned empty scheduler list.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct LegacyConnectableAdvertisingEmptyListLinkPrepared {
    definition: LegacyConnectableAdvertisingSetPrepared,
    portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
    memory: LegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared,
    reserved: PeripheralConnectionRuntimeGraphReserved,
    scheduler_window: LegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
#[expect(
    clippy::result_large_err,
    reason = "preparation and rollback return the exact affine graph, role allocation, or connection owner inline"
)]
impl LegacyConnectableAdvertisingEmptyListLinkPrepared {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    /// Freeze the memory graph while retaining every non-memory owner separately.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn prepare_publication(self) -> LegacyConnectableAdvertisingPublicationPrepared {
        LegacyConnectableAdvertisingPublicationPrepared {
            memory: self.memory.prepare_publication(),
            remainder: LegacyConnectableAdvertisingPublicationRemainder {
                definition: self.definition,
                portable: self.portable,
                reserved: self.reserved,
                scheduler_window: self.scheduler_window,
            },
        }
    }

    pub(crate) fn cancel(
        self,
    ) -> Result<
        LegacyConnectableAdvertisingCancelled,
        LegacyConnectableAdvertisingCancellationInvariant,
    > {
        LegacyConnectableAdvertisingEventImagePrepared {
            definition: self.definition,
            portable: self.portable,
            memory: self.memory.cancel().cancel(),
            reserved: self.reserved,
            scheduler_window: self.scheduler_window,
        }
        .cancel()
    }
}

/// Frozen CPU-owned graph and exact portable remainder before any MMIO edge.
#[cfg(target_arch = "riscv32")]
#[must_use = "the publication owner must enter the atomic MMIO suffix or be cancelled"]
pub(crate) struct LegacyConnectableAdvertisingPublicationPrepared {
    memory: LegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
    remainder: LegacyConnectableAdvertisingPublicationRemainder,
}

#[cfg(target_arch = "riscv32")]
impl LegacyConnectableAdvertisingPublicationPrepared {
    pub(crate) const fn random_address(&self) -> Option<ControllerRandomAddress> {
        self.remainder.definition.random_address()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
        LegacyConnectableAdvertisingPublicationRemainder,
    ) {
        (self.memory, self.remainder)
    }
}

/// Portable and connection-allocation owners retained across atomic publication.
#[cfg(target_arch = "riscv32")]
#[must_use = "the publication remainder must rejoin its exact memory graph"]
pub(crate) struct LegacyConnectableAdvertisingPublicationRemainder {
    definition: LegacyConnectableAdvertisingSetPrepared,
    portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
    reserved: PeripheralConnectionRuntimeGraphReserved,
    scheduler_window: LegacyAdvertisingEventWindow,
}

#[cfg(target_arch = "riscv32")]
impl LegacyConnectableAdvertisingPublicationRemainder {
    /// Seal the portable event only after the exact memory graph has joined RUN.
    pub(crate) fn into_running(
        self,
        memory: LegacyConnectableAdvertisingMemoryGraphRunning,
    ) -> LegacyConnectableAdvertisingRunning {
        LegacyConnectableAdvertisingRunning {
            definition: self.definition,
            portable: self.portable.into_submitted(),
            memory,
            reserved: self.reserved,
            phase: self.scheduler_window.phase(),
        }
    }
}

/// Role-owned state after the response graph and portable event both crossed RUN.
#[cfg(target_arch = "riscv32")]
#[must_use = "the running connectable event retains the graph, LL event, and peripheral reservation"]
pub(crate) struct LegacyConnectableAdvertisingRunning {
    definition: LegacyConnectableAdvertisingSetPrepared,
    portable: LegacyConnectableAdvertisingEventInFlight<'static>,
    memory: LegacyConnectableAdvertisingMemoryGraphRunning,
    reserved: PeripheralConnectionRuntimeGraphReserved,
    phase: LegacyAdvertisingEventPhase,
}

#[cfg(target_arch = "riscv32")]
impl LegacyConnectableAdvertisingRunning {
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.portable.identity()
    }

    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    /// Separate the role continuation from the common memory completion spine.
    ///
    /// The returned remainder records both exact memory identities. The common
    /// scheduler may carry only the memory owner through finished-list removal;
    /// role dispatch becomes possible only when the reclaimed graph rejoins this
    /// remainder through [`LegacyConnectableAdvertisingPostRunRemainder::classify_recycled`].
    pub(crate) fn into_memory_completion(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphRunning,
        LegacyConnectableAdvertisingPostRunRemainder,
    ) {
        let graph_identity = self.memory.identity();
        let receive_identity = self.memory.receive_identity();
        (
            self.memory,
            LegacyConnectableAdvertisingPostRunRemainder {
                definition: self.definition,
                portable: self.portable,
                reserved: self.reserved,
                phase: self.phase,
                graph_identity,
                receive_identity,
            },
        )
    }

    pub(crate) fn from_memory_completion(
        memory: LegacyConnectableAdvertisingMemoryGraphRunning,
        remainder: LegacyConnectableAdvertisingPostRunRemainder,
    ) -> Self {
        Self {
            definition: remainder.definition,
            portable: remainder.portable,
            memory,
            reserved: remainder.reserved,
            phase: remainder.phase,
        }
    }
}

/// Connectable role continuation paired with the exact completed memory graph.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed graph and role continuation must be recycled together"]
pub(crate) struct LegacyConnectableAdvertisingCompletionObserved {
    memory: LegacyConnectableAdvertisingMemoryGraphCompletionObserved,
    remainder: LegacyConnectableAdvertisingPostRunRemainder,
}

#[cfg(target_arch = "riscv32")]
impl LegacyConnectableAdvertisingCompletionObserved {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) fn new(
        memory: LegacyConnectableAdvertisingMemoryGraphCompletionObserved,
        remainder: LegacyConnectableAdvertisingPostRunRemainder,
    ) -> Self {
        Self { memory, remainder }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingMemoryGraphCompletionObserved,
        LegacyConnectableAdvertisingPostRunRemainder,
    ) {
        (self.memory, self.remainder)
    }
}

/// Portable and peripheral owners retained while common code reclaims memory.
#[cfg(target_arch = "riscv32")]
#[must_use = "rejoin the exact reclaimed memory graph before classifying the role outcome"]
pub(crate) struct LegacyConnectableAdvertisingPostRunRemainder {
    definition: LegacyConnectableAdvertisingSetPrepared,
    portable: LegacyConnectableAdvertisingEventInFlight<'static>,
    reserved: PeripheralConnectionRuntimeGraphReserved,
    phase: LegacyAdvertisingEventPhase,
    graph_identity: LegacyConnectableAdvertisingMemoryGraphIdentity,
    receive_identity: oer_esp32s31_bluetooth_memory::NonScanningRxMemoryIdentity,
}

#[cfg(target_arch = "riscv32")]
impl LegacyConnectableAdvertisingPostRunRemainder {
    /// Classify the exact copied receive batch without interpreting scheduler status.
    ///
    /// Malformed, differently addressed and unsupported requests leave the
    /// portable event in flight and therefore become `NoConnection`. A valid
    /// final `CONNECT_IND` transfers the exact receive allocation and packet
    /// metadata into the peripheral role. Missing PDU bytes, a foreign memory
    /// owner, or a packet after an accepted connection remains sealed fail-stop.
    pub(crate) fn classify_recycled(
        self,
        recycled: LegacyConnectableAdvertisingMemoryGraphRecycled,
    ) -> LegacyConnectableAdvertisingPostRunOutcome {
        if recycled.identity() != self.graph_identity
            || recycled.receive_identity() != self.receive_identity
        {
            return LegacyConnectableAdvertisingPostRunOutcome::FailStop(
                LegacyConnectableAdvertisingPostRunFailStop {
                    ownership:
                        LegacyConnectableAdvertisingPostRunFailStopOwnership::MemoryIdentity {
                            _remainder: self,
                            _recycled: recycled,
                        },
                },
            );
        }
        let dispatch = match recycled.prepare_rx_dispatch() {
            Ok(dispatch) => dispatch,
            Err(blocked) => {
                return LegacyConnectableAdvertisingPostRunOutcome::FailStop(
                    LegacyConnectableAdvertisingPostRunFailStop {
                        ownership:
                            LegacyConnectableAdvertisingPostRunFailStopOwnership::ReceivePduUnavailable {
                                _remainder: self,
                                blocked,
                            },
                    },
                );
            }
        };
        let (graph, pool, batch, scheduler_status) = dispatch.into_parts();
        let allocation = match self.reserved.rejoin_receive_pool(pool) {
            Ok(allocation) => allocation,
            Err(rejoin) => {
                return LegacyConnectableAdvertisingPostRunOutcome::FailStop(
                    LegacyConnectableAdvertisingPostRunFailStop {
                        ownership:
                            LegacyConnectableAdvertisingPostRunFailStopOwnership::ReceivePoolRejoin {
                                _definition: self.definition,
                                _portable: self.portable,
                                _graph: graph,
                                _batch: batch,
                                _scheduler_status: scheduler_status,
                                _phase: self.phase,
                                _rejoin: rejoin,
                            },
                    },
                );
            }
        };

        let packets = [batch.packet(0).copied(), batch.packet(1).copied()];
        match classify_received_pdus(self.portable, packets, 0, 0) {
            LegacyConnectableAdvertisingPortableRxOutcome::NoConnection {
                complete,
                rejected_packets,
            } => LegacyConnectableAdvertisingPostRunOutcome::NoConnection(
                LegacyConnectableAdvertisingNoConnection {
                    definition: self.definition,
                    graph,
                    allocation,
                    complete,
                    phase: self.phase,
                    scheduler_status,
                    rejected_packets,
                },
            ),
            LegacyConnectableAdvertisingPortableRxOutcome::ConnectionAccepted {
                accepted,
                packet,
                rejected_packets,
            } => {
                let (configured, identity, connection) = accepted.into_parts();
                LegacyConnectableAdvertisingPostRunOutcome::ConnectionAccepted(
                    LegacyConnectableAdvertisingConnectionAccepted {
                        graph,
                        configured,
                        identity,
                        peripheral: PeripheralConnectionAcceptedRequest::new(
                            allocation, connection, packet,
                        ),
                        phase: self.phase,
                        scheduler_status,
                        rejected_packets,
                    },
                )
            }
            LegacyConnectableAdvertisingPortableRxOutcome::PacketAfterConnection {
                accepted,
                packet,
                rejected_packets,
            } => LegacyConnectableAdvertisingPostRunOutcome::FailStop(
                LegacyConnectableAdvertisingPostRunFailStop {
                    ownership:
                        LegacyConnectableAdvertisingPostRunFailStopOwnership::PacketAfterConnection {
                            _definition: self.definition,
                            _accepted: accepted,
                            _accepted_packet: packet,
                            _graph: graph,
                            _allocation: allocation,
                            _batch: batch,
                            _scheduler_status: scheduler_status,
                            _phase: self.phase,
                            _rejected_packets: rejected_packets,
                        },
                },
            ),
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
enum LegacyConnectableAdvertisingPortableRxOutcome<P> {
    NoConnection {
        complete: LegacyConnectableAdvertisingEventComplete<'static>,
        rejected_packets: usize,
    },
    ConnectionAccepted {
        accepted: LegacyConnectableConnectionRequestAccepted<'static>,
        packet: P,
        rejected_packets: usize,
    },
    PacketAfterConnection {
        accepted: LegacyConnectableConnectionRequestAccepted<'static>,
        packet: P,
        rejected_packets: usize,
    },
}

#[cfg(any(target_arch = "riscv32", test))]
trait LegacyConnectableAdvertisingReceivedPdu: Copy {
    fn pdu_bytes(&self) -> &[u8];
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyConnectableAdvertisingReceivedPdu for LeReceivedPdu {
    fn pdu_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn classify_received_pdus<P: LegacyConnectableAdvertisingReceivedPdu>(
    in_flight: LegacyConnectableAdvertisingEventInFlight<'static>,
    packets: [Option<P>; BLUETOOTH_NON_SCANNING_RX_NODE_COUNT],
    index: usize,
    rejected_packets: usize,
) -> LegacyConnectableAdvertisingPortableRxOutcome<P> {
    let Some(packet) = packets.get(index).copied().flatten() else {
        return LegacyConnectableAdvertisingPortableRxOutcome::NoConnection {
            complete: in_flight.complete_without_connection(),
            rejected_packets,
        };
    };
    match in_flight.admit_connection_request(packet.pdu_bytes()) {
        LegacyConnectableConnectionRequestAdmission::Accepted(accepted) => {
            if packets.get(index + 1).is_some_and(Option::is_some) {
                LegacyConnectableAdvertisingPortableRxOutcome::PacketAfterConnection {
                    accepted,
                    packet,
                    rejected_packets,
                }
            } else {
                LegacyConnectableAdvertisingPortableRxOutcome::ConnectionAccepted {
                    accepted,
                    packet,
                    rejected_packets,
                }
            }
        }
        LegacyConnectableConnectionRequestAdmission::Rejected(rejected) => classify_received_pdus(
            rejected.into_in_flight(),
            packets,
            index + 1,
            rejected_packets + 1,
        ),
    }
}

/// Role-specific result after exact memory reclamation and portable RX dispatch.
#[cfg(target_arch = "riscv32")]
#[must_use = "restore the reusable owners or retain the sealed fail-stop state"]
#[expect(
    clippy::large_enum_variant,
    reason = "a classified result retains the reusable owners or the complete sealed hardware failure"
)]
pub(crate) enum LegacyConnectableAdvertisingPostRunOutcome {
    NoConnection(LegacyConnectableAdvertisingNoConnection),
    ConnectionAccepted(LegacyConnectableAdvertisingConnectionAccepted),
    FailStop(LegacyConnectableAdvertisingPostRunFailStop),
}

/// Completed event which accepted no connection and owns both reusable graphs.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "restore both originating runtime slots before scheduling another event"]
pub(crate) struct LegacyConnectableAdvertisingNoConnection {
    definition: LegacyConnectableAdvertisingSetPrepared,
    graph: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
    allocation: PeripheralConnectionRuntimeAllocation,
    complete: LegacyConnectableAdvertisingEventComplete<'static>,
    phase: LegacyAdvertisingEventPhase,
    scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

/// Restored no-connection event ready for a later recurrence decision.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "retain the completed portable event or schedule its next occurrence"]
pub(crate) struct LegacyConnectableAdvertisingNoConnectionRestored {
    definition: LegacyConnectableAdvertisingSetPrepared,
    complete: LegacyConnectableAdvertisingEventComplete<'static>,
    phase: LegacyAdvertisingEventPhase,
    scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyConnectableAdvertisingNoConnectionRestored {
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.complete.identity()
    }

    pub(crate) const fn definition(&self) -> LegacyConnectableAdvertisingSetPrepared {
        self.definition
    }

    pub(crate) const fn phase(&self) -> LegacyAdvertisingEventPhase {
        self.phase
    }

    pub(crate) const fn scheduler_status(
        &self,
    ) -> LegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.scheduler_status
    }

    pub(crate) const fn rejected_packets(&self) -> usize {
        self.rejected_packets
    }

    /// Attach the caller's fresh advertising delay through the portable LL.
    pub(crate) fn schedule_next(
        self,
        delay: AdvertisingDelay,
    ) -> LegacyConnectableAdvertisingNextEventScheduled {
        let Self {
            definition,
            complete,
            phase,
            scheduler_status,
            rejected_packets,
        } = self;
        let (portable, start_offset_micros) = match complete.schedule_next(delay) {
            Ok(scheduled) => {
                let start_offset_micros = scheduled.start_offset_micros();
                (
                    LegacyConnectableAdvertisingNextEventPortable::Event(scheduled.into_event()),
                    start_offset_micros,
                )
            }
            Err(exhausted) => (
                LegacyConnectableAdvertisingNextEventPortable::SequenceExhausted(
                    exhausted.into_complete(),
                ),
                0,
            ),
        };
        LegacyConnectableAdvertisingNextEventScheduled {
            definition,
            portable,
            start_offset_micros,
            previous_phase: phase,
            previous_scheduler_status: scheduler_status,
            rejected_packets,
        }
    }

    /// Stop at the already-restored CPU boundary without inventing a successor.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn prepare_recurrence_stop(
        self,
    ) -> (
        LegacyConnectableAdvertiserConfigured<'static>,
        LegacyConnectableAdvertisingRecurrenceStopped,
    ) {
        let Self {
            definition: _,
            complete,
            phase,
            scheduler_status,
            rejected_packets,
        } = self;
        let identity = complete.identity();
        let configured = complete.disable();
        let stopped = LegacyConnectableAdvertisingRecurrenceStopped::from_portable_set(
            configured.set(),
            identity,
            phase,
            scheduler_status,
            rejected_packets,
        );
        (configured, stopped)
    }
}

/// Portable successor plus the exact completed S31 phase and diagnostics.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "prepare, cancel, or retain the exact scheduled successor"]
pub(crate) struct LegacyConnectableAdvertisingNextEventScheduled {
    definition: LegacyConnectableAdvertisingSetPrepared,
    portable: LegacyConnectableAdvertisingNextEventPortable,
    start_offset_micros: u64,
    previous_phase: LegacyAdvertisingEventPhase,
    previous_scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) enum LegacyConnectableAdvertisingNextEventPortable {
    Event(LegacyConnectableAdvertisingEvent<'static>),
    SequenceExhausted(LegacyConnectableAdvertisingEventComplete<'static>),
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyConnectableAdvertisingNextEventPortable {
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        match self {
            Self::Event(event) => event.identity(),
            Self::SequenceExhausted(complete) => complete.identity(),
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyConnectableAdvertisingNextEventScheduled {
    pub(crate) fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingSetPrepared,
        LegacyConnectableAdvertisingNextEventPortable,
        u64,
        LegacyAdvertisingEventPhase,
        LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        usize,
    ) {
        (
            self.definition,
            self.portable,
            self.start_offset_micros,
            self.previous_phase,
            self.previous_scheduler_status,
            self.rejected_packets,
        )
    }
}

/// CPU-only terminal owner after recurrence was cancelled before publication.
#[cfg(target_arch = "riscv32")]
#[must_use = "retain the stopped portable set and its completed-event diagnostics"]
pub(crate) struct LegacyConnectableAdvertisingRecurrenceStopped {
    portable_set: LegacyConnectableAdvertisingSet<'static>,
    identity: LegacyAdvertisingEventIdentity,
    previous_phase: LegacyAdvertisingEventPhase,
    previous_scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

#[cfg(target_arch = "riscv32")]
impl LegacyConnectableAdvertisingRecurrenceStopped {
    pub(crate) const fn from_restored_definition(
        definition: LegacyConnectableAdvertisingSetPrepared,
        identity: LegacyAdvertisingEventIdentity,
        previous_phase: LegacyAdvertisingEventPhase,
        previous_scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        rejected_packets: usize,
    ) -> Self {
        Self::from_portable_set(
            definition.set(),
            identity,
            previous_phase,
            previous_scheduler_status,
            rejected_packets,
        )
    }

    pub(crate) const fn from_portable_set(
        portable_set: LegacyConnectableAdvertisingSet<'static>,
        identity: LegacyAdvertisingEventIdentity,
        previous_phase: LegacyAdvertisingEventPhase,
        previous_scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        rejected_packets: usize,
    ) -> Self {
        Self {
            portable_set,
            identity,
            previous_phase,
            previous_scheduler_status,
            rejected_packets,
        }
    }

    pub(crate) const fn portable_set(&self) -> LegacyConnectableAdvertisingSet<'static> {
        self.portable_set
    }

    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.identity
    }

    pub(crate) const fn previous_phase(&self) -> LegacyAdvertisingEventPhase {
        self.previous_phase
    }

    pub(crate) const fn previous_scheduler_status(
        &self,
    ) -> LegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.previous_scheduler_status
    }

    pub(crate) const fn rejected_packets(&self) -> usize {
        self.rejected_packets
    }
}

/// Accepted connection while the advertising graph is still checked out.
#[cfg(target_arch = "riscv32")]
#[must_use = "restore the advertising graph and transfer the peripheral owner"]
pub(crate) struct LegacyConnectableAdvertisingConnectionAccepted {
    graph: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
    configured: LegacyConnectableAdvertiserConfigured<'static>,
    identity: LegacyAdvertisingEventIdentity,
    peripheral: PeripheralConnectionAcceptedRequest,
    phase: LegacyAdvertisingEventPhase,
    scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

/// Peripheral handoff after the reusable advertising graph was restored.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "normalize the accepted packet and prepare the first peripheral event"]
pub(crate) struct LegacyConnectableAdvertisingConnectionTransfer {
    advertising_set: LegacyConnectableAdvertisingSet<'static>,
    identity: LegacyAdvertisingEventIdentity,
    peripheral: PeripheralConnectionAcceptedRequest,
    phase: LegacyAdvertisingEventPhase,
    scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

#[cfg(any(target_arch = "riscv32", test))]
#[expect(
    clippy::result_large_err,
    reason = "preparation and rollback return the exact affine graph, role allocation, or connection owner inline"
)]
impl LegacyConnectableAdvertisingConnectionTransfer {
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.identity
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn peripheral(&self) -> &PeripheralConnectionAcceptedRequest {
        &self.peripheral
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingSet<'static>,
        LegacyAdvertisingEventIdentity,
        PeripheralConnectionAcceptedRequest,
        LegacyAdvertisingEventPhase,
        LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        usize,
    ) {
        (
            self.advertising_set,
            self.identity,
            self.peripheral,
            self.phase,
            self.scheduler_status,
            self.rejected_packets,
        )
    }

    /// Retire the accepted portable connection only for an explicit Reset.
    ///
    /// A rejected runtime restore reconstructs this complete transfer without
    /// losing its causal packet or advertising diagnostics.
    pub(crate) fn cancel_peripheral_for_reset(
        self,
        runtime: &mut PeripheralConnectionRuntimeResources,
    ) -> Result<
        LegacyConnectableAdvertisingPeripheralResetEvidence,
        LegacyConnectableAdvertisingPeripheralResetCancellationFailure,
    > {
        let Self {
            advertising_set,
            identity,
            peripheral,
            phase,
            scheduler_status,
            rejected_packets,
        } = self;
        match runtime.cancel_accepted_for_reset(peripheral) {
            Ok(cancelled) => Ok(LegacyConnectableAdvertisingPeripheralResetEvidence {
                advertising_set,
                identity,
                accepted_packet: cancelled.into_packet(),
                phase,
                scheduler_status,
                rejected_packets,
            }),
            Err(failure) => {
                let cause = failure.error();
                Err(
                    LegacyConnectableAdvertisingPeripheralResetCancellationFailure {
                        cause,
                        _transfer: Self {
                            advertising_set,
                            identity,
                            peripheral: failure.into_accepted(),
                            phase,
                            scheduler_status,
                            rejected_packets,
                        },
                    },
                )
            }
        }
    }
}

/// Advertising evidence retained after an accepted connection is cancelled for Reset.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "retain the causal advertising evidence through Reset completion"]
pub(crate) struct LegacyConnectableAdvertisingPeripheralResetEvidence {
    advertising_set: LegacyConnectableAdvertisingSet<'static>,
    identity: LegacyAdvertisingEventIdentity,
    accepted_packet: LeReceivedPdu,
    phase: LegacyAdvertisingEventPhase,
    scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyConnectableAdvertisingPeripheralResetEvidence {
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.identity
    }

    pub(crate) const fn advertising_set(&self) -> LegacyConnectableAdvertisingSet<'static> {
        self.advertising_set
    }

    pub(crate) const fn accepted_packet(&self) -> &LeReceivedPdu {
        &self.accepted_packet
    }

    pub(crate) const fn phase(&self) -> LegacyAdvertisingEventPhase {
        self.phase
    }

    pub(crate) const fn scheduler_status(
        &self,
    ) -> LegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.scheduler_status
    }

    pub(crate) const fn rejected_packets(&self) -> usize {
        self.rejected_packets
    }
}

/// Rejected accepted-connection Reset cancellation with every owner retained.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "retry only through the originating runtime or retain the sealed transfer"]
pub(crate) struct LegacyConnectableAdvertisingPeripheralResetCancellationFailure {
    cause: PeripheralConnectionAcceptedResetCancellationError,
    _transfer: LegacyConnectableAdvertisingConnectionTransfer,
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyConnectableAdvertisingPeripheralResetCancellationFailure {
    pub(crate) const fn cause(&self) -> PeripheralConnectionAcceptedResetCancellationError {
        self.cause
    }

    #[cfg(test)]
    pub(crate) fn into_transfer(self) -> LegacyConnectableAdvertisingConnectionTransfer {
        self._transfer
    }
}

/// Sealed post-RUN ownership when a safe role outcome cannot be proven.
#[cfg(target_arch = "riscv32")]
#[must_use = "the indeterminate hardware outcome retains every affine owner"]
pub(crate) struct LegacyConnectableAdvertisingPostRunFailStop {
    ownership: LegacyConnectableAdvertisingPostRunFailStopOwnership,
}

#[cfg(target_arch = "riscv32")]
#[expect(
    clippy::large_enum_variant,
    reason = "each failure seals all graph, receive-pool, and portable event owners without allocation"
)]
enum LegacyConnectableAdvertisingPostRunFailStopOwnership {
    MemoryIdentity {
        _remainder: LegacyConnectableAdvertisingPostRunRemainder,
        _recycled: LegacyConnectableAdvertisingMemoryGraphRecycled,
    },
    ReceivePduUnavailable {
        _remainder: LegacyConnectableAdvertisingPostRunRemainder,
        blocked: LegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked,
    },
    ReceivePoolRejoin {
        _definition: LegacyConnectableAdvertisingSetPrepared,
        _portable: LegacyConnectableAdvertisingEventInFlight<'static>,
        _graph: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        _batch: LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
        _scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        _phase: LegacyAdvertisingEventPhase,
        _rejoin: PeripheralConnectionRuntimeGraphRejoinFailure,
    },
    PacketAfterConnection {
        _definition: LegacyConnectableAdvertisingSetPrepared,
        _accepted: LegacyConnectableConnectionRequestAccepted<'static>,
        _accepted_packet: LeReceivedPdu,
        _graph: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        _allocation: PeripheralConnectionRuntimeAllocation,
        _batch: LeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
        _scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        _phase: LegacyAdvertisingEventPhase,
        _rejected_packets: usize,
    },
}

#[cfg(target_arch = "riscv32")]
impl LegacyConnectableAdvertisingPostRunFailStop {
    pub(crate) const fn cause(&self) -> LegacyConnectableAdvertisingPostRunFailStopCause {
        match &self.ownership {
            LegacyConnectableAdvertisingPostRunFailStopOwnership::MemoryIdentity { .. } => {
                LegacyConnectableAdvertisingPostRunFailStopCause::MemoryIdentity
            }
            LegacyConnectableAdvertisingPostRunFailStopOwnership::ReceivePduUnavailable {
                blocked,
                ..
            } => LegacyConnectableAdvertisingPostRunFailStopCause::ReceivePduUnavailable {
                discarded: blocked.discarded_count(),
            },
            LegacyConnectableAdvertisingPostRunFailStopOwnership::ReceivePoolRejoin { .. } => {
                LegacyConnectableAdvertisingPostRunFailStopCause::ReceivePoolIdentity
            }
            LegacyConnectableAdvertisingPostRunFailStopOwnership::PacketAfterConnection {
                ..
            } => LegacyConnectableAdvertisingPostRunFailStopCause::PacketAfterConnection,
        }
    }
}

/// Finite diagnostic for a sealed post-RUN owner.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LegacyConnectableAdvertisingPostRunFailStopCause {
    MemoryIdentity,
    ReceivePduUnavailable { discarded: usize },
    ReceivePoolIdentity,
    PacketAfterConnection,
}

/// Unpublished event reduced back to ordinary CPU-owned allocations.
#[must_use = "both allocations must be restored to their originating runtimes"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct LegacyConnectableAdvertisingCancelled {
    definition: LegacyConnectableAdvertisingSetPrepared,
    configured: LegacyConnectableAdvertiserConfigured<'static>,
    graph: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
    allocation: PeripheralConnectionRuntimeAllocation,
}

/// Failed cancellation retaining every owner in a sealed fail-stop state.
#[must_use = "the identity disagreement retains all event allocations"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct LegacyConnectableAdvertisingCancellationInvariant {
    _definition: LegacyConnectableAdvertisingSetPrepared,
    _configured: LegacyConnectableAdvertiserConfigured<'static>,
    _graph: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
    _rejoin: PeripheralConnectionRuntimeGraphRejoinFailure,
}

/// A disabled portable advertiser could not rejoin its exact idle runtime.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the generation owner remains sealed in the failed restore"]
pub(crate) struct LegacyConnectableAdvertisingDisabledRestoreFailure {
    _configured: LegacyConnectableAdvertiserConfigured<'static>,
}

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) enum LegacyConnectableAdvertisingOwnershipInvariant {
    ReceivePoolRejoin {
        _definition: LegacyConnectableAdvertisingSetPrepared,
        _configured: LegacyConnectableAdvertiserConfigured<'static>,
        _graph: LegacyConnectableAdvertisingMemoryGraphCpuOwned,
        _rejoin: PeripheralConnectionRuntimeGraphRejoinFailure,
    },
    RuntimeRestore {
        _cancelled: LegacyConnectableAdvertisingCancelled,
    },
}

/// Why one response-capable event did not reach scheduler preparation.
#[must_use = "inspect the ordinary error or retain the fail-stop owner"]
#[cfg(any(target_arch = "riscv32", test))]
#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::large_enum_variant,
        reason = "the fail-stop variant retains every affine owner inline without allocation"
    )
)]
pub(crate) enum LegacyConnectableAdvertisingRuntimeBeginFailure {
    GenerationExhausted,
    #[cfg_attr(
        not(target_arch = "riscv32"),
        expect(
            dead_code,
            reason = "target boot and recurrence handlers recover this PDU-fit payload; validated host fixtures cannot exceed the packet extent"
        )
    )]
    PduFit {
        definition: LegacyConnectableAdvertisingSetPrepared,
        error: LegacyConnectableAdvertisingPduFitError,
    },
    AdvertisingEventActive {
        definition: LegacyConnectableAdvertisingSetPrepared,
    },
    PeripheralEventActive {
        definition: LegacyConnectableAdvertisingSetPrepared,
        error: PeripheralConnectionRuntimeBeginError,
    },
    MemoryPreparation {
        definition: LegacyConnectableAdvertisingSetPrepared,
        error: LegacyConnectableAdvertisingMemoryGraphPrepareError,
    },
    OwnershipInvariant {
        _invariant: LegacyConnectableAdvertisingOwnershipInvariant,
    },
}

#[cfg(test)]
mod tests;
