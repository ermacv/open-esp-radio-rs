//! Pre-publication ownership for one response-capable legacy advertisement.
//!
//! This boundary converts one typed HCI snapshot into portable Link Layer
//! `ADV_IND` and `SCAN_RSP` state, then atomically checks out the independent
//! response graph and the peripheral role's reusable RX pool. It performs only
//! controller-SRAM preparation. Scheduler admission, MMIO publication and any
//! claim of radio progress remain outside this module.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_hci::{
    LeLegacyAdvertisingAddress, LeLegacyAdvertisingRole,
    LeLegacyConnectableAdvertisingEnableRequest,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_bluetooth_ll::connectable_advertising::{
    LegacyConnectableAdvertisingEventComplete, LegacyConnectableAdvertisingEventInFlight,
    LegacyConnectableConnectionRequestAccepted, LegacyConnectableConnectionRequestAdmission,
};
use open_esp_radio_bluetooth_ll::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertising::{
        AdvertisingInterval, AdvertisingIntervalError, LegacyAdvertisingData,
        LegacyAdvertisingDataError, PrimaryAdvertisingChannel, PrimaryAdvertisingChannelMap,
        PrimaryAdvertisingChannelMapError,
    },
    advertising_lifecycle::LegacyAdvertisingEventIdentity,
    connectable_advertising::{
        LeChannelSelectionAlgorithmTwoSupport, LegacyConnectableAdvertisement,
        LegacyConnectableAdvertiserConfigured, LegacyConnectableAdvertiserStandby,
        LegacyConnectableAdvertisingSet, LegacyPreparedConnectableAdvertisingEvent,
        LegacyScanResponseData,
    },
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_bluetooth_ll::{
    advertising::AdvertisingDelay, connectable_advertising::LegacyConnectableAdvertisingEvent,
};
#[cfg(not(target_arch = "riscv32"))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BLUETOOTH_NON_SCANNING_RX_NODE_COUNT, BluetoothLeReceivedBatch, BluetoothLeReceivedPdu,
    BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved,
    BluetoothLegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared,
    BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
    BluetoothLegacyConnectableAdvertisingMemoryGraphRecycled,
    BluetoothLegacyConnectableAdvertisingMemoryGraphRunning,
    BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked,
    BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLegacyAdvertisingPrimaryChannel, BluetoothLegacyConnectableAdvIndPacketInput,
    BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure,
    BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
    BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared,
    BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
    BluetoothLegacyConnectableAdvertisingMemoryInput,
    BluetoothLegacyConnectableAdvertisingOwnAddress,
    BluetoothLegacyConnectableAdvertisingPduFitError,
    BluetoothLegacyConnectableScanResponsePacketInput,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerRandomAddress, BluetoothControllerSramAddress,
};

#[cfg(any(target_arch = "riscv32", test))]
use crate::BluetoothLegacyAdvertisingRecurringTimingObservation;
use crate::{
    BluetoothLegacyAdvertisingDefaultTxPowerDbm, BluetoothPeripheralConnectionRuntimeBeginError,
    BluetoothPeripheralConnectionRuntimeResources,
    peripheral_connection::{
        BluetoothPeripheralConnectionRuntimeAllocation,
        BluetoothPeripheralConnectionRuntimeGraphRejoinFailure,
        BluetoothPeripheralConnectionRuntimeGraphReserved,
    },
};
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    BluetoothLegacyAdvertisingEventPhase, BluetoothLegacyAdvertisingEventWindow,
    BluetoothLegacyAdvertisingTimingObservation, BluetoothSchedulerRawWindow,
    BluetoothSchedulerSoftwareConfig,
    peripheral_connection::{
        BluetoothPeripheralConnectionAcceptedRequest,
        BluetoothPeripheralConnectionAcceptedResetCancellationError,
    },
};

/// Why one typed HCI snapshot cannot become the restricted portable role.
///
/// Every case is defensive: the HCI layer already validates all but the S31
/// one-channel policy. Keeping the projection fallible prevents later HCI
/// expansion from silently widening the hardware contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothLegacyConnectableAdvertisingSetError {
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
pub(crate) struct BluetoothLegacyConnectableAdvertisingSetPrepared {
    set: LegacyConnectableAdvertisingSet<'static>,
    primary_channel: BluetoothLegacyAdvertisingPrimaryChannel,
    own_address: BluetoothLegacyConnectableAdvertisingOwnAddress,
}

impl BluetoothLegacyConnectableAdvertisingSetPrepared {
    /// Portable configuration retained independently of controller memory.
    pub(crate) const fn set(self) -> LegacyConnectableAdvertisingSet<'static> {
        self.set
    }

    /// Random-address publication intent, without exposing a register image.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn random_address(self) -> Option<BluetoothControllerRandomAddress> {
        match self.own_address {
            BluetoothLegacyConnectableAdvertisingOwnAddress::Public => None,
            BluetoothLegacyConnectableAdvertisingOwnAddress::Random(wire) => {
                Some(BluetoothControllerRandomAddress::from_hci_wire_bytes(wire))
            }
        }
    }
}

fn selected_primary_channel(
    channels: PrimaryAdvertisingChannelMap,
) -> Result<BluetoothLegacyAdvertisingPrimaryChannel, BluetoothLegacyConnectableAdvertisingSetError>
{
    let selected = channels.channel_count();
    if selected != 1 {
        return Err(
            BluetoothLegacyConnectableAdvertisingSetError::MultiplePrimaryChannels { selected },
        );
    }
    if channels.contains(PrimaryAdvertisingChannel::Channel37) {
        Ok(BluetoothLegacyAdvertisingPrimaryChannel::Channel37)
    } else if channels.contains(PrimaryAdvertisingChannel::Channel38) {
        Ok(BluetoothLegacyAdvertisingPrimaryChannel::Channel38)
    } else {
        Ok(BluetoothLegacyAdvertisingPrimaryChannel::Channel39)
    }
}

pub(crate) fn refine_portable_set(
    set: LegacyConnectableAdvertisingSet<'static>,
) -> Result<
    BluetoothLegacyConnectableAdvertisingSetPrepared,
    BluetoothLegacyConnectableAdvertisingSetError,
> {
    let primary_channel = selected_primary_channel(set.channels())?;
    let advertiser = set.advertisement().advertiser();
    let own_address = match advertiser.kind() {
        LeDeviceAddressKind::Public => BluetoothLegacyConnectableAdvertisingOwnAddress::Public,
        LeDeviceAddressKind::Random => {
            BluetoothLegacyConnectableAdvertisingOwnAddress::Random(advertiser.wire_bytes())
        }
    };
    Ok(BluetoothLegacyConnectableAdvertisingSetPrepared {
        set,
        primary_channel,
        own_address,
    })
}

/// Convert an accepted HCI `ADV_IND` snapshot into a self-contained LL set.
///
/// The S31 first slice accepts exactly one selected primary channel. This is
/// checked here, before either static runtime can be checked out.
pub(crate) fn prepare_legacy_connectable_advertising_set(
    request: LeLegacyConnectableAdvertisingEnableRequest,
) -> Result<
    BluetoothLegacyConnectableAdvertisingSetPrepared,
    BluetoothLegacyConnectableAdvertisingSetError,
> {
    let parameters = request.parameters();
    if parameters.role() != LeLegacyAdvertisingRole::Connectable {
        return Err(BluetoothLegacyConnectableAdvertisingSetError::Role);
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
            .map_err(BluetoothLegacyConnectableAdvertisingSetError::AdvertisingData)?,
        LeChannelSelectionAlgorithmTwoSupport::Unsupported,
    );
    let scan_response = LegacyScanResponseData::new_owned(request.scan_response_data().as_bytes())
        .map_err(BluetoothLegacyConnectableAdvertisingSetError::ScanResponseData)?;
    let selected = parameters.channels();
    let channels = PrimaryAdvertisingChannelMap::new(
        selected.channel_37(),
        selected.channel_38(),
        selected.channel_39(),
    )
    .map_err(BluetoothLegacyConnectableAdvertisingSetError::Channels)?;
    let interval =
        AdvertisingInterval::new(u32::from(parameters.interval().minimum_units_625_us()))
            .map_err(BluetoothLegacyConnectableAdvertisingSetError::Interval)?;
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
pub struct BluetoothLegacyConnectableAdvertisingRuntimeResources {
    default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    graph_identity: BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity,
    standby: Option<LegacyConnectableAdvertiserStandby>,
    idle: Option<BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned>,
}

impl BluetoothLegacyConnectableAdvertisingRuntimeResources {
    fn from_claimed_graph(
        graph: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
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
        storage: &'static mut BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
        default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<Self, BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure> {
        let graph = BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::pin_static(storage)?;
        Ok(Self::from_claimed_graph(graph, default_tx_power_dbm))
    }

    /// Bind one native model graph at a deterministic controller address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn claim_static_model(
        storage: &'static mut BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
        base: BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress,
        default_tx_power_dbm: BluetoothLegacyAdvertisingDefaultTxPowerDbm,
    ) -> Result<Self, BluetoothLegacyConnectableAdvertisingMemoryGraphBindFailure> {
        let graph = BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::pin_static_model(
            storage, base,
        )?;
        Ok(Self::from_claimed_graph(graph, default_tx_power_dbm))
    }

    /// Physical transmit-power request retained with the graph.
    pub const fn default_tx_power_dbm(&self) -> BluetoothLegacyAdvertisingDefaultTxPowerDbm {
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
    ) -> Result<(), BluetoothLegacyConnectableAdvertisingDisabledRestoreFailure> {
        if self.standby.is_some() || self.idle.is_none() {
            return Err(
                BluetoothLegacyConnectableAdvertisingDisabledRestoreFailure {
                    _configured: configured,
                },
            );
        }
        self.standby = Some(configured.into_standby());
        Ok(())
    }

    /// Atomically prepare one portable event and loan the peripheral RX pool.
    ///
    /// All ordinary errors restore both runtime slots before returning. An
    /// impossible identity disagreement is retained as an opaque fail-stop
    /// owner rather than fabricating either CPU owner.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure retains complete portable and affine ownership"
    )]
    pub(crate) fn begin_event(
        &mut self,
        definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        peripheral: &mut BluetoothPeripheralConnectionRuntimeResources,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingPrepared,
        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
    > {
        if self.standby.is_none() || self.idle.is_none() {
            return Err(
                BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
                    definition,
                },
            );
        }
        let Some(standby) = self.standby.take() else {
            return Err(
                BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
                    definition,
                },
            );
        };
        let portable = match standby.configure(definition.set).enable() {
            Ok(event) => event.prepare(),
            Err(failure) => {
                self.standby = Some(failure.into_configured().into_standby());
                return Err(
                    BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::GenerationExhausted,
                );
            }
        };
        self.begin_prepared_event(definition, portable, peripheral)
    }

    /// Rebuild a portable successor selected by `schedule_next` in the two
    /// exact static runtime slots.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn begin_scheduled_event(
        &mut self,
        definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        event: LegacyConnectableAdvertisingEvent<'static>,
        peripheral: &mut BluetoothPeripheralConnectionRuntimeResources,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingPrepared,
        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
    > {
        self.begin_prepared_event(definition, event.prepare(), peripheral)
    }

    fn begin_prepared_event(
        &mut self,
        definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
        peripheral: &mut BluetoothPeripheralConnectionRuntimeResources,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingPrepared,
        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
    > {
        let adv_ind = portable.adv_ind_pdu();
        let scan_response = portable.scan_response_pdu();
        let adv_ind = match BluetoothLegacyConnectableAdvIndPacketInput::try_from_encoded_extent(
            adv_ind.as_bytes(),
            adv_ind.payload_length(),
        ) {
            Ok(input) => input,
            Err(error) => {
                self.standby = Some(portable.disable().into_standby());
                return Err(
                    BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::PduFit {
                        definition,
                        error,
                    },
                );
            }
        };
        let scan_response =
            match BluetoothLegacyConnectableScanResponsePacketInput::try_from_encoded_extent(
                scan_response.as_bytes(),
                scan_response.payload_length(),
            ) {
                Ok(input) => input,
                Err(error) => {
                    self.standby = Some(portable.disable().into_standby());
                    return Err(
                        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::PduFit {
                            definition,
                            error,
                        },
                    );
                }
            };

        let Some(graph) = self.idle.take() else {
            self.standby = Some(portable.disable().into_standby());
            return Err(
                BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
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
                    BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::PeripheralEventActive {
                        definition,
                        error,
                    },
                );
            }
        };
        let (reserved, pool) = allocation.reserve_graph();
        let input = BluetoothLegacyConnectableAdvertisingMemoryInput::new(
            adv_ind,
            scan_response,
            definition.own_address,
            definition.primary_channel,
        );
        match graph.prepare_response_capable_event(input, pool, self.default_tx_power_dbm.dbm()) {
            Ok(memory) => Ok(BluetoothLegacyConnectableAdvertisingPrepared {
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
                            BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::OwnershipInvariant {
                                _invariant:
                                    BluetoothLegacyConnectableAdvertisingOwnershipInvariant::ReceivePoolRejoin {
                                        _definition: definition,
                                        _configured: configured,
                                        _graph: graph,
                                        _rejoin: rejoin,
                                    },
                            },
                        );
                    }
                };
                let cancelled = BluetoothLegacyConnectableAdvertisingCancelled {
                    definition,
                    configured,
                    graph,
                    allocation,
                };
                match self.restore_cancelled(cancelled, peripheral) {
                    Ok(definition) => Err(
                        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::MemoryPreparation {
                            definition,
                            error,
                        },
                    ),
                    Err(cancelled) => Err(
                        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::OwnershipInvariant {
                            _invariant:
                                BluetoothLegacyConnectableAdvertisingOwnershipInvariant::RuntimeRestore {
                                    _cancelled: cancelled,
                                },
                        },
                    ),
                }
            }
        }
    }

    /// Restore an unpublished cancellation only to both originating runtimes.
    #[expect(
        clippy::result_large_err,
        reason = "a rejected restore returns every exact affine owner"
    )]
    pub(crate) fn restore_cancelled(
        &mut self,
        cancelled: BluetoothLegacyConnectableAdvertisingCancelled,
        peripheral: &mut BluetoothPeripheralConnectionRuntimeResources,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingSetPrepared,
        BluetoothLegacyConnectableAdvertisingCancelled,
    > {
        if self.standby.is_some()
            || self.idle.is_some()
            || cancelled.graph.identity() != self.graph_identity
        {
            return Err(cancelled);
        }
        let BluetoothLegacyConnectableAdvertisingCancelled {
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
        Err(BluetoothLegacyConnectableAdvertisingCancelled {
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
    #[expect(
        clippy::result_large_err,
        reason = "a rejected no-connection restore returns every exact affine owner"
    )]
    pub(crate) fn restore_no_connection(
        &mut self,
        outcome: BluetoothLegacyConnectableAdvertisingNoConnection,
        peripheral: &mut BluetoothPeripheralConnectionRuntimeResources,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingNoConnectionRestored,
        BluetoothLegacyConnectableAdvertisingNoConnection,
    > {
        if self.standby.is_some()
            || self.idle.is_some()
            || outcome.graph.identity() != self.graph_identity
        {
            return Err(outcome);
        }
        let BluetoothLegacyConnectableAdvertisingNoConnection {
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
                return Ok(BluetoothLegacyConnectableAdvertisingNoConnectionRestored {
                    definition,
                    complete,
                    phase,
                    scheduler_status,
                    rejected_packets,
                });
            }
            Err(allocation) => allocation,
        };
        Err(BluetoothLegacyConnectableAdvertisingNoConnection {
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
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn restore_connection_accepted(
        &mut self,
        outcome: BluetoothLegacyConnectableAdvertisingConnectionAccepted,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingConnectionTransfer,
        BluetoothLegacyConnectableAdvertisingConnectionAccepted,
    > {
        if self.idle.is_some() || outcome.graph.identity() != self.graph_identity {
            return Err(outcome);
        }
        let BluetoothLegacyConnectableAdvertisingConnectionAccepted {
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
        Ok(BluetoothLegacyConnectableAdvertisingConnectionTransfer {
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
pub(crate) struct BluetoothLegacyConnectableAdvertisingPrepared {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
    memory: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepared,
    reserved: BluetoothPeripheralConnectionRuntimeGraphReserved,
}

impl BluetoothLegacyConnectableAdvertisingPrepared {
    #[cfg(test)]
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.portable.identity()
    }

    /// Form the complete response-capable scheduler window without changing SRAM.
    #[cfg(any(target_arch = "riscv32", test))]
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc timing failure retains every affine owner"
        )
    )]
    pub(crate) fn form_first_event_candidate(
        self,
        timing: BluetoothLegacyAdvertisingTimingObservation,
        config: BluetoothSchedulerSoftwareConfig,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingEventCandidate,
        BluetoothLegacyConnectableAdvertisingEventTimingFailure,
    > {
        let Some((scheduler_window, raw_window)) =
            timing.first_connectable_window(config, self.memory.post_anchor_duration())
        else {
            return Err(BluetoothLegacyConnectableAdvertisingEventTimingFailure { prepared: self });
        };
        Ok(BluetoothLegacyConnectableAdvertisingEventCandidate {
            prepared: self,
            scheduler_window,
            raw_window,
        })
    }

    /// Project a phase-locked successor selected by the portable Link Layer.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn form_recurring_event_candidate(
        self,
        timing: BluetoothLegacyAdvertisingRecurringTimingObservation,
        previous_phase: BluetoothLegacyAdvertisingEventPhase,
        start_offset_micros: u64,
        config: BluetoothSchedulerSoftwareConfig,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingEventCandidate,
        BluetoothLegacyConnectableAdvertisingEventTimingFailure,
    > {
        let Some((scheduler_window, raw_window)) = timing.recurring_connectable_window(
            previous_phase,
            start_offset_micros,
            config,
            self.memory.post_anchor_duration(),
        ) else {
            return Err(BluetoothLegacyConnectableAdvertisingEventTimingFailure { prepared: self });
        };
        Ok(BluetoothLegacyConnectableAdvertisingEventCandidate {
            prepared: self,
            scheduler_window,
            raw_window,
        })
    }

    /// Recover all ordinary CPU owners before any scheduler publication.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc invariant retains every affine owner fail-stop"
    )]
    pub(crate) fn cancel(
        self,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
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
            Ok(allocation) => Ok(BluetoothLegacyConnectableAdvertisingCancelled {
                definition,
                configured,
                graph,
                allocation,
            }),
            Err(rejoin) => Err(BluetoothLegacyConnectableAdvertisingCancellationInvariant {
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
pub(crate) struct BluetoothLegacyConnectableAdvertisingEventTimingFailure {
    prepared: BluetoothLegacyConnectableAdvertisingPrepared,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingEventTimingFailure {
    pub(crate) fn into_prepared(self) -> BluetoothLegacyConnectableAdvertisingPrepared {
        self.prepared
    }
}

/// Complete response graph with live timing but no common-timeline reservation.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the candidate must enter common scheduling, cancel, or remain retained"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingEventCandidate {
    prepared: BluetoothLegacyConnectableAdvertisingPrepared,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
    raw_window: BluetoothSchedulerRawWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingEventCandidate {
    pub(crate) const fn raw_window(&self) -> BluetoothSchedulerRawWindow {
        self.raw_window
    }

    #[cfg(test)]
    pub(crate) const fn phase(&self) -> BluetoothLegacyAdvertisingEventPhase {
        self.scheduler_window.phase()
    }

    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc failure retains the complete affine candidate"
        )
    )]
    pub(crate) fn prepare_resolved_event_image(
        self,
        resolved_window: BluetoothSchedulerRawWindow,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingEventImagePrepared,
        BluetoothLegacyConnectableAdvertisingEventImagePrepareFailure,
    > {
        let Self {
            prepared,
            scheduler_window,
            raw_window,
        } = self;
        let BluetoothLegacyConnectableAdvertisingPrepared {
            definition,
            portable,
            memory,
            reserved,
        } = prepared;
        match memory.prepare_event_fields(resolved_window.start(), resolved_window.end()) {
            Ok(memory) => Ok(BluetoothLegacyConnectableAdvertisingEventImagePrepared {
                definition,
                portable,
                memory,
                reserved,
                scheduler_window,
            }),
            Err(failure) => {
                let (memory, error) = failure.into_parts();
                Err(
                    BluetoothLegacyConnectableAdvertisingEventImagePrepareFailure {
                        candidate: Self {
                            prepared: BluetoothLegacyConnectableAdvertisingPrepared {
                                definition,
                                portable,
                                memory,
                                reserved,
                            },
                            scheduler_window,
                            raw_window,
                        },
                        error,
                    },
                )
            }
        }
    }

    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc cancellation invariant retains every affine owner"
        )
    )]
    pub(crate) fn cancel(
        self,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
    > {
        self.prepared.cancel()
    }
}

/// Event-field failure retaining the unchanged candidate and both memory owners.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged connectable advertising candidate remains recoverable"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingEventImagePrepareFailure {
    candidate: BluetoothLegacyConnectableAdvertisingEventCandidate,
    error: BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingEventImagePrepareFailure {
    pub(crate) const fn error(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError {
        self.error
    }

    pub(crate) fn into_candidate(self) -> BluetoothLegacyConnectableAdvertisingEventCandidate {
        self.candidate
    }
}

/// Complete event fields paired with every portable and affine owner.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the event image must remain paired with its scheduler reservation"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingEventImagePrepared {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
    memory: BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepared,
    reserved: BluetoothPeripheralConnectionRuntimeGraphReserved,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingEventImagePrepared {
    pub(crate) fn prepare_scheduler_bookkeeping(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerBookkeepingPrepared {
        BluetoothLegacyConnectableAdvertisingSchedulerBookkeepingPrepared {
            definition: self.definition,
            portable: self.portable,
            memory: self.memory.prepare_scheduler_bookkeeping(),
            reserved: self.reserved,
            scheduler_window: self.scheduler_window,
        }
    }

    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc cancellation invariant retains every affine owner"
        )
    )]
    pub(crate) fn cancel(
        self,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
    > {
        BluetoothLegacyConnectableAdvertisingPrepared {
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
pub(crate) struct BluetoothLegacyConnectableAdvertisingSchedulerBookkeepingPrepared {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
    memory: BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerBookkeepingPrepared,
    reserved: BluetoothPeripheralConnectionRuntimeGraphReserved,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingSchedulerBookkeepingPrepared {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) fn prepare_empty_list_link(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingEmptyListLinkPrepared {
        BluetoothLegacyConnectableAdvertisingEmptyListLinkPrepared {
            definition: self.definition,
            portable: self.portable,
            memory: self.memory.prepare_empty_list_link(),
            reserved: self.reserved,
            scheduler_window: self.scheduler_window,
        }
    }

    pub(crate) fn cancel(self) -> BluetoothLegacyConnectableAdvertisingEventImagePrepared {
        BluetoothLegacyConnectableAdvertisingEventImagePrepared {
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
pub(crate) struct BluetoothLegacyConnectableAdvertisingEmptyListLinkPrepared {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
    memory: BluetoothLegacyConnectableAdvertisingMemoryGraphEmptyListLinkPrepared,
    reserved: BluetoothPeripheralConnectionRuntimeGraphReserved,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingEmptyListLinkPrepared {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    /// Freeze the memory graph while retaining every non-memory owner separately.
    pub(crate) fn prepare_publication(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingPublicationPrepared {
        BluetoothLegacyConnectableAdvertisingPublicationPrepared {
            memory: self.memory.prepare_publication(),
            remainder: BluetoothLegacyConnectableAdvertisingPublicationRemainder {
                definition: self.definition,
                portable: self.portable,
                reserved: self.reserved,
                scheduler_window: self.scheduler_window,
            },
        }
    }

    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc cancellation invariant retains every affine owner"
        )
    )]
    pub(crate) fn cancel(
        self,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
    > {
        BluetoothLegacyConnectableAdvertisingEventImagePrepared {
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
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the publication owner must enter the atomic MMIO suffix or be cancelled"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingPublicationPrepared {
    memory: BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
    remainder: BluetoothLegacyConnectableAdvertisingPublicationRemainder,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingPublicationPrepared {
    pub(crate) const fn random_address(&self) -> Option<BluetoothControllerRandomAddress> {
        self.remainder.definition.random_address()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationPrepared,
        BluetoothLegacyConnectableAdvertisingPublicationRemainder,
    ) {
        (self.memory, self.remainder)
    }
}

/// Portable and connection-allocation owners retained across atomic publication.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the publication remainder must rejoin its exact memory graph"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingPublicationRemainder {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    portable: LegacyPreparedConnectableAdvertisingEvent<'static>,
    reserved: BluetoothPeripheralConnectionRuntimeGraphReserved,
    scheduler_window: BluetoothLegacyAdvertisingEventWindow,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingPublicationRemainder {
    /// Seal the portable event only after the exact memory graph has joined RUN.
    pub(crate) fn into_running(
        self,
        memory: BluetoothLegacyConnectableAdvertisingMemoryGraphRunning,
    ) -> BluetoothLegacyConnectableAdvertisingRunning {
        BluetoothLegacyConnectableAdvertisingRunning {
            definition: self.definition,
            portable: self.portable.into_submitted(),
            memory,
            reserved: self.reserved,
            phase: self.scheduler_window.phase(),
        }
    }
}

/// Role-owned state after the response graph and portable event both crossed RUN.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the running connectable event retains the graph, LL event, and peripheral reservation"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingRunning {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    portable: LegacyConnectableAdvertisingEventInFlight<'static>,
    memory: BluetoothLegacyConnectableAdvertisingMemoryGraphRunning,
    reserved: BluetoothPeripheralConnectionRuntimeGraphReserved,
    phase: BluetoothLegacyAdvertisingEventPhase,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingRunning {
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
    /// remainder through [`BluetoothLegacyConnectableAdvertisingPostRunRemainder::classify_recycled`].
    pub(crate) fn into_memory_completion(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphRunning,
        BluetoothLegacyConnectableAdvertisingPostRunRemainder,
    ) {
        let graph_identity = self.memory.identity();
        let receive_identity = self.memory.receive_identity();
        (
            self.memory,
            BluetoothLegacyConnectableAdvertisingPostRunRemainder {
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
        memory: BluetoothLegacyConnectableAdvertisingMemoryGraphRunning,
        remainder: BluetoothLegacyConnectableAdvertisingPostRunRemainder,
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
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the completed graph and role continuation must be recycled together"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingCompletionObserved {
    memory: BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved,
    remainder: BluetoothLegacyConnectableAdvertisingPostRunRemainder,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingCompletionObserved {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.memory.scheduler_item_address()
    }

    pub(crate) fn new(
        memory: BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved,
        remainder: BluetoothLegacyConnectableAdvertisingPostRunRemainder,
    ) -> Self {
        Self { memory, remainder }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingMemoryGraphCompletionObserved,
        BluetoothLegacyConnectableAdvertisingPostRunRemainder,
    ) {
        (self.memory, self.remainder)
    }
}

/// Portable and peripheral owners retained while common code reclaims memory.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "rejoin the exact reclaimed memory graph before classifying the role outcome"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingPostRunRemainder {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    portable: LegacyConnectableAdvertisingEventInFlight<'static>,
    reserved: BluetoothPeripheralConnectionRuntimeGraphReserved,
    phase: BluetoothLegacyAdvertisingEventPhase,
    graph_identity: BluetoothLegacyConnectableAdvertisingMemoryGraphIdentity,
    receive_identity:
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothNonScanningRxMemoryIdentity,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingPostRunRemainder {
    /// Classify the exact copied receive batch without interpreting scheduler status.
    ///
    /// Malformed, differently addressed and unsupported requests leave the
    /// portable event in flight and therefore become `NoConnection`. A valid
    /// final `CONNECT_IND` transfers the exact receive allocation and packet
    /// metadata into the peripheral role. Missing PDU bytes, a foreign memory
    /// owner, or a packet after an accepted connection remains sealed fail-stop.
    pub(crate) fn classify_recycled(
        self,
        recycled: BluetoothLegacyConnectableAdvertisingMemoryGraphRecycled,
    ) -> BluetoothLegacyConnectableAdvertisingPostRunOutcome {
        if recycled.identity() != self.graph_identity
            || recycled.receive_identity() != self.receive_identity
        {
            return BluetoothLegacyConnectableAdvertisingPostRunOutcome::FailStop(
                BluetoothLegacyConnectableAdvertisingPostRunFailStop {
                    ownership:
                        BluetoothLegacyConnectableAdvertisingPostRunFailStopOwnership::MemoryIdentity {
                            _remainder: self,
                            _recycled: recycled,
                        },
                },
            );
        }
        let dispatch = match recycled.prepare_rx_dispatch() {
            Ok(dispatch) => dispatch,
            Err(blocked) => {
                return BluetoothLegacyConnectableAdvertisingPostRunOutcome::FailStop(
                    BluetoothLegacyConnectableAdvertisingPostRunFailStop {
                        ownership:
                            BluetoothLegacyConnectableAdvertisingPostRunFailStopOwnership::ReceivePduUnavailable {
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
                return BluetoothLegacyConnectableAdvertisingPostRunOutcome::FailStop(
                    BluetoothLegacyConnectableAdvertisingPostRunFailStop {
                        ownership:
                            BluetoothLegacyConnectableAdvertisingPostRunFailStopOwnership::ReceivePoolRejoin {
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
            BluetoothLegacyConnectableAdvertisingPortableRxOutcome::NoConnection {
                complete,
                rejected_packets,
            } => BluetoothLegacyConnectableAdvertisingPostRunOutcome::NoConnection(
                BluetoothLegacyConnectableAdvertisingNoConnection {
                    definition: self.definition,
                    graph,
                    allocation,
                    complete,
                    phase: self.phase,
                    scheduler_status,
                    rejected_packets,
                },
            ),
            BluetoothLegacyConnectableAdvertisingPortableRxOutcome::ConnectionAccepted {
                accepted,
                packet,
                rejected_packets,
            } => {
                let (configured, identity, connection) = accepted.into_parts();
                BluetoothLegacyConnectableAdvertisingPostRunOutcome::ConnectionAccepted(
                    BluetoothLegacyConnectableAdvertisingConnectionAccepted {
                        graph,
                        configured,
                        identity,
                        peripheral: BluetoothPeripheralConnectionAcceptedRequest::new(
                            allocation, connection, packet,
                        ),
                        phase: self.phase,
                        scheduler_status,
                        rejected_packets,
                    },
                )
            }
            BluetoothLegacyConnectableAdvertisingPortableRxOutcome::PacketAfterConnection {
                accepted,
                packet,
                rejected_packets,
            } => BluetoothLegacyConnectableAdvertisingPostRunOutcome::FailStop(
                BluetoothLegacyConnectableAdvertisingPostRunFailStop {
                    ownership:
                        BluetoothLegacyConnectableAdvertisingPostRunFailStopOwnership::PacketAfterConnection {
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
enum BluetoothLegacyConnectableAdvertisingPortableRxOutcome<P> {
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
trait BluetoothLegacyConnectableAdvertisingReceivedPdu: Copy {
    fn pdu_bytes(&self) -> &[u8];
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingReceivedPdu for BluetoothLeReceivedPdu {
    fn pdu_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn classify_received_pdus<P: BluetoothLegacyConnectableAdvertisingReceivedPdu>(
    in_flight: LegacyConnectableAdvertisingEventInFlight<'static>,
    packets: [Option<P>; BLUETOOTH_NON_SCANNING_RX_NODE_COUNT],
    index: usize,
    rejected_packets: usize,
) -> BluetoothLegacyConnectableAdvertisingPortableRxOutcome<P> {
    let Some(packet) = packets.get(index).copied().flatten() else {
        return BluetoothLegacyConnectableAdvertisingPortableRxOutcome::NoConnection {
            complete: in_flight.complete_without_connection(),
            rejected_packets,
        };
    };
    match in_flight.admit_connection_request(packet.pdu_bytes()) {
        LegacyConnectableConnectionRequestAdmission::Accepted(accepted) => {
            if packets.get(index + 1).is_some_and(Option::is_some) {
                BluetoothLegacyConnectableAdvertisingPortableRxOutcome::PacketAfterConnection {
                    accepted,
                    packet,
                    rejected_packets,
                }
            } else {
                BluetoothLegacyConnectableAdvertisingPortableRxOutcome::ConnectionAccepted {
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
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "restore the reusable owners or retain the sealed fail-stop state"]
pub(crate) enum BluetoothLegacyConnectableAdvertisingPostRunOutcome {
    NoConnection(BluetoothLegacyConnectableAdvertisingNoConnection),
    ConnectionAccepted(BluetoothLegacyConnectableAdvertisingConnectionAccepted),
    FailStop(BluetoothLegacyConnectableAdvertisingPostRunFailStop),
}

/// Completed event which accepted no connection and owns both reusable graphs.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "restore both originating runtime slots before scheduling another event"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingNoConnection {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    graph: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
    allocation: BluetoothPeripheralConnectionRuntimeAllocation,
    complete: LegacyConnectableAdvertisingEventComplete<'static>,
    phase: BluetoothLegacyAdvertisingEventPhase,
    scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

/// Restored no-connection event ready for a later recurrence decision.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "retain the completed portable event or schedule its next occurrence"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingNoConnectionRestored {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    complete: LegacyConnectableAdvertisingEventComplete<'static>,
    phase: BluetoothLegacyAdvertisingEventPhase,
    scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingNoConnectionRestored {
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.complete.identity()
    }

    pub(crate) const fn definition(&self) -> BluetoothLegacyConnectableAdvertisingSetPrepared {
        self.definition
    }

    pub(crate) const fn phase(&self) -> BluetoothLegacyAdvertisingEventPhase {
        self.phase
    }

    pub(crate) const fn scheduler_status(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.scheduler_status
    }

    pub(crate) const fn rejected_packets(&self) -> usize {
        self.rejected_packets
    }

    /// Attach the caller's fresh advertising delay through the portable LL.
    pub(crate) fn schedule_next(
        self,
        delay: AdvertisingDelay,
    ) -> BluetoothLegacyConnectableAdvertisingNextEventScheduled {
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
                    BluetoothLegacyConnectableAdvertisingNextEventPortable::Event(
                        scheduled.into_event(),
                    ),
                    start_offset_micros,
                )
            }
            Err(exhausted) => (
                BluetoothLegacyConnectableAdvertisingNextEventPortable::SequenceExhausted(
                    exhausted.into_complete(),
                ),
                0,
            ),
        };
        BluetoothLegacyConnectableAdvertisingNextEventScheduled {
            definition,
            portable,
            start_offset_micros,
            previous_phase: phase,
            previous_scheduler_status: scheduler_status,
            rejected_packets,
        }
    }

    /// Stop at the already-restored CPU boundary without inventing a successor.
    pub(crate) fn prepare_recurrence_stop(
        self,
    ) -> (
        LegacyConnectableAdvertiserConfigured<'static>,
        BluetoothLegacyConnectableAdvertisingRecurrenceStopped,
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
        let stopped = BluetoothLegacyConnectableAdvertisingRecurrenceStopped::from_portable_set(
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
pub(crate) struct BluetoothLegacyConnectableAdvertisingNextEventScheduled {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    portable: BluetoothLegacyConnectableAdvertisingNextEventPortable,
    start_offset_micros: u64,
    previous_phase: BluetoothLegacyAdvertisingEventPhase,
    previous_scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) enum BluetoothLegacyConnectableAdvertisingNextEventPortable {
    Event(LegacyConnectableAdvertisingEvent<'static>),
    SequenceExhausted(LegacyConnectableAdvertisingEventComplete<'static>),
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingNextEventPortable {
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        match self {
            Self::Event(event) => event.identity(),
            Self::SequenceExhausted(complete) => complete.identity(),
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingNextEventScheduled {
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingSetPrepared,
        BluetoothLegacyConnectableAdvertisingNextEventPortable,
        u64,
        BluetoothLegacyAdvertisingEventPhase,
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
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
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "retain the stopped portable set and its completed-event diagnostics"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingRecurrenceStopped {
    portable_set: LegacyConnectableAdvertisingSet<'static>,
    identity: LegacyAdvertisingEventIdentity,
    previous_phase: BluetoothLegacyAdvertisingEventPhase,
    previous_scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingRecurrenceStopped {
    pub(crate) const fn from_restored_definition(
        definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        identity: LegacyAdvertisingEventIdentity,
        previous_phase: BluetoothLegacyAdvertisingEventPhase,
        previous_scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
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
        previous_phase: BluetoothLegacyAdvertisingEventPhase,
        previous_scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
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

    pub(crate) const fn previous_phase(&self) -> BluetoothLegacyAdvertisingEventPhase {
        self.previous_phase
    }

    pub(crate) const fn previous_scheduler_status(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.previous_scheduler_status
    }

    pub(crate) const fn rejected_packets(&self) -> usize {
        self.rejected_packets
    }
}

/// Accepted connection while the advertising graph is still checked out.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "restore the advertising graph and transfer the peripheral owner"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingConnectionAccepted {
    graph: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
    configured: LegacyConnectableAdvertiserConfigured<'static>,
    identity: LegacyAdvertisingEventIdentity,
    peripheral: BluetoothPeripheralConnectionAcceptedRequest,
    phase: BluetoothLegacyAdvertisingEventPhase,
    scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

/// Peripheral handoff after the reusable advertising graph was restored.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "normalize the accepted packet and prepare the first peripheral event"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingConnectionTransfer {
    advertising_set: LegacyConnectableAdvertisingSet<'static>,
    identity: LegacyAdvertisingEventIdentity,
    peripheral: BluetoothPeripheralConnectionAcceptedRequest,
    phase: BluetoothLegacyAdvertisingEventPhase,
    scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingConnectionTransfer {
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.identity
    }

    pub(crate) const fn peripheral(&self) -> &BluetoothPeripheralConnectionAcceptedRequest {
        &self.peripheral
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingSet<'static>,
        LegacyAdvertisingEventIdentity,
        BluetoothPeripheralConnectionAcceptedRequest,
        BluetoothLegacyAdvertisingEventPhase,
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
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
        runtime: &mut BluetoothPeripheralConnectionRuntimeResources,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingPeripheralResetEvidence,
        BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailure,
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
            Ok(cancelled) => Ok(
                BluetoothLegacyConnectableAdvertisingPeripheralResetEvidence {
                    advertising_set,
                    identity,
                    accepted_packet: cancelled.into_packet(),
                    phase,
                    scheduler_status,
                    rejected_packets,
                },
            ),
            Err(failure) => {
                let cause = failure.error();
                Err(
                    BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailure {
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
pub(crate) struct BluetoothLegacyConnectableAdvertisingPeripheralResetEvidence {
    advertising_set: LegacyConnectableAdvertisingSet<'static>,
    identity: LegacyAdvertisingEventIdentity,
    accepted_packet: BluetoothLeReceivedPdu,
    phase: BluetoothLegacyAdvertisingEventPhase,
    scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingPeripheralResetEvidence {
    pub(crate) const fn identity(&self) -> LegacyAdvertisingEventIdentity {
        self.identity
    }

    pub(crate) const fn advertising_set(&self) -> LegacyConnectableAdvertisingSet<'static> {
        self.advertising_set
    }

    pub(crate) const fn accepted_packet(&self) -> &BluetoothLeReceivedPdu {
        &self.accepted_packet
    }

    pub(crate) const fn phase(&self) -> BluetoothLegacyAdvertisingEventPhase {
        self.phase
    }

    pub(crate) const fn scheduler_status(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.scheduler_status
    }

    pub(crate) const fn rejected_packets(&self) -> usize {
        self.rejected_packets
    }
}

/// Rejected accepted-connection Reset cancellation with every owner retained.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "retry only through the originating runtime or retain the sealed transfer"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailure {
    cause: BluetoothPeripheralConnectionAcceptedResetCancellationError,
    _transfer: BluetoothLegacyConnectableAdvertisingConnectionTransfer,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailure {
    pub(crate) const fn cause(
        &self,
    ) -> BluetoothPeripheralConnectionAcceptedResetCancellationError {
        self.cause
    }

    #[cfg(test)]
    pub(crate) fn into_transfer(self) -> BluetoothLegacyConnectableAdvertisingConnectionTransfer {
        self._transfer
    }
}

/// Sealed post-RUN ownership when a safe role outcome cannot be proven.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the indeterminate hardware outcome retains every affine owner"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingPostRunFailStop {
    ownership: BluetoothLegacyConnectableAdvertisingPostRunFailStopOwnership,
}

#[cfg(any(target_arch = "riscv32", test))]
enum BluetoothLegacyConnectableAdvertisingPostRunFailStopOwnership {
    MemoryIdentity {
        _remainder: BluetoothLegacyConnectableAdvertisingPostRunRemainder,
        _recycled: BluetoothLegacyConnectableAdvertisingMemoryGraphRecycled,
    },
    ReceivePduUnavailable {
        _remainder: BluetoothLegacyConnectableAdvertisingPostRunRemainder,
        blocked: BluetoothLegacyConnectableAdvertisingMemoryGraphRxDispatchBlocked,
    },
    ReceivePoolRejoin {
        _definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        _portable: LegacyConnectableAdvertisingEventInFlight<'static>,
        _graph: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        _batch: BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
        _scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        _phase: BluetoothLegacyAdvertisingEventPhase,
        _rejoin: BluetoothPeripheralConnectionRuntimeGraphRejoinFailure,
    },
    PacketAfterConnection {
        _definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        _accepted: LegacyConnectableConnectionRequestAccepted<'static>,
        _accepted_packet: BluetoothLeReceivedPdu,
        _graph: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        _allocation: BluetoothPeripheralConnectionRuntimeAllocation,
        _batch: BluetoothLeReceivedBatch<BLUETOOTH_NON_SCANNING_RX_NODE_COUNT>,
        _scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        _phase: BluetoothLegacyAdvertisingEventPhase,
        _rejected_packets: usize,
    },
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyConnectableAdvertisingPostRunFailStop {
    pub(crate) const fn cause(&self) -> BluetoothLegacyConnectableAdvertisingPostRunFailStopCause {
        match &self.ownership {
            BluetoothLegacyConnectableAdvertisingPostRunFailStopOwnership::MemoryIdentity {
                ..
            } => BluetoothLegacyConnectableAdvertisingPostRunFailStopCause::MemoryIdentity,
            BluetoothLegacyConnectableAdvertisingPostRunFailStopOwnership::ReceivePduUnavailable {
                blocked,
                ..
            } => BluetoothLegacyConnectableAdvertisingPostRunFailStopCause::ReceivePduUnavailable {
                discarded: blocked.discarded_count(),
            },
            BluetoothLegacyConnectableAdvertisingPostRunFailStopOwnership::ReceivePoolRejoin {
                ..
            } => BluetoothLegacyConnectableAdvertisingPostRunFailStopCause::ReceivePoolIdentity,
            BluetoothLegacyConnectableAdvertisingPostRunFailStopOwnership::PacketAfterConnection {
                ..
            } => BluetoothLegacyConnectableAdvertisingPostRunFailStopCause::PacketAfterConnection,
        }
    }
}

/// Finite diagnostic for a sealed post-RUN owner.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothLegacyConnectableAdvertisingPostRunFailStopCause {
    MemoryIdentity,
    ReceivePduUnavailable { discarded: usize },
    ReceivePoolIdentity,
    PacketAfterConnection,
}

/// Unpublished event reduced back to ordinary CPU-owned allocations.
#[must_use = "both allocations must be restored to their originating runtimes"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingCancelled {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    configured: LegacyConnectableAdvertiserConfigured<'static>,
    graph: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
    allocation: BluetoothPeripheralConnectionRuntimeAllocation,
}

/// Failed cancellation retaining every owner in a sealed fail-stop state.
#[must_use = "the identity disagreement retains all event allocations"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingCancellationInvariant {
    _definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    _configured: LegacyConnectableAdvertiserConfigured<'static>,
    _graph: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
    _rejoin: BluetoothPeripheralConnectionRuntimeGraphRejoinFailure,
}

/// A disabled portable advertiser could not rejoin its exact idle runtime.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the generation owner remains sealed in the failed restore"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingDisabledRestoreFailure {
    _configured: LegacyConnectableAdvertiserConfigured<'static>,
}

pub(crate) enum BluetoothLegacyConnectableAdvertisingOwnershipInvariant {
    ReceivePoolRejoin {
        _definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        _configured: LegacyConnectableAdvertiserConfigured<'static>,
        _graph: BluetoothLegacyConnectableAdvertisingMemoryGraphCpuOwned,
        _rejoin: BluetoothPeripheralConnectionRuntimeGraphRejoinFailure,
    },
    RuntimeRestore {
        _cancelled: BluetoothLegacyConnectableAdvertisingCancelled,
    },
}

/// Why one response-capable event did not reach scheduler preparation.
#[must_use = "inspect the ordinary error or retain the fail-stop owner"]
pub(crate) enum BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure {
    GenerationExhausted,
    PduFit {
        definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        error: BluetoothLegacyConnectableAdvertisingPduFitError,
    },
    AdvertisingEventActive {
        definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    },
    PeripheralEventActive {
        definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        error: BluetoothPeripheralConnectionRuntimeBeginError,
    },
    MemoryPreparation {
        definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        error: BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
    },
    OwnershipInvariant {
        _invariant: BluetoothLegacyConnectableAdvertisingOwnershipInvariant,
    },
}

#[cfg(test)]
mod tests {
    use open_esp_radio_bluetooth_ll::{
        LeDeviceAddress, LeDeviceAddressKind,
        advertising::{
            AdvertisingDelay, AdvertisingInterval, LegacyAdvertisingData,
            PrimaryAdvertisingChannelMap,
        },
        connectable_advertising::{
            LeChannelSelectionAlgorithmTwoSupport, LegacyConnectableAdvertisement,
            LegacyConnectableAdvertiserStandby, LegacyConnectableAdvertisingEventInFlight,
            LegacyConnectableAdvertisingSet, LegacyScanResponseData,
        },
        connection::{
            LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES,
            LeLegacyConnectionRequest, LePeripheralConnection,
        },
    };
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothLeReceivedPdu, BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress,
        BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
        BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        BluetoothNonScanningRxMemoryModelAddress, BluetoothNonScanningRxMemoryStorage,
        BluetoothPeripheralConnectionDefaultTxPowerDbm,
        BluetoothPeripheralConnectionMemoryGraphModelAddress,
        BluetoothPeripheralConnectionMemoryGraphStorage,
    };
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::{
        BluetoothLegacyConnectableAdvertisingPortableRxOutcome,
        BluetoothLegacyConnectableAdvertisingReceivedPdu,
        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
        BluetoothLegacyConnectableAdvertisingRuntimeResources,
        BluetoothLegacyConnectableAdvertisingSetError,
        BluetoothPeripheralConnectionAcceptedRequest,
        BluetoothPeripheralConnectionAcceptedResetCancellationError, classify_received_pdus,
        refine_portable_set,
    };
    use crate::{
        BluetoothLegacyAdvertisingDefaultTxPowerDbm, BluetoothPeripheralConnectionRuntimeConfig,
        BluetoothPeripheralConnectionRuntimeResources, BluetoothSchedulerSoftwareConfig,
        controller_time::{BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample},
    };

    fn definition(
        channels: PrimaryAdvertisingChannelMap,
    ) -> Result<
        super::BluetoothLegacyConnectableAdvertisingSetPrepared,
        BluetoothLegacyConnectableAdvertisingSetError,
    > {
        let advertiser =
            LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 6], LeDeviceAddressKind::Public);
        refine_portable_set(LegacyConnectableAdvertisingSet::new(
            LegacyConnectableAdvertisement::new(
                advertiser,
                LegacyAdvertisingData::new_owned(&[2, 1, 6]).unwrap(),
                LeChannelSelectionAlgorithmTwoSupport::Unsupported,
            ),
            LegacyScanResponseData::new_owned(&[3, 3, 0xaa, 0xfe]).unwrap(),
            channels,
            AdvertisingInterval::new(32).unwrap(),
        ))
    }

    #[derive(Clone, Copy)]
    struct TestReceivedPdu<'a>(&'a [u8]);

    impl BluetoothLegacyConnectableAdvertisingReceivedPdu for TestReceivedPdu<'_> {
        fn pdu_bytes(&self) -> &[u8] {
            self.0
        }
    }

    fn connection_request(advertiser: [u8; 6]) -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
        let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
        pdu[0] = 0b0101 | (1 << 6);
        pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
        pdu[2..8].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
        pdu[8..14].copy_from_slice(&advertiser);
        pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
        pdu[21] = 2;
        pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
        pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
        pdu[26..28].copy_from_slice(&0u16.to_le_bytes());
        pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
        pdu[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
        pdu[35] = 5 | (4 << 5);
        pdu
    }

    fn portable_in_flight(
        definition: super::BluetoothLegacyConnectableAdvertisingSetPrepared,
    ) -> LegacyConnectableAdvertisingEventInFlight<'static> {
        LegacyConnectableAdvertiserStandby::new()
            .configure(definition.set())
            .enable()
            .unwrap_or_else(|_| panic!("the fresh validation generation must be available"))
            .prepare()
            .into_submitted()
    }

    fn connectable_runtime(base: u32) -> BluetoothLegacyConnectableAdvertisingRuntimeResources {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::new(),
        ));
        BluetoothLegacyConnectableAdvertisingRuntimeResources::claim_static_model(
            storage,
            BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress::new(base).unwrap(),
            BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(4),
        )
        .unwrap()
    }

    fn peripheral_runtime(base: u32) -> BluetoothPeripheralConnectionRuntimeResources {
        let graph = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPeripheralConnectionMemoryGraphStorage::new(),
        ));
        let receive = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothNonScanningRxMemoryStorage::new(),
        ));
        BluetoothPeripheralConnectionRuntimeResources::claim_static_model(
            graph,
            BluetoothPeripheralConnectionMemoryGraphModelAddress::new(base).unwrap(),
            receive,
            BluetoothNonScanningRxMemoryModelAddress::new(base + 0x1000).unwrap(),
            BluetoothPeripheralConnectionRuntimeConfig::new(
                BluetoothPeripheralConnectionDefaultTxPowerDbm::new(4),
            ),
        )
        .unwrap()
    }

    fn sample_phase() -> crate::BluetoothLegacyAdvertisingEventPhase {
        let mut connectable = connectable_runtime(0x2f03_0000);
        let mut peripheral = peripheral_runtime(0x2f03_3000);
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
        let prepared = connectable
            .begin_event(definition, &mut peripheral)
            .unwrap_or_else(|_| panic!("the isolated validation resources must prepare"));
        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            scale,
        );
        let candidate = prepared
            .form_first_event_candidate(
                crate::BluetoothLegacyAdvertisingTimingObservation {
                    current: crate::BluetoothSchedulerInstant::from_image(30_000),
                    radio_ready: crate::BluetoothSchedulerInstant::from_image(32_000),
                    epoch,
                },
                BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            )
            .unwrap_or_else(|_| panic!("the isolated timing window must project"));
        let phase = candidate.scheduler_window.phase();
        let cancelled = candidate
            .cancel()
            .unwrap_or_else(|_| panic!("the exact validation receive pool must rejoin"));
        let _definition = connectable
            .restore_cancelled(cancelled, &mut peripheral)
            .unwrap_or_else(|_| panic!("the validation resources must restore"));
        phase
    }

    fn accepted_transfer(
        peripheral: &mut BluetoothPeripheralConnectionRuntimeResources,
        phase: crate::BluetoothLegacyAdvertisingEventPhase,
        captured_time: u32,
    ) -> super::BluetoothLegacyConnectableAdvertisingConnectionTransfer {
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
        let request_bytes =
            connection_request(definition.set().advertisement().advertiser().wire_bytes());
        let request = LeLegacyConnectionRequest::decode(&request_bytes)
            .unwrap_or_else(|_| panic!("the validation CONNECT_IND must decode"));
        let packet =
            BluetoothLeReceivedPdu::from_parts_for_validation(&request_bytes, -37, captured_time)
                .unwrap_or_else(|| panic!("the validation packet must be complete"));
        let allocation = peripheral
            .begin_event()
            .unwrap_or_else(|_| panic!("the validation peripheral allocation must be idle"));
        let in_flight = portable_in_flight(definition);
        let identity = in_flight.identity();
        let _configured = in_flight.complete_without_connection().disable();
        super::BluetoothLegacyConnectableAdvertisingConnectionTransfer {
            advertising_set: definition.set(),
            identity,
            peripheral: BluetoothPeripheralConnectionAcceptedRequest::new(
                allocation,
                LePeripheralConnection::from_request(request),
                packet,
            ),
            phase,
            scheduler_status:
                BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
            rejected_packets: 2,
        }
    }

    #[test]
    fn empty_dispatch_completes_without_using_scheduler_status_as_an_outcome() {
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
        let in_flight = portable_in_flight(definition);

        let outcome = classify_received_pdus::<TestReceivedPdu<'_>>(in_flight, [None, None], 0, 0);
        let BluetoothLegacyConnectableAdvertisingPortableRxOutcome::NoConnection {
            complete,
            rejected_packets,
        } = outcome
        else {
            panic!("an empty copied batch cannot accept a connection")
        };
        assert_eq!(rejected_packets, 0);
        assert_eq!(complete.disable().set(), definition.set());
    }

    #[test]
    fn rejected_packet_then_addressed_connect_ind_transfers_the_connection() {
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(false, true, false).unwrap()).unwrap();
        let advertiser = definition.set().advertisement().advertiser().wire_bytes();
        let request = connection_request(advertiser);
        let unrelated_pdu = [0x03, 0];
        let in_flight = portable_in_flight(definition);

        let outcome = classify_received_pdus(
            in_flight,
            [
                Some(TestReceivedPdu(&unrelated_pdu)),
                Some(TestReceivedPdu(&request)),
            ],
            0,
            0,
        );
        let BluetoothLegacyConnectableAdvertisingPortableRxOutcome::ConnectionAccepted {
            accepted,
            packet,
            rejected_packets,
        } = outcome
        else {
            panic!("the final addressed CONNECT_IND must be admitted")
        };
        assert_eq!(rejected_packets, 1);
        assert_eq!(packet.pdu_bytes(), request.as_slice());
        assert_eq!(accepted.request().advertiser().wire_bytes(), advertiser);
        let (configured, _identity, connection) = accepted.into_parts();
        assert_eq!(configured.set(), definition.set());
        assert_eq!(connection.event_counter(), 0);
    }

    #[test]
    fn packet_after_accepted_connect_ind_is_not_silently_discarded() {
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(false, false, true).unwrap()).unwrap();
        let request =
            connection_request(definition.set().advertisement().advertiser().wire_bytes());
        let later_pdu = [0x03, 0];
        let in_flight = portable_in_flight(definition);

        let outcome = classify_received_pdus(
            in_flight,
            [
                Some(TestReceivedPdu(&request)),
                Some(TestReceivedPdu(&later_pdu)),
            ],
            0,
            0,
        );
        assert!(matches!(
            outcome,
            BluetoothLegacyConnectableAdvertisingPortableRxOutcome::PacketAfterConnection {
                rejected_packets: 0,
                ..
            }
        ));
    }

    #[test]
    fn multiple_channels_fail_before_any_runtime_checkout() {
        let mut connectable = connectable_runtime(0x2f00_1000);
        let peripheral = peripheral_runtime(0x2f00_3000);

        let error = definition(PrimaryAdvertisingChannelMap::all()).unwrap_err();

        assert_eq!(
            error,
            BluetoothLegacyConnectableAdvertisingSetError::MultiplePrimaryChannels { selected: 3 }
        );
        assert!(connectable.event_is_idle());
        assert!(peripheral.allocation_is_idle());
        assert_eq!(connectable.default_tx_power_dbm().dbm(), 4);
        // Keep the mutable binding honest for the following tests' API shape.
        let _ = &mut connectable;
    }

    #[test]
    fn preparation_and_cancel_restore_both_originating_runtimes() {
        let mut connectable = connectable_runtime(0x2f00_6000);
        let mut peripheral = peripheral_runtime(0x2f00_8000);
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
        let prepared = match connectable.begin_event(definition, &mut peripheral) {
            Ok(prepared) => prepared,
            Err(_) => panic!("the disjoint idle allocations must prepare"),
        };
        assert!(!connectable.event_is_idle());
        assert!(!peripheral.allocation_is_idle());
        let cancelled = match prepared.cancel() {
            Ok(cancelled) => cancelled,
            Err(_) => panic!("an internally paired pool must cancel losslessly"),
        };
        let restored = connectable
            .restore_cancelled(cancelled, &mut peripheral)
            .unwrap_or_else(|_| panic!("both exact runtime slots must accept their owners"));
        assert_eq!(restored, definition);
        assert!(connectable.event_is_idle());
        assert!(peripheral.allocation_is_idle());

        let prepared = match connectable.begin_event(restored, &mut peripheral) {
            Ok(prepared) => prepared,
            Err(_) => panic!("cancel must restore a graph ready for another event"),
        };
        let cancelled = match prepared.cancel() {
            Ok(cancelled) => cancelled,
            Err(_) => panic!("the repeated event keeps the exact pool"),
        };
        let _restored = connectable
            .restore_cancelled(cancelled, &mut peripheral)
            .unwrap_or_else(|_| panic!("the repeated cancellation restores both runtimes"));
    }

    #[test]
    fn no_connection_restore_returns_both_runtime_slots_atomically() {
        let mut connectable = connectable_runtime(0x2f00_6100);
        let mut peripheral = peripheral_runtime(0x2f00_8100);
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
        let prepared = connectable
            .begin_event(definition, &mut peripheral)
            .unwrap_or_else(|_| panic!("the disjoint idle owners must prepare"));
        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            scale,
        );
        let candidate = prepared
            .form_first_event_candidate(
                crate::BluetoothLegacyAdvertisingTimingObservation {
                    current: crate::BluetoothSchedulerInstant::from_image(10_000),
                    radio_ready: crate::BluetoothSchedulerInstant::from_image(12_000),
                    epoch,
                },
                BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            )
            .unwrap_or_else(|_| panic!("the first event timing must project"));
        let phase = candidate.scheduler_window.phase();
        let cancelled = candidate
            .cancel()
            .unwrap_or_else(|_| panic!("the exact receive pool must rejoin"));
        let super::BluetoothLegacyConnectableAdvertisingCancelled {
            definition,
            configured,
            graph,
            allocation,
        } = cancelled;
        let complete = configured
            .enable()
            .unwrap_or_else(|_| panic!("the retained validation generation must advance"))
            .prepare()
            .into_submitted()
            .complete_without_connection();
        let outcome = super::BluetoothLegacyConnectableAdvertisingNoConnection {
            definition,
            graph,
            allocation,
            complete,
            phase,
            scheduler_status: open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero,
            rejected_packets: 1,
        };

        let restored = connectable
            .restore_no_connection(outcome, &mut peripheral)
            .unwrap_or_else(|_| panic!("both exact runtime slots must accept their owners"));
        assert!(connectable.event_is_idle());
        assert!(peripheral.allocation_is_idle());
        assert_eq!(restored.definition(), definition);
        assert_eq!(restored.phase(), phase);
        assert_eq!(restored.rejected_packets(), 1);
        assert_eq!(
            restored.scheduler_status(),
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero
        );
        let completed_identity = restored.identity();
        let delay = AdvertisingDelay::from_micros(7_500)
            .unwrap_or_else(|_| panic!("the portable maximum delay contains this value"));
        let scheduled = restored.schedule_next(delay);
        let (
            scheduled_definition,
            portable,
            start_offset_micros,
            scheduled_from_phase,
            previous_status,
            previous_rejected_packets,
        ) = scheduled.into_parts();
        assert_eq!(scheduled_definition, definition);
        let successor_identity = portable.identity();
        assert_eq!(
            successor_identity.generation(),
            completed_identity.generation()
        );
        assert_eq!(
            successor_identity.event().get(),
            completed_identity.event().get() + 1
        );
        let configured = match portable {
            super::BluetoothLegacyConnectableAdvertisingNextEventPortable::Event(event) => {
                let configured = event.disable();
                assert_eq!(configured.set(), definition.set());
                configured
            }
            super::BluetoothLegacyConnectableAdvertisingNextEventPortable::SequenceExhausted(_) => {
                panic!("the first validation recurrence cannot exhaust event identity")
            }
        };
        connectable
            .restore_disabled_advertiser(configured)
            .unwrap_or_else(|_| panic!("the stopped advertiser must rejoin its idle runtime"));
        assert_eq!(
            start_offset_micros,
            definition.set().interval().as_micros() + u64::from(delay.as_micros())
        );
        assert_eq!(scheduled_from_phase, phase);
        assert_eq!(
            previous_status,
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero
        );
        assert_eq!(previous_rejected_packets, 1);

        let next_enable = connectable
            .begin_event(definition, &mut peripheral)
            .unwrap_or_else(|_| panic!("disable must restore the next Enable generation"));
        assert_ne!(
            next_enable.identity().generation(),
            completed_identity.generation()
        );
        assert_eq!(next_enable.identity().event().get(), 0);
    }

    #[test]
    fn foreign_no_connection_restore_preserves_the_exact_role_owner() {
        let mut origin = connectable_runtime(0x2f02_0000);
        let mut foreign = connectable_runtime(0x2f02_2000);
        let mut peripheral = peripheral_runtime(0x2f02_4000);
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();
        let prepared = origin
            .begin_event(definition, &mut peripheral)
            .unwrap_or_else(|_| panic!("the disjoint idle owners must prepare"));
        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            scale,
        );
        let candidate = prepared
            .form_first_event_candidate(
                crate::BluetoothLegacyAdvertisingTimingObservation {
                    current: crate::BluetoothSchedulerInstant::from_image(20_000),
                    radio_ready: crate::BluetoothSchedulerInstant::from_image(22_000),
                    epoch,
                },
                BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            )
            .unwrap_or_else(|_| panic!("the bounded response window must project"));
        let phase = candidate.scheduler_window.phase();
        let cancelled = candidate
            .cancel()
            .unwrap_or_else(|_| panic!("the exact receive pool must rejoin"));
        let super::BluetoothLegacyConnectableAdvertisingCancelled {
            definition,
            configured,
            graph,
            allocation,
        } = cancelled;
        let complete = configured
            .enable()
            .unwrap_or_else(|_| panic!("the retained validation generation must advance"))
            .prepare()
            .into_submitted()
            .complete_without_connection();
        let outcome = super::BluetoothLegacyConnectableAdvertisingNoConnection {
            definition,
            graph,
            allocation,
            complete,
            phase,
            scheduler_status: open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::Zero,
            rejected_packets: 0,
        };

        let outcome = match foreign.restore_no_connection(outcome, &mut peripheral) {
            Ok(_) => panic!("a foreign runtime cannot consume the exact completed graph"),
            Err(outcome) => outcome,
        };
        assert!(foreign.event_is_idle());
        assert!(!origin.event_is_idle());
        assert!(!peripheral.allocation_is_idle());

        let restored = origin
            .restore_no_connection(outcome, &mut peripheral)
            .unwrap_or_else(|_| panic!("the origin must recover the unchanged owner"));
        assert!(origin.event_is_idle());
        assert!(peripheral.allocation_is_idle());
        assert_eq!(restored.phase(), phase);
        assert_eq!(restored.rejected_packets(), 0);
    }

    #[test]
    fn busy_peripheral_rolls_back_the_connectable_graph() {
        let mut connectable = connectable_runtime(0x2f00_b000);
        let mut peripheral = peripheral_runtime(0x2f00_d000);
        let held = peripheral.begin_event().unwrap();
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(false, true, false).unwrap()).unwrap();

        let failure = match connectable.begin_event(definition, &mut peripheral) {
            Ok(_) => panic!("the checked-out connection allocation must reject advertising"),
            Err(failure) => failure,
        };
        assert!(matches!(
            failure,
            BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::PeripheralEventActive { .. }
        ));
        assert!(connectable.event_is_idle());
        assert!(!peripheral.allocation_is_idle());

        peripheral
            .restore_idle(held)
            .unwrap_or_else(|_| panic!("the retained connection allocation remains exact"));
        assert!(peripheral.allocation_is_idle());
    }

    #[test]
    fn memory_preparation_failure_restores_both_runtime_slots() {
        let mut connectable = connectable_runtime(0x2f00_f000);
        let mut peripheral = peripheral_runtime(0x2f00_e000);
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap()).unwrap();

        let failure = match connectable.begin_event(definition, &mut peripheral) {
            Ok(_) => panic!("overlapping role allocations must fail before publication"),
            Err(failure) => failure,
        };
        assert!(matches!(
            failure,
            BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::MemoryPreparation {
                error: open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError::ReceivePoolOverlapsGraph,
                ..
            }
        ));
        assert!(connectable.event_is_idle());
        assert!(peripheral.allocation_is_idle());
    }

    #[test]
    fn active_connectable_event_does_not_touch_an_unrelated_peripheral_runtime() {
        let mut connectable = connectable_runtime(0x2f01_0000);
        let mut first_peripheral = peripheral_runtime(0x2f01_2000);
        let mut second_peripheral = peripheral_runtime(0x2f01_5000);
        let definition =
            definition(PrimaryAdvertisingChannelMap::new(false, false, true).unwrap()).unwrap();
        let prepared = match connectable.begin_event(definition, &mut first_peripheral) {
            Ok(prepared) => prepared,
            Err(_) => panic!("the first response event must prepare"),
        };

        let failure = match connectable.begin_event(definition, &mut second_peripheral) {
            Ok(_) => panic!("one response graph cannot start a second event"),
            Err(failure) => failure,
        };
        assert!(matches!(
            failure,
            BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive { .. }
        ));
        assert!(second_peripheral.allocation_is_idle());

        let cancelled = match prepared.cancel() {
            Ok(cancelled) => cancelled,
            Err(_) => panic!("the first event must retain its exact pool"),
        };
        let _restored = connectable
            .restore_cancelled(cancelled, &mut first_peripheral)
            .unwrap_or_else(|_| panic!("the first event restores its exact runtimes"));
        assert!(connectable.event_is_idle());
        assert!(first_peripheral.allocation_is_idle());
    }

    #[test]
    fn explicit_reset_restores_the_exact_accepted_connection_allocation_losslessly() {
        let phase = sample_phase();
        let mut origin = peripheral_runtime(0x2f04_0000);
        let mut foreign = peripheral_runtime(0x2f04_3000);
        let mut busy = peripheral_runtime(0x2f04_6000);
        let foreign_held = foreign
            .begin_event()
            .unwrap_or_else(|_| panic!("the foreign validation allocation must begin idle"));
        let transfer = accepted_transfer(&mut origin, phase, 41_234);
        assert!(!origin.allocation_is_idle());

        let failure = match transfer.cancel_peripheral_for_reset(&mut busy) {
            Ok(_) => panic!("an occupied runtime must not retire the accepted connection"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.cause(),
            BluetoothPeripheralConnectionAcceptedResetCancellationError::RuntimeBusy
        );
        assert!(busy.allocation_is_idle());

        let failure = match failure
            .into_transfer()
            .cancel_peripheral_for_reset(&mut foreign)
        {
            Ok(_) => panic!("a foreign runtime must not retire the accepted connection"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.cause(),
            BluetoothPeripheralConnectionAcceptedResetCancellationError::GraphIdentityMismatch
        );
        assert!(!origin.allocation_is_idle());
        assert!(!foreign.allocation_is_idle());

        let evidence = failure
            .into_transfer()
            .cancel_peripheral_for_reset(&mut origin)
            .unwrap_or_else(|_| panic!("the originating runtime must accept its exact owner"));
        assert!(origin.allocation_is_idle());
        assert_eq!(
            evidence.advertising_set().advertisement().advertiser(),
            definition(PrimaryAdvertisingChannelMap::new(true, false, false).unwrap())
                .unwrap()
                .set()
                .advertisement()
                .advertiser()
        );
        assert_eq!(
            evidence.accepted_packet().as_bytes(),
            connection_request(
                evidence
                    .advertising_set()
                    .advertisement()
                    .advertiser()
                    .wire_bytes()
            )
            .as_slice()
        );
        assert_eq!(evidence.phase(), phase);
        assert_eq!(
            evidence.scheduler_status(),
            BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus::NonZero
        );
        assert_eq!(evidence.rejected_packets(), 2);

        foreign
            .restore_idle(foreign_held)
            .unwrap_or_else(|_| panic!("the foreign allocation was not changed by rejection"));
        assert!(foreign.allocation_is_idle());

        let reusable = origin
            .begin_event()
            .unwrap_or_else(|_| panic!("Reset cancellation must make the allocation reusable"));
        origin
            .restore_idle(reusable)
            .unwrap_or_else(|_| panic!("the restored allocation must retain its identity"));
    }
}
