#![forbid(unsafe_code)]

use embassy_executor::{SendSpawner, Spawner};
use embassy_time::Instant;
use esp_hal::rng::Trng;
use open_esp_radio::{
    esp32s31::{
        hal::ColdRadioRegisters,
        phy::phy_cold::PhyColdState,
        wifi::sta::attempt::{Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStation},
    },
    wifi::{
        ieee80211::station::StaTxSequenceCounters,
        softmac::interface::BoundVirtualInterface,
        sta::station::{StaNextCandidate, StaReconnectPolicy},
        wpa2::Pmk,
    },
};
use open_esp_radio_esp32s31_wifi_embassy::{
    embassy_irq::Esp32s31MacInterruptEpoch,
    station::{
        Esp32s31StationConfig, Esp32s31StationControlResources, Esp32s31StationExit,
        Esp32s31StationStartResources, prepare_esp32s31_station_task,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::{
    EspHalRadioPeripheral, mac_interrupt_epoch::EspHalMacInterruptRoute,
};
use open_esp_radio_hil_protocol::{
    NetworkCredentials, NetworkIpv4Configuration, StationLifecycleEvent,
};

use crate::{
    console::emergency_log,
    radio_hil::{
        NetworkResources, NetworkTxPool, OPEN_RADIO_IRQ_RUNTIME, OPEN_RADIO_NETWORK_RESOURCES,
        OPEN_RADIO_NETWORK_TX_POOL, OPEN_RADIO_POWER_IRQ_RUNTIME,
        OPEN_RADIO_STATION_CONTROL_RESOURCES, STA_ASSOCIATION_PREFERENCE, connected_epoch_bindings,
        connected_rx_bindings, connected_task_bindings, network_report_bindings,
        open_radio_mac_interrupt, open_radio_power_interrupt, radio_hil_message4_protection,
        station_epoch_coordinator,
    },
};

use super::{
    RadioHilAuthenticationReady, RadioHilColdScanHandoff, RadioHilConnectedFixture,
    RadioHilStaAttemptRunner, RadioHilStaLifecycleFailure, RadioHilStaLifecycleOwner,
    RadioHilStaNetwork, protocol_station_failure_reason, protocol_station_failure_stage,
    run_cold_station_scan, station_control_task,
};

/// Allocate the station/network ownership graph exactly once.
///
/// Keep both frame queues out of the task stack. A reconnect receives the
/// already-running network owner from the completed connected epoch instead.
fn initialize_sta_network(station_address: [u8; 6]) -> RadioHilStaNetwork {
    let resources = OPEN_RADIO_NETWORK_RESOURCES.init_with(NetworkResources::new);
    let tx_pool =
        NetworkTxPool::pin_static(OPEN_RADIO_NETWORK_TX_POOL.init_with(NetworkTxPool::new));
    let (device, runner) = resources.split(tx_pool, station_address);
    RadioHilStaNetwork::Unstarted { device, runner }
}

/// Run the complete active-scan, join, security and connected STA scenario.
///
/// This is deliberately not named `promiscuous`: promiscuous receive is one
/// cold MAC initialization primitive, while the scenario owns the complete
/// production STA lifecycle.
pub(in crate::radio_hil) async fn run_full_station_hil(
    spawner: Spawner,
    protocol_spawner: SendSpawner,
    state: &mut PhyColdState,
    mut platform: EspHalRadioPeripheral,
    cold_mmio: ColdRadioRegisters,
    trng: &Trng,
    network_credentials: &mut NetworkCredentials,
    network_ipv4: NetworkIpv4Configuration,
    station_interface: BoundVirtualInterface,
) -> bool {
    let platform = &mut platform;
    let Some(handoff) = run_cold_station_scan(
        state,
        platform,
        cold_mmio,
        network_credentials,
        station_interface,
    )
    .await
    else {
        return false;
    };
    let RadioHilColdScanHandoff {
        station_address,
        cold_mmio,
        rx: scan_rx,
        rx_storage: storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        scan_table,
        scan_frame,
        ethernet_frame,
        target,
        scan_qualified,
    } = handoff;

    // No cold MAC operation is permitted beyond this point. Consume the cold
    // owner before authentication and retain the inactive interrupt setup
    // token until WPA2 has opened the controlled port.
    let (running_mmio, interrupt_setup) = cold_mmio.into_running();
    let mmio = running_mmio;
    let mut interrupt_epoch = Esp32s31MacInterruptEpoch::new(
        EspHalMacInterruptRoute::new(open_radio_mac_interrupt, open_radio_power_interrupt),
        interrupt_setup,
        &OPEN_RADIO_IRQ_RUNTIME,
        &OPEN_RADIO_POWER_IRQ_RUNTIME,
    );
    let (sta_auth_pass, sta_assoc_pass, wpa2_message_1_pass, wpa2_message_3_pass) = match target {
        Some(access_point) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL stage=sta-target ssid={:?} bssid={:02x?} \
                 channel={} rssi={} rsn={}",
                access_point.ssid_bytes(),
                access_point.bssid,
                access_point.channel,
                access_point.rssi,
                access_point.rsn,
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL stage=wpa2-pmk-derive start iterations=4096"
            ));
            let pmk_started = Instant::now();
            let pmk_result =
                Pmk::derive(network_credentials.passphrase(), network_credentials.ssid());
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
            // Management/non-QoS traffic and every QoS TID own independent
            // twelve-bit sequence spaces. The seed is visible on air and is
            // deliberately not treated as cryptographic key material.
            let sequence_seed = (trng.random() & 0x0fff) as u16;
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL stage=sta-sequence-session seed={sequence_seed}"
            ));
            let mut sequences = StaTxSequenceCounters::new(sequence_seed);
            let target = Esp32s31StaAttemptStation {
                station_address,
                access_point,
                association_preference: STA_ASSOCIATION_PREFERENCE,
            };
            let fixture = RadioHilConnectedFixture {
                state,
                spawner,
                protocol_spawner,
                platform,
                mmio,
                interrupt_epoch: &mut interrupt_epoch,
                rx_storage: storage,
                tx_storage,
                descriptor_base,
                buffer_addresses,
                scan_table,
                frame: scan_frame,
                ethernet: ethernet_frame,
                connected_tasks: connected_task_bindings(),
                connected_rx: connected_rx_bindings(network_ipv4),
                network_report: network_report_bindings(network_ipv4),
                connected_epoch: connected_epoch_bindings(network_ipv4),
            };
            let owner = RadioHilStaLifecycleOwner::Authenticate(RadioHilAuthenticationReady {
                fixture,
                target,
                rx: scan_rx,
                network: initialize_sta_network(station_address),
                security: Esp32s31StaAttemptSecurity {
                    pmk: &pmk,
                    supplicant_nonce,
                    sequences: &mut sequences,
                    message4_protection: radio_hil_message4_protection(),
                },
            });
            let policy = StaReconnectPolicy::new(3, 100, 1_000, 100)
                .expect("fixed HIL station reconnect policy is valid");
            let station_control =
                OPEN_RADIO_STATION_CONTROL_RESOURCES.init(Esp32s31StationControlResources::new());
            let (controller, station) = prepare_esp32s31_station_task(
                Esp32s31StationConfig::new(policy).with_initial_candidate(StaNextCandidate::Reuse),
                Esp32s31StationStartResources::new(owner),
                station_control,
                RadioHilStaAttemptRunner::new(),
            )
            .unwrap_or_else(|_| panic!("HIL station control requires radio reset"));
            spawner.spawn(
                station_control_task(controller, station_epoch_coordinator())
                    .unwrap_or_else(|_| panic!("station controller task allocation failed")),
            );
            let progress = match station.run().await {
                Esp32s31StationExit::Stopped {
                    resources,
                    progress,
                    reason,
                } => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=PASS \
                         stage=production-sta-lifecycle-stop \
                         connected_epochs={} attempts={} reason={reason:?}",
                        progress.connected_epochs, progress.attempts_started,
                    ));
                    let (owner, _runner) = resources.into_parts();
                    let completed_join = progress.connected_epochs != 0;
                    let registers_reclaimed = match owner.try_reclaim_registers() {
                        Ok(_registers) => true,
                        Err(error) => {
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result=FAIL \
                                 stage=production-sta-lifecycle-reclaim error={error:?}"
                            ));
                            false
                        }
                    };
                    (
                        completed_join && registers_reclaimed,
                        completed_join,
                        completed_join,
                        completed_join,
                    )
                }
                Esp32s31StationExit::RetryExhausted {
                    resources,
                    progress,
                    failure,
                } => {
                    crate::console::publish_station_lifecycle(
                        StationLifecycleEvent::RetryExhausted {
                            generation: progress.connected_epochs,
                            attempts: progress.final_generation_attempt,
                            stage: protocol_station_failure_stage(failure.stage),
                            reason: protocol_station_failure_reason(failure.error),
                        },
                    )
                    .await;
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=OBSERVE \
                         stage=production-sta-lifecycle-exhausted \
                         connected_epochs={} attempts={} failure={failure:?}",
                        progress.connected_epochs, progress.attempts_started,
                    ));
                    let result = match failure.error {
                        RadioHilStaLifecycleFailure::Authentication => (false, false, false, false),
                        RadioHilStaLifecycleFailure::InitialJoin {
                            associated,
                            message1,
                            message3,
                        } => (true, associated, message1, message3),
                        _ => (true, true, true, true),
                    };
                    let (owner, _runner) = resources.into_parts();
                    let _owner = owner;
                    result
                }
                Esp32s31StationExit::Terminal {
                    resources,
                    progress,
                    failure,
                } => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-sta-lifecycle-terminal \
                         connected_epochs={} attempts={} failure={failure:?}",
                        progress.connected_epochs, progress.attempts_started,
                    ));
                    let (owner, _runner) = resources.into_parts();
                    let completed_join = progress.connected_epochs != 0;
                    let _owner = owner;
                    (
                        completed_join,
                        completed_join,
                        completed_join,
                        completed_join,
                    )
                }
            };
            supplicant_nonce.fill(0);
            progress
        }
        None => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-target ssid={:?}",
                network_credentials.ssid(),
            ));
            let _rx = scan_rx;
            (false, false, false, false)
        }
    };
    scan_qualified && sta_auth_pass && sta_assoc_pass && wpa2_message_1_pass && wpa2_message_3_pass
}
