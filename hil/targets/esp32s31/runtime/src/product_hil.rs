//! Product-level HIL composition.
//!
//! This module is deliberately an application of the public driver API. PAC,
//! DMA, ISR and station internals stay in `open-esp-radio-esp32s31-embassy-wifi`.

use core::{
    num::NonZeroU16,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_executor::{SendSpawner, Spawner};
use embassy_futures::select::{Either, select};
use embassy_net::{
    Config as NetworkConfig, Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4,
};
use embassy_time::{Instant, Timer};
use esp_hal::{
    efuse::{self, InterfaceMacAddress},
    rng::Trng,
};
use open_esp_radio::{
    MonitorCapturePolicy, MonitorRequest, StationRequest, StationScanChannels, StationScanPolicy,
    StationSecurity, WifiMacAddress, WifiMonitorConfig, WifiScanRequest as DriverWifiScanRequest,
    WifiSsid,
    esp32s31::phy::{PhyCalibrationIdentity, phy_rfpll::phy_get_rf_cal_version},
    esp32s31::wifi::mac::rx::RxBasebandFormat,
    wifi::{
        ieee80211::{channel::WifiChannel, station::StaAssociationPreference},
        softmac::MacRxEvidence,
        sta::station::StaReconnectPolicy,
        wpa2::Pmk,
    },
};
use open_esp_radio_esp32s31_embassy_wifi::{
    Esp32s31MacIrqObservation, Esp32s31MonitorFrame, Esp32s31MonitorFrames,
    Esp32s31QualificationHooks, Esp32s31RadioConfig, Esp32s31RadioParts, Esp32s31RadioRunner,
    Esp32s31WifiControl, Esp32s31WifiDevice, Esp32s31WifiParts,
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use open_esp_radio_hil_esp32s31_telemetry::{
    aggregate_tx::AggregateTxCounters, mac_irq::MacIrqClassificationCounters,
    rx_pipeline::RxPipelineCounters, task_poll::TaskPollSet,
};
use open_esp_radio_hil_protocol::{
    Capabilities, Event as HilEvent, FeatureCapabilities, MAX_WIRE_FRAME_BYTES, NetworkInfo,
    NetworkIpv4Configuration, StartupArtifactDisposition, StationDisconnectReason,
    StationEpochEvidence, StationLifecycleEvent, WIFI_MONITOR_FRAME_CHUNK_MAX_LEN,
    WifiMonitorCaptureRequest, WifiMonitorEvidence, WifiMonitorEvidenceSource,
    WifiMonitorFrameChunk, WifiMonitorObserved, WifiMonitorPhyEvidence, WifiMonitorPhyFormat,
    WifiRole, WifiRoleTransitionEvidence, WifiScanEvidence,
};
use static_cell::ConstStaticCell;

use crate::console::{
    WifiControlRequest, complete_monitor_capture, complete_monitor_start, complete_monitor_stop,
    complete_station_epoch_cycle, complete_wifi_role_transition, complete_wifi_scan, emergency_log,
    publish_event_reliably, publish_monitor_frame, publish_startup_artifact,
    publish_station_lifecycle, receive_wifi_control_request, set_wifi_role,
};

mod rx_qualification;
mod traffic;

use traffic::{connected_traffic_task, tcp_rx_pattern_worker_task, tcp_tx_pattern_worker_task};

const NETWORK_SOCKET_COUNT: usize = 5;
const SCAN_DWELL_MS: u16 = 200;
const MAXIMUM_TX_POWER_QUARTER_DBM: i8 = 80;
pub(crate) const OPEN_RADIO_TCP_BENCH: bool = option_env!("OPEN_RADIO_TCP_BENCH").is_some();
pub(crate) const OPEN_RADIO_TX_BENCH: bool = option_env!("OPEN_RADIO_TX_BENCH").is_some();
pub(crate) const OPEN_RADIO_BIDIRECTIONAL_BENCH: bool =
    option_env!("OPEN_RADIO_BIDIRECTIONAL_BENCH").is_some();
pub(crate) const OPEN_RADIO_TASK_POLL_TELEMETRY: bool = cfg!(feature = "task-poll-telemetry");
pub(crate) const OPEN_RADIO_TCP_CHUNK_CAPACITY: usize = 32_768;

static DIAGNOSTIC_STAGE: AtomicU32 = AtomicU32::new(0);
static NETWORK_RESOURCES: ConstStaticCell<StackResources<NETWORK_SOCKET_COUNT>> =
    ConstStaticCell::new(StackResources::new());
static CONNECTED_RX_OBSERVER: ConstStaticCell<rx_qualification::HilConnectedRxObserver> =
    ConstStaticCell::new(rx_qualification::HilConnectedRxObserver::new(4_323));
static PHY_CALIBRATION_ARTIFACT: ConstStaticCell<
    [u8; crate::phy_calibration_artifact::MAX_ENCODED_LEN],
> = ConstStaticCell::new([0; crate::phy_calibration_artifact::MAX_ENCODED_LEN]);

// Qualification observers execute on the RX/TX hot paths. Their atomics stay
// in internal SRAM so measuring a production image does not introduce PSRAM
// cache traffic into the path being measured.
#[unsafe(link_section = ".critical.data.open_radio_rx_telemetry")]
pub(crate) static RX_PIPELINE: RxPipelineCounters = RxPipelineCounters::new(now_micros);
#[unsafe(link_section = ".critical.bss.open_radio_tx_telemetry")]
pub(crate) static AGGREGATE_TX: AggregateTxCounters = AggregateTxCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
pub(crate) static MAC_IRQ: MacIrqClassificationCounters = MacIrqClassificationCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_task_poll_telemetry")]
pub(crate) static TASK_POLLS: TaskPollSet = TaskPollSet::new();

fn now_micros() -> u64 {
    Instant::now().as_micros()
}

fn protocol_observed<T>(evidence: MacRxEvidence<T>) -> Option<WifiMonitorObserved<T>> {
    match evidence {
        MacRxEvidence::HardwareObserved(value) => Some(WifiMonitorObserved {
            source: WifiMonitorEvidenceSource::Hardware,
            value,
        }),
        MacRxEvidence::ProtocolValidated(value) => Some(WifiMonitorObserved {
            source: WifiMonitorEvidenceSource::Protocol,
            value,
        }),
        MacRxEvidence::Unavailable => None,
    }
}

fn protocol_rate(
    evidence: MacRxEvidence<open_esp_radio::esp32s31::wifi::mac::rx::RxPhyInfo>,
) -> Option<WifiMonitorObserved<WifiMonitorPhyEvidence>> {
    protocol_observed(evidence).map(|observed| WifiMonitorObserved {
        source: observed.source,
        value: WifiMonitorPhyEvidence {
            format: match observed.value.baseband_format() {
                RxBasebandFormat::Dot11b => WifiMonitorPhyFormat::Dot11b,
                RxBasebandFormat::Ofdm => WifiMonitorPhyFormat::Ofdm,
                RxBasebandFormat::Ht => WifiMonitorPhyFormat::Ht,
                RxBasebandFormat::Vht => WifiMonitorPhyFormat::Vht,
                RxBasebandFormat::HeSu => WifiMonitorPhyFormat::HeSu,
                RxBasebandFormat::HeMu => WifiMonitorPhyFormat::HeMu,
                RxBasebandFormat::HeExtendedRangeSu => WifiMonitorPhyFormat::HeExtendedRangeSu,
                RxBasebandFormat::HeTriggerBased => WifiMonitorPhyFormat::HeTriggerBased,
                RxBasebandFormat::VhtMu => WifiMonitorPhyFormat::VhtMu,
                RxBasebandFormat::Unknown(raw) => WifiMonitorPhyFormat::Unknown(raw),
            },
            hardware_rate_code: observed.value.rate,
            he_siga1: observed.value.he_siga1,
            he_siga2: observed.value.he_siga2,
        },
    })
}

struct ExportedMonitorFrame {
    captured_bytes: u64,
    generation_mismatch: bool,
    channel_mismatch: bool,
    channel_unavailable: bool,
    last_observed_channel: u8,
}

async fn export_monitor_frame(
    request_id: u32,
    generation: u32,
    frame_sequence: u32,
    requested_channel: u8,
    frame: &Esp32s31MonitorFrame,
) -> ExportedMonitorFrame {
    let channel = protocol_observed(frame.metadata().rx.channel);
    let (channel_mismatch, channel_unavailable, last_observed_channel) = match channel {
        Some(observed) => (observed.value != requested_channel, false, observed.value),
        None => (false, true, 0),
    };
    let rssi_dbm = protocol_observed(frame.metadata().rx.rssi_dbm);
    let rate = protocol_rate(frame.metadata().rx.rate);
    let captured_length = u16::try_from(frame.captured_length())
        .expect("monitor frame fits the configured capture slot");
    let logical_length = u16::try_from(frame.metadata().logical_length.min(u16::MAX as usize))
        .expect("bounded logical length fits u16");
    let dequeued_at_micros = now_micros();
    for (index, bytes) in frame
        .bytes()
        .chunks(WIFI_MONITOR_FRAME_CHUNK_MAX_LEN)
        .enumerate()
    {
        let offset = u16::try_from(index * WIFI_MONITOR_FRAME_CHUNK_MAX_LEN)
            .expect("capture offset fits u16");
        let chunk = WifiMonitorFrameChunk::try_new(
            generation,
            frame_sequence,
            dequeued_at_micros,
            captured_length,
            logical_length,
            offset,
            channel,
            rssi_dbm,
            rate,
            bytes,
        )
        .expect("bounded monitor chunk fits the HIL protocol");
        publish_monitor_frame(request_id, chunk).await;
    }
    ExportedMonitorFrame {
        captured_bytes: u64::from(captured_length),
        generation_mismatch: frame.metadata().generation != generation,
        channel_mismatch,
        channel_unavailable,
        last_observed_channel,
    }
}

async fn run_finite_monitor_capture(
    idle: Esp32s31WifiControl,
    monitor_frames: &Esp32s31MonitorFrames,
    request_id: u32,
    request: WifiMonitorCaptureRequest,
) -> Esp32s31WifiControl {
    let mut monitor_request = MonitorRequest::new(
        WifiChannel::mhz20(request.channel).expect("console validates the monitor channel"),
        WifiMonitorConfig::normalized(),
    );
    if let Some(snapshot_length) = NonZeroU16::new(request.snapshot_length) {
        monitor_request =
            monitor_request.with_capture_policy(MonitorCapturePolicy::truncate_at(snapshot_length));
    }
    let owner = idle
        .start_monitor(monitor_request)
        .await
        .unwrap_or_else(|error| panic!("production finite monitor start failed: {error:?}"));
    let generation = owner.generation().value();
    set_wifi_role(WifiRole::Monitor);
    complete_monitor_start(
        request_id,
        WifiRoleTransitionEvidence {
            previous: WifiRole::Idle,
            current: WifiRole::Monitor,
            generation,
        },
    )
    .await;

    let deadline = Timer::after_millis(u64::from(request.duration_millis));
    let mut deadline = core::pin::pin!(deadline);
    let mut captured_frames = 0_u32;
    let mut captured_bytes = 0_u64;
    let mut generation_mismatches = 0_u32;
    let mut channel_mismatches = 0_u32;
    let mut channel_unavailable = 0_u32;
    let mut last_observed_channel = 0_u8;
    loop {
        match select(deadline.as_mut(), monitor_frames.receive()).await {
            Either::First(()) => break,
            Either::Second(frame) => {
                let observation = export_monitor_frame(
                    request_id,
                    generation,
                    captured_frames,
                    request.channel,
                    &frame,
                )
                .await;
                captured_frames = captured_frames.saturating_add(1);
                captured_bytes = captured_bytes.saturating_add(observation.captured_bytes);
                generation_mismatches = generation_mismatches
                    .saturating_add(u32::from(observation.generation_mismatch));
                channel_mismatches =
                    channel_mismatches.saturating_add(u32::from(observation.channel_mismatch));
                channel_unavailable =
                    channel_unavailable.saturating_add(u32::from(observation.channel_unavailable));
                if !observation.channel_unavailable {
                    last_observed_channel = observation.last_observed_channel;
                }
            }
        }
    }
    let idle = owner
        .stop()
        .await
        .unwrap_or_else(|error| panic!("production finite monitor stop failed: {error:?}"));
    let statistics = monitor_frames.statistics();
    set_wifi_role(WifiRole::Idle);
    complete_monitor_capture(
        request_id,
        WifiMonitorEvidence {
            generation,
            channel: request.channel,
            captured_frames,
            captured_bytes,
            generation_mismatches,
            channel_mismatches,
            channel_unavailable,
            last_observed_channel,
            published_frames: statistics.published_frames,
            full_drops: statistics.full_drops,
            oversized_drops: statistics.oversized_drops,
            discarded_frames: statistics.discarded_frames,
            exported_frames: captured_frames,
        },
    )
    .await;
    idle
}

fn observe_mac_irq(observation: Esp32s31MacIrqObservation) {
    match observation {
        Esp32s31MacIrqObservation::RxEpoch => RX_PIPELINE.record_rx_irq_epoch(),
        Esp32s31MacIrqObservation::TxEpoch => AGGREGATE_TX.record_tx_irq_epoch(now_micros),
        Esp32s31MacIrqObservation::Entry {
            first_status,
            observed_status,
            nonzero_snapshots,
        } => MAC_IRQ.record(first_status, observed_status, u32::from(nonzero_snapshots)),
    }
}

pub fn diagnostic_snapshot() -> (u32, u32) {
    (DIAGNOSTIC_STAGE.load(Ordering::Acquire), 0)
}

pub const fn hil_capabilities() -> Capabilities {
    Capabilities {
        features: FeatureCapabilities {
            udp: !OPEN_RADIO_TCP_BENCH,
            tcp: OPEN_RADIO_TCP_BENCH,
            rx: OPEN_RADIO_TCP_BENCH || !OPEN_RADIO_TX_BENCH || OPEN_RADIO_BIDIRECTIONAL_BENCH,
            tx: OPEN_RADIO_TCP_BENCH || OPEN_RADIO_TX_BENCH,
            bidirectional: OPEN_RADIO_TCP_BENCH || OPEN_RADIO_BIDIRECTIONAL_BENCH,
            network_provisioning: true,
            runtime_configuration: true,
            structured_evidence: true,
            startup_artifact: true,
            station_epoch_control: true,
            wifi_role_control: true,
            wifi_monitor_capture: true,
            station_lifecycle_events: true,
        },
        maximum_payload_bytes: if OPEN_RADIO_TCP_BENCH {
            OPEN_RADIO_TCP_CHUNK_CAPACITY as u16
        } else {
            1_472
        },
        maximum_wire_frame_bytes: MAX_WIRE_FRAME_BYTES as u16,
    }
}

#[embassy_executor::task]
async fn radio_runner_task(runner: Esp32s31RadioRunner) {
    runner.run().await
}

type ProductNetworkRunner = embassy_net::Runner<'static, Esp32s31WifiDevice>;

#[embassy_executor::task]
async fn network_runner_task(mut runner: ProductNetworkRunner) {
    runner.run().await
}

#[embassy_executor::task]
async fn network_report_task(stack: Stack<'static>) {
    report_network(stack).await
}

fn network_config(configuration: NetworkIpv4Configuration) -> NetworkConfig {
    match configuration {
        NetworkIpv4Configuration::Dhcp => NetworkConfig::dhcpv4(Default::default()),
        NetworkIpv4Configuration::Static {
            address,
            prefix_length,
            gateway,
        } => NetworkConfig::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::from_octets(address), prefix_length),
            gateway: gateway.map(Ipv4Address::from_octets),
            dns_servers: Default::default(),
        }),
    }
}

fn station_request(ssid: &[u8], passphrase: &[u8]) -> StationRequest {
    let ssid = WifiSsid::new(ssid).expect("validated HIL SSID must fit the driver request");
    let pmk = Pmk::derive(passphrase, ssid.as_bytes())
        .expect("validated HIL WPA2 credentials must derive a PMK");
    StationRequest::new(
        ssid,
        StationSecurity::wpa2_personal(pmk),
        StaReconnectPolicy::new(3, 100, 1_000, 100).expect("fixed HIL reconnect policy is valid"),
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(SCAN_DWELL_MS).expect("scan dwell is nonzero"),
            StaAssociationPreference::PreferHe20,
        ),
    )
}

async fn report_network(stack: Stack<'static>) -> ! {
    stack.wait_config_up().await;
    loop {
        if let Some(config) = stack.config_v4() {
            publish_event_reliably(
                0,
                0,
                HilEvent::NetworkReady(NetworkInfo {
                    address: config.address.address().octets(),
                    prefix_length: config.address.prefix_len(),
                    gateway: config.gateway.map(|address| address.octets()),
                }),
            )
            .await;
            emergency_log(format_args!(
                "OPEN_RADIO_HIL result=PASS stage=network-ready address={} gateway={:?}",
                config.address, config.gateway,
            ));
            loop {
                Timer::after_secs(60).await;
            }
        }
        Timer::after_millis(10).await;
    }
}

pub async fn run(
    spawner: Spawner,
    protocol_spawner: SendSpawner,
    platform: EspHalRadioPeripheral,
    trng: Trng,
) {
    DIAGNOSTIC_STAGE.store(10, Ordering::Release);
    let startup = crate::console::receive_startup_configuration().await;
    if OPEN_RADIO_TCP_BENCH {
        protocol_spawner
            .spawn(tcp_rx_pattern_worker_task().expect("TCP RX pattern task must allocate once"));
        protocol_spawner
            .spawn(tcp_tx_pattern_worker_task().expect("TCP TX pattern task must allocate once"));
    }
    let credentials = startup.network.credentials;
    let efuse_registers = esp_hal::peripherals::EFUSE::regs();
    let mut station_address = [0; 6];
    station_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let mut access_point_address = [0; 6];
    access_point_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    let station_mac = WifiMacAddress::new(station_address)
        .expect("ESP32-S31 station eFuse address must be unicast");
    let access_point_mac = WifiMacAddress::new(access_point_address)
        .expect("ESP32-S31 AP eFuse address must be unicast");
    let config = Esp32s31RadioConfig::new(
        station_mac,
        access_point_mac,
        PhyCalibrationIdentity {
            rf_cal_version: phy_get_rf_cal_version(),
            mac_sys0: efuse_registers.rd_mac_sys0().read().bits(),
            mac_sys1: efuse_registers.rd_mac_sys1().read().bits(),
        },
        WifiChannel::mhz20(1).expect("initial channel is valid"),
    )
    .with_maximum_tx_power_quarter_dbm(MAXIMUM_TX_POWER_QUARTER_DBM)
    .with_qualification_hooks(Esp32s31QualificationHooks {
        rx_pipeline: &RX_PIPELINE,
        aggregate_tx: &AGGREGATE_TX,
        connected_rx: CONNECTED_RX_OBSERVER.take(),
        mac_irq: observe_mac_irq,
    });
    let artifact_was_supplied = startup.phy_calibration_artifact.is_some();
    let config = match startup.phy_calibration_artifact {
        Some(artifact) => config.with_calibration_cache(
            crate::phy_calibration_artifact::decode(artifact.bytes())
                .expect("host PHY calibration artifact has an unsupported or invalid schema"),
        ),
        None => config,
    };

    let started_at = Instant::now();
    let (radio, runner) =
        open_esp_radio_esp32s31_embassy_wifi::new(protocol_spawner, platform, trng, config)
            .await
            .unwrap_or_else(|error| panic!("production radio initialization failed: {error:?}"));
    let Esp32s31RadioParts {
        wifi,
        initialization,
    } = radio.into_parts();
    if let Some(cache) = initialization.calibration_cache {
        let disposition = if artifact_was_supplied {
            StartupArtifactDisposition::Restored
        } else {
            StartupArtifactDisposition::Created
        };
        let encoded =
            crate::phy_calibration_artifact::encode(&cache, PHY_CALIBRATION_ARTIFACT.take())
                .expect("typed PHY calibration artifact exceeds its explicit HIL storage budget");
        publish_startup_artifact(disposition, started_at.elapsed().as_micros(), encoded).await;
    }
    let Esp32s31WifiParts {
        control,
        device,
        monitor_frames,
        qualification,
    } = wifi.into_parts();
    spawner
        .spawn(radio_runner_task(runner).expect("production radio runner task must allocate once"));

    let seed = u64::from_le_bytes([
        station_address[0],
        station_address[1],
        station_address[2],
        station_address[3],
        station_address[4],
        station_address[5],
        0xa5,
        0x31,
    ]);
    let (stack, network_runner) = embassy_net::new(
        device,
        network_config(startup.network.ipv4),
        NETWORK_RESOURCES.take(),
        seed,
    );
    spawner.spawn(
        network_runner_task(network_runner).expect("network runner task must allocate once"),
    );

    DIAGNOSTIC_STAGE.store(20, Ordering::Release);
    let station = control
        .start_station(station_request(
            credentials.ssid(),
            credentials.passphrase(),
        ))
        .await
        .unwrap_or_else(|error| panic!("production station start failed: {error:?}"));
    DIAGNOSTIC_STAGE.store(30, Ordering::Release);
    publish_station_lifecycle(StationLifecycleEvent::Connected {
        generation: station.generation().value(),
    })
    .await;
    set_wifi_role(WifiRole::Station);
    emergency_log(format_args!(
        "OPEN_RADIO_HIL result=PASS stage=station-connected generation={}",
        station.generation().value(),
    ));
    spawner.spawn(network_report_task(stack).expect("network report task must allocate once"));
    spawner.spawn(
        connected_traffic_task(stack, qualification)
            .expect("connected traffic task must allocate once"),
    );

    enum ProductWifiRole<P> {
        Idle(open_esp_radio::WifiIdle<P>),
        Station(open_esp_radio::WifiStation<P>),
        Monitor {
            owner: open_esp_radio::WifiMonitor<P>,
            channel: u8,
            captured_frames: u32,
            captured_bytes: u64,
            generation_mismatches: u32,
            channel_mismatches: u32,
            channel_unavailable: u32,
            last_observed_channel: u8,
        },
    }

    let mut role = ProductWifiRole::Station(station);
    loop {
        role = match role {
            ProductWifiRole::Station(station) => match receive_wifi_control_request().await {
                WifiControlRequest::Cycle { request_id } => {
                    let stopped_generation = station.generation().value();
                    let idle = station.stop().await.unwrap_or_else(|error| {
                        panic!("production station stop failed: {error:?}")
                    });
                    publish_station_lifecycle(StationLifecycleEvent::Disconnected {
                        generation: stopped_generation,
                        reason: StationDisconnectReason::ReconnectRequested,
                    })
                    .await;
                    let station = idle
                        .start_station(station_request(
                            credentials.ssid(),
                            credentials.passphrase(),
                        ))
                        .await
                        .unwrap_or_else(|error| {
                            panic!("production station restart failed: {error:?}")
                        });
                    publish_station_lifecycle(StationLifecycleEvent::Connected {
                        generation: station.generation().value(),
                    })
                    .await;
                    complete_station_epoch_cycle(request_id, StationEpochEvidence::COMPLETE).await;
                    ProductWifiRole::Station(station)
                }
                WifiControlRequest::StopStation { request_id } => {
                    let stopped_generation = station.generation().value();
                    let idle = station.stop().await.unwrap_or_else(|error| {
                        panic!("production station stop failed: {error:?}")
                    });
                    publish_station_lifecycle(StationLifecycleEvent::Disconnected {
                        generation: stopped_generation,
                        reason: StationDisconnectReason::LinkPolicy,
                    })
                    .await;
                    set_wifi_role(WifiRole::Idle);
                    complete_wifi_role_transition(
                        request_id,
                        WifiRoleTransitionEvidence {
                            previous: WifiRole::Station,
                            current: WifiRole::Idle,
                            generation: stopped_generation,
                        },
                    )
                    .await;
                    ProductWifiRole::Idle(idle)
                }
                _ => unreachable!("console admits only station commands while station owns Wi-Fi"),
            },
            ProductWifiRole::Idle(idle) => match receive_wifi_control_request().await {
                WifiControlRequest::StartStation { request_id } => {
                    let station = idle
                        .start_station(station_request(
                            credentials.ssid(),
                            credentials.passphrase(),
                        ))
                        .await
                        .unwrap_or_else(|error| {
                            panic!("production station start failed: {error:?}")
                        });
                    publish_station_lifecycle(StationLifecycleEvent::Connected {
                        generation: station.generation().value(),
                    })
                    .await;
                    set_wifi_role(WifiRole::Station);
                    complete_wifi_role_transition(
                        request_id,
                        WifiRoleTransitionEvidence {
                            previous: WifiRole::Idle,
                            current: WifiRole::Station,
                            generation: station.generation().value(),
                        },
                    )
                    .await;
                    ProductWifiRole::Station(station)
                }
                WifiControlRequest::Scan {
                    request_id,
                    request,
                } => {
                    let mut channels = [0_u8; 13];
                    let mut channel_count = 0_usize;
                    for channel in 1_u8..=13 {
                        if request.channel_mask_2_4_ghz & (1_u16 << (channel - 1)) != 0 {
                            channels[channel_count] = channel;
                            channel_count += 1;
                        }
                    }
                    let scan_channels =
                        StationScanChannels::from_primary_channels(&channels[..channel_count])
                            .expect("console validates the scan channel mask");
                    let completed = idle
                        .scan(DriverWifiScanRequest::new(
                            scan_channels,
                            NonZeroU16::new(request.dwell_millis)
                                .expect("console validates nonzero scan dwell"),
                        ))
                        .await
                        .unwrap_or_else(|error| {
                            panic!("production standalone scan failed: {error:?}")
                        });
                    let (idle, report) = completed.into_parts();
                    let configured = report
                        .results()
                        .iter()
                        .find(|result| result.ssid() == credentials.ssid());
                    let evidence = WifiScanEvidence {
                        generation: report.generation().value(),
                        observed_frames: report.observed_frames,
                        unique_bss: report.results().len() as u8,
                        dropped_unique_bss: report.dropped_unique_bss,
                        configured_ssid_found: configured.is_some(),
                        configured_ssid_channel: configured.map_or(0, |result| result.channel),
                        configured_ssid_rssi_dbm: configured
                            .map_or(i8::MIN, |result| result.rssi_dbm),
                    };
                    set_wifi_role(WifiRole::Idle);
                    complete_wifi_scan(request_id, evidence).await;
                    ProductWifiRole::Idle(idle)
                }
                WifiControlRequest::StartMonitor {
                    request_id,
                    request,
                } => {
                    let mut monitor_request = MonitorRequest::new(
                        WifiChannel::mhz20(request.channel)
                            .expect("console validates the monitor channel"),
                        WifiMonitorConfig::normalized(),
                    );
                    if let Some(snapshot_length) = NonZeroU16::new(request.snapshot_length) {
                        monitor_request = monitor_request.with_capture_policy(
                            MonitorCapturePolicy::truncate_at(snapshot_length),
                        );
                    }
                    let monitor =
                        idle.start_monitor(monitor_request)
                            .await
                            .unwrap_or_else(|error| {
                                panic!("production monitor start failed: {error:?}")
                            });
                    set_wifi_role(WifiRole::Monitor);
                    complete_monitor_start(
                        request_id,
                        WifiRoleTransitionEvidence {
                            previous: WifiRole::Idle,
                            current: WifiRole::Monitor,
                            generation: monitor.generation().value(),
                        },
                    )
                    .await;
                    ProductWifiRole::Monitor {
                        owner: monitor,
                        channel: request.channel,
                        captured_frames: 0,
                        captured_bytes: 0,
                        generation_mismatches: 0,
                        channel_mismatches: 0,
                        channel_unavailable: 0,
                        last_observed_channel: 0,
                    }
                }
                WifiControlRequest::CaptureMonitor {
                    request_id,
                    request,
                } => ProductWifiRole::Idle(
                    run_finite_monitor_capture(idle, &monitor_frames, request_id, request).await,
                ),
                _ => unreachable!("console admits only idle commands while Wi-Fi is idle"),
            },
            ProductWifiRole::Monitor {
                owner,
                channel,
                mut captured_frames,
                mut captured_bytes,
                mut generation_mismatches,
                mut channel_mismatches,
                mut channel_unavailable,
                mut last_observed_channel,
            } => {
                let request_id = loop {
                    match select(receive_wifi_control_request(), monitor_frames.receive()).await {
                        Either::First(WifiControlRequest::StopMonitor { request_id }) => {
                            break request_id;
                        }
                        Either::First(_) => unreachable!(
                            "console admits only monitor stop while monitor owns Wi-Fi"
                        ),
                        Either::Second(frame) => {
                            captured_frames = captured_frames.saturating_add(1);
                            captured_bytes =
                                captured_bytes.saturating_add(frame.captured_length() as u64);
                            if frame.metadata().generation != owner.generation().value() {
                                generation_mismatches = generation_mismatches.saturating_add(1);
                            }
                            match frame.metadata().rx.channel {
                                MacRxEvidence::HardwareObserved(observed)
                                | MacRxEvidence::ProtocolValidated(observed) => {
                                    last_observed_channel = observed;
                                    if observed != channel {
                                        channel_mismatches = channel_mismatches.saturating_add(1);
                                    }
                                }
                                MacRxEvidence::Unavailable => {
                                    channel_unavailable = channel_unavailable.saturating_add(1);
                                }
                            }
                        }
                    }
                };
                let generation = owner.generation().value();
                let idle = owner
                    .stop()
                    .await
                    .unwrap_or_else(|error| panic!("production monitor stop failed: {error:?}"));
                let statistics = monitor_frames.statistics();
                set_wifi_role(WifiRole::Idle);
                complete_monitor_stop(
                    request_id,
                    WifiMonitorEvidence {
                        generation,
                        channel,
                        captured_frames,
                        captured_bytes,
                        generation_mismatches,
                        channel_mismatches,
                        channel_unavailable,
                        last_observed_channel,
                        published_frames: statistics.published_frames,
                        full_drops: statistics.full_drops,
                        oversized_drops: statistics.oversized_drops,
                        discarded_frames: statistics.discarded_frames,
                        exported_frames: captured_frames,
                    },
                )
                .await;
                ProductWifiRole::Idle(idle)
            }
        };
    }
}
