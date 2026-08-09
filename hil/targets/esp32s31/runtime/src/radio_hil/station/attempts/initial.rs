#![forbid(unsafe_code)]

use open_esp_radio::{
    esp32s31::{hal::RadioRegisters, wifi::sta::attempt::Esp32s31StaAttemptStage},
    wifi::sta::station::{StaAttemptFailure, StaAttemptOutcome},
};
use open_esp_radio_esp32s31_wifi_embassy::{
    phy_delay::EmbassyEsp32s31PhyDelay as EmbassyPhyDelay,
    station::{
        Esp32s31StationJoinOutcome, Esp32s31StationJoinResources, run_esp32s31_station_join,
    },
};

use crate::{
    console::emergency_log,
    radio_hil::{
        HilPhyObserver, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE, RX_DESCRIPTOR_COUNT,
        RadioHilConnectedEpochResources, RadioHilConnectedServiceResources,
        RadioHilConnectedTaskFixture, RadioHilStaJoinObserver, RadioHilStaLifecycleFailure,
        RadioHilStaLifecycleOwner, RadioHilStaNetwork, RadioHilStationCommandReceiver,
        RadioHilStationPhase, WPA2_MESSAGE_4_HARDWARE_PROTECTED, run_connected_network,
    },
};
use open_esp_radio::esp32s31::wifi::sta::attempt::{
    Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStation,
};

use super::connected_attempt_outcome;

pub(in crate::radio_hil) async fn run_initial_station_attempt<'fixture, 'security>(
    mut fixture: RadioHilConnectedTaskFixture<'fixture>,
    mut hardware: RadioRegisters,
    target: Esp32s31StaAttemptStation,
    rx: crate::radio_hil::RadioHilJoinRx<'static>,
    network: RadioHilStaNetwork,
    security: Esp32s31StaAttemptSecurity<'security>,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
    generation: u32,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let (radio_resources, storage_resources, _) = fixture.split_mut();
    let (state, platform, _) = radio_resources.parts_mut();
    let (dma, tx_storage, _, frame, _) = storage_resources.parts_mut();
    let join = run_esp32s31_station_join::<
        _,
        _,
        _,
        EmbassyPhyDelay,
        _,
        _,
        RadioHilStaJoinObserver,
        _,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
        RX_BUFFER_STORAGE_SIZE,
    >(Esp32s31StationJoinResources {
        hardware: &mut hardware,
        phy: state,
        platform,
        phy_observer: HilPhyObserver,
        receive: rx,
        rx_storage: dma.storage(),
        transmit: tx_storage
            .control_mut()
            .expect("initial station attempt owns control TX"),
        frame,
        station: target,
        security,
        attempt_observer: (),
    })
    .await;
    match join {
        Esp32s31StationJoinOutcome::Failed {
            returned,
            report,
            stage,
            disposition,
            error,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-sta-attempt \
                 phase={stage:?} disposition={disposition:?} error={error:?}"
            ));
            let associated = progress.completed(Esp32s31StaAttemptStage::Association);
            let message1 = report
                .wpa2_handshake
                .is_some_and(|telemetry| telemetry.message2_transmissions != 0);
            let message3 = progress.completed(Esp32s31StaAttemptStage::Wpa2Handshake);
            let lifecycle_error = if progress.completed(Esp32s31StaAttemptStage::Authentication) {
                RadioHilStaLifecycleFailure::InitialJoin {
                    associated,
                    message1,
                    message3,
                }
            } else {
                RadioHilStaLifecycleFailure::Authentication
            };
            StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::new(
                    fixture,
                    RadioHilStationPhase::InitialJoin {
                        hardware,
                        receive: returned.receive,
                        network,
                        station: returned.station,
                    },
                    returned.security,
                ),
                failure: StaAttemptFailure::new(
                    stage.lifecycle_stage(),
                    disposition,
                    lifecycle_error,
                ),
            }
        }
        Esp32s31StationJoinOutcome::Connected {
            returned,
            peer,
            pairwise,
            group,
            report,
            progress: _,
        } => {
            let target = returned.station;
            let authentication = report
                .authentication
                .expect("successful station attempt reports Authentication");
            let association = report
                .association
                .expect("successful station attempt reports Association");
            let wpa2 = report
                .wpa2
                .expect("successful station attempt reports WPA2 key install");
            let message4 = report
                .message4
                .expect("successful station attempt reports Message 4 completion");
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=sta-auth-response \
                 attempt={} frames={} bssid={:02x?}",
                authentication.attempt,
                authentication.total_received_frames,
                target.access_point.bssid,
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=sta-assoc-response \
                 status={} aid={} frames={} bssid={:02x?}",
                association.response.status_code,
                association.response.association_id,
                association.total_received_frames,
                target.access_point.bssid,
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-4-tx \
                 protected={} replay={} status={} primary={:#010x}",
                WPA2_MESSAGE_4_HARDWARE_PROTECTED,
                wpa2.replay_counter,
                message4.status,
                message4.primary_word,
            ));
            let board = fixture.board();
            let interface = board.interface();
            let config = board.connected_station_config();
            let connected_return = run_connected_network(
                RadioHilConnectedServiceResources::new(
                    fixture,
                    RadioHilConnectedEpochResources::Initial {
                        hardware,
                        receive: returned.receive,
                    },
                    network,
                    interface,
                    config,
                    peer,
                    pairwise,
                    group,
                    returned.security,
                ),
                generation,
                station_control,
            )
            .await;
            connected_attempt_outcome(connected_return, target)
        }
    }
}
