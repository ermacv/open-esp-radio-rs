#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, with_timeout};
use open_esp_radio::{
    esp32s31::wifi::mac::tx::TxCompletion,
    esp32s31::wifi::sta::{
        association::Esp32s31StaAssociationProfile, join::Esp32s31StaJoinObserver,
    },
};
use open_esp_radio_hil_protocol::StationEpochEvidence;

use crate::console::StationControlRequest;
use crate::console::emergency_log;
use crate::radio_hil::RadioHilStationController;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::radio_hil) enum RadioHilStationEpochProgress {
    RunnerStopped,
    ScanOwnersReturned,
    JoinCompleted,
    ConnectedRunnerStarted,
}

pub(in crate::radio_hil) type RadioHilStationEpochProgressChannel =
    Channel<CriticalSectionRawMutex, RadioHilStationEpochProgress, 4>;

#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilStationEpochReporter {
    active: &'static AtomicBool,
    progress: &'static RadioHilStationEpochProgressChannel,
}

impl RadioHilStationEpochReporter {
    pub(in crate::radio_hil) const fn new(
        active: &'static AtomicBool,
        progress: &'static RadioHilStationEpochProgressChannel,
    ) -> Self {
        Self { active, progress }
    }

    pub(in crate::radio_hil) fn report(self, progress: RadioHilStationEpochProgress) {
        if self.active.load(Ordering::Acquire) && self.progress.try_send(progress).is_err() {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-station-epoch-evidence \
                 error=progress-queue-full progress={progress:?}"
            ));
        }
    }
}

/// HIL command/evidence coordinator for explicit station epoch cycling.
///
/// It owns no station resources: the production controller remains the only
/// source of reconnect commands and the production lifecycle reports each
/// ownership frontier through the paired reporter.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilStationEpochCoordinator {
    active: &'static AtomicBool,
    progress: &'static RadioHilStationEpochProgressChannel,
    stop_request: &'static AtomicU32,
    internal_connected: &'static AtomicBool,
}

impl RadioHilStationEpochCoordinator {
    pub(in crate::radio_hil) const fn new(
        active: &'static AtomicBool,
        progress: &'static RadioHilStationEpochProgressChannel,
        stop_request: &'static AtomicU32,
        internal_connected: &'static AtomicBool,
    ) -> Self {
        Self {
            active,
            progress,
            stop_request,
            internal_connected,
        }
    }

    pub(in crate::radio_hil) fn take_stop_request(self) -> Option<u32> {
        let request_id = self.stop_request.swap(0, Ordering::AcqRel);
        (request_id != 0).then_some(request_id)
    }

    pub(in crate::radio_hil) fn begin_internal_epoch(self) {
        while self.progress.try_receive().is_ok() {}
        self.internal_connected.store(false, Ordering::Release);
        self.active.store(true, Ordering::Release);
    }

    pub(in crate::radio_hil) async fn receive_progress(self) -> RadioHilStationEpochProgress {
        self.progress.receive().await
    }

    pub(in crate::radio_hil) fn finish_internal_epoch(self, connected: bool) {
        self.internal_connected.store(connected, Ordering::Release);
        self.active.store(false, Ordering::Release);
    }

    pub(in crate::radio_hil) fn internal_epoch_reached_connected(self) -> bool {
        self.internal_connected.load(Ordering::Acquire)
    }
}

#[embassy_executor::task]
pub(in crate::radio_hil) async fn station_restart_control_task(
    mut controller: RadioHilStationController<'static>,
    coordinator: RadioHilStationEpochCoordinator,
) {
    coordinator.begin_internal_epoch();
    let connected = with_timeout(Duration::from_secs(30), async {
        loop {
            if coordinator.receive_progress().await
                == RadioHilStationEpochProgress::ConnectedRunnerStarted
            {
                break;
            }
        }
    })
    .await
    .is_ok();
    coordinator.finish_internal_epoch(connected);
    let queued = controller.request_stop();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=OBSERVE \
         stage=production-station-restart-controller connected={} command=stop queued={}",
        u8::from(connected),
        u8::from(queued),
    ));
    let completion = controller.wait_completion().await;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=OBSERVE \
         stage=production-station-restart-controller completion={completion:?}"
    ));
}

#[embassy_executor::task]
pub(in crate::radio_hil) async fn station_control_task(
    controller: RadioHilStationController<'static>,
    coordinator: RadioHilStationEpochCoordinator,
) {
    loop {
        let request_id = match crate::console::receive_station_control_request().await {
            StationControlRequest::Cycle { request_id } => request_id,
            StationControlRequest::Stop { request_id } => {
                coordinator
                    .stop_request
                    .store(request_id, Ordering::Release);
                let queued = controller.request_stop();
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=OBSERVE \
                     stage=production-station-controller command=stop queued={}",
                    u8::from(queued),
                ));
                return;
            }
        };
        let was_active = coordinator.active.swap(true, Ordering::AcqRel);
        if was_active {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-station-epoch-evidence error=overlapping-request"
            ));
        }
        let queued = controller.request_reconnect();
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=OBSERVE \
             stage=production-station-controller command=reconnect queued={}",
            u8::from(queued),
        ));
        let mut evidence = StationEpochEvidence {
            runner_stopped: false,
            scan_owners_returned: false,
            join_completed: false,
            connected_runner_started: false,
        };
        loop {
            match coordinator.progress.receive().await {
                RadioHilStationEpochProgress::RunnerStopped => evidence.runner_stopped = true,
                RadioHilStationEpochProgress::ScanOwnersReturned => {
                    evidence.scan_owners_returned = true;
                }
                RadioHilStationEpochProgress::JoinCompleted => evidence.join_completed = true,
                RadioHilStationEpochProgress::ConnectedRunnerStarted => {
                    evidence.connected_runner_started = true;
                    coordinator.active.store(false, Ordering::Release);
                    crate::console::complete_station_epoch_cycle(request_id, evidence).await;
                    break;
                }
            }
        }
    }
}

/// HIL diagnostics attached to the production join port. These callbacks do
/// not select policy, access DMA ownership or wrap a driver transaction.
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::radio_hil) struct RadioHilStaJoinObserver;

impl Esp32s31StaJoinObserver for RadioHilStaJoinObserver {
    fn authentication_transmitted(&mut self, _completion: TxCompletion) {}

    fn association_profile_selected(&mut self, profile: Esp32s31StaAssociationProfile) {
        let (Some(power), Some(capability), Some(rate_power)) = (
            profile.power_capability,
            profile.he_ul_mu_power,
            profile.rate_16_through_25,
        ) else {
            return;
        };
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=sta-he-ul-mu-power \
             minimum_dbm={} maximum_dbm={} rate_16_through_25={rate_power:?} \
             relative_to_rate_16={:?}",
            power.minimum_dbm(),
            power.maximum_dbm(),
            capability.relative_to_rate_16(),
        ));
    }
}
