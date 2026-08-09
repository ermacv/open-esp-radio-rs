#![forbid(unsafe_code)]

use embassy_executor::{SendSpawner, Spawner};
use embassy_time::Instant;
use esp_hal::rng::Trng;
use open_esp_radio::{
    esp32s31::{
        hal::RadioRegisters,
        registers::MacInterruptSetup,
        wifi::sta::attempt::{Esp32s31StaAttemptSecurity, Esp32s31StaIdentity},
    },
    wifi::{
        ieee80211::station::StaTxSequenceCounters, softmac::interface::BoundVirtualInterface,
        sta::station::StaReconnectPolicy, wpa2::Pmk,
    },
};
use open_esp_radio_esp32s31_wifi_embassy::{
    embassy_irq::Esp32s31MacInterruptEpoch,
    station::{
        Esp32s31StationConfig, Esp32s31StationControlResources, Esp32s31StationExit,
        Esp32s31StationRadioResources, Esp32s31StationRoleOwner, Esp32s31StationRuntimeResources,
        Esp32s31StationStartResources, Esp32s31StationStorageResources,
        prepare_esp32s31_station_task,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::{
    EspHalRadioPeripheral, mac_interrupt_epoch::EspHalMacInterruptRoute,
};
use open_esp_radio_hil_protocol::{
    NetworkCredentials, NetworkIpv4Configuration, StationLifecycleEvent, StationStopEvidence,
};

use crate::{
    console::emergency_log,
    radio_hil::{
        NetworkTxPool, OPEN_RADIO_IRQ_RUNTIME, OPEN_RADIO_NETWORK_RESOURCES,
        OPEN_RADIO_NETWORK_TX_POOL, OPEN_RADIO_POWER_IRQ_RUNTIME,
        OPEN_RADIO_STATION_CONTROL_RESOURCES, connected_epoch_bindings, connected_rx_bindings,
        connected_task_bindings, network_report_bindings,
        open_radio_mac_interrupt, open_radio_power_interrupt, radio_hil_message4_protection,
        station_epoch_coordinator,
    },
};

use super::{
    RadioHilInitialScanResources, RadioHilStaLifecycleFailure, RadioHilStaLifecycleOwner,
    RadioHilStaNetwork, RadioHilStationBoardResources, RadioHilStationDmaResources,
    RadioHilStationEngine, RadioHilStationEngineObserver, RadioHilStationEnginePort,
    RadioHilStationPhase, prepare_initial_station_scan, protocol_station_failure_reason,
    protocol_station_failure_stage, qualify_station_monitor_station_owner_round_trip,
    radio_hil_station_discovery, station_control_task, try_reclaim_station_runtime,
};

/// Allocate the station/network ownership graph exactly once.
///
/// Keep both frame queues out of the task stack. A reconnect receives the
/// already-running network owner from the completed connected epoch instead.
fn initialize_sta_network(station_address: [u8; 6]) -> RadioHilStaNetwork {
    let resources = OPEN_RADIO_NETWORK_RESOURCES.take();
    let tx_pool = NetworkTxPool::pin_static(OPEN_RADIO_NETWORK_TX_POOL.take());
    let (device, runner) = resources.split(tx_pool, station_address);
    RadioHilStaNetwork::Unstarted { device, runner }
}

/// Run the complete actor-owned active-scan, join, security and connected STA
/// scenario. Board setup prepares halted resources only; candidate identity is
/// created exclusively by the `InitialScan` phase.
pub(in crate::radio_hil) async fn run_full_station_hil(
    spawner: Spawner,
    protocol_spawner: SendSpawner,
    mut role: Esp32s31StationRoleOwner<EspHalRadioPeripheral>,
    mmio: RadioRegisters,
    interrupt_setup: MacInterruptSetup,
    trng: &Trng,
    network_credentials: &mut NetworkCredentials,
    network_ipv4: NetworkIpv4Configuration,
    station_interface: BoundVirtualInterface,
) -> bool {
    let discovery = match radio_hil_station_discovery(network_credentials.ssid()) {
        Ok(discovery) => discovery,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=station-discovery error={error}"
            ));
            return false;
        }
    };
    let (state, _) = role.radio_mut();
    let Some(prepared) = prepare_initial_station_scan(state, mmio, station_interface) else {
        return false;
    };
    let RadioHilInitialScanResources {
        station_address,
        mmio,
        rx: scan_rx,
        rx_storage: storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        scan_table,
        scan_frame,
        ethernet_frame,
    } = prepared;

    // The common cold-to-runtime transition occurred before station
    // materialization. Retain its inactive interrupt setup until WPA2 has
    // opened the controlled port.
    let interrupt_epoch = Esp32s31MacInterruptEpoch::new(
        EspHalMacInterruptRoute::new(open_radio_mac_interrupt, open_radio_power_interrupt),
        interrupt_setup,
        &OPEN_RADIO_IRQ_RUNTIME,
        &OPEN_RADIO_POWER_IRQ_RUNTIME,
    );

    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL stage=wpa2-pmk-derive start iterations=4096"
    ));
    let pmk_started = Instant::now();
    let pmk_result = Pmk::derive(network_credentials.passphrase(), network_credentials.ssid());
    network_credentials.clear_passphrase();
    let pmk = match pmk_result {
        Ok(pmk) => pmk,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-pmk-derive error={error:?}"
            ));
            return false;
        }
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-pmk-derive elapsed_ms={}",
        pmk_started.elapsed().as_millis(),
    ));
    let mut supplicant_nonce = [0; 32];
    for word in supplicant_nonce.chunks_exact_mut(4) {
        word.copy_from_slice(&trng.random().to_le_bytes());
    }
    let sequence_seed = (trng.random() & 0x0fff) as u16;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL stage=sta-sequence-session seed={sequence_seed}"
    ));
    let sequences = StaTxSequenceCounters::new(sequence_seed);
    let connected_epoch = match connected_epoch_bindings(network_ipv4) {
        Ok(bindings) => bindings,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=connected-static-resources error={error:?}"
            ));
            return false;
        }
    };
    let station_control: &'static Esp32s31StationControlResources<_> =
        OPEN_RADIO_STATION_CONTROL_RESOURCES.take();

    let runtime = Esp32s31StationRuntimeResources::new(
        Esp32s31StationRadioResources::new(role, interrupt_epoch),
        Esp32s31StationStorageResources::new(
            RadioHilStationDmaResources::new(storage, descriptor_base, buffer_addresses),
            tx_storage,
            scan_table,
            scan_frame,
            ethernet_frame,
        ),
        RadioHilStationBoardResources::new(
            spawner,
            protocol_spawner,
            station_interface,
            connected_task_bindings(),
            connected_rx_bindings(network_ipv4),
            network_report_bindings(network_ipv4),
            connected_epoch,
            station_control,
        ),
    );
    let owner = RadioHilStaLifecycleOwner::new(
        runtime,
        RadioHilStationPhase::InitialScan {
            hardware: mmio,
            receive: scan_rx,
            network: initialize_sta_network(station_address),
            identity: Esp32s31StaIdentity {
                station_address,
                association_preference: discovery.scan().association_preference(),
            },
        },
        Esp32s31StaAttemptSecurity::new(
            pmk,
            supplicant_nonce,
            sequences,
            radio_hil_message4_protection(),
        ),
    );
    let policy = StaReconnectPolicy::new(3, 100, 1_000, 100)
        .expect("fixed HIL station reconnect policy is valid");
    let (controller, station) = prepare_esp32s31_station_task(
        Esp32s31StationConfig::new(policy),
        Esp32s31StationStartResources::new(owner),
        station_control,
        RadioHilStationEngine::with_observer(
            RadioHilStationEnginePort::new(),
            discovery,
            RadioHilStationEngineObserver,
        ),
    )
    .unwrap_or_else(|_| panic!("HIL station control requires radio reset"));
    spawner.spawn(
        station_control_task(controller, station_epoch_coordinator())
            .unwrap_or_else(|_| panic!("station controller task allocation failed")),
    );

    let mut clean_stop = None;
    let (progress, scan_qualified) = match station.run().await {
        Esp32s31StationExit::Stopped {
            resources,
            progress,
            reason,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=production-sta-lifecycle-stop \
                 connected_epochs={} attempts={} reason={reason:?}",
                progress.connected_epochs, progress.attempts_started,
            ));
            let (owner, runner) = resources.into_parts();
            let scan_qualified = runner.into_port().scan_qualified();
            let completed_join = progress.connected_epochs != 0;
            let registers_reclaimed = match try_reclaim_station_runtime(owner) {
                Ok(reclaimed) => {
                    clean_stop = Some(reclaimed);
                    true
                }
                Err(error) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-sta-lifecycle-reclaim error={error:?}"
                    ));
                    false
                }
            };
            (
                (
                    completed_join && registers_reclaimed,
                    completed_join,
                    completed_join,
                    completed_join,
                ),
                scan_qualified,
            )
        }
        Esp32s31StationExit::RetryExhausted {
            resources,
            progress,
            failure,
        } => {
            crate::console::publish_station_lifecycle(StationLifecycleEvent::RetryExhausted {
                generation: progress.connected_epochs,
                attempts: progress.final_generation_attempt,
                stage: protocol_station_failure_stage(failure.stage),
                reason: protocol_station_failure_reason(failure.error),
            })
            .await;
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-sta-lifecycle-exhausted \
                 connected_epochs={} attempts={} failure={failure:?}",
                progress.connected_epochs, progress.attempts_started,
            ));
            let result = match failure.error {
                RadioHilStaLifecycleFailure::InitialScanNoCandidate
                | RadioHilStaLifecycleFailure::InitialScanTransaction
                | RadioHilStaLifecycleFailure::InitialScanPlan
                | RadioHilStaLifecycleFailure::InitialScanReceiveHandoff
                | RadioHilStaLifecycleFailure::Authentication => (false, false, false, false),
                RadioHilStaLifecycleFailure::InitialJoin {
                    associated,
                    message1,
                    message3,
                } => (true, associated, message1, message3),
                _ => (true, true, true, true),
            };
            let (owner, runner) = resources.into_parts();
            let scan_qualified = runner.into_port().scan_qualified();
            let _owner = owner;
            (result, scan_qualified)
        }
        Esp32s31StationExit::Terminal {
            resources,
            progress,
            failure,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-sta-lifecycle-terminal \
                 connected_epochs={} attempts={} failure={failure:?}",
                progress.connected_epochs, progress.attempts_started,
            ));
            let (owner, runner) = resources.into_parts();
            let scan_qualified = runner.into_port().scan_qualified();
            let completed_join = progress.connected_epochs != 0;
            let _owner = owner;
            (
                (
                    completed_join,
                    completed_join,
                    completed_join,
                    completed_join,
                ),
                scan_qualified,
            )
        }
    };

    if let Some(reclaimed) = clean_stop {
        let (route, interrupt_setup, _, _) = match reclaimed.interrupt.try_into_inactive_parts() {
            Ok(parts) => parts,
            Err(_) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                     stage=production-sta-lifecycle-regroup error=interrupt-active"
                ));
                return false;
            }
        };
        let channel = reclaimed
            .channel
            .expect("a completed station epoch has a selected channel");
        let mut role = reclaimed.role;
        role.set_current_channel(channel);
        let stopped = role.into_stopped(reclaimed.registers, interrupt_setup, reclaimed.resources);
        let (wifi, resources) = (stopped.wifi, stopped.resources);
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=production-sta-lifecycle-regroup \
             channel={} resources={:?}",
            wifi.current_channel().primary(),
            resources.stopped_phase(),
        ));
        let role_transition = qualify_station_monitor_station_owner_round_trip(
            spawner,
            network_credentials.ssid(),
            wifi,
            resources,
            route,
        )
        .await;
        if role_transition && let Some(request_id) = station_epoch_coordinator().take_stop_request()
        {
            crate::console::complete_station_stop(request_id, StationStopEvidence::COMPLETE).await;
        }
    }
    supplicant_nonce.fill(0);
    let (sta_auth_pass, sta_assoc_pass, wpa2_message_1_pass, wpa2_message_3_pass) = progress;
    scan_qualified && sta_auth_pass && sta_assoc_pass && wpa2_message_1_pass && wpa2_message_3_pass
}
