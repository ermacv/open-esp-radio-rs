#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc role-resume state retains the concrete returned station owner"
)]
#![expect(
    clippy::result_large_err,
    reason = "AP teardown returns the exact physical and role owners on failure"
)]

//! Private ESP32-S31 access-point epoch composition.

use super::*;
#[cfg(feature = "diagnostics")]
use core::cell::RefCell;
#[cfg(feature = "diagnostics")]
use embassy_sync::blocking_mutex::Mutex;
#[cfg(feature = "diagnostics")]
use open_esp_radio_esp32s31_wifi_embassy::diagnostics::access_point::{
    AccessPointTerminalObservation, AccessPointTerminalObserver,
};

#[cfg(feature = "diagnostics")]
struct ProductionAccessPointTerminalObserver;

#[cfg(feature = "diagnostics")]
static ACCESS_POINT_TERMINAL_OBSERVER: ProductionAccessPointTerminalObserver =
    ProductionAccessPointTerminalObserver;
#[cfg(feature = "diagnostics")]
static ACCESS_POINT_TERMINAL_OBSERVATION: Mutex<
    CriticalSectionRawMutex,
    RefCell<Option<AccessPointTerminalObservation>>,
> = Mutex::new(RefCell::new(None));

#[cfg(feature = "diagnostics")]
impl AccessPointTerminalObserver for ProductionAccessPointTerminalObserver {
    fn observe(&self, observation: AccessPointTerminalObservation) {
        ACCESS_POINT_TERMINAL_OBSERVATION.lock(|slot| {
            slot.replace(Some(observation));
        });
    }
}

#[cfg(feature = "diagnostics")]
pub(super) fn begin_access_point_observation() -> &'static dyn AccessPointTerminalObserver {
    ACCESS_POINT_TERMINAL_OBSERVATION.lock(|slot| {
        slot.replace(None);
    });
    &ACCESS_POINT_TERMINAL_OBSERVER
}

#[cfg(feature = "diagnostics")]
fn take_access_point_observation() -> Option<AccessPointTerminalObservation> {
    ACCESS_POINT_TERMINAL_OBSERVATION.lock(|slot| slot.take())
}

type ProductionAccessPointControl = Esp32s31AccessPointControl<
    'static,
    'static,
    'static,
    ProductionAccessPointRxProducer,
    ProductionAccessPointRxConsumer,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::datapath::tx::time::EmbassyWifiTxTimer,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_BUFFER_STORAGE_SIZE,
    TX_BUFFER_SIZE,
>;
type ProductionHaltedRx =
    open_esp_radio_esp32s31_wifi_mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>;
type ProductionScanRx =
    Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
type ProductionWifiTxResources = WifiTxResources<
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::datapath::tx::time::EmbassyWifiTxTimer,
    TX_BUFFER_SIZE,
>;
type ProductionAccessPointStopped = EmbassyAccessPointStopped<
    'static,
    'static,
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::datapath::tx::time::EmbassyWifiTxTimer,
    ProductionAccessPointRxProducer,
    ProductionAccessPointRxConsumer,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_BUFFER_STORAGE_SIZE,
    TX_BUFFER_SIZE,
>;
type ProductionAccessPointAmpdu =
    open_esp_radio_esp32s31_wifi_embassy::roles::access_point::Esp32s31AccessPointAmpdu<
        'static,
        RadioTxBacking,
        {
            open_esp_radio_esp32s31_wifi_embassy::composition::resources::ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT
        },
        0,
    >;
type ProductionAccessPointRxBlockAck =
    open_esp_radio_esp32s31_wifi_embassy::roles::concurrent::Esp32s31StaApRxBlockAck;
type ProductionAccessPointRxReorder = Esp32s31AccessPointRxReorder<'static, RX_BUFFER_SIZE>;
type ProductionAccessPointRxReorderStorage =
    open_esp_radio_esp32s31_wifi_embassy::datapath::rx::reorder::RxReorderFrameStorage<
        RX_BUFFER_SIZE,
        {
            open_esp_radio_esp32s31_wifi_embassy::datapath::rx::reorder::RX_REORDER_BACKING_SLOT_COUNT
        },
    >;

#[cfg(feature = "diagnostics")]
#[inline(never)]
pub(super) fn publish_access_point_observation(
    hook: fn(crate::Esp32s31AccessPointObservation),
    channel: WifiChannel,
    observation: &AccessPointTerminalObservation,
    rx_hardware_buffer_full: u16,
    rx_hardware_fifo_overflow: u16,
) {
    let control = &observation.control;
    let mac = &observation.mac;
    let engine = &observation.engine;
    hook(crate::Esp32s31AccessPointObservation {
        channel: channel.primary(),
        bandwidth_mhz: channel.bandwidth_mhz(),
        beacons_transmitted: mac.beacons_transmitted,
        missed_beacon_intervals: control.missed_beacon_intervals,
        maximum_beacon_lateness_micros: control.maximum_beacon_lateness_micros,
        tx_interrupt_wakes: control.tx_interrupt_wakes,
        tx_deadline_wakes: control.tx_deadline_wakes,
        maximum_tx_pending_micros: control.maximum_tx_pending_micros,
        maximum_network_tx_pending_micros: control.maximum_network_tx_pending_micros,
        network_tx_attempts_at_maximum_pending: control.network_tx_attempts_at_maximum_pending,
        maximum_rx_service_micros: control.maximum_rx_service_micros,
        maximum_rx_dma_service_micros: control.maximum_rx_dma_service_micros,
        total_rx_dma_service_micros: control.total_rx_dma_service_micros,
        rx_dma_service_calls: control.rx_dma_service_calls,
        maximum_rx_protocol_service_micros: control.maximum_rx_protocol_service_micros,
        maximum_rx_protected_data_service_micros: control.maximum_rx_protected_data_service_micros,
        total_rx_protected_data_service_micros: control.total_rx_protected_data_service_micros,
        maximum_rx_management_service_micros: control.maximum_rx_management_service_micros,
        maximum_rx_eapol_service_micros: control.maximum_rx_eapol_service_micros,
        maximum_network_backpressure_micros: control.maximum_network_backpressure_micros,
        authentication_responses: mac.authentication_responses_transmitted,
        association_responses: mac.association_responses_transmitted,
        authorized_peers: engine.authorized_peers,
        maximum_associated_peers: engine.maximum_associated_peers,
        maximum_authorized_peers: engine.maximum_authorized_peers,
        peer_removals: engine.peer_removals,
        authentication_timeouts: engine.authentication_timeouts,
        wpa2_response_windows: engine.wpa2_response_windows,
        wpa2_pending_on_stop: engine.wpa2_pending_on_stop,
        wpa2_retransmissions: engine.wpa2_retransmissions,
        wpa2_handshake_failures: engine.wpa2_handshake_failures,
        wpa2_handshake_timeouts: engine.wpa2_handshake_timeouts,
        inactivity_timeouts: engine.inactivity_timeouts,
        disassociations_prepared: engine.disassociations_prepared,
        disassociations_published: mac.disassociations_published,
        disassociations_acknowledged: mac.disassociations_acknowledged,
        deauthentications_prepared: engine.deauthentications_prepared,
        deauthentications_published: mac.deauthentications_published,
        deauthentications_acknowledged: mac.deauthentications_acknowledged,
        tx_block_ack_requests_prepared: engine.tx_block_ack_requests_prepared,
        tx_block_ack_responses_observed: engine.tx_block_ack_responses_observed,
        tx_block_ack_agreements_operational: engine.tx_block_ack_agreements_operational,
        tx_block_ack_responses_rejected: engine.tx_block_ack_responses_rejected,
        tx_block_ack_negotiation_timeouts: engine.tx_block_ack_negotiation_timeouts,
        rx_block_ack_responses_transmitted: mac.rx_block_ack_responses_transmitted,
        rx_hardware_buffer_full,
        rx_hardware_fifo_overflow,
        retained_rx_descriptors: control.retained_rx_descriptors,
        ignored_rx_frames: control.ignored_rx_frames,
        rx_mic_failures: control.rx_mic_failures,
        rx_quarantined_frames: control.rx_quarantined_frames,
        rx_view_rejected: control.rx_view_rejected,
        control_frames_staged: control.control_frames_staged,
        control_frames_dropped_while_busy: control.control_frames_dropped_while_busy,
        ethernet_frames_staged: control.ethernet_frames_staged,
        ethernet_arp_requests_staged: control.ethernet_arp_requests_staged,
        ethernet_tcp_frames_staged: control.ethernet_tcp_frames_staged,
        network_tx_frames_observed: control.network_tx_frames_observed,
        network_tx_arp_requests: control.network_tx_arp_requests,
        network_tx_arp_replies: control.network_tx_arp_replies,
        network_tx_rejected_no_peer: control.network_tx_rejected_no_peer,
        network_tx_rejected_destination: control.network_tx_rejected_destination,
        network_tx_frames_rejected: control.network_tx_frames_rejected,
        rx_ht_data_frames: control.rx_ht_data_frames,
        rx_ht_mpdus_with_aggregation_bit: control.rx_ht_mpdus_with_aggregation_bit,
        rx_rssi_samples: control.rx_rssi_samples,
        rx_rssi_sum_dbm: control.rx_rssi_sum_dbm,
        rx_rssi_min_dbm: control.rx_rssi_min_dbm,
        rx_rssi_max_dbm: control.rx_rssi_max_dbm,
        rx_ht40_mcs_frames: control.rx_ht40_mcs_frames,
        rx_ht40_mcs32_frames: control.rx_ht40_mcs32_frames,
        tx_ht_aggregates: control.tx_ht_aggregates,
        tx_ht40_mcs7_aggregates: control.tx_ht40_mcs7_aggregates,
        data_frames_transmitted: mac.data_frames_transmitted,
        ht_duplicate_tx_requests: mac.ht_duplicate_tx_requests,
        ht_duplicate_tx_selection: mac.ht_duplicate_tx_selection,
        data_tx_attempts: mac.data_tx.attempts,
        data_tx_retried_frames: mac.data_tx.retried_frames,
        data_tx_maximum_attempts: mac.data_tx.maximum_attempts,
        data_tx_minimum_final_rate_kbps: mac.data_tx.minimum_final_rate_kbps,
        data_tx_ack_snr_samples: mac.data_tx.ack_snr_samples,
        data_tx_minimum_ack_snr_db: mac.data_tx.minimum_ack_snr_db,
        data_tx_maximum_ack_snr_db: mac.data_tx.maximum_ack_snr_db,
        tx_ack_timeout_retries: mac.data_tx.ack_timeout_retries,
        tx_cts_timeout_retries: mac.data_tx.cts_timeout_retries,
        tx_collision_retries: mac.data_tx.collision_retries,
        tx_hardware_failures: mac.tx_failures.hardware_failures,
        tx_hardware_timeouts: mac.tx_failures.hardware_timeouts,
        tx_collision_limits: mac.tx_failures.collision_limits,
        tx_last_hardware_status: mac.tx_failures.last_hardware_status,
        protected_data_frames: control.protected_data_frames,
        protected_data_unauthorized: control.protected_data_unauthorized,
        protected_data_foreign: control.protected_data_foreign,
        protected_data_duplicates: control.protected_data_duplicates,
        rx_reorder_buffered_mpdus: control.rx_reorder_buffered_mpdus,
        rx_reorder_dispatched_mpdus: control.rx_reorder_dispatched_mpdus,
        rx_reorder_hardware_window_resets: control.rx_reorder_hardware_window_resets,
        rx_reorder_gap_timeouts: control.rx_reorder_gap_timeouts,
        protected_data_radio_rejected: control.protected_data_radio_rejected,
        protected_data_protocol_rejected: control.protected_data_protocol_rejected,
    });
}

#[cfg(feature = "diagnostics")]
#[inline(never)]
pub(super) fn publish_stored_access_point_observation(
    hook: fn(crate::Esp32s31AccessPointObservation),
    channel: WifiChannel,
    rx_hardware_buffer_full: u16,
    rx_hardware_fifo_overflow: u16,
) {
    let observation = take_access_point_observation()
        .expect("successful AP teardown emits one terminal observation");
    publish_access_point_observation(
        hook,
        channel,
        &observation,
        rx_hardware_buffer_full,
        rx_hardware_fifo_overflow,
    );
}

pub(super) struct ProductionStationRoleResources {
    scan_table: &'static mut ScanTable,
    scan_frame: &'static mut [u8],
    ethernet: &'static mut [u8],
    resume: ProductionStationRoleResume,
    board: ProductionStationBoardResources,
    station_address: [u8; 6],
}

impl ProductionStationRoleResources {
    pub(super) const fn station_address(&self) -> [u8; 6] {
        self.station_address
    }

    pub(super) fn scan_storage(&mut self) -> (&mut ScanTable, &mut [u8]) {
        (&mut *self.scan_table, &mut *self.scan_frame)
    }

    pub(super) fn scan_table(&self) -> &ScanTable {
        self.scan_table
    }
}

pub(super) struct ProductionAccessPointParked {
    dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    tx_epoch: &'static mut TxStorage,
    station: ProductionStationRoleResources,
    monitor: ProductionMonitorResources,
    aggregate_tx: Option<RadioAmpduStorage>,
}

/// Exact station lifecycle state retained while AP temporarily owns Wi-Fi.
///
/// Only the halted RX ring and the shared TX/network endpoints leave this
/// frontier. The next STA epoch therefore resumes the same allocation and
/// register-arena capabilities; AP switching is not a hidden reset.
enum ProductionStationRoleResume {
    Fresh {
        network: WifiNetworkResources,
    },
    Returned {
        phase: ProductionStationReturnedPhase,
        security: Esp32s31StaAttemptSecurity<'static>,
        interrupt_route: EspHalMacInterruptRoute,
        mac_runtime: &'static EmbassyMacIrqRuntime<CriticalSectionRawMutex>,
        power_runtime: &'static EmbassyPowerIrqRuntime<CriticalSectionRawMutex>,
    },
}

enum ProductionStationReturnedPhase {
    InitialScan {
        network: WifiNetworkResources,
        identity: Esp32s31StaIdentity,
    },
    InitialJoin {
        network: WifiNetworkResources,
        station: Esp32s31StaAttemptStation,
    },
    Disconnected {
        network: RunningWifiNetwork,
        rx: ConnectedRxEpochResources,
        control: &'static ControlResources,
        station: Esp32s31StaAttemptStation,
        registers: Esp32s31RadioOwnerRepublish<'static>,
    },
    Reconnected {
        network: WifiNetworkResources,
        rx: ConnectedRxEpochResources,
        control: &'static ControlResources,
        station: Esp32s31StaAttemptStation,
        registers: Esp32s31RadioOwnerRepublish<'static>,
    },
}

impl ProductionStationRoleResume {
    fn radio_runner(&self) -> &NetworkRunner {
        match self {
            Self::Fresh { network }
            | Self::Returned {
                phase:
                    ProductionStationReturnedPhase::InitialScan { network, .. }
                    | ProductionStationReturnedPhase::InitialJoin { network, .. }
                    | ProductionStationReturnedPhase::Reconnected { network, .. },
                ..
            } => network.radio_runner(),
            Self::Returned {
                phase: ProductionStationReturnedPhase::Disconnected { network, .. },
                ..
            } => network.radio_runner(),
        }
    }
}

pub(super) struct ProductionWifiPhysicalResources {
    dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    rx_ring:
        Option<open_esp_radio_esp32s31_wifi_mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>>,
    tx: ProductionOrdinaryTxResources,
    aggregate_tx: RadioAmpduStorage,
}

impl ProductionWifiPhysicalResources {
    pub(super) const fn new(
        dma: Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
        rx_ring: Option<
            open_esp_radio_esp32s31_wifi_mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>,
        >,
        tx: ProductionOrdinaryTxResources,
        aggregate_tx: RadioAmpduStorage,
    ) -> Self {
        Self {
            dma,
            rx_ring,
            tx,
            aggregate_tx,
        }
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        Esp32s31StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
        Option<open_esp_radio_esp32s31_wifi_mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>>,
        ProductionOrdinaryTxResources,
        RadioAmpduStorage,
    ) {
        (self.dma, self.rx_ring, self.tx, self.aggregate_tx)
    }

    pub(super) fn take_halted_rx(self) -> (Self, Option<ProductionHaltedRx>) {
        let Self {
            dma,
            rx_ring,
            tx,
            aggregate_tx,
        } = self;
        (
            Self {
                dma,
                rx_ring: None,
                tx,
                aggregate_tx,
            },
            rx_ring,
        )
    }

    pub(super) fn restore_halted_rx(self, rx_ring: ProductionHaltedRx) -> Self {
        let Self {
            dma,
            rx_ring: previous,
            tx,
            aggregate_tx,
        } = self;
        assert!(previous.is_none(), "physical RX ring is already present");
        Self {
            dma,
            rx_ring: Some(rx_ring),
            tx,
            aggregate_tx,
        }
    }
}

pub(super) struct ProductionAccessPointTask {
    channel: WifiChannel,
    owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    registers: RadioRuntimeOwner,
    interrupts: MacInterruptEpoch,
    service: ProductionAccessPointControl,
    aggregate: ProductionAccessPointAmpdu,
    parked: ProductionAccessPointParked,
}

pub(super) struct ProductionAccessPointPreflightFault {
    _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRuntimeOwner,
    _interrupt_setup: open_esp_radio_esp32s31_hal::MacInterruptSetup,
    _physical: ProductionWifiPhysicalResources,
    _station_role: ProductionStationRoleResources,
    _access_point: ProductionAccessPointResources,
    _monitor: ProductionMonitorResources,
    _detached_control: Option<ControlTx>,
    _ring: Option<ProductionHaltedRx>,
}

pub(super) struct ProductionAccessPointRxOwnerFault {
    _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRuntimeOwner,
    _interrupt_setup: open_esp_radio_esp32s31_hal::MacInterruptSetup,
    _scan_rx: ProductionScanRx,
    _physical: ProductionWifiPhysicalResources,
    _station_role: ProductionStationRoleResources,
    _access_point: ProductionAccessPointResources,
    _monitor: ProductionMonitorResources,
}

pub(super) struct ProductionAccessPointEngineFault {
    _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRuntimeOwner,
    _interrupt_setup: open_esp_radio_esp32s31_hal::MacInterruptSetup,
    _ring: ProductionHaltedRx,
    _transmit: ProductionWifiTxResources,
    _engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineStartFailure<'static>,
    _parked: ProductionAccessPointParked,
    _rx_dispatcher: &'static mut open_esp_radio_esp32s31_wifi_ap::rx::Esp32s31ApRxDispatcher,
    _rx_block_ack: &'static ProductionAccessPointRxBlockAck,
    _rx_reorder: &'static mut ProductionAccessPointRxReorder,
    _rx_reorder_storage: &'static ProductionAccessPointRxReorderStorage,
    #[cfg(feature = "diagnostics")]
    _observation_storage: &'static mut open_esp_radio_esp32s31_wifi_embassy::diagnostics::access_point::AccessPointObservationStorage,
    _rx_frame: &'static mut [u8],
    _tx_frame: &'static mut [u8],
}

pub(super) struct ProductionAccessPointSecurityMaterialFault {
    _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
    _registers: RadioRuntimeOwner,
    _interrupt_setup: open_esp_radio_esp32s31_hal::MacInterruptSetup,
    _ring: ProductionHaltedRx,
    _transmit: ProductionWifiTxResources,
    _parked: ProductionAccessPointParked,
    _beacon: &'static mut [u8; open_esp_radio_ieee80211::beacon::WPA2_BEACON_CAPACITY],
    _peer_storage: &'static mut open_esp_radio_wifi_ap::AccessPointPeerStorage,
    _pairwise_storage:
        &'static mut open_esp_radio_esp32s31_wifi_ap::security::Esp32s31ApPairwiseKeyStorage,
    _rx_dispatcher: &'static mut open_esp_radio_esp32s31_wifi_ap::rx::Esp32s31ApRxDispatcher,
    _rx_block_ack: &'static ProductionAccessPointRxBlockAck,
    _rx_reorder: &'static mut ProductionAccessPointRxReorder,
    _rx_reorder_storage: &'static ProductionAccessPointRxReorderStorage,
    #[cfg(feature = "diagnostics")]
    _observation_storage: &'static mut open_esp_radio_esp32s31_wifi_embassy::diagnostics::access_point::AccessPointObservationStorage,
    _rx_frame: &'static mut [u8],
    _tx_frame: &'static mut [u8],
}

pub(super) enum ProductionAccessPointPreparationFault {
    Preflight {
        _fault: ProductionAccessPointPreflightFault,
    },
    RxOwner {
        _fault: ProductionAccessPointRxOwnerFault,
    },
    SecurityMaterial {
        _fault: ProductionAccessPointSecurityMaterialFault,
    },
    Engine {
        _fault: ProductionAccessPointEngineFault,
    },
}

pub(super) enum ProductionAccessPointTeardownFault {
    Interrupt {
        _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
        _registers: RadioRuntimeOwner,
        _interrupts: MacInterruptEpoch,
        _stopped: ProductionAccessPointStopped,
        _parked: ProductionAccessPointParked,
    },
    Aggregate {
        _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
        _registers: RadioRuntimeOwner,
        _interrupt_setup: open_esp_radio_esp32s31_hal::MacInterruptSetup,
        _stopped: ProductionAccessPointStopped,
        _aggregate: ProductionAccessPointAmpdu,
        _parked: ProductionAccessPointParked,
    },
    TxRestore {
        _owner: Esp32s31WifiRoleOwner<EspHalRadioPeripheral>,
        _registers: RadioRuntimeOwner,
        _interrupt_setup: open_esp_radio_esp32s31_hal::MacInterruptSetup,
        _ring: open_esp_radio_esp32s31_wifi_mac::rx::RxRingHalted<'static, RX_DESCRIPTOR_COUNT>,
        _storage: &'static RxStorage,
        _rx_frame: &'static mut [u8],
        _tx_frame: &'static mut [u8],
        _data_rx: &'static mut open_esp_radio_esp32s31_wifi_ap::rx::Esp32s31ApRxDispatcher,
        _rx_block_ack: &'static ProductionAccessPointRxBlockAck,
        _rx_reorder: &'static mut ProductionAccessPointRxReorder,
        _rx_reorder_storage: &'static ProductionAccessPointRxReorderStorage,
        #[cfg(feature = "diagnostics")]
        _observation_storage: &'static mut open_esp_radio_esp32s31_wifi_embassy::diagnostics::access_point::AccessPointObservationStorage,
        _engine: open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineStop<'static>,
        _parked: ProductionAccessPointParked,
        _returned_control: ControlTx,
    },
}

#[allow(clippy::result_large_err)]
pub(super) fn try_split_wifi_stopped_resources(
    resources: ProductionWifiStoppedResources,
) -> Result<
    (
        ProductionWifiPhysicalResources,
        ProductionStationRoleResources,
    ),
    ProductionWifiStoppedResources,
> {
    let returned = match resources {
        ProductionWifiStoppedResources::Fresh(fresh) => {
            let ProductionWifiFreshResources {
                dma,
                rx_ring,
                tx,
                scan_table,
                scan_frame,
                ethernet,
                network,
                mut board,
                station_address,
            } = fresh;
            let aggregate_tx = board
                .initial_connected
                .as_mut()
                .expect("fresh station owns initial connected resources")
                .take_aggregate();
            return Ok((
                ProductionWifiPhysicalResources {
                    dma,
                    rx_ring,
                    tx,
                    aggregate_tx,
                },
                ProductionStationRoleResources {
                    scan_table,
                    scan_frame,
                    ethernet,
                    resume: ProductionStationRoleResume::Fresh { network },
                    board,
                    station_address,
                },
            ));
        }
        ProductionWifiStoppedResources::Returned(returned) => returned,
    };
    let ProductionWifiReusableResources {
        storage,
        mut board,
        phase,
        security,
        interrupt_route,
        mac_runtime,
        power_runtime,
    } = returned;
    let (dma, tx_epoch, scan_table, scan_frame, ethernet) = storage.into_parts();
    let station_address = board.interface.interface.address;
    let (ring, phase, aggregate_tx) = match phase {
        Esp32s31StationStoppedPhaseResources::InitialScan {
            receive,
            network,
            identity,
        } => match receive.into_halted() {
            Ok(ring) => {
                let aggregate_tx = board
                    .initial_connected
                    .as_mut()
                    .expect("initial scan retains initial connected resources")
                    .take_aggregate();
                (
                    ring,
                    ProductionStationReturnedPhase::InitialScan { network, identity },
                    aggregate_tx,
                )
            }
            Err(receive) => {
                return Err(ProductionWifiStoppedResources::Returned(
                    ProductionWifiReusableResources {
                        storage: ProductionStationStorage::new(
                            dma, tx_epoch, scan_table, scan_frame, ethernet,
                        ),
                        board,
                        phase: Esp32s31StationStoppedPhaseResources::InitialScan {
                            receive,
                            network,
                            identity,
                        },
                        security,
                        interrupt_route,
                        mac_runtime,
                        power_runtime,
                    },
                ));
            }
        },
        Esp32s31StationStoppedPhaseResources::InitialJoin {
            receive,
            network,
            station,
        } => match receive.try_into_halted() {
            Ok(ring) => {
                let aggregate_tx = board
                    .initial_connected
                    .as_mut()
                    .expect("initial join retains initial connected resources")
                    .take_aggregate();
                (
                    ring,
                    ProductionStationReturnedPhase::InitialJoin { network, station },
                    aggregate_tx,
                )
            }
            Err(receive) => {
                return Err(ProductionWifiStoppedResources::Returned(
                    ProductionWifiReusableResources {
                        storage: ProductionStationStorage::new(
                            dma, tx_epoch, scan_table, scan_frame, ethernet,
                        ),
                        board,
                        phase: Esp32s31StationStoppedPhaseResources::InitialJoin {
                            receive,
                            network,
                            station,
                        },
                        security,
                        interrupt_route,
                        mac_runtime,
                        power_runtime,
                    },
                ));
            }
        },
        Esp32s31StationStoppedPhaseResources::Disconnected {
            network,
            receive,
            aggregate_tx,
            control,
            station,
            registers,
        } => {
            let (ring, rx) = receive.into_epoch_parts();
            (
                ring,
                ProductionStationReturnedPhase::Disconnected {
                    network,
                    rx,
                    control,
                    station,
                    registers,
                },
                aggregate_tx,
            )
        }
        Esp32s31StationStoppedPhaseResources::Reconnected {
            network,
            receive,
            rx,
            aggregate_tx,
            control,
            station,
            registers,
        } => match receive.try_into_halted() {
            Ok(ring) => (
                ring,
                ProductionStationReturnedPhase::Reconnected {
                    network,
                    rx,
                    control,
                    station,
                    registers,
                },
                aggregate_tx,
            ),
            Err(receive) => {
                return Err(ProductionWifiStoppedResources::Returned(
                    ProductionWifiReusableResources {
                        storage: ProductionStationStorage::new(
                            dma, tx_epoch, scan_table, scan_frame, ethernet,
                        ),
                        board,
                        phase: Esp32s31StationStoppedPhaseResources::Reconnected {
                            network,
                            receive,
                            rx,
                            aggregate_tx,
                            control,
                            station,
                            registers,
                        },
                        security,
                        interrupt_route,
                        mac_runtime,
                        power_runtime,
                    },
                ));
            }
        },
    };
    Ok((
        ProductionWifiPhysicalResources {
            dma,
            rx_ring: Some(ring),
            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
            aggregate_tx,
        },
        ProductionStationRoleResources {
            scan_table,
            scan_frame,
            ethernet,
            resume: ProductionStationRoleResume::Returned {
                phase,
                security,
                interrupt_route,
                mac_runtime,
                power_runtime,
            },
            board,
            station_address,
        },
    ))
}

pub(super) fn join_station_activation_resources(
    physical: ProductionWifiPhysicalResources,
    station: ProductionStationRoleResources,
) -> ProductionWifiStoppedResources {
    let ProductionWifiPhysicalResources {
        dma,
        rx_ring,
        tx,
        aggregate_tx,
    } = physical;
    let ProductionStationRoleResources {
        scan_table,
        scan_frame,
        ethernet,
        resume,
        mut board,
        station_address,
    } = station;
    match resume {
        ProductionStationRoleResume::Fresh { network } => {
            board
                .initial_connected
                .as_mut()
                .expect("fresh station retains connected resources")
                .restore_aggregate(aggregate_tx);
            ProductionWifiStoppedResources::Fresh(ProductionWifiFreshResources {
                dma,
                rx_ring,
                tx,
                scan_table,
                scan_frame,
                ethernet,
                network,
                board,
                station_address,
            })
        }
        ProductionStationRoleResume::Returned {
            phase,
            security,
            interrupt_route,
            mac_runtime,
            power_runtime,
        } => {
            let ring = rx_ring.expect("returned physical Wi-Fi resources own a halted RX ring");
            let tx_epoch = match tx {
                ProductionOrdinaryTxResources::Epoch(tx_epoch) => tx_epoch,
                ProductionOrdinaryTxResources::Uninitialized(_) => {
                    unreachable!("a returned Wi-Fi epoch owns initialized TX storage")
                }
            };
            let mut aggregate_tx = Some(aggregate_tx);
            if matches!(
                &phase,
                ProductionStationReturnedPhase::InitialScan { .. }
                    | ProductionStationReturnedPhase::InitialJoin { .. }
            ) {
                board
                    .initial_connected
                    .as_mut()
                    .expect("initial station phase retains connected resources")
                    .restore_aggregate(
                        aggregate_tx
                            .take()
                            .expect("the physical frontier owns aggregate storage"),
                    );
            }
            let phase = match phase {
                ProductionStationReturnedPhase::InitialScan { network, identity } => {
                    Esp32s31StationStoppedPhaseResources::InitialScan {
                        receive: Esp32s31ScanRx::from_halted(ring, dma.storage()),
                        network,
                        identity,
                    }
                }
                ProductionStationReturnedPhase::InitialJoin { network, station } => {
                    Esp32s31StationStoppedPhaseResources::InitialJoin {
                        receive: Esp32s31RxFrontier::from_halted(ring),
                        network,
                        station,
                    }
                }
                ProductionStationReturnedPhase::Disconnected {
                    network,
                    rx,
                    control,
                    station,
                    registers,
                } => Esp32s31StationStoppedPhaseResources::Disconnected {
                    network,
                    receive: rx.with_halted_ring(ring),
                    aggregate_tx: aggregate_tx
                        .take()
                        .expect("disconnected phase reclaims AP aggregate owner"),
                    control,
                    station,
                    registers,
                },
                ProductionStationReturnedPhase::Reconnected {
                    network,
                    rx,
                    control,
                    station,
                    registers,
                } => Esp32s31StationStoppedPhaseResources::Reconnected {
                    network,
                    receive: Esp32s31RxFrontier::from_halted(ring),
                    rx,
                    aggregate_tx: aggregate_tx
                        .take()
                        .expect("reconnected phase reclaims AP aggregate owner"),
                    control,
                    station,
                    registers,
                },
            };
            ProductionWifiStoppedResources::Returned(ProductionWifiReusableResources {
                storage: ProductionStationStorage::new(
                    dma, tx_epoch, scan_table, scan_frame, ethernet,
                ),
                board,
                phase,
                security,
                interrupt_route,
                mac_runtime,
                power_runtime,
            })
        }
    }
}

/// Static resources reserved for one exclusive AP epoch.
pub(super) struct ProductionAccessPointResources {
    pub(super) address: [u8; 6],
    pub(super) beacon: &'static mut [u8; open_esp_radio_ieee80211::beacon::WPA2_BEACON_CAPACITY],
    pub(super) rx_frame: &'static mut [u8],
    pub(super) tx_frame: &'static mut [u8],
    pub(super) peer_storage: &'static mut open_esp_radio_wifi_ap::AccessPointPeerStorage,
    pub(super) pairwise_storage:
        &'static mut open_esp_radio_esp32s31_wifi_ap::security::Esp32s31ApPairwiseKeyStorage,
    pub(super) rx_dispatcher:
        &'static mut open_esp_radio_esp32s31_wifi_ap::rx::Esp32s31ApRxDispatcher,
    pub(super) rx_block_ack: &'static ProductionAccessPointRxBlockAck,
    pub(super) rx_reorder: &'static mut Esp32s31AccessPointRxReorder<'static, RX_BUFFER_SIZE>,
    pub(super) rx_reorder_storage:
        &'static open_esp_radio_esp32s31_wifi_embassy::datapath::rx::reorder::RxReorderFrameStorage<
            RX_BUFFER_SIZE,
            {
                open_esp_radio_esp32s31_wifi_embassy::datapath::rx::reorder::RX_REORDER_BACKING_SLOT_COUNT
            },
        >,
    #[cfg(feature = "diagnostics")]
    pub(super) observation_storage: &'static mut open_esp_radio_esp32s31_wifi_embassy::diagnostics::access_point::AccessPointObservationStorage,
}

impl ProductionWifiEpochRunner {
    pub(super) async fn prepare_access_point_task(
        &self,
        wifi: open_esp_radio_esp32s31_wifi::runtime::Esp32s31WifiStopped<EspHalRadioPeripheral>,
        physical: ProductionWifiPhysicalResources,
        station: ProductionStationRoleResources,
        access_point: ProductionAccessPointResources,
        monitor: ProductionMonitorResources,
        request: AccessPointRequest,
    ) -> Result<ProductionAccessPointTask, ProductionAccessPointPreparationFault> {
        let current_channel = wifi.current_channel();
        let mut materialized = materialize_esp32s31_wifi_role(wifi, physical);
        let requested_channel = request.channel();
        if requested_channel != current_channel {
            let lowered_channel = lower_wifi_channel(requested_channel);
            let observer = NoopPhyTargetObserver;
            let (phy, platform) = materialized.owner.radio_mut();
            let mut channel =
                Esp32s31ScanPhy::<_, _, EmbassyEsp32s31PhyDelay>::new(phy, platform, observer);
            if await_stack_boundary!(channel.select_channel(
                lowered_channel.channel_or_frequency,
                lowered_channel.cbw,
                &mut materialized.registers,
            ))
            .is_err()
            {
                return Err(ProductionAccessPointPreparationFault::Preflight {
                    _fault: ProductionAccessPointPreflightFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _physical: materialized.resources,
                        _station_role: station,
                        _access_point: access_point,
                        _monitor: monitor,
                        _detached_control: None,
                        _ring: None,
                    },
                });
            }
            materialized.owner.set_current_channel(requested_channel);
        }

        let ProductionWifiPhysicalResources {
            dma,
            rx_ring,
            tx,
            aggregate_tx,
        } = materialized.resources;
        let ProductionStationRoleResources {
            scan_table,
            scan_frame,
            ethernet,
            resume,
            board,
            station_address,
        } = station;
        let power = materialized.owner.radio_mut().0.tx_target_power_profile();
        let tx_epoch = self.initialize_tx_epoch(tx, power);
        let scan_rx = match rx_ring {
            Some(ring) => Esp32s31ScanRx::from_halted(ring, dma.storage()),
            None => match Esp32s31ScanRx::prepare_initial(
                &mut materialized.registers,
                dma.storage(),
                dma.descriptor_base(),
                dma.buffer_addresses(),
            ) {
                Ok(receive) => receive,
                Err(_) => {
                    return Err(ProductionAccessPointPreparationFault::Preflight {
                        _fault: ProductionAccessPointPreflightFault {
                            _owner: materialized.owner,
                            _registers: materialized.registers,
                            _interrupt_setup: materialized.interrupt_setup,
                            _physical: ProductionWifiPhysicalResources {
                                dma,
                                rx_ring: None,
                                tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                                aggregate_tx,
                            },
                            _station_role: ProductionStationRoleResources {
                                scan_table,
                                scan_frame,
                                ethernet,
                                resume,
                                board,
                                station_address,
                            },
                            _access_point: access_point,
                            _monitor: monitor,
                            _detached_control: None,
                            _ring: None,
                        },
                    });
                }
            },
        };
        let halted = match scan_rx.into_halted() {
            Ok(halted) => halted,
            Err(scan_rx) => {
                return Err(ProductionAccessPointPreparationFault::RxOwner {
                    _fault: ProductionAccessPointRxOwnerFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _scan_rx: scan_rx,
                        _physical: ProductionWifiPhysicalResources {
                            dma,
                            rx_ring: None,
                            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                            aggregate_tx,
                        },
                        _station_role: ProductionStationRoleResources {
                            scan_table,
                            scan_frame,
                            ethernet,
                            resume,
                            board,
                            station_address,
                        },
                        _access_point: access_point,
                        _monitor: monitor,
                    },
                });
            }
        };
        let control = match tx_epoch.take_control() {
            Ok(control) => control,
            Err(_) => {
                return Err(ProductionAccessPointPreparationFault::Preflight {
                    _fault: ProductionAccessPointPreflightFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _physical: ProductionWifiPhysicalResources {
                            dma,
                            rx_ring: None,
                            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                            aggregate_tx,
                        },
                        _station_role: ProductionStationRoleResources {
                            scan_table,
                            scan_frame,
                            ethernet,
                            resume,
                            board,
                            station_address,
                        },
                        _access_point: access_point,
                        _monitor: monitor,
                        _detached_control: None,
                        _ring: Some(halted),
                    },
                });
            }
        };
        let transmit = match control.try_into_resources() {
            Ok(resources) => resources,
            Err(control) => {
                return Err(ProductionAccessPointPreparationFault::Preflight {
                    _fault: ProductionAccessPointPreflightFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _physical: ProductionWifiPhysicalResources {
                            dma,
                            rx_ring: None,
                            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
                            aggregate_tx,
                        },
                        _station_role: ProductionStationRoleResources {
                            scan_table,
                            scan_frame,
                            ethernet,
                            resume,
                            board,
                            station_address,
                        },
                        _access_point: access_point,
                        _monitor: monitor,
                        _detached_control: Some(control),
                        _ring: Some(halted),
                    },
                });
            }
        };

        let (
            ssid,
            security,
            channel,
            client_limit,
            inactive_timeout,
            beacon_interval,
            dtim_period,
        ) = request.into_parts();
        let ProductionAccessPointResources {
            address,
            beacon,
            rx_frame,
            tx_frame,
            peer_storage,
            pairwise_storage,
            rx_dispatcher,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage,
        } = access_point;
        let mut gtk_key = [0_u8; 16];
        for word in gtk_key.chunks_exact_mut(4) {
            word.copy_from_slice(&self.trng.random().to_le_bytes());
        }
        let gtk = match Wpa2Gtk::new(1, true, gtk_key) {
            Ok(gtk) => gtk,
            Err(_) => {
                return Err(ProductionAccessPointPreparationFault::SecurityMaterial {
                    _fault: ProductionAccessPointSecurityMaterialFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _ring: halted,
                        _transmit: transmit,
                        _parked: ProductionAccessPointParked {
                            dma,
                            tx_epoch,
                            station: ProductionStationRoleResources {
                                scan_table,
                                scan_frame,
                                ethernet,
                                resume,
                                board,
                                station_address,
                            },
                            monitor,
                            aggregate_tx: Some(aggregate_tx),
                        },
                        _beacon: beacon,
                        _peer_storage: peer_storage,
                        _pairwise_storage: pairwise_storage,
                        _rx_dispatcher: rx_dispatcher,
                        _rx_block_ack: rx_block_ack,
                        _rx_reorder: rx_reorder,
                        _rx_reorder_storage: rx_reorder_storage,
                        #[cfg(feature = "diagnostics")]
                        _observation_storage: observation_storage,
                        _rx_frame: rx_frame,
                        _tx_frame: tx_frame,
                    },
                });
            }
        };
        let service = AccessPointService::new(
            address,
            security.into_pmk(),
            gtk,
            client_limit,
            inactive_timeout,
            peer_storage,
        );
        let engine = match Esp32s31ApEngine::start(
            &mut materialized.registers,
            service,
            beacon,
            pairwise_storage,
            &ssid,
            channel,
            beacon_interval.tu(),
            dtim_period.get(),
        ) {
            Ok(engine) => engine,
            Err(engine) => {
                return Err(ProductionAccessPointPreparationFault::Engine {
                    _fault: ProductionAccessPointEngineFault {
                        _owner: materialized.owner,
                        _registers: materialized.registers,
                        _interrupt_setup: materialized.interrupt_setup,
                        _ring: halted,
                        _transmit: transmit,
                        _engine: engine,
                        _parked: ProductionAccessPointParked {
                            dma,
                            tx_epoch,
                            station: ProductionStationRoleResources {
                                scan_table,
                                scan_frame,
                                ethernet,
                                resume,
                                board,
                                station_address,
                            },
                            monitor,
                            aggregate_tx: Some(aggregate_tx),
                        },
                        _rx_dispatcher: rx_dispatcher,
                        _rx_block_ack: rx_block_ack,
                        _rx_reorder: rx_reorder,
                        _rx_reorder_storage: rx_reorder_storage,
                        #[cfg(feature = "diagnostics")]
                        _observation_storage: observation_storage,
                        _rx_frame: rx_frame,
                        _tx_frame: tx_frame,
                    },
                });
            }
        };
        let maximum_aggregate_bytes = transmit.policy.ht_ampdu().maximum_aggregate_bytes();
        let aggregate = ProductionAccessPointAmpdu::new(
            aggregate_tx,
            maximum_aggregate_bytes,
            open_esp_radio_esp32s31_wifi_mac::tx_runtime::VENDOR_LONG_RETRY_LIMIT,
        );
        let mac = Esp32s31ApMac::new(
            engine,
            transmit,
            Esp32s31ApTxConfig {
                publication_timeout_micros: TX_COMPLETION_TIMEOUT_US,
            },
        );
        let (receive, protocol_rx) = access_point_rx_pipeline(
            halted,
            dma.storage(),
            #[cfg(feature = "diagnostics")]
            board
                .diagnostics
                .expect("diagnostics AP retains its pipeline observer")
                .rx_pipeline,
        );
        let service = Esp32s31AccessPointControl::new(
            receive,
            protocol_rx,
            mac,
            rx_frame,
            tx_frame,
            rx_dispatcher,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage,
        );
        #[cfg(feature = "diagnostics")]
        let service = service.with_terminal_observer(begin_access_point_observation());
        Ok(ProductionAccessPointTask {
            channel,
            owner: materialized.owner,
            registers: materialized.registers,
            interrupts: mac_interrupt_epoch(materialized.interrupt_setup),
            service,
            aggregate,
            parked: ProductionAccessPointParked {
                dma,
                tx_epoch,
                station: ProductionStationRoleResources {
                    scan_table,
                    scan_frame,
                    ethernet,
                    resume,
                    board,
                    station_address,
                },
                monitor,
                aggregate_tx: None,
            },
        })
    }

    pub(super) fn finish_access_point_task(
        &self,
        task: ProductionAccessPointTask,
    ) -> Result<ProductionSupervisorStopped, ProductionWifiFault> {
        let ProductionAccessPointTask {
            channel,
            owner,
            mut registers,
            interrupts,
            service,
            aggregate,
            parked,
        } = task;
        let stopped = match service.try_finish(&mut registers) {
            Ok(stopped) => stopped,
            Err(service) => {
                return Err(ProductionWifiFault::AccessPointRuntime {
                    _task: ProductionAccessPointTask {
                        channel,
                        owner,
                        registers,
                        interrupts,
                        service,
                        aggregate,
                        parked,
                    },
                });
            }
        };
        let (_route, interrupt_setup, _mac_runtime, _power_runtime) =
            match interrupts.try_into_inactive_parts() {
                Ok(parts) => parts,
                Err(interrupts) => {
                    return Err(ProductionWifiFault::AccessPointTeardown {
                        _fault: ProductionAccessPointTeardownFault::Interrupt {
                            _owner: owner,
                            _registers: registers,
                            _interrupts: interrupts,
                            _stopped: stopped,
                            _parked: parked,
                        },
                    });
                }
            };
        let ProductionAccessPointParked {
            dma,
            tx_epoch,
            station,
            monitor,
            aggregate_tx: parked_aggregate,
        } = parked;
        debug_assert!(parked_aggregate.is_none());
        let aggregate_tx = match aggregate.try_into_resources() {
            Ok(resources) => resources,
            Err(aggregate) => {
                return Err(ProductionWifiFault::AccessPointTeardown {
                    _fault: ProductionAccessPointTeardownFault::Aggregate {
                        _owner: owner,
                        _registers: registers,
                        _interrupt_setup: interrupt_setup,
                        _stopped: stopped,
                        _aggregate: aggregate,
                        _parked: ProductionAccessPointParked {
                            dma,
                            tx_epoch,
                            station,
                            monitor,
                            aggregate_tx: None,
                        },
                    },
                });
            }
        };
        let ring = match stopped.receive.try_into_halted() {
            Ok(ring) => ring,
            Err(_) => unreachable!("completed AP run returns a halted staged-RX producer"),
        };
        if let Err((_error, returned_control)) = tx_epoch.restore_resources(stopped.transmit) {
            return Err(ProductionWifiFault::AccessPointTeardown {
                _fault: ProductionAccessPointTeardownFault::TxRestore {
                    _owner: owner,
                    _registers: registers,
                    _interrupt_setup: interrupt_setup,
                    _ring: ring,
                    _storage: dma.storage(),
                    _rx_frame: stopped.rx_frame,
                    _tx_frame: stopped.tx_frame,
                    _data_rx: stopped.data_rx,
                    _rx_block_ack: stopped.rx_block_ack,
                    _rx_reorder: stopped.rx_reorder,
                    _rx_reorder_storage: stopped.rx_reorder_storage,
                    #[cfg(feature = "diagnostics")]
                    _observation_storage: stopped.observation_storage,
                    _engine: stopped.engine,
                    _parked: ProductionAccessPointParked {
                        dma,
                        tx_epoch,
                        station,
                        monitor,
                        aggregate_tx: Some(aggregate_tx),
                    },
                    _returned_control: returned_control,
                },
            });
        }
        let open_esp_radio_esp32s31_wifi_ap::engine::Esp32s31ApEngineStop {
            service,
            beacon_storage,
            pairwise_storage,
            security: _,
        } = stopped.engine;
        let address = service.address();
        let peer_storage = service.into_peer_storage();
        let access_point = ProductionAccessPointResources {
            address,
            beacon: beacon_storage,
            rx_frame: stopped.rx_frame,
            tx_frame: stopped.tx_frame,
            peer_storage,
            pairwise_storage,
            rx_dispatcher: stopped.data_rx,
            rx_block_ack: stopped.rx_block_ack,
            rx_reorder: stopped.rx_reorder,
            rx_reorder_storage: stopped.rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage: stopped.observation_storage,
        };
        let physical = ProductionWifiPhysicalResources {
            dma,
            rx_ring: Some(ring),
            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
            aggregate_tx,
        };
        let wifi = owner.into_stopped(registers, interrupt_setup, ());
        Ok(Esp32s31WifiSupervisorStopped::new(
            wifi.wifi,
            physical,
            station,
            access_point,
            monitor,
        ))
    }
}

pub(super) async fn wait_for_active_wifi_role_stop(
    endpoint: &mut EmbassyWifiSupervisorEndpoint<'_, CriticalSectionRawMutex, Esp32s31RadioError>,
) {
    loop {
        match endpoint.receive().await {
            EmbassyWifiSupervisorCommand::Stop => return,
            EmbassyWifiSupervisorCommand::Scan(request) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Scan(Err(
                        WifiScanFailure::Rejected {
                            request,
                            error: Esp32s31RadioError::RoleActive(
                                EmbassyWifiStartKind::StandaloneScan,
                            ),
                        },
                    )))
                    .await;
            }
            EmbassyWifiSupervisorCommand::StartStation(request) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Station(Err(
                        WifiStartFailure::rejected(
                            request,
                            Esp32s31RadioError::RoleActive(EmbassyWifiStartKind::Station),
                        ),
                    )))
                    .await;
            }
            EmbassyWifiSupervisorCommand::StartAccessPoint(request) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::AccessPoint(Err(
                        WifiStartFailure::rejected(
                            request,
                            Esp32s31RadioError::RoleActive(EmbassyWifiStartKind::AccessPoint),
                        ),
                    )))
                    .await;
            }
            EmbassyWifiSupervisorCommand::StartStationAccessPoint(request) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::StationAccessPoint(Err(
                        WifiStartFailure::rejected(
                            request,
                            Esp32s31RadioError::RoleActive(
                                EmbassyWifiStartKind::StationAccessPoint,
                            ),
                        ),
                    )))
                    .await;
            }
            EmbassyWifiSupervisorCommand::StartMonitor(request) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Monitor(Err(
                        WifiStartFailure::rejected(
                            request,
                            Esp32s31RadioError::RoleActive(EmbassyWifiStartKind::StandaloneMonitor),
                        ),
                    )))
                    .await;
            }
        }
    }
}
impl ProductionWifiEpochRunner {
    pub(super) async fn run_access_point_service(
        &mut self,
        endpoint: &mut EmbassyWifiSupervisorEndpoint<
            '_,
            CriticalSectionRawMutex,
            Esp32s31RadioError,
        >,
        stopped: ProductionSupervisorStopped,
        request: AccessPointRequest,
        generation: open_esp_radio::RadioSubsystemGeneration,
    ) -> EmbassyWifiRoleEpochOutcome<ProductionSupervisorStopped, ProductionWifiFault> {
        let (wifi, physical, station, access_point, monitor) = stopped.into_parts();
        let mut task = match await_stack_boundary!(self.prepare_access_point_task(
            wifi,
            physical,
            station,
            access_point,
            monitor,
            request,
        )) {
            Ok(task) => task,
            Err(fault) => {
                let faulted = ProductionWifiFault::AccessPointPreparation { _fault: fault };
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::AccessPoint(Err(
                        WifiStartFailure::faulted(self.fault_error(&faulted)),
                    )))
                    .await;
                return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
            }
        };
        endpoint
            .respond(EmbassyWifiSupervisorResponse::AccessPoint(Ok(
                WifiStartReport::new(generation),
            )))
            .await;
        #[cfg(feature = "diagnostics")]
        let rx_statistics_before = task.registers.receive_statistics_snapshot();
        #[cfg(feature = "diagnostics")]
        let rx_policy_before = task.registers.access_point_receive_policy_snapshot();
        #[cfg(feature = "diagnostics")]
        let rx_delivery_observer = task
            .parked
            .station
            .board
            .diagnostics
            .and_then(|hooks| hooks.rx_delivery);
        let result = {
            let network = task.parked.station.resume.radio_runner();
            let (_, platform) = task.owner.radio_mut();
            await_stack_boundary!(
                task.service.run_until_stopped(
                    &mut task.registers,
                    &mut task.interrupts,
                    &*platform,
                    network,
                    &mut task.aggregate,
                    #[cfg(feature = "diagnostics")]
                    task.parked
                        .station
                        .board
                        .diagnostics
                        .map(|hooks| hooks.aggregate_tx),
                    #[cfg(feature = "diagnostics")]
                    rx_delivery_observer,
                    publish_access_point_shared_network_rx,
                    wait_for_active_wifi_role_stop(endpoint),
                    |status| crate::status::publish_access_point_status(
                        generation, status
                    ),
                    || {
                        let mut nonce = [0_u8; 32];
                        for word in nonce.chunks_exact_mut(4) {
                            word.copy_from_slice(&self.trng.random().to_le_bytes());
                        }
                        let replay =
                            (u64::from(self.trng.random()) << 32) | u64::from(self.trng.random());
                        (nonce, replay)
                    },
                )
            )
        };
        crate::status::publish_access_point_stopped();
        #[cfg(feature = "diagnostics")]
        if let Err(error) = &result {
            // Publish the typed owner failure before the larger terminal RX
            // snapshot. A saturated AP can fill the bounded diagnostic log;
            // the cause must not be displaced by secondary state evidence.
            log::error!("open-radio: access-point runtime fault: {error:?}");
        }
        #[cfg(feature = "diagnostics")]
        let (rx_hardware_buffer_full, rx_hardware_fifo_overflow) = {
            // A rare repeated STA/AP lifecycle failure leaves the AP receive
            // path after exactly one completed descriptor. Capture the
            // hardware-owned frontier before teardown republishes the ring;
            // production images compile this one-shot diagnostic out.
            let rx_dma = task.registers.receive_dma_snapshot();
            let rx_statistics_after = task.registers.receive_statistics_snapshot();
            let rx_delta = rx_statistics_after
                .primary
                .wrapping_delta_since(rx_statistics_before.primary);
            let rx_decode_delta = rx_statistics_after
                .decode_errors
                .wrapping_delta_since(rx_statistics_before.decode_errors);
            let rx_hang_delta = rx_statistics_after
                .hang
                .wrapping_delta_since(rx_statistics_before.hang);
            let rx_policy_after = task.registers.access_point_receive_policy_snapshot();
            let rx_match_after = task.registers.he_trigger_receive_diagnostics();
            for index in 0..8 {
                if let Some(snapshot) = task.registers.rx_block_ack_entry_snapshot(index)
                    && snapshot.control & (1 << 30) != 0
                {
                    diagnostics_event!(
                        "open-radio: access-point RX BA bank={} control={:#010x} peer={:02x?} interface={:?} window={} current={} loaded_start={} bitmap_status={:016x} bitmap_load={:016x}",
                        index,
                        snapshot.control,
                        snapshot.peer,
                        snapshot.interface,
                        snapshot.window,
                        snapshot.current_sequence,
                        snapshot.loaded_start_sequence,
                        snapshot.bitmap_status,
                        snapshot.bitmap_load,
                    );
                }
            }
            let rx_head = task.service.rx_descriptor_snapshot(0);
            let rx_second = task.service.rx_descriptor_snapshot(1);
            let rx_tail = task
                .service
                .rx_descriptor_snapshot(RX_DESCRIPTOR_COUNT.saturating_sub(1));
            let descriptor_base_low = rx_head.map(|descriptor| descriptor.address & 0x000f_ffff);
            let descriptor_index = |low: u32| {
                let offset = low.checked_sub(descriptor_base_low?)?;
                // ESP32-S31 Wi-Fi DMA descriptors are exactly three words.
                (offset % 12 == 0)
                    .then(|| usize::try_from(offset / 12).ok())
                    .flatten()
                    .filter(|index| *index < RX_DESCRIPTOR_COUNT)
            };
            let rx_base = descriptor_index(rx_dma.descriptor_base & 0x000f_ffff)
                .and_then(|index| task.service.rx_descriptor_snapshot(index));
            let rx_next = descriptor_index(rx_dma.next_descriptor_low)
                .and_then(|index| task.service.rx_descriptor_snapshot(index));
            let rx_last = descriptor_index(rx_dma.last_descriptor_low)
                .and_then(|index| task.service.rx_descriptor_snapshot(index));
            diagnostics_event!(
                "open-radio: access-point RX DMA stop walker={} reload={} base={:#010x} next={:#07x} last={:#07x}",
                rx_dma.walker_enabled,
                rx_dma.reload_pending,
                rx_dma.descriptor_base,
                rx_dma.next_descriptor_low,
                rx_dma.last_descriptor_low,
            );
            diagnostics_event!(
                "open-radio: access-point RX descriptors head={:?} second={:?} tail={:?}",
                rx_head,
                rx_second,
                rx_tail,
            );
            diagnostics_event!(
                "open-radio: access-point RX hardware descriptors base={:?} next={:?} last={:?}",
                rx_base,
                rx_next,
                rx_last,
            );
            diagnostics_event!(
                "open-radio: access-point RX hardware delta mpdu={} data={} other_unicast={} fcs={} abort={} abort_fcs_pass={} power_drop={} he_sig_b={} same_bm={} signal_field={} end={}",
                rx_delta.mpdu_count,
                rx_delta.data_success,
                rx_delta.other_unicast,
                rx_delta.fcs_error,
                rx_delta.abort,
                rx_delta.abort_fcs_pass,
                rx_delta.power_drop_error,
                rx_delta.he_sig_b_error,
                rx_delta.same_bm_error,
                rx_delta.signal_field,
                rx_delta.end,
            );
            diagnostics_event!(
                "open-radio: access-point RX hardware faults buffer_full={} fifo_overflow={} tkip={} bt_block={} freq_hop={} last_unmatched={} ack_irq={} rts_irq={}",
                rx_delta.buffer_full,
                rx_delta.fifo_overflow,
                rx_delta.tkip_error,
                rx_delta.bt_block_error,
                rx_delta.frequency_hop_error,
                rx_delta.last_unmatched_error,
                rx_delta.ack_interrupt,
                rx_delta.rts_interrupt,
            );
            diagnostics_event!(
                "open-radio: access-point RX policy before={:?} after={:?}",
                rx_policy_before,
                rx_policy_after,
            );
            diagnostics_event!(
                "open-radio: access-point RX match ax_bssid1={} ax_bssid0={} color_valid={} ampdu_auto_ack_valid={}",
                rx_match_after.ax_match_bssid1,
                rx_match_after.ax_match_bssid0,
                rx_match_after.bss_color_valid,
                rx_match_after.rx_ampdu_auto_ack_valid,
            );
            diagnostics_event!(
                "open-radio: access-point RX decode delta brx_agc={} brx={} nrx={} nrx_abort={} nrx_agc_exit={} nrx_baseband_off={} nrx_fdm_watchdog={} nrx_restart={} nrx_service={} nrx_tx_over={} nrx_unsupported={} nrx_he_format={} nrx_ht_sig={} nrx_he_unsupported={} nrx_he_sig_a_crc={} hang_rx={} hang_tx={} rx_tx_hang={} rx_tx_panic={}",
                rx_decode_delta.brx_agc,
                rx_decode_delta.brx,
                rx_decode_delta.nrx,
                rx_decode_delta.nrx_abort,
                rx_decode_delta.nrx_agc_exit,
                rx_decode_delta.nrx_baseband_off,
                rx_decode_delta.nrx_fdm_watchdog,
                rx_decode_delta.nrx_restart,
                rx_decode_delta.nrx_service,
                rx_decode_delta.nrx_tx_over,
                rx_decode_delta.nrx_unsupported,
                rx_decode_delta.nrx_he_format,
                rx_decode_delta.nrx_ht_sig,
                rx_decode_delta.nrx_he_unsupported,
                rx_decode_delta.nrx_he_sig_a_crc,
                rx_hang_delta.rx,
                rx_hang_delta.tx,
                rx_hang_delta.rx_tx_hang,
                rx_hang_delta.rx_tx_panic,
            );
            (rx_delta.buffer_full, rx_delta.fifo_overflow)
        };
        #[cfg(feature = "diagnostics")]
        if let Ok(report) = &result {
            diagnostics_event!(
                "open-radio: access-point RX scheduler stop {:?}",
                report.rx_scheduler,
            );
        }
        if let Err(_error) = result {
            #[cfg(not(feature = "diagnostics"))]
            let _ = _error;
            let faulted = ProductionWifiFault::AccessPointRuntime { _task: task };
            endpoint
                .respond(EmbassyWifiSupervisorResponse::Stop(Err(
                    self.fault_error(&faulted)
                )))
                .await;
            return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
        }
        #[cfg(feature = "diagnostics")]
        let diagnostic_destination = (task.parked.station.board.diagnostics, task.channel);
        let stopped = match self.finish_access_point_task(task) {
            Ok(stopped) => stopped,
            Err(faulted) => {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Stop(Err(
                        self.fault_error(&faulted)
                    )))
                    .await;
                return EmbassyWifiRoleEpochOutcome::Faulted(faulted);
            }
        };
        #[cfg(feature = "diagnostics")]
        if let (Some(hooks), channel) = diagnostic_destination {
            publish_stored_access_point_observation(
                hooks.access_point,
                channel,
                rx_hardware_buffer_full,
                rx_hardware_fifo_overflow,
            );
        }
        endpoint
            .respond(EmbassyWifiSupervisorResponse::Stop(Ok(
                WifiStopReport::new(generation),
            )))
            .await;
        EmbassyWifiRoleEpochOutcome::Stopped(stopped)
    }
}
