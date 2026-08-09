#![forbid(unsafe_code)]

use open_esp_radio::{
    esp32s31::wifi::sta::attempt::{Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStation},
    wifi::sta::station::{
        StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition, StaLifecycleStage,
    },
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
        RadioHilConnectedTaskFixture, RadioHilReconnectedEpoch, RadioHilStaJoinObserver,
        RadioHilStaLifecycleFailure, RadioHilStaLifecycleOwner, RadioHilStaNetwork,
        RadioHilStationCommandReceiver, RadioHilStationEpochProgress, RadioHilStationPhase,
        run_connected_network, station_epoch_reporter,
    },
};

use super::connected_attempt_outcome;

pub(in crate::radio_hil) async fn run_reconnected_station_attempt<'fixture, 'security>(
    mut fixture: RadioHilConnectedTaskFixture<'fixture>,
    target: Esp32s31StaAttemptStation,
    mut epoch: RadioHilReconnectedEpoch,
    network: RadioHilStaNetwork,
    security: Esp32s31StaAttemptSecurity<'security>,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
    generation: u32,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let (hardware, rx_slot) = epoch.hardware_and_rx_mut();
    let receive = match rx_slot.take() {
        Ok(receive) => receive,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-attempt-rx error={error:?}"
            ));
            return StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::new(
                    fixture,
                    RadioHilStationPhase::Reconnected {
                        epoch,
                        network,
                        station: target,
                    },
                    security,
                ),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    RadioHilStaLifecycleFailure::MissingReconnectReceiveOwner,
                ),
            };
        }
    };
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
        hardware,
        phy: state,
        platform,
        phy_observer: HilPhyObserver,
        receive,
        rx_storage: dma.storage(),
        transmit: tx_storage
            .control_mut()
            .expect("reconnected station attempt owns control TX"),
        frame,
        station: target,
        security,
        attempt_observer: (),
    })
    .await;
    match join {
        Esp32s31StationJoinOutcome::Failed {
            returned,
            stage,
            disposition,
            error,
            ..
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-attempt phase={stage:?} \
                 disposition={disposition:?} error={error:?}"
            ));
            let (_, rx_slot) = epoch.hardware_and_rx_mut();
            *rx_slot = returned.receive;
            StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::new(
                    fixture,
                    RadioHilStationPhase::Reconnected {
                        epoch,
                        network,
                        station: returned.station,
                    },
                    returned.security,
                ),
                failure: StaAttemptFailure::new(
                    stage.lifecycle_stage(),
                    disposition,
                    RadioHilStaLifecycleFailure::StationAttempt(stage),
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
            let (_, rx_slot) = epoch.hardware_and_rx_mut();
            *rx_slot = returned.receive;
            let authentication = report
                .authentication
                .expect("successful reconnect reports Authentication");
            let association = report
                .association
                .expect("successful reconnect reports Association");
            let wpa2 = report
                .wpa2
                .expect("successful reconnect reports WPA2 key install");
            let message4 = report
                .message4
                .expect("successful reconnect reports Message 4 completion");
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-reconnect-authentication attempt={} frames={} bssid={:02x?}",
                authentication.attempt,
                authentication.total_received_frames,
                target.access_point.bssid,
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-reconnect-association status={} aid={} frames={}",
                association.response.status_code,
                association.response.association_id,
                association.total_received_frames,
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-reconnect-wpa2-complete replay={} \
                 message4_status={} message4_primary={:#010x} \
                 pairwise_slot={} group_slot={}",
                wpa2.replay_counter,
                message4.status,
                message4.primary_word,
                pairwise.hardware_index(),
                group.hardware_index(),
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-reconnect-connected-enter"
            ));
            station_epoch_reporter().report(RadioHilStationEpochProgress::JoinCompleted);
            let board = fixture.board();
            let interface = board.interface();
            let config = board.connected_station_config();
            let returned = run_connected_network(
                RadioHilConnectedServiceResources::new(
                    fixture,
                    RadioHilConnectedEpochResources::Reconnected(epoch),
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
            connected_attempt_outcome(returned, target)
        }
    }
}
