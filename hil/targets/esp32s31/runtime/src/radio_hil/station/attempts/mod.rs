#![forbid(unsafe_code)]

mod initial;
mod reconnect;
mod refresh;

use crate::{
    console::emergency_log,
    radio_hil::{
        RadioHilConnectedEpochReturn, RadioHilConnectedExit, RadioHilStaLifecycleFailure,
        RadioHilStaLifecycleOwner, RadioHilStationPhase,
    },
};
use open_esp_radio::esp32s31::wifi::{
    mac::{
        crypto::CcmpKeyHardware,
        he::He20PeerHardware,
        init::{StaLinkRxPolicyHardware, StaNoiseFloorHardware},
        rate_control::BeamformingReportHardware,
        rx::RxDma,
        tx::TxHardware,
    },
    sta::attempt::Esp32s31StaAttemptStation,
};
use open_esp_radio::wifi::sta::station::{
    StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition, StaLifecycleStage,
    StaNextCandidate,
};

fn assert_join_hardware_capabilities<
    H: RxDma
        + TxHardware
        + CcmpKeyHardware
        + He20PeerHardware
        + BeamformingReportHardware
        + StaLinkRxPolicyHardware
        + StaNoiseFloorHardware,
>(
    _: &H,
) {
}

pub(in crate::radio_hil) use initial::run_initial_station_attempt;
pub(in crate::radio_hil) use reconnect::run_reconnected_station_attempt;
pub(in crate::radio_hil) use refresh::run_running_scan_attempt;

fn connected_attempt_outcome<'fixture, 'security>(
    returned: RadioHilConnectedEpochReturn<'fixture, 'security>,
    target: Esp32s31StaAttemptStation,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let RadioHilConnectedEpochReturn {
        fixture,
        disconnected,
        security,
        exit,
    } = returned;
    let owner = RadioHilStaLifecycleOwner::new(
        fixture,
        RadioHilStationPhase::RunningScan {
            disconnected,
            station: target,
        },
        security,
    );
    match exit {
        RadioHilConnectedExit::Disconnected { .. } | RadioHilConnectedExit::ReconnectRequested => {
            StaAttemptOutcome::Disconnected {
                owner,
                next_candidate: StaNextCandidate::Refresh,
            }
        }
        RadioHilConnectedExit::StationStopped(command) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-station-stop command={command:?}"
            ));
            StaAttemptOutcome::Stopped { owner }
        }
        RadioHilConnectedExit::InjectedTxFault { .. } | RadioHilConnectedExit::HardwareFailure => {
            StaAttemptOutcome::Failed {
                owner,
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    RadioHilStaLifecycleFailure::ConnectedHardware,
                ),
            }
        }
    }
}
