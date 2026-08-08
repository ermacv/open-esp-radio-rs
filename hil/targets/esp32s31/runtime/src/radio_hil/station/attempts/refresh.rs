#![forbid(unsafe_code)]

use open_esp_radio::{
    esp32s31::wifi::sta::{attempt::Esp32s31StaAttemptStation, scan::Esp32s31StaScanError},
    wifi::sta::station::{
        StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition, StaLifecycleStage,
    },
};
use open_esp_radio_esp32s31_wifi_embassy::preconnected_rx::EmbassyEsp32s31PreconnectedRxDelay;

use crate::{
    console::emergency_log,
    radio_hil::{
        RadioHilConnectedEpochResources, RadioHilReconnectReady, RadioHilRunningScanContext,
        RadioHilRunningScanFailure, RadioHilRunningScanPortError, RadioHilRunningScanReady,
        RadioHilStaLifecycleFailure, RadioHilStaLifecycleOwner, RadioHilStaNetwork,
        RadioHilStationCommandReceiver, qualify_disconnected_running_scan, station_epoch_reporter,
    },
};

use super::{assert_join_hardware_capabilities, run_reconnected_station_attempt};

/// Execute the explicit candidate-refresh phase selected by the outer STA
/// lifecycle, then continue with fresh Authentication on the returned
/// cooperative hardware owner.
pub(in crate::radio_hil) async fn run_running_scan_attempt<'fixture, 'security>(
    ready: RadioHilRunningScanReady<'fixture, 'security>,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
    generation: u32,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let RadioHilRunningScanReady {
        fixture,
        previous_target,
        disconnected,
        security,
    } = ready;
    let scan_result = qualify_disconnected_running_scan(
        disconnected,
        RadioHilRunningScanContext {
            state: &mut *fixture.state,
            platform: &mut *fixture.platform,
            tx_storage: &mut *fixture.tx_storage,
            interrupt_setup: fixture
                .interrupt_epoch
                .setup()
                .expect("connected teardown returned the quiesced interrupt owner"),
            scan_table: &mut *fixture.scan_table,
            scan_frame: &mut *fixture.frame,
            station_address: previous_target.station_address,
            target_ssid: previous_target.access_point.ssid_bytes(),
            sequence: security.sequences.non_qos_mut(),
        },
        station_epoch_reporter(),
    )
    .await;
    let scan_return = match scan_result {
        Ok(scan_return) => scan_return,
        Err(recovery) => {
            let owner = RadioHilStaLifecycleOwner::RunningScan(RadioHilRunningScanReady {
                fixture,
                previous_target,
                disconnected: recovery.disconnected,
                security,
            });
            let (disposition, error) = match recovery.failure {
                RadioHilRunningScanFailure::NoCandidate { .. } => (
                    StaFailureDisposition::RefreshCandidate,
                    RadioHilStaLifecycleFailure::RunningScanNoCandidate,
                ),
                RadioHilRunningScanFailure::Stopped { .. } => {
                    return StaAttemptOutcome::Stopped { owner };
                }
                RadioHilRunningScanFailure::Transaction { error, .. } => {
                    let disposition = match error {
                        Esp32s31StaScanError::ActiveProbe(
                            RadioHilRunningScanPortError::Transmit(_),
                        )
                        | Esp32s31StaScanError::ReceiveStop(_) => StaFailureDisposition::Terminal,
                        _ => StaFailureDisposition::RefreshCandidate,
                    };
                    (
                        disposition,
                        RadioHilStaLifecycleFailure::RunningScanTransaction(error),
                    )
                }
                RadioHilRunningScanFailure::InvalidPlan(error) => (
                    StaFailureDisposition::Terminal,
                    RadioHilStaLifecycleFailure::RunningScanPlan(error),
                ),
            };
            return StaAttemptOutcome::Failed {
                owner,
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::CandidateSelection,
                    disposition,
                    error,
                ),
            };
        }
    };
    let target = Esp32s31StaAttemptStation {
        station_address: previous_target.station_address,
        access_point: scan_return.candidate,
        association_preference: previous_target.association_preference,
    };
    assert_join_hardware_capabilities(scan_return.disconnected.hardware());
    let (network, epoch) = scan_return
        .disconnected
        .prepare_reconnect::<EmbassyEsp32s31PreconnectedRxDelay>();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-reconnect-owner-ready \
         candidate_channel={} candidate_bssid={:02x?}",
        target.access_point.channel, target.access_point.bssid,
    ));
    run_reconnected_station_attempt(
        RadioHilReconnectReady {
            fixture,
            target,
            network: RadioHilStaNetwork::Running(network),
            epoch: RadioHilConnectedEpochResources::Reconnected(epoch),
            security,
        },
        station_control,
        generation,
    )
    .await
}
