#![forbid(unsafe_code)]

use open_esp_radio::{
    adapters::esp32s31::wifi_embassy::{
        phy_delay::EmbassyEsp32s31PhyDelay as EmbassyPhyDelay,
        sta_attempt_target::{
            Esp32s31StaAttemptRadio, Esp32s31StaAttemptStorage, Esp32s31StaAttemptTargetOwner,
            Esp32s31StaAttemptTargetPort,
        },
    },
    esp32s31::wifi::sta::{
        attempt::{
            Esp32s31StaAttempt, Esp32s31StaAttemptOutcome, Esp32s31StaAttemptSecurity,
            Esp32s31StaAttemptStation,
        },
        channel::Esp32s31ScanPhy,
    },
    wifi::sta::station::{
        StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition, StaLifecycleStage,
    },
};

use crate::{
    console::emergency_log,
    radio_hil::{
        ConnectedHardware, HilPhyObserver, RadioHilConnectedEpochResources, RadioHilReconnectReady,
        RadioHilStaLifecycleFailure, RadioHilStaLifecycleOwner, RadioHilStationCommandReceiver,
        RadioHilStationEpochProgress, STA_ASSOCIATION_PREFERENCE, StaAssociationSecurity,
        StaConnectedSession, radio_hil_message4_protection, run_connected_network,
        station_epoch_reporter,
    },
};

use super::{RadioHilStaAttemptOwner, connected_attempt_outcome};

pub(in crate::radio_hil) async fn run_reconnected_station_attempt<'fixture, 'security>(
    ready: RadioHilReconnectReady<'fixture, 'security>,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
    generation: u32,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let RadioHilReconnectReady {
        fixture,
        target,
        network,
        epoch,
        security,
    } = ready;
    let RadioHilConnectedEpochResources::Reconnected(mut epoch) = epoch else {
        return StaAttemptOutcome::Failed {
            owner: RadioHilStaLifecycleOwner::Reconnect(RadioHilReconnectReady {
                fixture,
                target,
                network,
                epoch,
                security,
            }),
            failure: StaAttemptFailure::new(
                StaLifecycleStage::Hardware,
                StaFailureDisposition::Terminal,
                RadioHilStaLifecycleFailure::InvalidEpochOwner,
            ),
        };
    };
    let StaAssociationSecurity {
        pmk,
        supplicant_nonce,
        sequences,
    } = security;
    let (hardware, rx_slot) = epoch.hardware_and_rx_mut();
    let receive = match rx_slot.take() {
        Ok(receive) => receive,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-attempt-rx error={error:?}"
            ));
            return StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::Reconnect(RadioHilReconnectReady {
                    fixture,
                    target,
                    network,
                    epoch: RadioHilConnectedEpochResources::Reconnected(epoch),
                    security: StaAssociationSecurity {
                        pmk,
                        supplicant_nonce,
                        sequences,
                    },
                }),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    RadioHilStaLifecycleFailure::InvalidEpochOwner,
                ),
            };
        }
    };
    let channel = Esp32s31ScanPhy::<_, _, EmbassyPhyDelay>::new(
        &mut *fixture.state,
        &mut *fixture.platform,
        HilPhyObserver,
    );
    let owner: RadioHilStaAttemptOwner<'_, '_, '_, '_, '_, ConnectedHardware> =
        Esp32s31StaAttemptTargetOwner::new(
            Esp32s31StaAttemptRadio::new(
                hardware,
                channel,
                receive,
                fixture.rx_storage,
                fixture
                    .tx_storage
                    .control_mut()
                    .expect("reconnected station attempt owns control TX"),
            ),
            Esp32s31StaAttemptStorage::new(&mut *fixture.frame),
            Esp32s31StaAttemptStation {
                station_address: target.station_address,
                access_point: target.access_point,
                association_preference: STA_ASSOCIATION_PREFERENCE,
            },
            Esp32s31StaAttemptSecurity {
                pmk,
                supplicant_nonce,
                sequences,
                message4_protection: radio_hil_message4_protection(),
            },
        );
    let mut attempt = Esp32s31StaAttempt::new(Esp32s31StaAttemptTargetPort::new());
    match attempt.run(owner).await {
        Esp32s31StaAttemptOutcome::Failed(failure) => {
            let (owner, stage, disposition, error, _progress) = failure.into_parts();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-attempt phase={stage:?} \
                 disposition={disposition:?} error={error:?}"
            ));
            let (radio, _storage, _station, security) = owner.into_parts();
            let Esp32s31StaAttemptRadio {
                hardware: _,
                channel,
                receive,
                rx_storage: _,
                transmit: _,
            } = radio;
            let _ = channel.into_parts();
            let (_, rx_slot) = epoch.hardware_and_rx_mut();
            *rx_slot = receive;
            StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::Reconnect(RadioHilReconnectReady {
                    fixture,
                    target,
                    network,
                    epoch: RadioHilConnectedEpochResources::Reconnected(epoch),
                    security: StaAssociationSecurity {
                        pmk: security.pmk,
                        supplicant_nonce: security.supplicant_nonce,
                        sequences: security.sequences,
                    },
                }),
                failure: StaAttemptFailure::new(
                    stage.lifecycle_stage(),
                    disposition,
                    RadioHilStaLifecycleFailure::StationAttempt(stage),
                ),
            }
        }
        Esp32s31StaAttemptOutcome::Connected {
            connected,
            progress: _,
        } => {
            let mut owner = connected.into_owner();
            let report = owner.report();
            let peer = owner
                .take_connected_peer()
                .expect("successful reconnect owns its connected peer");
            let (pairwise, group) = owner
                .take_installed_keys()
                .expect("successful reconnect owns both CCMP slots");
            let (radio, _storage, _station, security) = owner.into_parts();
            let Esp32s31StaAttemptRadio {
                hardware: _,
                channel,
                receive,
                rx_storage: _,
                transmit: _,
            } = radio;
            let _ = channel.into_parts();
            let (_, rx_slot) = epoch.hardware_and_rx_mut();
            *rx_slot = receive;
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
            let returned = run_connected_network(
                fixture,
                RadioHilConnectedEpochResources::Reconnected(epoch),
                StaConnectedSession {
                    generation,
                    peer,
                    network,
                    pmk: security.pmk,
                    supplicant_nonce: security.supplicant_nonce,
                    sequences: security.sequences,
                },
                pairwise,
                group,
                station_control,
            )
            .await;
            connected_attempt_outcome(returned, target)
        }
    }
}
