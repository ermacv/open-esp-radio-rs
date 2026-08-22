//! SoftAP protocol role borrowed by the common same-channel WDEV owner.

use super::*;
use crate::connected_rx_protocol::Esp32s31StagedRxFrame;
use crate::wdev::paired::{
    WdevPairRole, WdevPairedNetworkTxService, WdevPairedPhysicalTx, WdevPairedPhysicalTxError,
    WdevPairedRoleOwner, WdevPairedRoleTransitionError, WdevPairedStopProgress,
};

type Esp32s31StaApNetworkTxBacking<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

/// AP RX failure preserving protocol versus final network publication origin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApAccessPointRxError {
    Control(Esp32s31AccessPointControlError),
    Network(FrameLengthError),
}

/// Queue-independent AP protocol state and its role-local environment.
///
/// Physical RX/DMA, interrupt and final network endpoints remain outside this
/// value. TX/control state will be added to this same role owner rather than
/// manufacturing a second AP protocol object for the paired runtime.
pub struct Esp32s31StaApAccessPointRole<Processor, NetworkTx, Security, SharedRx> {
    protocol: Processor,
    network_tx: NetworkTx,
    security_material: Security,
    publish_shared_rx: SharedRx,
    network_backpressure_since_micros: Option<u64>,
    #[cfg(feature = "rx-delivery-observation")]
    delivery_observer: Option<&'static dyn RxNetworkDeliveryObserver>,
}

pub struct Esp32s31StaApAccessPointTxActive<Processor, Aggregate> {
    processor: Processor,
    aggregate: Aggregate,
}

pub struct Esp32s31StaApAccessPointTxParked<Processor, Aggregate> {
    processor: Processor,
    aggregate: Aggregate,
}

impl<Processor, Aggregate> Esp32s31StaApAccessPointTxActive<Processor, Aggregate> {
    pub const fn processor(&self) -> &Processor {
        &self.processor
    }

    pub fn processor_mut(&mut self) -> &mut Processor {
        &mut self.processor
    }

    pub const fn aggregate(&self) -> &Aggregate {
        &self.aggregate
    }

    pub fn aggregate_mut(&mut self) -> &mut Aggregate {
        &mut self.aggregate
    }

    pub fn into_parts(self) -> (Processor, Aggregate) {
        (self.processor, self.aggregate)
    }
}

impl<Processor, Aggregate> Esp32s31StaApAccessPointTxParked<Processor, Aggregate> {
    pub const fn processor(&self) -> &Processor {
        &self.processor
    }

    pub fn processor_mut(&mut self) -> &mut Processor {
        &mut self.processor
    }

    pub fn into_parts(self) -> (Processor, Aggregate) {
        (self.processor, self.aggregate)
    }
}

/// Failure to establish the paired AP ownership boundary. Both original
/// owners are returned; the caller never receives a half-parked role.
pub struct Esp32s31StaApAccessPointParkError<Role, Aggregate> {
    pub reason: Esp32s31StaApAccessPointParkFailure,
    pub role: Role,
    pub aggregate: Aggregate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApAccessPointParkFailure {
    Busy,
    Physical(WdevPairedPhysicalTxError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApAccessPointTxOwnershipError {
    AlreadyActive,
    AlreadyParked,
    Busy,
    Physical(WdevPairedPhysicalTxError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApAccessPointTxError {
    Operation(Esp32s31AccessPointWdevError),
    Ownership(Esp32s31StaApAccessPointTxOwnershipError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApAccessPointPairedRxError {
    Role(Esp32s31StaApAccessPointRxError),
    Ownership(Esp32s31StaApAccessPointTxOwnershipError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApAccessPointPairedControlError {
    Role(Esp32s31AccessPointControlError),
    Ownership(Esp32s31StaApAccessPointTxOwnershipError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaApAccessPointFinishReason {
    Activation(Esp32s31StaApAccessPointTxOwnershipError),
    AggregateBusy,
    ProtocolBusy,
}

/// Complete AP teardown result before the station reclaims physical TX.
pub struct Esp32s31StaApAccessPointFinished<Stopped, NetworkTx, Security, SharedRx, PhysicalTx> {
    pub stopped: Stopped,
    pub network_tx: NetworkTx,
    pub security_material: Security,
    pub publish_shared_rx: SharedRx,
    pub physical_tx: PhysicalTx,
}

/// Exact paired AP frontier retained when shutdown cannot cross an idle edge.
pub struct Esp32s31StaApAccessPointFinishFailure<Role, PhysicalTx> {
    pub reason: Esp32s31StaApAccessPointFinishReason,
    pub role: Role,
    pub physical_tx: PhysicalTx,
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
/// Return an initialized AP role to the already-existing paired physical
/// owner. `physical` must record a prior `Second` lend used to construct
/// `role` and `aggregate`; this function cannot manufacture another owner.
pub fn park_sta_ap_access_point_role<
    'storage,
    'beacon,
    'slot,
    'ampdu,
    P,
    E,
    T,
    NetworkTx,
    Security,
    SharedRx,
    B,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>(
    role: Esp32s31StaApAccessPointRole<
        Esp32s31AccessPointProtocolProcessor<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        NetworkTx,
        Security,
        SharedRx,
    >,
    aggregate: Esp32s31AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
    physical: &mut WdevPairedPhysicalTx<
        WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
        crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
    >,
) -> Result<
    Esp32s31StaApAccessPointRole<
        WdevPairedRoleOwner<
            Esp32s31StaApAccessPointTxActive<
                Esp32s31AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                Esp32s31AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
            >,
            Esp32s31StaApAccessPointTxParked<
                Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::Esp32s31AccessPointAmpduParked,
            >,
        >,
        NetworkTx,
        Security,
        SharedRx,
    >,
    Esp32s31StaApAccessPointParkError<
        Esp32s31StaApAccessPointRole<
            Esp32s31AccessPointProtocolProcessor<
                'storage,
                'beacon,
                'slot,
                P,
                E,
                T,
                DMA_BUFFER_SIZE,
                TX_BUFFER_SIZE,
            >,
            NetworkTx,
            Security,
            SharedRx,
        >,
        Esp32s31AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
    >,
>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    B: StableDmaBacking + 'ampdu,
{
    let Esp32s31StaApAccessPointRole {
        protocol,
        network_tx,
        security_material,
        publish_shared_rx,
        network_backpressure_since_micros,
        #[cfg(feature = "rx-delivery-observation")]
        delivery_observer,
    } = role;
    let (ordinary, protocol) = match protocol.try_park() {
        Ok(parts) => parts,
        Err(protocol) => {
            return Err(Esp32s31StaApAccessPointParkError {
                reason: Esp32s31StaApAccessPointParkFailure::Busy,
                role: Esp32s31StaApAccessPointRole {
                    protocol,
                    network_tx,
                    security_material,
                    publish_shared_rx,
                    network_backpressure_since_micros,
                    #[cfg(feature = "rx-delivery-observation")]
                    delivery_observer,
                },
                aggregate,
            });
        }
    };
    let (aggregate_resources, aggregate) = match aggregate.try_park() {
        Ok(parts) => parts,
        Err(aggregate) => {
            return Err(Esp32s31StaApAccessPointParkError {
                reason: Esp32s31StaApAccessPointParkFailure::Busy,
                role: Esp32s31StaApAccessPointRole {
                    protocol: Esp32s31AccessPointProtocolProcessor::resume(ordinary, protocol),
                    network_tx,
                    security_material,
                    publish_shared_rx,
                    network_backpressure_since_micros,
                    #[cfg(feature = "rx-delivery-observation")]
                    delivery_observer,
                },
                aggregate,
            });
        }
    };
    if let Err((error, ordinary, aggregate_resources)) =
        physical.restore(WdevPairRole::Second, ordinary, aggregate_resources)
    {
        return Err(Esp32s31StaApAccessPointParkError {
            reason: Esp32s31StaApAccessPointParkFailure::Physical(error),
            role: Esp32s31StaApAccessPointRole {
                protocol: Esp32s31AccessPointProtocolProcessor::resume(ordinary, protocol),
                network_tx,
                security_material,
                publish_shared_rx,
                network_backpressure_since_micros,
                #[cfg(feature = "rx-delivery-observation")]
                delivery_observer,
            },
            aggregate: Esp32s31AccessPointAmpdu::resume(aggregate_resources, aggregate),
        });
    }
    Ok(Esp32s31StaApAccessPointRole {
        protocol: WdevPairedRoleOwner::parked(Esp32s31StaApAccessPointTxParked {
            processor: protocol,
            aggregate,
        }),
        network_tx,
        security_material,
        publish_shared_rx,
        network_backpressure_since_micros,
        #[cfg(feature = "rx-delivery-observation")]
        delivery_observer,
    })
}

/// Stop and detach a quiescent paired AP role, returning the exact physical
/// TX pair to the shared owner.
///
/// The paired WDEV must first drive `service_stop` to `Stopped`.  This
/// transaction then verifies that both protocol and aggregate owners are
/// idle, stops the AP engine, and restores ordinary/A-MPDU resources without
/// constructing replacements.
#[allow(clippy::result_large_err, clippy::type_complexity)]
pub fn finish_sta_ap_access_point_role<
    'storage,
    'beacon,
    'slot,
    'ampdu,
    P,
    E,
    T,
    NetworkTx,
    Security,
    SharedRx,
    B,
    H,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>(
    mut role: Esp32s31StaApAccessPointRole<
        WdevPairedRoleOwner<
            Esp32s31StaApAccessPointTxActive<
                Esp32s31AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                Esp32s31AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
            >,
            Esp32s31StaApAccessPointTxParked<
                Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::Esp32s31AccessPointAmpduParked,
            >,
        >,
        NetworkTx,
        Security,
        SharedRx,
    >,
    mut physical_tx: WdevPairedPhysicalTx<
        WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
        crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
    >,
    hardware: &mut H,
) -> Result<
    Esp32s31StaApAccessPointFinished<
        Esp32s31AccessPointProtocolFinished<'storage, 'beacon, DMA_BUFFER_SIZE>,
        NetworkTx,
        Security,
        SharedRx,
        WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
        >,
    >,
    Esp32s31StaApAccessPointFinishFailure<
        Esp32s31StaApAccessPointRole<
            WdevPairedRoleOwner<
                Esp32s31StaApAccessPointTxActive<
                    Esp32s31AccessPointProtocolProcessor<
                        'storage,
                        'beacon,
                        'slot,
                        P,
                        E,
                        T,
                        DMA_BUFFER_SIZE,
                        TX_BUFFER_SIZE,
                    >,
                    Esp32s31AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
                >,
                Esp32s31StaApAccessPointTxParked<
                    Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                    super::ampdu::Esp32s31AccessPointAmpduParked,
                >,
            >,
            NetworkTx,
            Security,
            SharedRx,
        >,
        WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
        >,
    >,
>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    B: StableDmaBacking + 'ampdu,
    H: Esp32s31ApRuntimeHardware,
{
    if let Err(reason) = role.activate_tx(&mut physical_tx) {
        return Err(Esp32s31StaApAccessPointFinishFailure {
            reason: Esp32s31StaApAccessPointFinishReason::Activation(reason),
            role,
            physical_tx,
        });
    }
    let Esp32s31StaApAccessPointRole {
        protocol,
        network_tx,
        security_material,
        publish_shared_rx,
        network_backpressure_since_micros,
        #[cfg(feature = "rx-delivery-observation")]
        delivery_observer,
    } = role;
    let active = match protocol.try_into_active() {
        Ok(active) => active,
        Err(protocol) => {
            return Err(Esp32s31StaApAccessPointFinishFailure {
                reason: Esp32s31StaApAccessPointFinishReason::Activation(
                    Esp32s31StaApAccessPointTxOwnershipError::AlreadyParked,
                ),
                role: Esp32s31StaApAccessPointRole {
                    protocol,
                    network_tx,
                    security_material,
                    publish_shared_rx,
                    network_backpressure_since_micros,
                    #[cfg(feature = "rx-delivery-observation")]
                    delivery_observer,
                },
                physical_tx,
            });
        }
    };
    let (processor, aggregate) = active.into_parts();
    let (aggregate_resources, aggregate_state) = match aggregate.try_park() {
        Ok(parts) => parts,
        Err(aggregate) => {
            return Err(Esp32s31StaApAccessPointFinishFailure {
                reason: Esp32s31StaApAccessPointFinishReason::AggregateBusy,
                role: Esp32s31StaApAccessPointRole {
                    protocol: WdevPairedRoleOwner::from_active(Esp32s31StaApAccessPointTxActive {
                        processor,
                        aggregate,
                    }),
                    network_tx,
                    security_material,
                    publish_shared_rx,
                    network_backpressure_since_micros,
                    #[cfg(feature = "rx-delivery-observation")]
                    delivery_observer,
                },
                physical_tx,
            });
        }
    };
    let stopped = match processor.try_finish_paired(hardware) {
        Ok(stopped) => stopped,
        Err(processor) => {
            return Err(Esp32s31StaApAccessPointFinishFailure {
                reason: Esp32s31StaApAccessPointFinishReason::ProtocolBusy,
                role: Esp32s31StaApAccessPointRole {
                    protocol: WdevPairedRoleOwner::from_active(Esp32s31StaApAccessPointTxActive {
                        processor,
                        aggregate: Esp32s31AccessPointAmpdu::resume(
                            aggregate_resources,
                            aggregate_state,
                        ),
                    }),
                    network_tx,
                    security_material,
                    publish_shared_rx,
                    network_backpressure_since_micros,
                    #[cfg(feature = "rx-delivery-observation")]
                    delivery_observer,
                },
                physical_tx,
            });
        }
    };
    let (ordinary, stopped) = stopped.into_parts();
    physical_tx
        .restore(WdevPairRole::Second, ordinary, aggregate_resources)
        .unwrap_or_else(|_| unreachable!("paired AP activation records the second role"));

    Ok(Esp32s31StaApAccessPointFinished {
        stopped,
        network_tx,
        security_material,
        publish_shared_rx,
        physical_tx,
    })
}

impl<
    'storage,
    'beacon,
    'slot,
    'ampdu,
    P,
    E,
    T,
    NetworkTx,
    Security,
    SharedRx,
    B,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>
    Esp32s31StaApAccessPointRole<
        WdevPairedRoleOwner<
            Esp32s31StaApAccessPointTxActive<
                Esp32s31AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                Esp32s31AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
            >,
            Esp32s31StaApAccessPointTxParked<
                Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::Esp32s31AccessPointAmpduParked,
            >,
        >,
        NetworkTx,
        Security,
        SharedRx,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    B: StableDmaBacking + 'ampdu,
{
    pub fn activate_tx(
        &mut self,
        physical: &mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
        >,
    ) -> Result<(), Esp32s31StaApAccessPointTxOwnershipError> {
        if !self.protocol.is_parked() {
            return Err(Esp32s31StaApAccessPointTxOwnershipError::AlreadyActive);
        }
        let (ordinary, aggregate) = physical
            .try_lend(WdevPairRole::Second)
            .map_err(Esp32s31StaApAccessPointTxOwnershipError::Physical)?;
        self.protocol
            .try_activate(|parked| {
                let (processor, aggregate_state) = parked.into_parts();
                Ok::<_, (core::convert::Infallible, _)>(Esp32s31StaApAccessPointTxActive {
                    processor: Esp32s31AccessPointProtocolProcessor::resume(ordinary, processor),
                    aggregate: Esp32s31AccessPointAmpdu::resume(aggregate, aggregate_state),
                })
            })
            .map_err(|error| match error {
                WdevPairedRoleTransitionError::AlreadyActive => {
                    Esp32s31StaApAccessPointTxOwnershipError::AlreadyActive
                }
                WdevPairedRoleTransitionError::AlreadyParked => {
                    Esp32s31StaApAccessPointTxOwnershipError::AlreadyParked
                }
                WdevPairedRoleTransitionError::Conversion(never) => match never {},
            })
    }

    pub fn park_tx(
        &mut self,
        physical: &mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
        >,
    ) -> Result<(), Esp32s31StaApAccessPointTxOwnershipError> {
        if self.protocol.is_parked() {
            return Err(Esp32s31StaApAccessPointTxOwnershipError::AlreadyParked);
        }
        self.protocol
            .try_park(|active| {
                let (processor, aggregate) = active.into_parts();
                let (ordinary, processor_state) = match processor.try_park() {
                    Ok(parts) => parts,
                    Err(processor) => {
                        return Err((
                            Esp32s31StaApAccessPointTxOwnershipError::Busy,
                            Esp32s31StaApAccessPointTxActive {
                                processor,
                                aggregate,
                            },
                        ));
                    }
                };
                let (aggregate_resources, aggregate_state) = match aggregate.try_park() {
                    Ok(parts) => parts,
                    Err(aggregate) => {
                        return Err((
                            Esp32s31StaApAccessPointTxOwnershipError::Busy,
                            Esp32s31StaApAccessPointTxActive {
                                processor: Esp32s31AccessPointProtocolProcessor::resume(
                                    ordinary,
                                    processor_state,
                                ),
                                aggregate,
                            },
                        ));
                    }
                };
                match physical.restore(WdevPairRole::Second, ordinary, aggregate_resources) {
                    Ok(()) => Ok(Esp32s31StaApAccessPointTxParked {
                        processor: processor_state,
                        aggregate: aggregate_state,
                    }),
                    Err((error, ordinary, aggregate_resources)) => Err((
                        Esp32s31StaApAccessPointTxOwnershipError::Physical(error),
                        Esp32s31StaApAccessPointTxActive {
                            processor: Esp32s31AccessPointProtocolProcessor::resume(
                                ordinary,
                                processor_state,
                            ),
                            aggregate: Esp32s31AccessPointAmpdu::resume(
                                aggregate_resources,
                                aggregate_state,
                            ),
                        },
                    )),
                }
            })
            .map_err(|error| match error {
                WdevPairedRoleTransitionError::AlreadyActive => {
                    Esp32s31StaApAccessPointTxOwnershipError::AlreadyActive
                }
                WdevPairedRoleTransitionError::AlreadyParked => {
                    Esp32s31StaApAccessPointTxOwnershipError::AlreadyParked
                }
                WdevPairedRoleTransitionError::Conversion(error) => error,
            })
    }
}

impl<
    'resources,
    'storage,
    'beacon,
    'slot,
    'ampdu,
    M,
    H,
    P,
    E,
    T,
    Security,
    SharedRx,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>
    WdevPairedNetworkTxService<
        'resources,
        M,
        H,
        WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<
                'ampdu,
                Esp32s31StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
    >
    for Esp32s31StaApAccessPointRole<
        WdevPairedRoleOwner<
            Esp32s31StaApAccessPointTxActive<
                Esp32s31AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                Esp32s31AccessPointAmpdu<
                    'ampdu,
                    PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
                    AMPDU_SLOTS,
                    AMPDU_BUFFER_SIZE,
                >,
            >,
            Esp32s31StaApAccessPointTxParked<
                Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::Esp32s31AccessPointAmpduParked,
            >,
        >,
        network_tx::Esp32s31AccessPointNetworkTx<
            'ampdu,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        >,
        Security,
        SharedRx,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: TxHardware
        + Esp32s31ApRuntimeHardware
        + RxBlockAckHardware
        + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    'resources: 'ampdu,
{
    type Error = Esp32s31StaApAccessPointTxError;

    fn last_started_frame_count(&self) -> usize {
        self.network_tx.last_started_frame_count()
    }

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<
                'ampdu,
                Esp32s31StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            self.activate_tx(physical)
                .map_err(Esp32s31StaApAccessPointTxError::Ownership)?;
            let active = self.protocol.active_mut().expect("activated above");
            let progress = self
                .network_tx
                .start(
                    &mut active.aggregate,
                    &mut active.processor,
                    hardware,
                    frame,
                    network,
                )
                .await
                .map_err(Esp32s31StaApAccessPointTxError::Operation)?;
            if progress == WifiTxProgress::Complete && !self.network_tx.has_prepared() {
                self.park_tx(physical)
                    .map_err(Esp32s31StaApAccessPointTxError::Ownership)?;
            }
            Ok(progress)
        }
    }

    fn wait_deadline<'a>(
        &'a mut self,
        _physical: &'a mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<
                'ampdu,
                Esp32s31StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    ) -> impl Future<Output = ()> + 'a {
        async move {
            let active = self
                .protocol
                .active_mut()
                .expect("paired scheduler retains the active AP role until TX terminal");
            self.network_tx.wait_deadline(&mut active.processor).await;
        }
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<
                'ampdu,
                Esp32s31StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let active =
                self.protocol
                    .active_mut()
                    .ok_or(Esp32s31StaApAccessPointTxError::Ownership(
                        Esp32s31StaApAccessPointTxOwnershipError::AlreadyParked,
                    ))?;
            let progress = self
                .network_tx
                .service(&mut active.aggregate, &mut active.processor, hardware, wake)
                .await
                .map_err(Esp32s31StaApAccessPointTxError::Operation)?;
            if progress == WifiTxProgress::Complete && !self.network_tx.has_prepared() {
                self.park_tx(physical)
                    .map_err(Esp32s31StaApAccessPointTxError::Ownership)?;
            }
            Ok(progress)
        }
    }

    fn has_prepared(&self) -> bool {
        self.network_tx.has_prepared()
    }

    fn preferred_batch_size(&self) -> usize {
        self.protocol.active().map_or(1, |active| {
            if active.processor.has_operational_tx_block_ack() {
                AMPDU_SLOTS
            } else {
                1
            }
        })
    }

    fn prepared_frame_count(&self) -> usize {
        self.network_tx.prepared_frame_count()
    }

    fn start_prepared<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<
                'ampdu,
                Esp32s31StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let active =
                self.protocol
                    .active_mut()
                    .ok_or(Esp32s31StaApAccessPointTxError::Ownership(
                        Esp32s31StaApAccessPointTxOwnershipError::AlreadyParked,
                    ))?;
            let progress = self
                .network_tx
                .start_prepared(
                    &mut active.aggregate,
                    &mut active.processor,
                    hardware,
                    network,
                )
                .map_err(Esp32s31StaApAccessPointTxError::Operation)?;
            if progress == WifiTxProgress::Complete && !self.network_tx.has_prepared() {
                self.park_tx(physical)
                    .map_err(Esp32s31StaApAccessPointTxError::Ownership)?;
            }
            Ok(progress)
        }
    }

    fn cancel_prepared(
        &mut self,
        physical: &mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<
                'ampdu,
                Esp32s31StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    ) -> Result<(), Self::Error> {
        let active =
            self.protocol
                .active_mut()
                .ok_or(Esp32s31StaApAccessPointTxError::Ownership(
                    Esp32s31StaApAccessPointTxOwnershipError::AlreadyParked,
                ))?;
        self.network_tx
            .cancel_prepared(&mut active.aggregate)
            .map_err(Esp32s31StaApAccessPointTxError::Operation)?;
        self.park_tx(physical)
            .map_err(Esp32s31StaApAccessPointTxError::Ownership)
    }

    fn can_prepare(
        &self,
        _physical: &WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<
                'ampdu,
                Esp32s31StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    ) -> bool {
        self.protocol
            .active()
            .is_some_and(|active| self.network_tx.can_prepare(&active.aggregate))
    }

    fn prepare<'a>(
        &'a mut self,
        _physical: &'a mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<
                'ampdu,
                Esp32s31StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
    {
        async move {
            let active =
                self.protocol
                    .active_mut()
                    .ok_or(Esp32s31StaApAccessPointTxError::Ownership(
                        Esp32s31StaApAccessPointTxOwnershipError::AlreadyParked,
                    ))?;
            self.network_tx
                .prepare(&mut active.aggregate, &mut active.processor, frame, network)
                .map_err(Esp32s31StaApAccessPointTxError::Operation)
        }
    }
}

impl<
    'pool,
    'storage,
    'beacon,
    'slot,
    'ampdu,
    H,
    P,
    E,
    T,
    NetworkTx,
    Security,
    SharedRx,
    B,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>
    crate::sta_ap::Esp32s31StaApAccessPointRxRole<
        'pool,
        H,
        WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
        >,
        STAGE_CAPACITY,
        STAGE_SLOTS,
    >
    for Esp32s31StaApAccessPointRole<
        WdevPairedRoleOwner<
            Esp32s31StaApAccessPointTxActive<
                Esp32s31AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                Esp32s31AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
            >,
            Esp32s31StaApAccessPointTxParked<
                Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::Esp32s31AccessPointAmpduParked,
            >,
        >,
        NetworkTx,
        Security,
        SharedRx,
    >
where
    H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    B: StableDmaBacking + 'ampdu,
    Security: FnMut() -> ([u8; 32], u64),
    SharedRx: FnMut(u8),
{
    type Error = Esp32s31StaApAccessPointPairedRxError;

    fn publish_pending_rx(
        &mut self,
        _physical_tx: &mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
        >,
        network: &mut dyn WdevNetworkRx,
    ) -> Result<WdevRxProgress, Self::Error> {
        loop {
            let record = if let Some(active) = self.protocol.active() {
                active.processor.rx_batch_record()
            } else {
                self.protocol
                    .parked_state()
                    .expect("paired AP role is active or parked")
                    .processor
                    .rx_batch_record()
            }
            .map_err(|error| {
                Esp32s31StaApAccessPointPairedRxError::Role(
                    Esp32s31StaApAccessPointRxError::Control(error),
                )
            })?;
            let Some(record) = record else {
                break;
            };
            let frame = record.frame;
            let next_offset = record.next_offset;
            #[cfg(not(feature = "rx-delivery-observation"))]
            let result = network.try_send_parts(frame);
            #[cfg(feature = "rx-delivery-observation")]
            let result = {
                let delivery = RxNetworkDeliveryEvent { frame, raw: None };
                let observer = self.delivery_observer;
                let mut before_publish = || {
                    if let Some(observer) = observer {
                        observer.admitted(delivery);
                    }
                };
                network.try_send_parts_observed(frame, &mut before_publish)
            };

            match result {
                Ok(()) => {
                    let protocol = ethernet_parts_protocol(frame);
                    let report = if let Some(active) = self.protocol.active_mut() {
                        active.processor.commit_rx_batch_record(next_offset);
                        &mut active.processor.report
                    } else {
                        let parked = self
                            .protocol
                            .parked_state_mut()
                            .expect("paired AP role is active or parked");
                        parked.processor.commit_rx_batch_record(next_offset);
                        &mut parked.processor.report
                    };
                    report.ethernet_frames_staged = report.ethernet_frames_staged.saturating_add(1);
                    match protocol {
                        Some(EthernetProtocol::ArpRequest) => {
                            report.ethernet_arp_requests_staged =
                                report.ethernet_arp_requests_staged.saturating_add(1);
                        }
                        Some(EthernetProtocol::Ipv4Tcp) => {
                            report.ethernet_tcp_frames_staged =
                                report.ethernet_tcp_frames_staged.saturating_add(1);
                        }
                        _ => {}
                    }
                }
                Err(RxEnqueueError::QueueFull) => {
                    self.network_backpressure_since_micros
                        .get_or_insert_with(|| Instant::now().as_micros());
                    return Ok(WdevRxProgress::NetworkBackpressured);
                }
                Err(RxEnqueueError::InvalidLength(error)) => {
                    #[cfg(feature = "rx-delivery-observation")]
                    if let Some(observer) = self.delivery_observer {
                        observer.dropped(
                            RxNetworkDeliveryEvent { frame, raw: None },
                            RxEnqueueError::InvalidLength(error),
                        );
                    }
                    return Err(Esp32s31StaApAccessPointPairedRxError::Role(
                        Esp32s31StaApAccessPointRxError::Network(error),
                    ));
                }
            }
        }

        if let Some(started) = self.network_backpressure_since_micros.take() {
            let elapsed = Instant::now().as_micros().saturating_sub(started);
            let report = if let Some(active) = self.protocol.active_mut() {
                &mut active.processor.report
            } else {
                &mut self
                    .protocol
                    .parked_state_mut()
                    .expect("paired AP role is active or parked")
                    .processor
                    .report
            };
            report.maximum_network_backpressure_micros = report
                .maximum_network_backpressure_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
        }
        Ok(WdevRxProgress::Drained)
    }

    fn service_access_point_rx(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
        >,
        frame: Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
    ) -> Result<
        crate::sta_ap::Esp32s31RoutedRxDisposition<
            Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
        >,
        Self::Error,
    > {
        if self.protocol.is_parked() {
            self.activate_tx(physical_tx)
                .map_err(Esp32s31StaApAccessPointPairedRxError::Ownership)?;
        }
        let (nonce, replay_counter) = (self.security_material)();
        let result = self
            .protocol
            .active_mut()
            .expect("AP RX activated the physical TX owner")
            .processor
            .service_routed_rx(
                hardware,
                frame,
                nonce,
                replay_counter,
                Instant::now().as_micros(),
                &mut self.publish_shared_rx,
                #[cfg(feature = "rx-delivery-observation")]
                self.delivery_observer,
            )
            .map_err(|error| {
                Esp32s31StaApAccessPointPairedRxError::Role(
                    Esp32s31StaApAccessPointRxError::Control(error),
                )
            });
        let may_park = self
            .protocol
            .active()
            .is_some_and(|active| !active.processor.tx_pending());
        if may_park
            && let Err(error) = self.park_tx(physical_tx)
            && result.is_ok()
        {
            return Err(Esp32s31StaApAccessPointPairedRxError::Ownership(error));
        }
        result
    }

    fn service_access_point_rx_during_tx(
        &mut self,
        frame: Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
    ) -> Result<
        crate::sta_ap::Esp32s31RoutedRxDisposition<
            Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
        >,
        Self::Error,
    > {
        let Some(active) = self.protocol.active_mut() else {
            // The physical ordinary-TX owner belongs to the station role.
            // Keep the exact ordered AP head until that transaction ends;
            // recovering AP TX resources here would violate affine ownership.
            return Ok(crate::sta_ap::Esp32s31RoutedRxDisposition::Deferred(frame));
        };
        let (nonce, replay_counter) = (self.security_material)();
        active
            .processor
            .service_routed_rx_during_tx::<H, _, _>(
                frame,
                nonce,
                replay_counter,
                Instant::now().as_micros(),
                &mut self.publish_shared_rx,
                #[cfg(feature = "rx-delivery-observation")]
                self.delivery_observer,
            )
            .map_err(|error| {
                Esp32s31StaApAccessPointPairedRxError::Role(
                    Esp32s31StaApAccessPointRxError::Control(error),
                )
            })
    }

    fn has_pending_rx(&self) -> bool {
        self.protocol.active().map_or_else(
            || {
                self.protocol
                    .parked_state()
                    .expect("paired AP role is active or parked")
                    .processor
                    .rx_batch_pending()
            },
            |active| active.processor.rx_batch_pending(),
        )
    }

    fn tx_pending(&self) -> bool {
        self.protocol
            .active()
            .is_some_and(|active| active.processor.tx_pending())
    }
}

impl<
    'storage,
    'beacon,
    'slot,
    'ampdu,
    H,
    P,
    E,
    T,
    NetworkTx,
    Security,
    SharedRx,
    B,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>
    crate::sta_ap::Esp32s31StaApAccessPointControlRole<
        H,
        WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
        >,
    >
    for Esp32s31StaApAccessPointRole<
        WdevPairedRoleOwner<
            Esp32s31StaApAccessPointTxActive<
                Esp32s31AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                Esp32s31AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
            >,
            Esp32s31StaApAccessPointTxParked<
                Esp32s31AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::Esp32s31AccessPointAmpduParked,
            >,
        >,
        NetworkTx,
        Security,
        SharedRx,
    >
where
    H: TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    B: StableDmaBacking + 'ampdu,
{
    type Error = Esp32s31StaApAccessPointPairedControlError;

    fn beacon_publication_due(&self, now_micros: u32) -> bool {
        self.protocol.active().map_or_else(
            || {
                self.protocol
                    .parked_state()
                    .expect("paired AP role is active or parked")
                    .processor
                    .beacon_publication_due(now_micros)
            },
            |active| active.processor.beacon_publication_due(now_micros),
        )
    }

    fn service_access_point_control(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
        >,
        now_micros: u64,
        retain_physical_tx: bool,
    ) -> Result<crate::sta_ap::Esp32s31StaApAccessPointControlProgress, Self::Error> {
        if self.protocol.is_parked() {
            self.activate_tx(physical_tx)
                .map_err(Esp32s31StaApAccessPointPairedControlError::Ownership)?;
        }
        let processor = &mut self
            .protocol
            .active_mut()
            .expect("AP control activated the physical TX owner")
            .processor;
        processor
            .apply_pending_protocol_actions(hardware)
            .map_err(Esp32s31StaApAccessPointPairedControlError::Role)?;
        let progress = processor
            .service_control(hardware, now_micros)
            .map_err(Esp32s31StaApAccessPointPairedControlError::Role)?;
        match progress {
            WdevControlProgress::Idle => {
                if !retain_physical_tx {
                    self.park_tx(physical_tx)
                        .map_err(Esp32s31StaApAccessPointPairedControlError::Ownership)?;
                }
                Ok(crate::sta_ap::Esp32s31StaApAccessPointControlProgress::Idle)
            }
            WdevControlProgress::More => {
                if !retain_physical_tx {
                    self.park_tx(physical_tx)
                        .map_err(Esp32s31StaApAccessPointPairedControlError::Ownership)?;
                }
                Ok(crate::sta_ap::Esp32s31StaApAccessPointControlProgress::More)
            }
            WdevControlProgress::TxPending => {
                Ok(crate::sta_ap::Esp32s31StaApAccessPointControlProgress::TxPending)
            }
            WdevControlProgress::Exit(never) => match never {},
        }
    }

    fn service_access_point_stop(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut WdevPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::ampdu_resources::AggregateTxResources<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
        >,
    ) -> Result<WdevPairedStopProgress, Self::Error> {
        if self.protocol.is_parked() {
            self.activate_tx(physical_tx)
                .map_err(Esp32s31StaApAccessPointPairedControlError::Ownership)?;
        }
        match self
            .protocol
            .active_mut()
            .expect("AP stop activated the physical TX owner")
            .processor
            .service_stop(hardware)
            .map_err(Esp32s31StaApAccessPointPairedControlError::Role)?
        {
            WdevStopProgress::More => {
                self.park_tx(physical_tx)
                    .map_err(Esp32s31StaApAccessPointPairedControlError::Ownership)?;
                Ok(WdevPairedStopProgress::More)
            }
            WdevStopProgress::TxPending => {
                Ok(WdevPairedStopProgress::TxPending(WdevPairRole::Second))
            }
            WdevStopProgress::Stopped => {
                self.park_tx(physical_tx)
                    .map_err(Esp32s31StaApAccessPointPairedControlError::Ownership)?;
                Ok(WdevPairedStopProgress::Stopped)
            }
        }
    }

    fn next_access_point_control_delay_millis(&self, now_micros: u64) -> Result<u32, Self::Error> {
        if let Some(active) = self.protocol.active() {
            active.processor.next_control_delay_millis(now_micros)
        } else {
            self.protocol
                .parked_state()
                .expect("paired AP role is active or parked")
                .processor
                .next_control_delay_millis(now_micros)
        }
        .map_err(Esp32s31StaApAccessPointPairedControlError::Role)
    }
}

impl<Processor, NetworkTx, Security, SharedRx>
    Esp32s31StaApAccessPointRole<Processor, NetworkTx, Security, SharedRx>
{
    pub const fn new(
        protocol: Processor,
        network_tx: NetworkTx,
        security_material: Security,
        publish_shared_rx: SharedRx,
    ) -> Self {
        Self {
            protocol,
            network_tx,
            security_material,
            publish_shared_rx,
            network_backpressure_since_micros: None,
            #[cfg(feature = "rx-delivery-observation")]
            delivery_observer: None,
        }
    }

    pub const fn protocol(&self) -> &Processor {
        &self.protocol
    }

    pub fn protocol_mut(&mut self) -> &mut Processor {
        &mut self.protocol
    }

    pub fn into_parts(self) -> (Processor, NetworkTx, Security, SharedRx) {
        (
            self.protocol,
            self.network_tx,
            self.security_material,
            self.publish_shared_rx,
        )
    }
}
