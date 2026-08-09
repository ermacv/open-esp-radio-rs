//! Product-level HIL composition.
//!
//! This module is deliberately an application of the public driver API. PAC,
//! DMA, ISR and station internals stay in `open-esp-radio-esp32s31-embassy-wifi`.

use core::{
    num::NonZeroU16,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_executor::{SendSpawner, Spawner};
use embassy_net::{
    Config as NetworkConfig, Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4,
};
use embassy_time::{Instant, Timer};
use esp_hal::{
    efuse::{self, InterfaceMacAddress},
    rng::Trng,
};
use open_esp_radio::{
    MonitorRequest, StationRequest, StationScanChannels, StationScanPolicy, StationSecurity,
    WifiMacAddress, WifiMonitorConfig, WifiSsid,
    esp32s31::phy::{
        PhyCalibrationIdentity, phy_cold::PhyCalibrationRecord, phy_rfpll::phy_get_rf_cal_version,
    },
    wifi::{
        ieee80211::{channel::WifiChannel, station::StaAssociationPreference},
        sta::station::StaReconnectPolicy,
        wpa2::Pmk,
    },
};
use open_esp_radio_esp32s31_embassy_wifi::{
    Esp32s31QualificationHooks, Esp32s31RadioConfig, Esp32s31RadioParts, Esp32s31RadioRunner,
    Esp32s31WifiDevice, Esp32s31WifiParts,
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use open_esp_radio_hil_esp32s31_telemetry::{
    aggregate_tx::AggregateTxCounters, rx_pipeline::RxPipelineCounters, task_poll::TaskPollSet,
};
use open_esp_radio_hil_protocol::{
    Capabilities, Event as HilEvent, FeatureCapabilities, MAX_WIRE_FRAME_BYTES, NetworkInfo,
    NetworkIpv4Configuration, StartupArtifactDisposition, StationDisconnectReason,
    StationEpochEvidence, StationLifecycleEvent, StationStopEvidence,
};
use static_cell::ConstStaticCell;

use crate::console::{
    StationControlRequest, complete_station_epoch_cycle, complete_station_stop, emergency_log,
    publish_event_reliably, publish_startup_artifact, publish_station_lifecycle,
    receive_station_control_request,
};

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

// Qualification observers execute on the RX/TX hot paths. Their atomics stay
// in internal SRAM so measuring a production image does not introduce PSRAM
// cache traffic into the path being measured.
#[unsafe(link_section = ".critical.data.open_radio_rx_telemetry")]
pub(crate) static RX_PIPELINE: RxPipelineCounters = RxPipelineCounters::new(now_micros);
#[unsafe(link_section = ".critical.bss.open_radio_tx_telemetry")]
pub(crate) static AGGREGATE_TX: AggregateTxCounters = AggregateTxCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_task_poll_telemetry")]
pub(crate) static TASK_POLLS: TaskPollSet = TaskPollSet::new();

fn now_micros() -> u64 {
    Instant::now().as_micros()
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
            station_stop_control: true,
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
    let mut credentials = startup.network.credentials;
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
    });
    let config = match startup.phy_calibration_record {
        Some(bytes) => config.with_calibration_record(PhyCalibrationRecord::from_bytes(bytes)),
        None => config,
    };

    let started_at = Instant::now();
    let (radio, runner) =
        open_esp_radio_esp32s31_embassy_wifi::new(spawner, platform, trng, config)
            .await
            .unwrap_or_else(|error| panic!("production radio initialization failed: {error:?}"));
    let Esp32s31RadioParts {
        wifi,
        initialization,
    } = radio.into_parts();
    if let Some(record) = initialization.calibration_record {
        let disposition = if startup.phy_calibration_record.is_some() {
            StartupArtifactDisposition::Restored
        } else {
            StartupArtifactDisposition::Created
        };
        publish_startup_artifact(disposition, started_at.elapsed().as_micros(), &record).await;
    }
    let Esp32s31WifiParts {
        control,
        device,
        monitor_frames: _,
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
    let mut station = control
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
    emergency_log(format_args!(
        "OPEN_RADIO_HIL result=PASS stage=station-connected generation={}",
        station.generation().value(),
    ));
    spawner.spawn(network_report_task(stack).expect("network report task must allocate once"));
    spawner.spawn(
        connected_traffic_task(stack, qualification)
            .expect("connected traffic task must allocate once"),
    );

    loop {
        match receive_station_control_request().await {
            StationControlRequest::Cycle { request_id } => {
                let stopped_generation = station.generation().value();
                let idle = station
                    .stop()
                    .await
                    .unwrap_or_else(|error| panic!("production station stop failed: {error:?}"));
                publish_station_lifecycle(StationLifecycleEvent::Disconnected {
                    generation: stopped_generation,
                    reason: StationDisconnectReason::ReconnectRequested,
                })
                .await;
                station = idle
                    .start_station(station_request(
                        credentials.ssid(),
                        credentials.passphrase(),
                    ))
                    .await
                    .unwrap_or_else(|error| panic!("production station restart failed: {error:?}"));
                publish_station_lifecycle(StationLifecycleEvent::Connected {
                    generation: station.generation().value(),
                })
                .await;
                complete_station_epoch_cycle(request_id, StationEpochEvidence::COMPLETE).await;
            }
            StationControlRequest::Stop { request_id } => {
                let stopped_generation = station.generation().value();
                let idle = station
                    .stop()
                    .await
                    .unwrap_or_else(|error| panic!("production station stop failed: {error:?}"));
                publish_station_lifecycle(StationLifecycleEvent::Disconnected {
                    generation: stopped_generation,
                    reason: StationDisconnectReason::LinkPolicy,
                })
                .await;

                let monitor = idle
                    .start_monitor(MonitorRequest::new(
                        WifiChannel::mhz20(1).expect("monitor channel is valid"),
                        WifiMonitorConfig::normalized(),
                    ))
                    .await
                    .unwrap_or_else(|error| panic!("production monitor start failed: {error:?}"));
                let idle = monitor
                    .stop()
                    .await
                    .unwrap_or_else(|error| panic!("production monitor stop failed: {error:?}"));
                let monitor = idle
                    .start_monitor(MonitorRequest::new(
                        WifiChannel::mhz20(6).expect("monitor channel is valid"),
                        WifiMonitorConfig::normalized(),
                    ))
                    .await
                    .unwrap_or_else(|error| panic!("production monitor restart failed: {error:?}"));
                let idle = monitor.stop().await.unwrap_or_else(|error| {
                    panic!("production restarted monitor stop failed: {error:?}")
                });
                let restarted = idle
                    .start_station(station_request(
                        credentials.ssid(),
                        credentials.passphrase(),
                    ))
                    .await
                    .unwrap_or_else(|error| {
                        panic!("production station rematerialization failed: {error:?}")
                    });
                publish_station_lifecycle(StationLifecycleEvent::Connected {
                    generation: restarted.generation().value(),
                })
                .await;
                let final_generation = restarted.generation().value();
                let _idle = restarted.stop().await.unwrap_or_else(|error| {
                    panic!("production rematerialized station stop failed: {error:?}")
                });
                publish_station_lifecycle(StationLifecycleEvent::Disconnected {
                    generation: final_generation,
                    reason: StationDisconnectReason::LinkPolicy,
                })
                .await;
                credentials.clear_passphrase();
                complete_station_stop(request_id, StationStopEvidence::COMPLETE).await;
                loop {
                    Timer::after_secs(60).await;
                }
            }
        }
    }
}
