//! Production ESP32-S31 station composition.
//!
//! The target owns board allocation and application policy. Every radio
//! transition is supplied by a PAC-backed driver or reusable integration
//! owner; no HIL protocol, telemetry or benchmark configuration is linked.

use core::{future::Future, marker::PhantomData, pin::Pin};

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Instant, Timer};
use esp_hal::{
    efuse::{self, InterfaceMacAddress},
    rng::{Rng, Trng},
};
use open_esp_radio::esp32s31::wifi::dma::tx_storage::TxDmaStorage;
use open_esp_radio::esp32s31::wifi::sta::attempt::{
    Esp32s31StaAttempt, Esp32s31StaAttemptObserver, Esp32s31StaAttemptOutcome,
    Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStage, Esp32s31StaAttemptStation,
};
use open_esp_radio::esp32s31::wifi::sta::channel::Esp32s31ScanPhy;
use open_esp_radio::esp32s31::wifi::sta::scan::{
    Esp32s31StaScanBackend, Esp32s31StaScanConfig, Esp32s31StaScanError,
};
use open_esp_radio::esp32s31::wifi::sta::tx::ControlTxConfig;
use open_esp_radio::esp32s31::wifi::sta::tx_epoch::Esp32s31StaTxEpoch;
use open_esp_radio::{
    RadioConfig, WifiConfig, WifiMacAddress, WifiPlan, WifiStationConfig,
    esp32s31::{
        Esp32s31RadioStartConfig, Esp32s31WifiStartConfig,
        hal::{Radio, RadioRegisters},
        phy::{
            NoopPhyTargetObserver, PhyCalibrationIdentity, PhyTxTargetPowerProfile,
            phy_cold::PhyColdState, phy_rfpll::phy_get_rf_cal_version,
        },
        registers::MacInterruptSetup,
        start_esp32s31_radio,
        wifi::mac::{init::activate_promiscuous_receive, tx::TxSlot},
        wifi::sta::{control_tx::Esp32s31ControlTx, scan_tx::Esp32s31RunningScanTx},
    },
    wifi::{
        ieee80211::{
            channel::WifiChannel,
            scan::{ScanObservation, ScanTable},
            station::{StaAssociationPreference, StaSequenceCounter, StaTxSequenceCounters},
        },
        sta::{
            scan::{StaCandidateScanExit, StaCandidateScanService},
            station::{
                StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition,
                StaLifecycleStage, StaNextCandidate, StaReconnectPolicy,
            },
        },
        wpa2::Pmk,
    },
};
use open_esp_radio_esp32s31_wifi_embassy::{
    phy_delay::EmbassyEsp32s31PhyDelay,
    preconnected_rx::{EmbassyEsp32s31PreconnectedRxDelay, Esp32s31PreconnectedRx},
    rx_dma_service::Esp32s31RxDmaStorage,
    scan_port::{
        EmbassyEsp32s31ScanTimer, Esp32s31ScanPort, Esp32s31ScanPortError, Esp32s31ScanPortParts,
        Esp32s31ScanRadio, Esp32s31ScanStation, Esp32s31ScanStorage,
    },
    scan_rx::{Esp32s31RunningScanRx, Esp32s31ScanFrameObserver, Esp32s31ScanRx},
    scan_target::Esp32s31ColdScanTx,
    sta_attempt_target::{
        Esp32s31StaAttemptRadio, Esp32s31StaAttemptStorage, Esp32s31StaAttemptTargetOwner,
        Esp32s31StaAttemptTargetPort,
    },
    sta_tx_epoch::Esp32s31StaTxEpochExt,
    station::{
        Esp32s31StationAttemptRunner, Esp32s31StationCommandReceiver, Esp32s31StationConfig,
        Esp32s31StationControlResources, Esp32s31StationExit, Esp32s31StationStartResources,
        prepare_esp32s31_station_task,
    },
    station_epoch::Esp32s31RunningScanEpochParts,
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use static_cell::StaticCell;

use crate::connected::{
    ConnectedDisconnectedEpoch, ConnectedHardware, ConnectedReconnectedEpoch,
    ConnectedStationEpoch, ConnectedStationOutcome, ConnectedStationResources, MacInterruptEpoch,
    StationNetwork, initialize_station_network, mac_interrupt_epoch, run_connected,
};

const INITIAL_CHANNEL: u16 = 1;
const SCAN_CHANNELS: [u8; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
const SCAN_DWELL_MS: u16 = 200;
const PROBE_REQUEST_RATES: [u8; 12] = [
    0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
];
const PROBE_TX_DESCRIPTOR_CAPACITY: u32 = 88;
pub(super) const RX_DESCRIPTOR_COUNT: usize = 32;
pub(super) const RX_BUFFER_SIZE: usize = 1_700;
const RX_BUFFER_STORAGE_SIZE: usize = RX_BUFFER_SIZE + 4;
const RX_STAGE_CAPACITY: usize = 1_700;
pub(super) const TX_BUFFER_SIZE: usize = 1_700;
const MAC_HANDSHAKE_SAMPLE_LIMIT: u32 = 100_000;
const TX_COMPLETION_TIMEOUT_US: u64 = 250_000;

const STA_SSID: &str = match option_env!("ESP32S31_WIFI_SSID") {
    Some(value) => value,
    None => "",
};
const STA_PASSPHRASE: &str = match option_env!("ESP32S31_WIFI_PASSPHRASE") {
    Some(value) => value,
    None => "",
};

pub(super) type RxStorage =
    Esp32s31RxDmaStorage<RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
type ControlTx = Esp32s31ControlTx<
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    open_esp_radio_esp32s31_wifi_embassy::tx_time::EmbassyWifiTxTimer,
    TX_BUFFER_SIZE,
>;
pub(super) type TxStorage = Esp32s31StaTxEpoch<ControlTx>;
type StationAttemptChannel<'state> =
    Esp32s31ScanPhy<'state, EspHalRadioPeripheral, NoopPhyTargetObserver, EmbassyEsp32s31PhyDelay>;
type StationAttemptOwner<'hardware, 'transmit, 'state, 'scratch, 'security> =
    Esp32s31StaAttemptTargetOwner<
        'hardware,
        'transmit,
        'static,
        'scratch,
        'security,
        RadioRegisters,
        StationAttemptChannel<'state>,
        EmbassyEsp32s31PreconnectedRxDelay,
        ControlTx,
        (),
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
        RX_BUFFER_STORAGE_SIZE,
    >;
type ReconnectedStationAttemptOwner<'hardware, 'transmit, 'state, 'scratch, 'security> =
    Esp32s31StaAttemptTargetOwner<
        'hardware,
        'transmit,
        'static,
        'scratch,
        'security,
        ConnectedHardware,
        StationAttemptChannel<'state>,
        EmbassyEsp32s31PreconnectedRxDelay,
        ControlTx,
        (),
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
        RX_BUFFER_STORAGE_SIZE,
    >;

static RX_DMA_STORAGE: StaticCell<RxStorage> = StaticCell::new();
static RX_BUFFER_ADDRESSES: StaticCell<[u32; RX_DESCRIPTOR_COUNT]> = StaticCell::new();
static TX_DMA_STORAGE: StaticCell<TxDmaStorage<TX_BUFFER_SIZE>> = StaticCell::new();
static TX_SLOT_STORAGE: StaticCell<TxSlot<TX_BUFFER_SIZE>> = StaticCell::new();
static TX_STATE: StaticCell<TxStorage> = StaticCell::new();
static SCAN_TABLE: StaticCell<ScanTable> = StaticCell::new();
static SCAN_FRAME: StaticCell<[u8; RX_STAGE_CAPACITY]> = StaticCell::new();
static STATION_CONTROL: StaticCell<Esp32s31StationControlResources<CriticalSectionRawMutex>> =
    StaticCell::new();

#[derive(Clone, Copy, Debug, Default)]
struct ProductionScanObserver;

impl Esp32s31ScanFrameObserver for ProductionScanObserver {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {}
}

#[derive(Clone, Copy, Debug, Default)]
struct ProductionAttemptObserver;

impl Esp32s31StaAttemptObserver for ProductionAttemptObserver {
    fn stage_started(&mut self, stage: Esp32s31StaAttemptStage) {
        esp_println::println!("open-radio: attempt stage={stage:?} state=start");
    }

    fn stage_completed(&mut self, stage: Esp32s31StaAttemptStage) {
        esp_println::println!("open-radio: attempt stage={stage:?} state=complete");
    }
}

fn tx_entropy() -> u32 {
    Rng::new().random()
}

enum ProductionStationPhase {
    Initial {
        hardware: RadioRegisters,
        receive: Esp32s31PreconnectedRx<
            'static,
            EmbassyEsp32s31PreconnectedRxDelay,
            RX_DESCRIPTOR_COUNT,
            RX_BUFFER_SIZE,
        >,
        network: StationNetwork,
    },
    RunningScan(ConnectedDisconnectedEpoch),
    Reconnected {
        epoch: ConnectedReconnectedEpoch,
        network: StationNetwork,
    },
}

struct ProductionStationOwner<'state, 'security> {
    phase: ProductionStationPhase,
    phy: &'state mut PhyColdState,
    platform: &'state mut EspHalRadioPeripheral,
    rx_storage: &'static RxStorage,
    tx_storage: &'static mut TxStorage,
    scan_table: &'static mut ScanTable,
    frame: &'static mut [u8],
    station: Esp32s31StaAttemptStation,
    security: Esp32s31StaAttemptSecurity<'security>,
    ethernet: Option<&'static mut [u8]>,
}

struct ProductionStationRunner<O> {
    spawner: Spawner,
    interrupt_epoch: MacInterruptEpoch,
    wifi: WifiPlan,
    _owner: PhantomData<fn() -> O>,
}

impl<O> ProductionStationRunner<O> {
    fn new(spawner: Spawner, interrupt_setup: MacInterruptSetup, wifi: WifiPlan) -> Self {
        Self {
            spawner,
            interrupt_epoch: mac_interrupt_epoch(interrupt_setup),
            wifi,
            _owner: PhantomData,
        }
    }
}

impl<'state, 'security> ProductionStationRunner<ProductionStationOwner<'state, 'security>> {
    async fn run_connected_epoch(
        &mut self,
        owner: ProductionStationOwner<'state, 'security>,
        peer: open_esp_radio::esp32s31::wifi::sta::peer::Esp32s31ConnectedStaPeer,
        pairwise: open_esp_radio::esp32s31::wifi::mac::crypto::StaPairwiseCcmpSlot,
        group: open_esp_radio::esp32s31::wifi::mac::crypto::StaGroupCcmpSlot,
        control: &mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> StaAttemptOutcome<ProductionStationOwner<'state, 'security>, Esp32s31StaAttemptStage> {
        let ProductionStationOwner {
            phase,
            phy,
            platform,
            rx_storage,
            tx_storage,
            scan_table,
            frame,
            station,
            security,
            ethernet,
        } = owner;
        let (epoch, network) = match phase {
            ProductionStationPhase::Initial {
                hardware,
                receive,
                network,
            } => (
                ConnectedStationEpoch::Initial { hardware, receive },
                network,
            ),
            ProductionStationPhase::Reconnected { epoch, network } => {
                (ConnectedStationEpoch::Reconnected(epoch), network)
            }
            ProductionStationPhase::RunningScan(_) => {
                panic!("running scan owner cannot enter a connected epoch")
            }
        };
        let returned = run_connected(
            self.spawner,
            &mut self.interrupt_epoch,
            control,
            ConnectedStationResources {
                wifi: self.wifi,
                epoch,
                network,
                phy,
                platform,
                rx_storage,
                tx_storage,
                frame,
                peer,
                pairwise,
                group,
                pmk: security.pmk,
                sequences: &mut *security.sequences,
                ethernet,
            },
        )
        .await;
        let owner = ProductionStationOwner {
            phase: ProductionStationPhase::RunningScan(returned.disconnected),
            phy,
            platform,
            rx_storage,
            tx_storage,
            scan_table,
            frame: returned.frame,
            station,
            security,
            ethernet: Some(returned.ethernet),
        };
        match returned.outcome {
            ConnectedStationOutcome::Disconnected | ConnectedStationOutcome::ReconnectRequested => {
                StaAttemptOutcome::Disconnected {
                    owner,
                    next_candidate: StaNextCandidate::Refresh,
                }
            }
            ConnectedStationOutcome::StationStopped(_) => StaAttemptOutcome::Stopped { owner },
        }
    }
}

impl<'state, 'security> Esp32s31StationAttemptRunner<CriticalSectionRawMutex>
    for ProductionStationRunner<ProductionStationOwner<'state, 'security>>
{
    type Owner = ProductionStationOwner<'state, 'security>;
    type Error = open_esp_radio::esp32s31::wifi::sta::attempt::Esp32s31StaAttemptStage;

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
        control: &'a mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + 'a {
        async move {
            esp_println::println!(
                "open-radio: station lifecycle attempt generation={} attempt={}",
                context.generation,
                context.attempt
            );
            let mut owner = owner;
            loop {
                let ProductionStationOwner {
                    phase,
                    phy,
                    platform,
                    rx_storage,
                    tx_storage,
                    scan_table,
                    frame,
                    station,
                    security,
                    ethernet,
                } = owner;
                let outcome = match phase {
                    ProductionStationPhase::Initial {
                        mut hardware,
                        receive,
                        network,
                    } => {
                        let attempt_owner: StationAttemptOwner<'_, '_, '_, '_, '_> =
                            Esp32s31StaAttemptTargetOwner::new(
                                Esp32s31StaAttemptRadio::new(
                                    &mut hardware,
                                    Esp32s31ScanPhy::<_, _, EmbassyEsp32s31PhyDelay>::new(
                                        phy,
                                        platform,
                                        NoopPhyTargetObserver,
                                    ),
                                    receive,
                                    rx_storage,
                                    tx_storage
                                        .control_mut()
                                        .expect("station attempt owns ordinary TX"),
                                ),
                                Esp32s31StaAttemptStorage::new(frame),
                                station,
                                security,
                            );
                        let mut attempt =
                            Esp32s31StaAttempt::with_observer(
                                Esp32s31StaAttemptTargetPort::<
                                    StationAttemptOwner<'_, '_, '_, '_, '_>,
                                >::new(),
                                ProductionAttemptObserver,
                            );
                        match attempt.run(attempt_owner).await {
                            Esp32s31StaAttemptOutcome::Failed(failure) => {
                                let (owner, stage, disposition, error, progress) =
                                    failure.into_parts();
                                esp_println::println!(
                                    "open-radio: station attempt failed stage={stage:?} \
                                 disposition={disposition:?} completed={} error={error:?}",
                                    progress.completed_count()
                                );
                                let (radio, storage, station, security) = owner.into_parts();
                                let Esp32s31StaAttemptRadio {
                                    hardware: _returned_hardware,
                                    channel,
                                    receive,
                                    rx_storage,
                                    transmit: _,
                                } = radio;
                                let (phy, platform, _) = channel.into_parts();
                                StaAttemptOutcome::Failed {
                                    owner: ProductionStationOwner {
                                        phase: ProductionStationPhase::Initial {
                                            hardware,
                                            receive,
                                            network,
                                        },
                                        phy,
                                        platform,
                                        rx_storage,
                                        tx_storage,
                                        scan_table,
                                        frame: storage.frame,
                                        station,
                                        security,
                                        ethernet,
                                    },
                                    failure: StaAttemptFailure::new(
                                        stage.lifecycle_stage(),
                                        disposition,
                                        stage,
                                    ),
                                }
                            }
                            Esp32s31StaAttemptOutcome::Connected {
                                connected,
                                progress,
                            } => {
                                let connected_owner = connected.into_owner();
                                let report = connected_owner.report();
                                esp_println::println!(
                                    "open-radio: station joined phases={} auth={} assoc={} wpa2={} m4={}",
                                    progress.completed_count(),
                                    report.authentication.is_some(),
                                    report.association.is_some(),
                                    report.wpa2.is_some(),
                                    report.message4.is_some()
                                );
                                let mut connected_owner = connected_owner;
                                let peer = connected_owner
                                    .take_connected_peer()
                                    .expect("successful station attempt owns its peer");
                                let (pairwise, group) = connected_owner
                                    .take_installed_keys()
                                    .expect("successful station attempt owns both CCMP keys");
                                let (radio, storage, station, security) =
                                    connected_owner.into_parts();
                                let Esp32s31StaAttemptRadio {
                                    hardware: _returned_hardware,
                                    channel,
                                    receive,
                                    rx_storage,
                                    transmit: _,
                                } = radio;
                                let (phy, platform, _) = channel.into_parts();
                                self.run_connected_epoch(
                                    ProductionStationOwner {
                                        phase: ProductionStationPhase::Initial {
                                            hardware,
                                            receive,
                                            network,
                                        },
                                        phy,
                                        platform,
                                        rx_storage,
                                        tx_storage,
                                        scan_table,
                                        frame: storage.frame,
                                        station,
                                        security,
                                        ethernet,
                                    },
                                    peer,
                                    pairwise,
                                    group,
                                    control,
                                )
                                .await
                            }
                        }
                    }
                    ProductionStationPhase::Reconnected {
                        epoch: mut reconnect,
                        network,
                    } => {
                        let (hardware, receive_slot) = reconnect.hardware_and_rx_mut();
                        let receive = match receive_slot.take() {
                            Ok(receive) => receive,
                            Err(_) => {
                                return StaAttemptOutcome::Failed {
                                owner: ProductionStationOwner {
                                    phase: ProductionStationPhase::Reconnected {
                                        epoch: reconnect,
                                        network,
                                    },
                                    phy,
                                    platform,
                                    rx_storage,
                                    tx_storage,
                                    scan_table,
                                    frame,
                                    station,
                                    security,
                                    ethernet,
                                },
                                failure: StaAttemptFailure::new(
                                    open_esp_radio::wifi::sta::station::StaLifecycleStage::Hardware,
                                    open_esp_radio::wifi::sta::station::StaFailureDisposition::Terminal,
                                    Esp32s31StaAttemptStage::Candidate,
                                ),
                            };
                            }
                        };
                        let attempt_owner: ReconnectedStationAttemptOwner<'_, '_, '_, '_, '_> =
                            Esp32s31StaAttemptTargetOwner::new(
                                Esp32s31StaAttemptRadio::new(
                                    hardware,
                                    Esp32s31ScanPhy::<_, _, EmbassyEsp32s31PhyDelay>::new(
                                        phy,
                                        platform,
                                        NoopPhyTargetObserver,
                                    ),
                                    receive,
                                    rx_storage,
                                    tx_storage
                                        .control_mut()
                                        .expect("station attempt owns ordinary TX"),
                                ),
                                Esp32s31StaAttemptStorage::new(frame),
                                station,
                                security,
                            );
                        let mut attempt = Esp32s31StaAttempt::with_observer(
                            Esp32s31StaAttemptTargetPort::<
                                ReconnectedStationAttemptOwner<'_, '_, '_, '_, '_>,
                            >::new(),
                            ProductionAttemptObserver,
                        );
                        match attempt.run(attempt_owner).await {
                            Esp32s31StaAttemptOutcome::Failed(failure) => {
                                let (owner, stage, disposition, error, progress) =
                                    failure.into_parts();
                                esp_println::println!(
                                    "open-radio: reconnect attempt failed stage={stage:?} \
                                 disposition={disposition:?} completed={} error={error:?}",
                                    progress.completed_count()
                                );
                                let (radio, storage, station, security) = owner.into_parts();
                                let Esp32s31StaAttemptRadio {
                                    hardware: _,
                                    channel,
                                    receive,
                                    rx_storage,
                                    transmit: _,
                                } = radio;
                                let (phy, platform, _) = channel.into_parts();
                                let (_, receive_slot) = reconnect.hardware_and_rx_mut();
                                *receive_slot = receive;
                                StaAttemptOutcome::Failed {
                                    owner: ProductionStationOwner {
                                        phase: ProductionStationPhase::Reconnected {
                                            epoch: reconnect,
                                            network,
                                        },
                                        phy,
                                        platform,
                                        rx_storage,
                                        tx_storage,
                                        scan_table,
                                        frame: storage.frame,
                                        station,
                                        security,
                                        ethernet,
                                    },
                                    failure: StaAttemptFailure::new(
                                        stage.lifecycle_stage(),
                                        disposition,
                                        stage,
                                    ),
                                }
                            }
                            Esp32s31StaAttemptOutcome::Connected {
                                connected,
                                progress,
                            } => {
                                let connected_owner = connected.into_owner();
                                let report = connected_owner.report();
                                esp_println::println!(
                                    "open-radio: station rejoined phases={} auth={} assoc={} wpa2={} m4={}",
                                    progress.completed_count(),
                                    report.authentication.is_some(),
                                    report.association.is_some(),
                                    report.wpa2.is_some(),
                                    report.message4.is_some()
                                );
                                let mut connected_owner = connected_owner;
                                let peer = connected_owner
                                    .take_connected_peer()
                                    .expect("successful reconnect owns its peer");
                                let (pairwise, group) = connected_owner
                                    .take_installed_keys()
                                    .expect("successful reconnect owns both CCMP keys");
                                let (radio, storage, station, security) =
                                    connected_owner.into_parts();
                                let Esp32s31StaAttemptRadio {
                                    hardware: _,
                                    channel,
                                    receive,
                                    rx_storage,
                                    transmit: _,
                                } = radio;
                                let (phy, platform, _) = channel.into_parts();
                                let (_, receive_slot) = reconnect.hardware_and_rx_mut();
                                *receive_slot = receive;
                                self.run_connected_epoch(
                                    ProductionStationOwner {
                                        phase: ProductionStationPhase::Reconnected {
                                            epoch: reconnect,
                                            network,
                                        },
                                        phy,
                                        platform,
                                        rx_storage,
                                        tx_storage,
                                        scan_table,
                                        frame: storage.frame,
                                        station,
                                        security,
                                        ethernet,
                                    },
                                    peer,
                                    pairwise,
                                    group,
                                    control,
                                )
                                .await
                            }
                        }
                    }
                    ProductionStationPhase::RunningScan(disconnected) => {
                        if !context.refresh_candidate {
                            return StaAttemptOutcome::Failed {
                                owner: ProductionStationOwner {
                                    phase: ProductionStationPhase::RunningScan(disconnected),
                                    phy,
                                    platform,
                                    rx_storage,
                                    tx_storage,
                                    scan_table,
                                    frame,
                                    station,
                                    security,
                                    ethernet,
                                },
                                failure: StaAttemptFailure::new(
                                    StaLifecycleStage::CandidateSelection,
                                    StaFailureDisposition::Terminal,
                                    Esp32s31StaAttemptStage::Candidate,
                                ),
                            };
                        }

                        let Esp32s31RunningScanEpochParts {
                            retained,
                            hardware,
                            rx,
                        } = disconnected.into_running_scan_parts();
                        let control = tx_storage
                            .take_control()
                            .expect("connected teardown returned the ordinary TX owner");
                        let interrupt_setup = self
                            .interrupt_epoch
                            .setup()
                            .expect("running scan requires a quiesced interrupt epoch");
                        let scan_owner = Esp32s31ScanPort::new(
                            Esp32s31ScanRadio::new(
                                Esp32s31ScanPhy::<_, _, EmbassyEsp32s31PhyDelay>::new(
                                    phy,
                                    platform,
                                    NoopPhyTargetObserver,
                                ),
                                hardware,
                                Esp32s31RunningScanRx::from_stopped(rx),
                                Esp32s31RunningScanTx::new(control, interrupt_setup),
                            ),
                            Esp32s31ScanStorage::new(
                                scan_table,
                                frame,
                                ProductionScanObserver,
                                security.sequences.non_qos_mut(),
                            ),
                            Esp32s31ScanStation::new(
                                station.station_address,
                                STA_SSID.as_bytes(),
                                &PROBE_REQUEST_RATES,
                            )
                            .with_descriptor_capacity(PROBE_TX_DESCRIPTOR_CAPACITY),
                            EmbassyEsp32s31ScanTimer,
                        );
                        let scan_config = Esp32s31StaScanConfig::new(SCAN_DWELL_MS)
                            .expect("running scan dwell must be nonzero");
                        let mut scan =
                            StaCandidateScanService::new(Esp32s31StaScanBackend::new(scan_config));
                        let scan_started = Instant::now();
                        let (scan_owner, scan_result) = match scan
                            .run(scan_owner, &SCAN_CHANNELS)
                            .await
                        {
                            StaCandidateScanExit::Selected {
                                owner,
                                candidate,
                                progress,
                            } => {
                                esp_println::println!(
                                    "open-radio: running scan selected after {} channels in {} ms \
                                     bssid={:02x?} channel={} rssi={}",
                                    progress.channels_completed,
                                    scan_started.elapsed().as_millis(),
                                    candidate.bssid,
                                    candidate.channel,
                                    candidate.rssi,
                                );
                                (owner, Ok(candidate))
                            }
                            StaCandidateScanExit::NoCandidate { owner, progress } => {
                                esp_println::println!(
                                    "open-radio: running scan found no candidate after {} channels",
                                    progress.channels_completed,
                                );
                                (owner, Err(Some(StaFailureDisposition::RefreshCandidate)))
                            }
                            StaCandidateScanExit::Stopped { owner, progress } => {
                                esp_println::println!(
                                    "open-radio: running scan stopped after {} channels",
                                    progress.channels_completed,
                                );
                                (owner, Err(None))
                            }
                            StaCandidateScanExit::Failed {
                                owner,
                                error,
                                progress,
                            } => {
                                esp_println::println!(
                                    "open-radio: running scan failed after {} channels: {error:?}",
                                    progress.channels_completed,
                                );
                                let disposition = if matches!(
                                    error,
                                    Esp32s31StaScanError::ActiveProbe(
                                        Esp32s31ScanPortError::Transmit(_)
                                    ) | Esp32s31StaScanError::ReceiveStop(_)
                                ) {
                                    StaFailureDisposition::Terminal
                                } else {
                                    StaFailureDisposition::RefreshCandidate
                                };
                                (owner, Err(Some(disposition)))
                            }
                            StaCandidateScanExit::InvalidPlan { owner, error, .. } => {
                                esp_println::println!(
                                    "open-radio: invalid running scan plan: {error:?}"
                                );
                                (owner, Err(Some(StaFailureDisposition::Terminal)))
                            }
                        };

                        let Esp32s31ScanPortParts {
                            phy: scan_phy,
                            hardware,
                            rx,
                            tx,
                            table: scan_table,
                            frame,
                            telemetry,
                            ..
                        } = scan_owner.into_parts();
                        let (phy, platform, _) = scan_phy.into_parts();
                        let rx = rx.into_stopped().unwrap_or_else(|rx| {
                            panic!(
                                "running scan did not return a halted RX owner: {:?}",
                                rx.phase()
                            )
                        });
                        let (control, tx_summary) = tx.into_parts();
                        tx_storage.restore_control(control).unwrap_or_else(|_| {
                            panic!("running scan returned over a live TX owner")
                        });
                        esp_println::println!(
                            "open-radio: running scan owners returned raw={} rings={} tx={} failures={}",
                            telemetry.raw_frames,
                            telemetry.ring_epochs,
                            tx_summary.completions,
                            tx_summary.failures,
                        );
                        let disconnected = retained.restore(hardware, rx);
                        let returned_owner = |phase, access_point| {
                            let mut station = station;
                            station.access_point = access_point;
                            ProductionStationOwner {
                                phase,
                                phy,
                                platform,
                                rx_storage,
                                tx_storage,
                                scan_table,
                                frame,
                                station,
                                security,
                                ethernet,
                            }
                        };
                        match scan_result {
                            Ok(candidate) => {
                                let (network, epoch) = disconnected
                                    .prepare_reconnect::<EmbassyEsp32s31PreconnectedRxDelay>(
                                );
                                owner = returned_owner(
                                    ProductionStationPhase::Reconnected {
                                        epoch,
                                        network: StationNetwork::Running(network),
                                    },
                                    candidate,
                                );
                                continue;
                            }
                            Err(None) => StaAttemptOutcome::Stopped {
                                owner: returned_owner(
                                    ProductionStationPhase::RunningScan(disconnected),
                                    station.access_point,
                                ),
                            },
                            Err(Some(disposition)) => StaAttemptOutcome::Failed {
                                owner: returned_owner(
                                    ProductionStationPhase::RunningScan(disconnected),
                                    station.access_point,
                                ),
                                failure: StaAttemptFailure::new(
                                    StaLifecycleStage::CandidateSelection,
                                    disposition,
                                    Esp32s31StaAttemptStage::Candidate,
                                ),
                            },
                        }
                    }
                };
                return outcome;
            }
        }
    }
}

pub async fn run(spawner: Spawner, platform: EspHalRadioPeripheral, trng: Trng) -> ! {
    esp_println::println!("open-radio: cold PHY start");

    let efuse_registers = esp_hal::peripherals::EFUSE::regs();
    let mut station_address = [0; 6];
    station_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let mut access_point_address = [0; 6];
    access_point_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    let station_mac = WifiMacAddress::new(station_address)
        .expect("ESP32-S31 eFuse must contain a unicast station address");
    let access_point_mac = WifiMacAddress::new(access_point_address)
        .expect("ESP32-S31 eFuse must contain a unicast access-point address");
    let topology = RadioConfig::wifi(WifiConfig::station(WifiStationConfig::new(station_mac)));

    let owned = Radio::claim(platform)
        .unwrap_or_else(|_| panic!("open-radio register singleton was already claimed"));
    let calibration_identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        mac_sys0: efuse_registers.rd_mac_sys0().read().bits(),
        mac_sys1: efuse_registers.rd_mac_sys1().read().bits(),
    };
    let started = start_esp32s31_radio::<_, EmbassyEsp32s31PhyDelay, _>(
        owned,
        Esp32s31RadioStartConfig::new(
            topology,
            Esp32s31WifiStartConfig::new(
                calibration_identity,
                WifiChannel::mhz20(INITIAL_CHANNEL as u8)
                    .expect("the fixed initial station channel is valid"),
            ),
        ),
        None,
        NoopPhyTargetObserver,
    )
    .await
    .unwrap_or_else(|_| panic!("ESP32-S31 radio start failed"));
    let station = started
        .try_into_station()
        .unwrap_or_else(|_| panic!("validated radio topology did not materialize a station"));
    let station = station
        .start_mac(MAC_HANDSHAKE_SAMPLE_LIMIT, access_point_mac)
        .unwrap_or_else(|_| panic!("cold MAC initialization failed"));
    station_address = station.interface().interface.address;
    let (wifi_plan, mac) = station.into_parts();
    let report = mac.report();
    let (powered, mut phy, _calibration_record, _) = mac.into_parts();
    let (mut platform, mut registers) = powered.into_parts();
    esp_println::println!(
        "open-radio: cold PHY ready, full_calibration={}",
        report.wifi.registration.full_calibration_performed
    );

    if STA_SSID.is_empty() || STA_PASSPHRASE.is_empty() {
        esp_println::println!("open-radio: set ESP32S31_WIFI_SSID and ESP32S31_WIFI_PASSPHRASE");
        loop {
            Timer::after_secs(60).await;
        }
    }

    let rx_storage = RX_DMA_STORAGE.init_with(RxStorage::new);
    let buffer_addresses = RX_BUFFER_ADDRESSES.init([0; RX_DESCRIPTOR_COUNT]);
    let descriptor_base = rx_storage
        .dma_layout(buffer_addresses)
        .expect("RX DMA storage must be addressable by ESP32-S31");
    let tx_dma = TxDmaStorage::pin_static(TX_DMA_STORAGE.init_with(TxDmaStorage::new))
        .expect("TX DMA storage must be addressable by ESP32-S31");
    let tx_slot = Pin::static_mut(TX_SLOT_STORAGE.init(TxSlot::from_dma(tx_dma)));
    let tx_storage = TX_STATE.init(TxStorage::from_slot(
        tx_slot,
        phy.tx_target_power_profile(),
        tx_entropy as fn() -> u32,
        open_esp_radio_esp32s31_wifi_embassy::tx_time::EmbassyWifiTxTimer,
        ControlTxConfig {
            unicast_attempt_limit: 4,
            completion_timeout_us: TX_COMPLETION_TIMEOUT_US,
            poll_interval_us: 1,
        },
    ));

    activate_promiscuous_receive(&mut registers);
    let cold_interrupt_mask = registers.mac_interrupt_enable();
    registers.mask_and_clear_mac_interrupts(u32::MAX);
    let scan_rx = Esp32s31ScanRx::prepare_initial(
        &mut registers,
        rx_storage,
        descriptor_base,
        buffer_addresses,
    )
    .unwrap_or_else(|error| panic!("initial RX DMA ring failed: {error:?}"));
    esp_println::println!(
        "open-radio: MAC ready handshake_samples={} interrupt_mask={:#010x}",
        report.mac.handshake_samples,
        cold_interrupt_mask
    );

    let scan_table = SCAN_TABLE.init(ScanTable::new());
    scan_table.clear();
    let scan_frame = SCAN_FRAME.init([0; RX_STAGE_CAPACITY]);
    let mut scan_sequence = StaSequenceCounter::new(1);
    let scan_tx = Esp32s31ColdScanTx::new(
        tx_storage
            .take_control()
            .expect("cold scan owns the initial control TX owner"),
    );
    let mut scan_owner = Esp32s31ScanPort::new(
        Esp32s31ScanRadio::new(
            Esp32s31ScanPhy::<_, _, EmbassyEsp32s31PhyDelay>::new(
                &mut phy,
                &mut platform,
                NoopPhyTargetObserver,
            ),
            registers,
            scan_rx,
            scan_tx,
        ),
        Esp32s31ScanStorage::new(
            scan_table,
            scan_frame,
            ProductionScanObserver,
            &mut scan_sequence,
        ),
        Esp32s31ScanStation::new(station_address, STA_SSID.as_bytes(), &PROBE_REQUEST_RATES)
            .with_descriptor_capacity(PROBE_TX_DESCRIPTOR_CAPACITY),
        EmbassyEsp32s31ScanTimer,
    );
    let scan_config =
        Esp32s31StaScanConfig::new(SCAN_DWELL_MS).expect("scan dwell must be nonzero");
    let mut scan = StaCandidateScanService::new(Esp32s31StaScanBackend::new(scan_config));
    let (scan_owner, candidate) = loop {
        match scan.run(scan_owner, &SCAN_CHANNELS).await {
            StaCandidateScanExit::Selected {
                owner, candidate, ..
            } => break (owner, candidate),
            StaCandidateScanExit::NoCandidate { owner, progress } => {
                esp_println::println!(
                    "open-radio: scan found no candidate after {} channels; retrying",
                    progress.channels_completed
                );
                scan_owner = owner;
                Timer::after_millis(500).await;
            }
            StaCandidateScanExit::Stopped { .. } => panic!("active scan was stopped"),
            StaCandidateScanExit::Failed { error, .. } => {
                panic!("active scan failed: {error:?}")
            }
            StaCandidateScanExit::InvalidPlan { error, .. } => {
                panic!("invalid active scan plan: {error:?}")
            }
        }
    };
    let Esp32s31ScanPortParts {
        phy: scan_phy,
        hardware: registers,
        rx: scan_rx,
        tx: scan_tx,
        table,
        frame: scan_frame,
        telemetry,
        ..
    } = scan_owner.into_parts();
    let (phy, platform, _) = scan_phy.into_parts();
    let (control_tx, tx_summary) = scan_tx.into_parts();
    tx_storage
        .restore_control(control_tx)
        .unwrap_or_else(|_| panic!("cold scan returned over a live TX owner"));
    let halted_rx = scan_rx
        .into_halted()
        .unwrap_or_else(|_| panic!("cold scan did not return a halted RX ring"));
    let summary = table.summary();
    esp_println::println!(
        "open-radio: scan selected bssid={:02x?} channel={} rssi={} records={} raw={} tx={}",
        candidate.bssid,
        candidate.channel,
        candidate.rssi,
        summary.records,
        telemetry.raw_frames,
        tx_summary.completions
    );

    let pmk_started = Instant::now();
    let pmk = Pmk::derive(STA_PASSPHRASE.as_bytes(), STA_SSID.as_bytes())
        .unwrap_or_else(|error| panic!("invalid station credentials: {error:?}"));
    esp_println::println!(
        "open-radio: PMK derived in {} ms",
        pmk_started.elapsed().as_millis()
    );
    let mut supplicant_nonce = [0; 32];
    for word in supplicant_nonce.chunks_exact_mut(4) {
        word.copy_from_slice(&trng.random().to_le_bytes());
    }
    let mut sequences = StaTxSequenceCounters::new((trng.random() & 0x0fff) as u16);

    let (running_registers, interrupt_setup) = registers.into_running();
    let owner = ProductionStationOwner {
        phase: ProductionStationPhase::Initial {
            hardware: running_registers,
            receive: Esp32s31PreconnectedRx::<
                EmbassyEsp32s31PreconnectedRxDelay,
                RX_DESCRIPTOR_COUNT,
                RX_BUFFER_SIZE,
            >::from_halted(halted_rx),
            network: initialize_station_network(station_address),
        },
        phy,
        platform,
        rx_storage,
        tx_storage,
        scan_table: table,
        frame: scan_frame,
        station: Esp32s31StaAttemptStation {
            station_address,
            access_point: candidate,
            association_preference: StaAssociationPreference::PreferHe20,
        },
        security: Esp32s31StaAttemptSecurity {
            pmk: &pmk,
            supplicant_nonce,
            sequences: &mut sequences,
            message4_protection: open_esp_radio::esp32s31::wifi::sta::wpa2::Esp32s31Wpa2Message4Protection::Unprotected,
        },
        ethernet: None,
    };
    let policy = StaReconnectPolicy::new(3, 100, 1_000, 100)
        .expect("production reconnect policy must be valid");
    let station_control = STATION_CONTROL.init(Esp32s31StationControlResources::new());
    let (_controller, station) = prepare_esp32s31_station_task(
        Esp32s31StationConfig::new(policy).with_initial_candidate(StaNextCandidate::Reuse),
        Esp32s31StationStartResources::new(owner),
        station_control,
        ProductionStationRunner::new(spawner, interrupt_setup, wifi_plan),
    )
    .unwrap_or_else(|_| panic!("station control requires radio reset"));
    match station.run().await {
        Esp32s31StationExit::Stopped { .. } => panic!("station stopped unexpectedly"),
        Esp32s31StationExit::RetryExhausted {
            progress, failure, ..
        } => panic!(
            "station retries exhausted: attempts={} stage={:?}",
            progress.attempts_started, failure.stage
        ),
        Esp32s31StationExit::Terminal { failure, .. } => {
            panic!("terminal station failure: stage={:?}", failure.stage)
        }
    }
}
