#![forbid(unsafe_code)]

mod initial;
mod reconnect;
mod refresh;

use open_esp_radio::wifi::sta::station::{
    StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition, StaLifecycleStage,
    StaNextCandidate,
};
use open_esp_radio::{
    adapters::esp32s31::wifi_embassy::{
        control_tx::Esp32s31ControlTx, phy_delay::EmbassyEsp32s31PhyDelay as EmbassyPhyDelay,
        preconnected_rx::EmbassyEsp32s31PreconnectedRxDelay,
        sta_attempt_target::Esp32s31StaAttemptTargetOwner, tx_time::EmbassyWifiTxTimer,
    },
    esp32s31::{
        phy::PhyTxTargetPowerProfile,
        wifi::{
            lmac::{
                crypto::CcmpKeyHardware,
                he::He20PeerHardware,
                init::{StaLinkRxPolicyHardware, StaNoiseFloorHardware},
                rate_control::BeamformingReportHardware,
                rx::RxDma,
                tx::TxHardware,
            },
            sta::channel::Esp32s31ScanPhy,
        },
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;

use crate::{
    console::emergency_log,
    radio_hil::{
        HilPhyObserver, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE, RX_DESCRIPTOR_COUNT,
        RadioHilConnectedEpochReturn, RadioHilConnectedExit, RadioHilRunningScanReady,
        RadioHilStaJoinObserver, RadioHilStaLifecycleFailure, RadioHilStaLifecycleOwner,
        StaJoinTarget, TX_BUFFER_SIZE,
    },
};

type ControlTx = Esp32s31ControlTx<
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    EmbassyWifiTxTimer,
    TX_BUFFER_SIZE,
>;
type RadioHilStaAttemptChannel<'state> =
    Esp32s31ScanPhy<'state, EspHalRadioPeripheral, HilPhyObserver, EmbassyPhyDelay>;
type RadioHilStaAttemptOwner<'hardware, 'transmit, 'state, 'scratch, 'security, H> =
    Esp32s31StaAttemptTargetOwner<
        'hardware,
        'transmit,
        'static,
        'scratch,
        'security,
        H,
        RadioHilStaAttemptChannel<'state>,
        EmbassyEsp32s31PreconnectedRxDelay,
        ControlTx,
        RadioHilStaJoinObserver,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
        RX_BUFFER_STORAGE_SIZE,
    >;

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
    target: StaJoinTarget,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let RadioHilConnectedEpochReturn {
        fixture,
        disconnected,
        security,
        exit,
    } = returned;
    let owner = RadioHilRunningScanReady {
        fixture,
        previous_target: target,
        disconnected,
        security,
    };
    match exit {
        RadioHilConnectedExit::Disconnected { .. } | RadioHilConnectedExit::ReconnectRequested => {
            StaAttemptOutcome::Disconnected {
                owner: RadioHilStaLifecycleOwner::RunningScan(owner),
                next_candidate: StaNextCandidate::Refresh,
            }
        }
        RadioHilConnectedExit::StationStopped(command) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-station-stop command={command:?}"
            ));
            StaAttemptOutcome::Stopped {
                owner: RadioHilStaLifecycleOwner::RunningScan(owner),
            }
        }
        RadioHilConnectedExit::InjectedTxFault { .. } | RadioHilConnectedExit::HardwareFailure => {
            StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::RunningScan(owner),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    RadioHilStaLifecycleFailure::ConnectedHardware,
                ),
            }
        }
    }
}
