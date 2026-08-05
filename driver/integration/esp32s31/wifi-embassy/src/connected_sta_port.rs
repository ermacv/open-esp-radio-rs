//! Production owner graph for one connected ESP32-S31 station epoch.
//!
//! Board/HIL code supplies already allocated storage, a network RX sink and
//! executor task placement. This module owns the driver relationships between
//! the associated peer, RX dispatcher/protocol, control-TX handoff,
//! ordinary/A-MPDU TX, BlockAck control and the final [`Esp32s31ConnectedServices`].

use embassy_sync::channel::Receiver;
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_lmac::{
    capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    connected_rx::{ConnectedRxConfig, ConnectedRxDispatcher},
    rate_control::StaTxRatePolicy,
    rx::RxIngressConfig,
    rx_ampdu::{StaRxBlockAckSessions, StaRxBlockAckSessionsError},
    tx::{
        HeDcmRate, HeEdcaTxopLimit, HeMcs, HtGuardInterval, HtMcs, LegacyRate, TxPhyRate,
        TxSlotState,
    },
    tx_ampdu::{HtAmpduTxResources, StaTxBlockAckSessions, TxBlockAckError},
};
use open_esp_radio_esp32s31_wifi_sta::peer::{Esp32s31ConnectedStaPeer, Esp32s31StaConnectedLink};
use open_esp_radio_ieee80211::{
    he::HeDcmConstellation,
    station::{StaAssociationPhy, StaTxSequenceCounters},
    wmm::WmmAccessCategory,
};
use open_esp_radio_wifi_lmac::{
    MacServiceCapabilities, MacTxPlan,
    interface::{BoundVirtualInterface, ChannelContextId, VifId, VifRole, VirtualInterface},
};
use open_esp_radio_wifi_sta::link_monitor::{StaBeaconLossConfig, StaBeaconLossConfigError};

use crate::{
    aggregate_observer::AggregateTxCounters,
    aggregate_tx::{AggregateTxConfig, Esp32s31ConnectedTx},
    connected_control::Esp32s31ConnectedControl,
    connected_services::Esp32s31ConnectedServices,
    control_mailbox::ConnectedControlReceiver,
    control_tx::Esp32s31ControlTx,
    embassy_irq::EmbassyMacIrqRuntime,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    rx_observer::RxPipelineObserver,
    rx_reorder::{
        RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandReceiver, RxReorderCommandSender,
        RxReorderFrameStorage,
    },
    single_mpdu_tx::{ConnectedTxHandoff, SingleMpduTxConfig},
    staged_rx::{ConnectedRxProtocolSink, Esp32s31ConnectedRxProtocol, Esp32s31StagedRxFrame},
};

/// Runtime-selected rate policy independent of HIL environment variables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaRateConfig {
    pub high_throughput_enabled: bool,
    pub fallback_legacy_rate: LegacyRate,
    pub fallback_ht_mcs: HtMcs,
    pub fallback_ht_guard_interval: HtGuardInterval,
    pub ht_mcs_override: Option<HtMcs>,
    pub ht_guard_interval_override: Option<HtGuardInterval>,
    pub he_mcs_override: Option<HeMcs>,
    pub he_guard_interval_and_ltf_override:
        Option<open_esp_radio_esp32s31_wifi_lmac::rx::HeGuardIntervalAndLtf>,
    pub he_dcm_override: Option<HeDcmRate>,
}

/// Complete value policy for one connected driver epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaConfig {
    pub rate: Esp32s31ConnectedStaRateConfig,
    pub rx_ingress: RxIngressConfig,
    pub unicast_attempt_limit: u8,
    pub completion_timeout_us: u64,
    pub aggregate_frame_limit: u8,
    pub aggregate_he_txop_limit: HeEdcaTxopLimit,
    pub tx_block_ack_window: u16,
    pub tx_block_ack_negotiation_timeout_us: u32,
    pub tid0_amsdu: bool,
    pub rx_block_ack_maximum_window: u16,
    pub beacon_miss_limit: u8,
    pub request_initial_tx_block_ack: bool,
}

/// Configuration failure detected before any connected owner moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ConnectedStaConfigError {
    InterfaceRole(VifRole),
    InterfaceAddress {
        interface: [u8; 6],
        station: [u8; 6],
    },
    AggregateFrameLimit {
        limit: u8,
        capacity: usize,
    },
    RxBlockAckWindowExceedsStorage {
        window: u16,
        capacity: usize,
    },
    ZeroUnicastAttemptLimit,
    PeerDoesNotSupportQos,
    TxBlockAck(TxBlockAckError),
    RxBlockAck(StaRxBlockAckSessionsError),
    BeaconLoss(StaBeaconLossConfigError),
}

/// Complete owner return when connected policy validation fails.
#[derive(Debug)]
pub struct Esp32s31ConnectedStaPrepareFailure {
    pub error: Esp32s31ConnectedStaConfigError,
    pub peer: Esp32s31ConnectedStaPeer,
}

/// Validated driver plan derived from the exact associated peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaPlan {
    interface: BoundVirtualInterface,
    link: Esp32s31StaConnectedLink,
    config: Esp32s31ConnectedStaConfig,
    data_tx_rate: TxPhyRate,
    aggregate_tx_rate: TxPhyRate,
    beacon_loss: StaBeaconLossConfig,
}

impl Esp32s31ConnectedStaPlan {
    pub const fn interface(&self) -> BoundVirtualInterface {
        self.interface
    }

    pub const fn link(&self) -> Esp32s31StaConnectedLink {
        self.link
    }

    pub const fn data_tx_rate(&self) -> TxPhyRate {
        self.data_tx_rate
    }

    pub const fn aggregate_tx_rate(&self) -> TxPhyRate {
        self.aggregate_tx_rate
    }

    pub const fn beacon_loss(&self) -> StaBeaconLossConfig {
        self.beacon_loss
    }

    pub const fn rx_config(&self) -> ConnectedRxConfig {
        ConnectedRxConfig {
            station_address: self.link.station_address,
            bssid: self.link.bssid,
            association_id: self.link.association_id,
            ingress: self.config.rx_ingress,
        }
    }

    pub const fn single_mpdu_tx_config(&self) -> SingleMpduTxConfig {
        SingleMpduTxConfig {
            station_address: self.link.station_address,
            bssid: self.link.bssid,
            peer_qos: self.link.peer_qos,
            exchange: MacTxPlan {
                access_category: WmmAccessCategory::BestEffort,
                initial_rate: self.data_tx_rate,
                publication_limit: self.config.unicast_attempt_limit,
                publication_timeout_micros: self.config.completion_timeout_us,
            },
        }
    }

    pub const fn aggregate_tx_config(&self) -> AggregateTxConfig {
        AggregateTxConfig {
            rate: self.aggregate_tx_rate,
            frame_limit: self.config.aggregate_frame_limit,
            attempt_limit: self.config.unicast_attempt_limit,
            completion_timeout_us: self.config.completion_timeout_us,
            he_txop_limit: self.config.aggregate_he_txop_limit,
        }
    }
}

/// Stateless namespace for preparing and composing a connected owner graph.
pub struct Esp32s31ConnectedStaPort;

impl Esp32s31ConnectedStaPort {
    /// Return the portable service contract implemented by this production
    /// ESP32-S31 adapter. HMAC policy can inspect this value without importing
    /// PAC, DMA, interrupt or executor types.
    pub const fn capabilities() -> MacServiceCapabilities {
        ESP32S31_MAC_SERVICE_CAPABILITIES
    }

    /// Validate all value policy before consuming the peer's rate-control
    /// owner, pairwise key, sequences or pinned descriptor storage.
    #[allow(clippy::result_large_err)]
    pub fn prepare<const AGGREGATE_SLOTS: usize>(
        peer: Esp32s31ConnectedStaPeer,
        config: Esp32s31ConnectedStaConfig,
    ) -> Result<Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPrepareFailure> {
        Self::prepare_with_storage::<AGGREGATE_SLOTS, RX_REORDER_BACKING_SLOT_COUNT>(peer, config)
    }

    /// Validate connected policy against the concrete TX aggregate and RX
    /// reorder storage selected by the board composition.
    ///
    /// Compact SRAM profiles must use this entry point. It prevents a runtime
    /// Block Ack window from retaining more MPDUs than the statically allocated
    /// reorder backing can own.
    #[allow(clippy::result_large_err)]
    pub fn prepare_with_storage<const AGGREGATE_SLOTS: usize, const RX_REORDER_SLOTS: usize>(
        peer: Esp32s31ConnectedStaPeer,
        config: Esp32s31ConnectedStaConfig,
    ) -> Result<Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPrepareFailure> {
        let interface = BoundVirtualInterface::new(
            VirtualInterface::new(VifId::PRIMARY, VifRole::Station, peer.link.station_address),
            ChannelContextId::PRIMARY,
        );
        Self::prepare_for_interface_with_storage::<AGGREGATE_SLOTS, RX_REORDER_SLOTS>(
            peer, config, interface,
        )
    }

    /// Prepare one explicitly identified STA VIF on a hardware channel
    /// context. This is the multi-interface entry point; the compatibility
    /// `prepare*` methods bind the existing station to primary VIF/context.
    #[allow(clippy::result_large_err)]
    pub fn prepare_for_interface_with_storage<
        const AGGREGATE_SLOTS: usize,
        const RX_REORDER_SLOTS: usize,
    >(
        peer: Esp32s31ConnectedStaPeer,
        config: Esp32s31ConnectedStaConfig,
        interface: BoundVirtualInterface,
    ) -> Result<Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPrepareFailure> {
        if interface.interface.role != VifRole::Station {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::InterfaceRole(interface.interface.role),
                peer,
            });
        }
        if interface.interface.address != peer.link.station_address {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::InterfaceAddress {
                    interface: interface.interface.address,
                    station: peer.link.station_address,
                },
                peer,
            });
        }
        if config.aggregate_frame_limit == 0
            || usize::from(config.aggregate_frame_limit) > AGGREGATE_SLOTS
        {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::AggregateFrameLimit {
                    limit: config.aggregate_frame_limit,
                    capacity: AGGREGATE_SLOTS,
                },
                peer,
            });
        }
        if usize::from(config.rx_block_ack_maximum_window) > RX_REORDER_SLOTS {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::RxBlockAckWindowExceedsStorage {
                    window: config.rx_block_ack_maximum_window,
                    capacity: RX_REORDER_SLOTS,
                },
                peer,
            });
        }
        if config.unicast_attempt_limit == 0 {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::ZeroUnicastAttemptLimit,
                peer,
            });
        }
        if !peer.link.peer_qos {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::PeerDoesNotSupportQos,
                peer,
            });
        }
        if let Err(error) = StaTxBlockAckSessions::new(
            config.tx_block_ack_window,
            config.tx_block_ack_negotiation_timeout_us,
            config.tid0_amsdu,
        ) {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::TxBlockAck(error),
                peer,
            });
        }
        if let Err(error) =
            StaRxBlockAckSessions::with_maximum_window(config.rx_block_ack_maximum_window)
        {
            return Err(Esp32s31ConnectedStaPrepareFailure {
                error: Esp32s31ConnectedStaConfigError::RxBlockAck(error),
                peer,
            });
        }
        let beacon_loss = match StaBeaconLossConfig::new(
            peer.link.beacon_interval_tu,
            config.beacon_miss_limit,
        ) {
            Ok(beacon_loss) => beacon_loss,
            Err(error) => {
                return Err(Esp32s31ConnectedStaPrepareFailure {
                    error: Esp32s31ConnectedStaConfigError::BeaconLoss(error),
                    peer,
                });
            }
        };

        let data_policy = sta_tx_rate_policy(peer.link, config.rate, false);
        let aggregate_policy = sta_tx_rate_policy(peer.link, config.rate, true);
        Ok(Esp32s31ConnectedStaPlan {
            interface,
            link: peer.link,
            config,
            data_tx_rate: data_policy.fallback_rate(),
            aggregate_tx_rate: peer.rate_control.ampdu_tx_rate(aggregate_policy),
            beacon_loss,
        })
    }

    /// Bind the selected connected peer to the allocation-free staged RX
    /// protocol. The caller chooses the network/HIL sink but cannot replace
    /// the station identity or dispatcher policy.
    pub fn build_rx_protocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        const DEPTH: usize,
        const CAPACITY: usize,
        const SLOTS: usize,
        const REORDER_SLOTS: usize,
    >(
        plan: &Esp32s31ConnectedStaPlan,
        resources: Esp32s31ConnectedStaRxProtocolResources<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            DEPTH,
            CAPACITY,
            SLOTS,
            REORDER_SLOTS,
        >,
    ) -> Esp32s31ConnectedRxProtocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        DEPTH,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >
    where
        M: RawMutex,
        S: ConnectedRxProtocolSink,
    {
        let mut protocol = Esp32s31ConnectedRxProtocol::new_with_reorder_slots(
            resources.frames,
            resources.irq,
            ConnectedRxDispatcher::new(plan.rx_config()),
            resources.sink,
            resources.mpdu,
            resources.ethernet,
        )
        .with_rx_reorder_commands(resources.reorder_commands)
        .with_rx_reorder_storage(resources.reorder_storage);
        if let Some(counters) = resources.pipeline_observer {
            protocol = protocol.with_pipeline_observer(counters);
        }
        match resources.reorder_scratch {
            Some(scratch) => protocol.with_rx_reorder_scratch(scratch),
            None => protocol,
        }
    }

    /// Move a quiescent control TX owner into its connected ordinary/A-MPDU
    /// owner. A busy control owner returns every authority unchanged.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn build_tx<
        'slot,
        'resources,
        M,
        P,
        E,
        T,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const QUEUE_DEPTH: usize,
        const AGGREGATE_SLOTS: usize,
        const AGGREGATE_BUFFER_SIZE: usize,
        const ORDINARY_BUFFER_SIZE: usize,
    >(
        plan: &Esp32s31ConnectedStaPlan,
        resources: Esp32s31ConnectedStaTxResources<
            'slot,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> Result<
        Esp32s31ConnectedTx<
            'slot,
            'resources,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        Esp32s31ConnectedStaTxHandoffFailure<
            'slot,
            'resources,
            P,
            E,
            T,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    >
    where
        M: RawMutex,
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        assert_eq!(
            resources.aggregate.state(),
            TxSlotState::Free,
            "a connected epoch requires returned idle aggregate storage"
        );
        let handoff = ConnectedTxHandoff {
            key: resources.pairwise_key,
            sequences: resources.sequences,
            config: plan.single_mpdu_tx_config(),
        };
        let ordinary = match resources.control.try_into_connected(handoff) {
            Ok(ordinary) => ordinary,
            Err((control, handoff)) => {
                return Err(Esp32s31ConnectedStaTxHandoffFailure {
                    control,
                    handoff,
                    aggregate: resources.aggregate,
                    counters: resources.counters,
                });
            }
        };
        let mut tx =
            Esp32s31ConnectedTx::new(ordinary, resources.aggregate, plan.aggregate_tx_config())
                .expect(
                    "connected STA config and idle aggregate storage were validated before handoff",
                );
        if let Some(counters) = resources.counters {
            tx = tx.with_counters(counters);
        }
        Ok(tx)
    }

    /// Construct BlockAck, beacon-loss and RX-reorder control from the same
    /// connected plan used by RX and TX.
    pub fn build_control<'resources, M: RawMutex, const CAPACITY: usize>(
        plan: &Esp32s31ConnectedStaPlan,
        resources: Esp32s31ConnectedStaControlResources<'resources, M, CAPACITY>,
    ) -> Esp32s31ConnectedControl<'resources, M, CAPACITY> {
        let tx_block_ack = StaTxBlockAckSessions::new(
            plan.config.tx_block_ack_window,
            plan.config.tx_block_ack_negotiation_timeout_us,
            plan.config.tid0_amsdu,
        )
        .expect("connected STA plan validated TX BlockAck policy");
        let mut control = Esp32s31ConnectedControl::new(
            resources.receiver,
            plan.link.bssid,
            plan.link.association_phy == StaAssociationPhy::He20,
            tx_block_ack,
        )
        .with_rx_block_ack_maximum_window(plan.config.rx_block_ack_maximum_window)
        .expect("connected STA plan validated RX BlockAck policy")
        .with_rx_reorder_commands(resources.reorder_commands);
        control.enable_beacon_loss(plan.beacon_loss);
        if plan.config.request_initial_tx_block_ack
            && matches!(plan.aggregate_tx_rate, TxPhyRate::Ht(_) | TxPhyRate::He(_))
        {
            control.queue_initial_tx_block_ack();
        }
        control
    }

    /// Join the already prepared hardware/RX/TX/control owners into the only
    /// services accepted by [`crate::connected_runner::ConnectedRunner`].
    pub fn assemble<H, R, X, C, P>(
        plan: Esp32s31ConnectedStaPlan,
        parts: Esp32s31ConnectedStaDriverParts<H, R, X, C, P>,
    ) -> Esp32s31ConnectedStaDrivers<H, R, X, C, P> {
        Esp32s31ConnectedStaDrivers {
            services: Esp32s31ConnectedServices::with_control(
                parts.hardware,
                parts.rx,
                parts.tx,
                parts.control,
            ),
            protocol: parts.protocol,
            report: Esp32s31ConnectedStaReport {
                link: plan.link,
                data_tx_rate: plan.data_tx_rate,
                aggregate_tx_rate: plan.aggregate_tx_rate,
            },
        }
    }
}

const fn sta_tx_rate_policy(
    link: Esp32s31StaConnectedLink,
    config: Esp32s31ConnectedStaRateConfig,
    use_peer_capabilities: bool,
) -> StaTxRatePolicy {
    StaTxRatePolicy {
        association_phy: link.association_phy,
        high_throughput_enabled: config.high_throughput_enabled && link.peer_qos,
        fallback_legacy_rate: config.fallback_legacy_rate,
        fallback_ht_mcs: config.fallback_ht_mcs,
        fallback_ht_guard_interval: config.fallback_ht_guard_interval,
        ht_mcs_override: config.ht_mcs_override,
        ht_guard_interval_override: config.ht_guard_interval_override,
        he_mcs_override: config.he_mcs_override,
        he_guard_interval_and_ltf_override: config.he_guard_interval_and_ltf_override,
        he_dcm_override: config.he_dcm_override,
        he_800ns_gi_ltf: if use_peer_capabilities && link.peer_supports_one_ltf_800ns_gi {
            open_esp_radio_esp32s31_wifi_lmac::rx::HeGuardIntervalAndLtf::OneLtf800Ns
        } else {
            open_esp_radio_esp32s31_wifi_lmac::rx::HeGuardIntervalAndLtf::TwoLtf800Ns
        },
        peer_supports_ldpc: use_peer_capabilities && link.peer_supports_ldpc,
        peer_dcm_receive: if use_peer_capabilities {
            link.peer_dcm_receive
        } else {
            HeDcmConstellation::NotSupported
        },
    }
}

/// Named protocol resources supplied by the platform composition.
pub struct Esp32s31ConnectedStaRxProtocolResources<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    pub frames: Receiver<'queue, M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    pub irq: &'irq EmbassyMacIrqRuntime<M>,
    pub sink: S,
    pub mpdu: &'scratch mut [u8],
    pub ethernet: &'scratch mut [u8],
    pub reorder_commands: RxReorderCommandReceiver<'queue, M>,
    pub reorder_storage: &'pool RxReorderFrameStorage<CAPACITY, REORDER_SLOTS>,
    pub reorder_scratch: Option<&'scratch mut [u8]>,
    /// Optional observation-only counters used by qualification fixtures.
    pub pipeline_observer: Option<&'queue dyn RxPipelineObserver>,
}

/// Named resources consumed by the control-to-connected TX handoff.
pub struct Esp32s31ConnectedStaTxResources<
    'slot,
    'resources,
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const AGGREGATE_SLOTS: usize,
    const AGGREGATE_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> {
    pub control: Esp32s31ControlTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
    pub aggregate: HtAmpduTxResources<'resources, AGGREGATE_SLOTS, AGGREGATE_BUFFER_SIZE>,
    pub pairwise_key: open_esp_radio_esp32s31_wifi_lmac::crypto::StaPairwiseCcmpSlot,
    pub sequences: StaTxSequenceCounters,
    pub counters: Option<&'resources AggregateTxCounters>,
    pub network_domain: Esp32s31ConnectedStaNetworkTxDomain<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
    >,
}

/// Type-only binding to the pinned `embassy-net` TX resource domain.
///
/// The runner, rather than this port, owns the actual consumer. Carrying its
/// lifetime and const geometry here prevents inference from selecting a TX
/// owner incompatible with that runner without introducing another runtime
/// pointer or capability.
pub struct Esp32s31ConnectedStaNetworkTxDomain<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    #[allow(clippy::type_complexity)]
    marker: core::marker::PhantomData<&'resources (
        M,
        [u8; FRAME_CAPACITY],
        [u8; HEADROOM],
        [u8; TRAILER],
        [u8; QUEUE_DEPTH],
    )>,
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> Default
    for Esp32s31ConnectedStaNetworkTxDomain<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
    >
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
>
    Esp32s31ConnectedStaNetworkTxDomain<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
    >
{
    pub const fn new() -> Self {
        Self {
            marker: core::marker::PhantomData,
        }
    }
}

/// Complete owner return when control TX was still active at handoff.
pub struct Esp32s31ConnectedStaTxHandoffFailure<
    'slot,
    'resources,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    const AGGREGATE_SLOTS: usize,
    const AGGREGATE_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> {
    pub control: Esp32s31ControlTx<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
    pub handoff: ConnectedTxHandoff,
    pub aggregate: HtAmpduTxResources<'resources, AGGREGATE_SLOTS, AGGREGATE_BUFFER_SIZE>,
    pub counters: Option<&'resources AggregateTxCounters>,
}

/// Named control-plane resources for one connected epoch.
pub struct Esp32s31ConnectedStaControlResources<'resources, M: RawMutex, const CAPACITY: usize> {
    pub receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
    pub reorder_commands: RxReorderCommandSender<'resources, M>,
}

/// Final owner graph immediately before the connected services begin running.
pub struct Esp32s31ConnectedStaDriverParts<H, R, X, C, P> {
    pub hardware: H,
    pub rx: R,
    pub tx: X,
    pub control: C,
    pub protocol: P,
}

/// Driver composition returned to the executor/application layer.
pub struct Esp32s31ConnectedStaDrivers<H, R, X, C, P> {
    pub services: Esp32s31ConnectedServices<H, R, X, C>,
    pub protocol: P,
    pub report: Esp32s31ConnectedStaReport,
}

/// Copy-only observations useful to qualification and application policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ConnectedStaReport {
    pub link: Esp32s31StaConnectedLink,
    pub data_tx_rate: TxPhyRate,
    pub aggregate_tx_rate: TxPhyRate,
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_embassy_net::NoopRawMutex;
    use open_esp_radio_esp32s31_wifi_lmac::connected_rx::{ConnectedRxEvent, ConnectedRxSink};
    use open_esp_radio_esp32s31_wifi_lmac::rate_control::{
        HeLowMetricReportFeatures, StaLinkMetric, StaRateControlAssociation,
        StaRateControlAssociationInput, StaRateControlPhy,
    };

    use crate::{
        control_mailbox::ConnectedControlResources,
        rx_reorder::RxReorderCommandResources,
        staged_rx::{AlwaysReadyConnectedRxSink, Esp32s31StagedRxQueue},
    };

    struct Sink;

    impl ConnectedRxSink for Sink {
        fn publish(&mut self, _event: ConnectedRxEvent<'_>) {}
    }

    fn peer() -> Esp32s31ConnectedStaPeer {
        let link_metric = StaLinkMetric::from_rssi_and_noise_floor(-45, -95);
        Esp32s31ConnectedStaPeer {
            link: Esp32s31StaConnectedLink {
                station_address: [1, 2, 3, 4, 5, 6],
                bssid: [7, 8, 9, 10, 11, 12],
                association_id: 7,
                beacon_interval_tu: 100,
                peer_qos: true,
                association_phy: StaAssociationPhy::He20,
                peer_supports_one_ltf_800ns_gi: true,
                peer_supports_ldpc: true,
                peer_dcm_receive: HeDcmConstellation::Qam16,
            },
            rate_control: StaRateControlAssociation::new(StaRateControlAssociationInput {
                phy: StaRateControlPhy::He,
                link_metric,
                p2p: false,
                peer_highest_rate: None,
                long_range_rates_present: false,
                he_low_metric_report: HeLowMetricReportFeatures::default(),
            }),
        }
    }

    fn config() -> Esp32s31ConnectedStaConfig {
        Esp32s31ConnectedStaConfig {
            rate: Esp32s31ConnectedStaRateConfig {
                high_throughput_enabled: true,
                fallback_legacy_rate: LegacyRate::Ofdm24M,
                fallback_ht_mcs: HtMcs::Mcs7,
                fallback_ht_guard_interval: HtGuardInterval::Long800Ns,
                ht_mcs_override: None,
                ht_guard_interval_override: None,
                he_mcs_override: None,
                he_guard_interval_and_ltf_override: None,
                he_dcm_override: None,
            },
            rx_ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            unicast_attempt_limit: 4,
            completion_timeout_us: 250_000,
            aggregate_frame_limit: 32,
            aggregate_he_txop_limit: HeEdcaTxopLimit::DEFAULT,
            tx_block_ack_window: 32,
            tx_block_ack_negotiation_timeout_us: 500_000,
            tid0_amsdu: false,
            rx_block_ack_maximum_window: 32,
            beacon_miss_limit: 10,
            request_initial_tx_block_ack: true,
        }
    }

    #[test]
    fn plan_owns_rate_rx_tx_block_ack_and_beacon_policy() {
        let capabilities = Esp32s31ConnectedStaPort::capabilities();
        assert_eq!(capabilities.resources.channel_contexts, 1);
        assert!(capabilities.supports_rx_block_ack_window(32));
        assert!(capabilities.supports_tx_block_ack_window(32));

        let plan = Esp32s31ConnectedStaPort::prepare::<32>(peer(), config()).unwrap();
        assert_eq!(plan.interface().interface.id, VifId::PRIMARY);
        assert_eq!(plan.interface().interface.role, VifRole::Station);
        assert_eq!(plan.interface().channel_context, ChannelContextId::PRIMARY);
        assert_eq!(plan.rx_config().association_id, 7);
        assert_eq!(
            plan.single_mpdu_tx_config().exchange.initial_rate.code(),
            23
        );
        assert!(matches!(plan.aggregate_tx_rate(), TxPhyRate::He(_)));
        assert_eq!(plan.beacon_loss().window_micros(), 1_024_000);
    }

    #[test]
    fn port_binds_rx_and_control_to_one_validated_peer_plan() {
        let plan = Esp32s31ConnectedStaPort::prepare::<32>(peer(), config()).unwrap();
        let reorder_storage = RxReorderFrameStorage::<128>::new();
        let queue: Esp32s31StagedRxQueue<'_, NoopRawMutex, 2, 128, 2> =
            Esp32s31StagedRxQueue::new();
        let (_, frames) = queue.split();
        let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        let reorder_commands = RxReorderCommandResources::<NoopRawMutex>::new();
        let (reorder_sender, reorder_receiver) = reorder_commands.split();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        let protocol = Esp32s31ConnectedStaPort::build_rx_protocol(
            &plan,
            Esp32s31ConnectedStaRxProtocolResources {
                frames,
                irq: &irq,
                sink: AlwaysReadyConnectedRxSink(Sink),
                mpdu: &mut mpdu,
                ethernet: &mut ethernet,
                reorder_commands: reorder_receiver,
                reorder_storage: &reorder_storage,
                reorder_scratch: None,
                pipeline_observer: None,
            },
        );
        assert_eq!(protocol.dispatcher().config(), plan.rx_config());

        let control_resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
        let (_, receiver) = control_resources.split();
        let control = Esp32s31ConnectedStaPort::build_control(
            &plan,
            Esp32s31ConnectedStaControlResources {
                receiver,
                reorder_commands: reorder_sender,
            },
        );
        assert_eq!(control.rx_block_ack().maximum_window(), 32);
        assert_eq!(
            control
                .beacon_monitor()
                .expect("plan enables beacon loss")
                .config(),
            plan.beacon_loss()
        );
    }

    #[test]
    fn invalid_config_returns_the_exact_peer_before_owner_handoff() {
        let original = peer();
        let link = original.link;
        let mut invalid = config();
        invalid.aggregate_frame_limit = 33;
        let failure = Esp32s31ConnectedStaPort::prepare::<32>(original, invalid).unwrap_err();
        assert_eq!(
            failure.error,
            Esp32s31ConnectedStaConfigError::AggregateFrameLimit {
                limit: 33,
                capacity: 32,
            }
        );
        assert_eq!(failure.peer.link, link);
    }

    #[test]
    fn compact_profile_rejects_rx_window_larger_than_reorder_storage() {
        let original = peer();
        let link = original.link;
        let failure = Esp32s31ConnectedStaPort::prepare_with_storage::<32, 8>(original, config())
            .unwrap_err();
        assert_eq!(
            failure.error,
            Esp32s31ConnectedStaConfigError::RxBlockAckWindowExceedsStorage {
                window: 32,
                capacity: 8,
            }
        );
        assert_eq!(failure.peer.link, link);
    }

    #[test]
    fn explicit_vif_binding_rejects_unimplemented_role_before_owner_handoff() {
        let original = peer();
        let link = original.link;
        let interface = BoundVirtualInterface::new(
            VirtualInterface::new(VifId::new(1), VifRole::AccessPoint, link.station_address),
            ChannelContextId::PRIMARY,
        );
        let failure = Esp32s31ConnectedStaPort::prepare_for_interface_with_storage::<32, 32>(
            original,
            config(),
            interface,
        )
        .unwrap_err();
        assert_eq!(
            failure.error,
            Esp32s31ConnectedStaConfigError::InterfaceRole(VifRole::AccessPoint)
        );
        assert_eq!(failure.peer.link, link);
    }
}
