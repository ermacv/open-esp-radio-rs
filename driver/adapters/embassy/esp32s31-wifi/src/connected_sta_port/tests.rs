use super::*;
use open_esp_radio_embassy_net::NoopRawMutex;
use open_esp_radio_esp32s31_wifi_mac::rate_control::{
    HeLowMetricReportFeatures, StaLinkMetric, StaRateControlAssociation,
    StaRateControlAssociationInput, StaRateControlPhy,
};
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{ConnectedRxEvent, ConnectedRxSink};
use open_esp_radio_wifi_softmac::{
    WifiConfig, WifiMacAddress, WifiMonitorConfig, WifiStationConfig,
    interface::{BoundVirtualInterface, ChannelContextId, VifId, VifRole, VirtualInterface},
};
use std::boxed::Box;

use crate::{
    connected_rx_protocol::{AlwaysReadyConnectedRxSink, Esp32s31StagedRxQueue},
    control_mailbox::ConnectedControlResources,
    rx_reorder::RxReorderCommandResources,
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
            peer_supports_ht_short_guard_interval: false,
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
        tx: Esp32s31ConnectedStaTxPolicy {
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
            unicast_attempt_limit: 4,
            completion_timeout_us: 250_000,
            aggregate_frame_limit: 32,
            aggregate_he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
        block_ack: Esp32s31ConnectedStaBlockAckPolicy {
            tx_block_ack_window: 32,
            tx_block_ack_negotiation_timeout_us: 500_000,
            tx_block_ack_negotiation_attempt_limit: 3,
            tid0_amsdu: false,
            rx_block_ack_maximum_window: 32,
            request_initial_tx_block_ack: true,
        },
        receive: Esp32s31ConnectedStaRxPolicy {
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            beacon_miss_limit: 10,
        },
    }
}

fn station_interface(peer: &Esp32s31ConnectedStaPeer) -> BoundVirtualInterface {
    BoundVirtualInterface::new(
        VirtualInterface::new(VifId::PRIMARY, VifRole::Station, peer.link.station_address),
        ChannelContextId::PRIMARY,
    )
}

fn prepare<const AGGREGATE_SLOTS: usize, const RX_REORDER_SLOTS: usize>(
    peer: Esp32s31ConnectedStaPeer,
    config: Esp32s31ConnectedStaConfig,
) -> Result<Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPrepareFailure> {
    let interface = station_interface(&peer);
    Esp32s31ConnectedStaPort::prepare_for_interface_with_storage::<AGGREGATE_SLOTS, RX_REORDER_SLOTS>(
        peer, config, interface,
    )
}

#[test]
fn plan_owns_rate_rx_tx_block_ack_and_beacon_policy() {
    let capabilities = Esp32s31ConnectedStaPort::capabilities();
    assert_eq!(capabilities.resources.channel_contexts, 1);
    assert!(capabilities.supports_rx_block_ack_window(32));
    assert!(capabilities.supports_tx_block_ack_window(32));

    let plan = prepare::<32, 32>(peer(), config()).unwrap();
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
    let plan = prepare::<32, 32>(peer(), config()).unwrap();
    let reorder_storage = RxReorderFrameStorage::<128>::new();
    let queue: Esp32s31StagedRxQueue<'_, NoopRawMutex, 2, 128, 2> = Esp32s31StagedRxQueue::new();
    let (_, frames) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let reorder_commands = RxReorderCommandResources::<NoopRawMutex>::new();
    let (reorder_sender, reorder_receiver) = reorder_commands.split();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];
    let protocol_runtime = Box::leak(Box::new(Esp32s31ConnectedRxProtocolStorage::new()));
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
            runtime: protocol_runtime,
            reorder_scratch: None,
            pipeline_observer: None,
        },
    );
    assert_eq!(protocol.dispatcher().config(), plan.rx_config());

    let control_resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (_, receiver) = control_resources.split();
    let rx_block_ack = Esp32s31StaApRxBlockAck::with_maximum_window(32).unwrap();
    let control = Esp32s31ConnectedStaPort::build_control(
        &plan,
        Esp32s31ConnectedStaControlResources {
            receiver,
            reorder_commands: reorder_sender,
            rx_block_ack: &rx_block_ack,
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
    invalid.tx.aggregate_frame_limit = 33;
    let failure = prepare::<32, 32>(original, invalid).unwrap_err();
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
fn zero_tx_block_ack_attempt_limit_is_rejected_before_owner_handoff() {
    let original = peer();
    let link = original.link;
    let mut invalid = config();
    invalid.block_ack.tx_block_ack_negotiation_attempt_limit = 0;
    let failure = prepare::<32, 32>(original, invalid).unwrap_err();
    assert_eq!(
        failure.error,
        Esp32s31ConnectedStaConfigError::ZeroTxBlockAckNegotiationAttemptLimit
    );
    assert_eq!(failure.peer.link, link);
}

#[test]
fn compact_profile_rejects_rx_window_larger_than_reorder_storage() {
    let original = peer();
    let link = original.link;
    let failure = prepare::<32, 8>(original, config()).unwrap_err();
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

#[test]
fn application_wifi_plan_materializes_the_selected_station_vif() {
    let mut original = peer();
    original.link.station_address = [2, 2, 3, 4, 5, 6];
    let address = WifiMacAddress::new(original.link.station_address).unwrap();
    let wifi = WifiConfig::station(WifiStationConfig::new(address))
        .validate(Esp32s31ConnectedStaPort::capabilities())
        .unwrap();
    let plan = Esp32s31ConnectedStaPort::prepare_for_wifi_plan_with_storage::<32, 32>(
        original,
        config(),
        wifi,
    )
    .unwrap();
    assert_eq!(plan.interface(), wifi.station().unwrap());
}

#[test]
fn wifi_plan_without_station_returns_the_exact_peer() {
    let original = peer();
    let link = original.link;
    let mut capabilities = Esp32s31ConnectedStaPort::capabilities();
    capabilities.interfaces.normalized_monitor_tap = true;
    let wifi = WifiConfig::monitor(WifiMonitorConfig::normalized())
        .validate(capabilities)
        .unwrap();
    let failure = Esp32s31ConnectedStaPort::prepare_for_wifi_plan_with_storage::<32, 32>(
        original,
        config(),
        wifi,
    )
    .unwrap_err();
    assert_eq!(
        failure.error,
        Esp32s31ConnectedStaConfigError::MissingStationInterface
    );
    assert_eq!(failure.peer.link, link);
}
