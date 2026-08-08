#![forbid(unsafe_code)]

use open_esp_radio::{
    esp32s31::{
        hal::RadioRegisters,
        wifi::sta::{
            attempt::{Esp32s31StaAttempt, Esp32s31StaAttemptOutcome, Esp32s31StaAttemptStage},
            channel::Esp32s31ScanPhy,
        },
    },
    wifi::sta::station::{StaAttemptFailure, StaAttemptOutcome},
};
use open_esp_radio_esp32s31_wifi_embassy::{
    phy_delay::EmbassyEsp32s31PhyDelay as EmbassyPhyDelay,
    sta_attempt_target::{
        Esp32s31StaAttemptRadio, Esp32s31StaAttemptStorage, Esp32s31StaAttemptTargetOwner,
        Esp32s31StaAttemptTargetPort,
    },
};

use crate::{
    console::emergency_log,
    radio_hil::{
        HilPhyObserver, RadioHilAuthenticationReady, RadioHilConnectedEpochResources,
        RadioHilStaLifecycleFailure, RadioHilStaLifecycleOwner, RadioHilStationCommandReceiver,
        StaConnectedSession, WPA2_MESSAGE_4_HARDWARE_PROTECTED, run_connected_network,
    },
};

use super::{RadioHilStaAttemptOwner, connected_attempt_outcome};

pub(in crate::radio_hil) async fn run_initial_station_attempt<'fixture, 'security>(
    ready: RadioHilAuthenticationReady<'fixture, 'security>,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
    generation: u32,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let RadioHilAuthenticationReady {
        mut fixture,
        target,
        rx,
        network,
        security,
    } = ready;
    let channel = Esp32s31ScanPhy::<_, _, EmbassyPhyDelay>::new(
        &mut *fixture.state,
        &mut *fixture.platform,
        HilPhyObserver,
    );
    let owner: RadioHilStaAttemptOwner<'_, '_, '_, '_, '_, RadioRegisters> =
        Esp32s31StaAttemptTargetOwner::new(
            Esp32s31StaAttemptRadio::new(
                &mut fixture.mmio,
                channel,
                rx,
                fixture.rx_storage,
                fixture
                    .tx_storage
                    .control_mut()
                    .expect("initial station attempt owns control TX"),
            ),
            Esp32s31StaAttemptStorage::new(&mut *fixture.frame),
            target,
            security,
        );
    let mut attempt = Esp32s31StaAttempt::new(Esp32s31StaAttemptTargetPort::new());
    match attempt.run(owner).await {
        Esp32s31StaAttemptOutcome::Failed(failure) => {
            let (owner, stage, disposition, error, progress) = failure.into_parts();
            let report = owner.report();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-sta-attempt \
                 phase={stage:?} disposition={disposition:?} error={error:?}"
            ));
            let (radio, _storage, target, security) = owner.into_parts();
            let Esp32s31StaAttemptRadio {
                hardware: _,
                channel,
                receive,
                rx_storage: _,
                transmit: _,
            } = radio;
            let _ = channel.into_parts();
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
                owner: RadioHilStaLifecycleOwner::Authenticate(RadioHilAuthenticationReady {
                    fixture,
                    target,
                    rx: receive,
                    network,
                    security,
                }),
                failure: StaAttemptFailure::new(
                    stage.lifecycle_stage(),
                    disposition,
                    lifecycle_error,
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
                .expect("successful station attempt owns its connected peer");
            let (pairwise, group) = owner
                .take_installed_keys()
                .expect("successful station attempt owns both CCMP slots");
            let (radio, _storage, target, security) = owner.into_parts();
            let Esp32s31StaAttemptRadio {
                hardware: _,
                channel,
                receive,
                rx_storage: _,
                transmit: _,
            } = radio;
            let _ = channel.into_parts();
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
            let (connected_fixture, registers) = fixture.into_task_fixture();
            let returned = run_connected_network(
                connected_fixture,
                RadioHilConnectedEpochResources::Initial {
                    registers,
                    rx: receive,
                },
                StaConnectedSession {
                    generation,
                    peer,
                    network,
                    security,
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
