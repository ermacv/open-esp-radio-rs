#![forbid(unsafe_code)]

use open_esp_radio::{
    esp32s31::wifi::sta::attempt::{Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStation},
    wifi::sta::station::StaFailureDisposition,
    wifi::sta::request::StationDiscovery,
};
use open_esp_radio_esp32s31_wifi_embassy::{
    preconnected_rx::EmbassyEsp32s31PreconnectedRxDelay,
    station::{
        Esp32s31StationRunningScanCompletion, Esp32s31StationRunningScanExit,
        complete_esp32s31_station_running_scan,
        esp32s31_station_scan_failure_disposition,
    },
};

use crate::{
    console::emergency_log,
    radio_hil::{
        RadioHilConnectedTaskFixture, RadioHilDisconnectedEpoch, RadioHilRunningScanContext,
        RadioHilRunningScanFailure, RadioHilStaLifecycleFailure, RadioHilStaLifecycleOwner,
        RadioHilStaNetwork, RadioHilStationPhase, qualify_disconnected_running_scan,
        station_epoch_reporter,
    },
};

use super::assert_join_hardware_capabilities;

/// Execute the explicit candidate-refresh phase selected by the outer STA
/// lifecycle, then continue with fresh Authentication on the returned
/// cooperative hardware owner.
pub(in crate::radio_hil) async fn run_running_scan_attempt<'fixture, 'security>(
    mut fixture: RadioHilConnectedTaskFixture<'fixture>,
    previous_target: Esp32s31StaAttemptStation,
    disconnected: RadioHilDisconnectedEpoch,
    mut security: Esp32s31StaAttemptSecurity<'security>,
    discovery: StationDiscovery,
) -> Esp32s31StationRunningScanExit<
    'security,
    RadioHilConnectedTaskFixture<'fixture>,
    crate::radio_hil::RadioHilReconnectedEpoch,
    RadioHilStaNetwork,
    RadioHilStaLifecycleOwner<'fixture, 'security>,
    RadioHilStaLifecycleFailure,
> {
    let (radio_resources, storage_resources, _) = fixture.split_mut();
    let (state, platform, interrupt_epoch) = radio_resources.parts_mut();
    let (_, tx_storage, scan_table, scan_frame, _) = storage_resources.parts_mut();
    let scan_result = qualify_disconnected_running_scan(
        disconnected,
        RadioHilRunningScanContext {
            state,
            platform,
            tx_storage,
            interrupt_setup: interrupt_epoch
                .setup()
                .expect("connected teardown returned the quiesced interrupt owner"),
            scan_table,
            scan_frame,
            station_address: previous_target.station_address,
            discovery,
            sequence: security.sequences.non_qos_mut(),
        },
        station_epoch_reporter(),
    )
    .await;
    let (disconnected, completion) = match scan_result {
        Ok(scan_return) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=production-reconnect-owner-ready \
                 candidate_channel={} candidate_bssid={:02x?}",
                scan_return.candidate.channel, scan_return.candidate.bssid,
            ));
            (
                scan_return.disconnected,
                Esp32s31StationRunningScanCompletion::Selected(scan_return.candidate),
            )
        }
        Err(recovery) => {
            let completion = match recovery.failure {
                RadioHilRunningScanFailure::NoCandidate { .. } => {
                    Esp32s31StationRunningScanCompletion::Failed {
                        disposition: StaFailureDisposition::RefreshCandidate,
                        error: RadioHilStaLifecycleFailure::RunningScanNoCandidate,
                    }
                }
                RadioHilRunningScanFailure::Stopped { .. } => {
                    Esp32s31StationRunningScanCompletion::Stopped
                }
                RadioHilRunningScanFailure::Transaction { error, .. } => {
                    let disposition = esp32s31_station_scan_failure_disposition(&error);
                    Esp32s31StationRunningScanCompletion::Failed {
                        disposition,
                        error: RadioHilStaLifecycleFailure::RunningScanTransaction(error),
                    }
                }
                RadioHilRunningScanFailure::InvalidPlan(error) => {
                    Esp32s31StationRunningScanCompletion::Failed {
                        disposition: StaFailureDisposition::Terminal,
                        error: RadioHilStaLifecycleFailure::RunningScanPlan(error),
                    }
                }
            };
            (recovery.disconnected, completion)
        }
    };
    complete_esp32s31_station_running_scan(
        fixture,
        disconnected,
        previous_target,
        security,
        completion,
        |disconnected| {
            assert_join_hardware_capabilities(disconnected.hardware());
            let (network, epoch) =
                disconnected.prepare_reconnect::<EmbassyEsp32s31PreconnectedRxDelay>();
            (RadioHilStaNetwork::Running(network), epoch)
        },
        |fixture, disconnected, station, security| {
            RadioHilStaLifecycleOwner::new(
                fixture,
                RadioHilStationPhase::RunningScan {
                    disconnected,
                    station,
                },
                security.into_role(),
            )
        },
    )
}
