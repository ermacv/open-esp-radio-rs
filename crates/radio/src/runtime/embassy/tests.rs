use core::{
    cell::Cell,
    future::Future,
    num::NonZeroU16,
    sync::atomic::{AtomicU8, Ordering},
};
use std::rc::Rc;

use embassy_futures::select::{Either, select};
use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::signal::Signal;
use oer_wifi_sta::station::StaReconnectPolicy;
use oer_wpa2::Pmk;
use {oer_ieee80211::channel::WifiChannel, oer_ieee80211::station::association::Preference};

use crate::wifi::{
    MonitorCapturePolicy, StationScanChannels, StationScanPolicy, StationSecurity,
    WIFI_SCAN_RESULT_CAPACITY, WifiMacAddress, WifiMonitorConfig, WifiScanResult, WifiSsid,
    WifiStartReport, WifiStationConfig,
};

use super::*;
use crate::wifi::test_support::TEST_CAPABILITIES;

fn station_request() -> StationRequest {
    StationRequest::new(
        WifiSsid::new(b"mailbox").unwrap(),
        StationSecurity::wpa2_personal(Pmk::derive(b"password", b"mailbox").unwrap()),
        StaReconnectPolicy::new(2, 10, 100, 10).unwrap(),
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(10).unwrap(),
            Preference::Automatic,
        ),
    )
}

fn scan_request() -> WifiScanRequest {
    WifiScanRequest::new(
        StationScanChannels::CHANNELS_1_TO_13,
        NonZeroU16::new(10).unwrap(),
    )
}

fn run<F: Future>(future: F) -> F::Output {
    block_on(future)
}

struct TestRoleControl<'a> {
    stop: &'a Signal<NoopRawMutex, ()>,
    stage: &'a AtomicU8,
}

impl EmbassyWifiActiveRoleControl for TestRoleControl<'_> {
    fn request_stop(&mut self) {
        self.stage.store(1, Ordering::Release);
        self.stop.signal(());
    }
}

struct RetainedCycleControls {
    started: Signal<NoopRawMutex, ()>,
    release: Signal<NoopRawMutex, ()>,
    completed: Signal<NoopRawMutex, ()>,
}

impl RetainedCycleControls {
    const fn new() -> Self {
        Self {
            started: Signal::new(),
            release: Signal::new(),
            completed: Signal::new(),
        }
    }
}

struct FakeLocalEpochRunner {
    retained_cycle: Option<Rc<RetainedCycleControls>>,
}

impl FakeLocalEpochRunner {
    const fn immediate() -> Self {
        Self {
            retained_cycle: None,
        }
    }

    fn with_retained_cycle(controls: Rc<RetainedCycleControls>) -> Self {
        Self {
            retained_cycle: Some(controls),
        }
    }
}

impl EmbassyWifiRoleEpochRunner<NoopRawMutex> for FakeLocalEpochRunner {
    type Stopped = Rc<Cell<u8>>;
    type Faulted = Rc<Cell<u8>>;
    type LifecycleFaulted = Rc<Cell<u8>>;
    type Error = &'static str;

    fn planning_error(&mut self, _error: WifiServicePlanningError) -> Self::Error {
        "planning"
    }

    fn fault_error(&mut self, _faulted: &Self::Faulted) -> Self::Error {
        "faulted"
    }

    fn lifecycle_fault_error(&mut self, _faulted: &Self::LifecycleFaulted) -> Self::Error {
        "faulted"
    }

    fn run_epoch<'a>(
        &'a mut self,
        endpoint: &'a mut EmbassyWifiSupervisorEndpoint<'_, NoopRawMutex, Self::Error>,
        stopped: Self::Stopped,
        service: WifiServiceRequest,
        generation: crate::wifi::RadioSubsystemGeneration,
    ) -> impl Future<Output = EmbassyWifiRoleEpochOutcome<Self::Stopped, Self::Faulted>> + 'a {
        async move {
            stopped.set(stopped.get() + 1);
            if service.scan_request().is_some() {
                endpoint
                    .respond(EmbassyWifiSupervisorResponse::Scan(Ok(
                        WifiScanReport::new(
                            generation,
                            [WifiScanResult::EMPTY; WIFI_SCAN_RESULT_CAPACITY],
                            0,
                            0,
                            0,
                        ),
                    )))
                    .await;
                return EmbassyWifiRoleEpochOutcome::Stopped(stopped);
            }
            let response = if service.station_request().is_some() {
                EmbassyWifiSupervisorResponse::Station(Ok(WifiStartReport::new(generation)))
            } else if service.monitor_request().is_some() {
                EmbassyWifiSupervisorResponse::Monitor(Ok(WifiStartReport::new(generation)))
            } else {
                return EmbassyWifiRoleEpochOutcome::Faulted(stopped);
            };
            endpoint.respond(response).await;

            match endpoint.receive().await {
                EmbassyWifiSupervisorCommand::Stop => {
                    // In a real runner this response is emitted through
                    // `finish_embassy_wifi_active_role` after classifying
                    // the concrete owner-bearing task exit.
                    endpoint
                        .respond(EmbassyWifiSupervisorResponse::Stop(Ok(
                            WifiStopReport::new(generation),
                        )))
                        .await;
                    EmbassyWifiRoleEpochOutcome::Stopped(stopped)
                }
                _ => EmbassyWifiRoleEpochOutcome::Faulted(stopped),
            }
        }
    }

    fn restart_radio<'a>(
        &'a mut self,
        slot: &'a mut Option<Self::Stopped>,
    ) -> impl Future<Output = Result<WifiRadioCalibrationPath, Self::LifecycleFaulted>> + 'a {
        async move {
            let stopped = slot
                .take()
                .expect("test actor retains stopped owner between epochs");
            stopped.set(stopped.get() + 10);
            *slot = Some(stopped);
            Ok(WifiRadioCalibrationPath::RestoredCache)
        }
    }

    fn cycle_retained_radio<'a>(
        &'a mut self,
        slot: &'a mut Option<Self::Stopped>,
    ) -> impl Future<Output = Result<(), Self::LifecycleFaulted>> + 'a {
        async move {
            let stopped = slot
                .take()
                .expect("test actor retains stopped owner between epochs");
            if let Some(controls) = &self.retained_cycle {
                controls.started.signal(());
                controls.release.wait().await;
            }
            stopped.set(stopped.get() + 20);
            *slot = Some(stopped);
            if let Some(controls) = &self.retained_cycle {
                controls.completed.signal(());
            }
            Ok(())
        }
    }
}

#[test]
fn controller_and_supervisor_exchange_typed_station_completion() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();

    let application = async {
        radio
            .into_wifi()
            .start_station(station_request())
            .await
            .unwrap()
    };
    let supervisor = async {
        let EmbassyWifiSupervisorCommand::StartStation(request) = endpoint.receive().await else {
            panic!("expected station request")
        };
        assert_eq!(request.ssid().as_bytes(), b"mailbox");
        endpoint
            .respond(EmbassyWifiSupervisorResponse::Station(Ok(
                WifiStartReport::new(crate::wifi::RadioSubsystemGeneration::INITIAL),
            )))
            .await;
    };
    let (report, ()) = run(join(application, supervisor));
    assert_eq!(
        report.generation(),
        crate::wifi::RadioSubsystemGeneration::INITIAL
    );
}

#[test]
fn stopped_dispatch_validates_before_returning_a_start_transaction() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES).with_station(
        WifiStationConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap()),
    );

    let application = async {
        radio
            .into_wifi()
            .start_station(station_request())
            .await
            .unwrap()
    };
    let supervisor = async {
        let dispatch = dispatch_embassy_wifi_stopped_command(
            &mut endpoint,
            configuration,
            crate::wifi::RadioSubsystemGeneration::INITIAL,
            |_| (),
        )
        .await;
        let EmbassyWifiStoppedDispatch::Start(service) = dispatch else {
            panic!("valid provisioned station request must be dispatched")
        };
        assert_eq!(
            service.station_request().unwrap().ssid().as_bytes(),
            b"mailbox"
        );
        endpoint
            .respond(EmbassyWifiSupervisorResponse::Station(Ok(
                WifiStartReport::new(crate::wifi::RadioSubsystemGeneration::INITIAL.next()),
            )))
            .await;
    };
    let (report, ()) = run(join(application, supervisor));
    assert_eq!(report.generation().value(), 1);
}

#[test]
fn stopped_dispatch_rejects_unprovisioned_start_without_moving_a_request() {
    let resources =
        EmbassyWifiSupervisorControlResources::<NoopRawMutex, WifiServicePlanningError>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES);

    let application = async { radio.into_wifi().start_station(station_request()).await };
    let supervisor = async {
        let dispatch = dispatch_embassy_wifi_stopped_command(
            &mut endpoint,
            configuration,
            crate::wifi::RadioSubsystemGeneration::INITIAL,
            |error| error,
        )
        .await;
        assert!(matches!(dispatch, EmbassyWifiStoppedDispatch::Handled));
    };
    let (result, ()) = run(join(application, supervisor));
    match result {
        Err(crate::wifi::WifiRoleStartFailure::Rejected {
            wifi: _,
            request,
            error:
                EmbassyWifiSupervisorError::Service(WifiServicePlanningError::StationNotProvisioned),
        }) => assert_eq!(request.ssid().as_bytes(), b"mailbox"),
        _ => panic!("planning rejection must return the exact station request"),
    }
}

#[test]
fn supervisor_actor_keeps_a_non_send_owner_across_role_epochs() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES)
        .with_station(WifiStationConfig::new(
            WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap(),
        ))
        .with_standalone_scan()
        .with_standalone_monitor();
    let owner = Rc::new(Cell::new(0));
    let observed_owner = Rc::clone(&owner);
    let (radio, task) = match prepare_embassy_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::immediate(),
        owner,
    ) {
        Ok(prepared) => prepared,
        Err(_) => panic!("fresh supervisor resources must prepare once"),
    };

    let application = async {
        let scan = radio.into_wifi().scan(scan_request()).await.unwrap();
        let scan_generation = scan.report.generation().value();
        let station = scan.wifi.start_station(station_request()).await.unwrap();
        let station_generation = station.generation().value();
        let wifi = station.stop().await.unwrap();
        let monitor = wifi
            .start_monitor(MonitorRequest::new(
                WifiChannel::mhz20(6).unwrap(),
                WifiMonitorConfig::normalized(),
            ))
            .await
            .unwrap();
        let monitor_generation = monitor.generation().value();
        let _wifi = monitor.stop().await.unwrap();
        (scan_generation, station_generation, monitor_generation)
    };
    let Either::First(generations) = run(select(application, task.run()));
    assert_eq!(generations, (1, 2, 3));
    assert_eq!(observed_owner.get(), 3);
}

#[test]
fn idle_restart_runs_in_the_owner_actor_and_advances_generation() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES).with_station(
        WifiStationConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap()),
    );
    let owner = Rc::new(Cell::new(0));
    let observed_owner = Rc::clone(&owner);
    let (radio, task) = prepare_embassy_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::immediate(),
        owner,
    )
    .unwrap_or_else(|_| panic!("fresh supervisor must prepare"));

    let application = async {
        let (wifi, restart) = radio.into_wifi().restart_radio().await.unwrap();
        assert_eq!(
            restart.calibration_path(),
            WifiRadioCalibrationPath::RestoredCache
        );
        let station = wifi.start_station(station_request()).await.unwrap();
        (restart.generation().value(), station.generation().value())
    };
    let Either::First(generations) = run(select(application, task.run()));
    assert_eq!(generations, (1, 2));
    assert_eq!(observed_owner.get(), 11);
}

#[test]
fn idle_retained_cycle_runs_in_the_owner_actor_and_advances_generation() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES).with_station(
        WifiStationConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap()),
    );
    let owner = Rc::new(Cell::new(0));
    let observed_owner = Rc::clone(&owner);
    let (radio, task) = prepare_embassy_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::immediate(),
        owner,
    )
    .unwrap_or_else(|_| panic!("fresh supervisor must prepare"));

    let application = async {
        let (wifi, retained) = radio.into_wifi().cycle_retained_radio().await.unwrap();
        let station = wifi.start_station(station_request()).await.unwrap();
        (retained.generation().value(), station.generation().value())
    };
    let Either::First(generations) = run(select(application, task.run()));
    assert_eq!(generations, (1, 2));
    assert_eq!(observed_owner.get(), 21);
}

#[test]
fn supervisor_prepare_failure_returns_runner_and_stopped_owner() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES);
    let (_radio, _task) = prepare_embassy_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::immediate(),
        Rc::new(Cell::new(1)),
    )
    .unwrap_or_else(|_| panic!("fresh supervisor resources must prepare once"));

    let retained = Rc::new(Cell::new(7));
    let failure = match prepare_embassy_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::immediate(),
        Rc::clone(&retained),
    ) {
        Ok(_) => panic!("one static control domain cannot prepare two actors"),
        Err(failure) => failure,
    };
    assert_eq!(failure.error(), EmbassyWifiSupervisorControlError::InUse);
    let (returned_configuration, _runner, returned_owner) = failure.into_parts();
    assert_eq!(returned_configuration, configuration);
    assert!(Rc::ptr_eq(&returned_owner, &retained));
}

#[test]
fn cancelled_retained_cycle_caller_does_not_cancel_the_owner_actor() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES).with_station(
        WifiStationConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap()),
    );
    let owner = Rc::new(Cell::new(0));
    let observed_owner = Rc::clone(&owner);
    let controls = Rc::new(RetainedCycleControls::new());
    let (radio, task) = prepare_embassy_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::with_retained_cycle(Rc::clone(&controls)),
        owner,
    )
    .unwrap_or_else(|_| panic!("fresh supervisor must prepare"));

    let application = async {
        let mut port = radio.into_wifi().into_port();
        let retained_cycle = port.cycle_retained_radio();
        assert!(matches!(
            select(retained_cycle, controls.started.wait()).await,
            Either::Second(())
        ));
        controls.release.signal(());
        controls.completed.wait().await;

        let station = port.start_station(station_request()).await.unwrap();
        station.generation().value()
    };
    let Either::First(generation) = run(select(application, task.run()));
    assert_eq!(generation, 2);
    assert_eq!(observed_owner.get(), 21);
}

#[test]
fn disappearing_supervisor_wakes_an_inflight_controller() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let application = async { radio.into_wifi().start_station(station_request()).await };
    let supervisor = async {
        let _ = endpoint.receive().await;
        drop(endpoint);
    };
    let (result, ()) = run(join(application, supervisor));
    assert!(matches!(
        result,
        Err(crate::wifi::WifiRoleStartFailure::Faulted {
            error: EmbassyWifiSupervisorError::SupervisorUnavailable
        })
    ));
}

#[test]
fn cancelled_transport_future_cannot_poison_the_next_command() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let mut port = radio.into_wifi().into_port();
    let cancel = Signal::<NoopRawMutex, ()>::new();
    let cancelled = Signal::<NoopRawMutex, ()>::new();

    let application = async {
        let first = port.start_station(station_request());
        assert!(matches!(
            select(first, cancel.wait()).await,
            Either::Second(())
        ));
        cancelled.signal(());

        let result = port
            .start_monitor(MonitorRequest::new(
                WifiChannel::mhz20(6).unwrap(),
                WifiMonitorConfig::normalized(),
            ))
            .await;
        assert!(result.is_ok());
    };
    let supervisor = async {
        assert!(matches!(
            endpoint.receive().await,
            EmbassyWifiSupervisorCommand::StartStation(_)
        ));
        cancel.signal(());
        cancelled.wait().await;
        endpoint
            .respond(EmbassyWifiSupervisorResponse::Station(Ok(
                WifiStartReport::new(crate::wifi::RadioSubsystemGeneration::INITIAL),
            )))
            .await;

        assert!(matches!(
            endpoint.receive().await,
            EmbassyWifiSupervisorCommand::StartMonitor(_)
        ));
        endpoint
            .respond(EmbassyWifiSupervisorResponse::Monitor(Ok(
                WifiStartReport::new(crate::wifi::RadioSubsystemGeneration::INITIAL.next()),
            )))
            .await;
    };

    run(join(application, supervisor));
}

#[test]
fn monitor_policy_remains_an_owned_typed_command() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let request = MonitorRequest::new(
        WifiChannel::mhz20(11).unwrap(),
        WifiMonitorConfig::normalized(),
    )
    .with_capture_policy(MonitorCapturePolicy::truncate_at(
        NonZeroU16::new(256).unwrap(),
    ));
    let application = async { radio.into_wifi().start_monitor(request).await };
    let supervisor = async {
        let EmbassyWifiSupervisorCommand::StartMonitor(request) = endpoint.receive().await else {
            panic!("expected monitor request")
        };
        assert_eq!(request.channel().primary(), 11);
        assert_eq!(request.capture_policy().snapshot_length(), Some(256));
        endpoint
            .respond(EmbassyWifiSupervisorResponse::Monitor(Ok(
                WifiStartReport::new(crate::wifi::RadioSubsystemGeneration::INITIAL),
            )))
            .await;
    };
    let (result, ()) = run(join(application, supervisor));
    assert!(result.is_ok());
}

#[test]
fn active_role_stop_is_acknowledged_only_after_owner_future_returns() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let stop = Signal::<NoopRawMutex, ()>::new();
    let stage = AtomicU8::new(0);
    let mut control = TestRoleControl {
        stop: &stop,
        stage: &stage,
    };

    let application = async {
        let mut port = radio.into_wifi().into_port();
        port.stop().await.unwrap()
    };
    let supervisor = async {
        let role = async {
            stop.wait().await;
            stage.store(2, Ordering::Release);
            "returned-owner"
        };
        let exit = drive_embassy_wifi_active_role(&mut endpoint, &mut control, role, |_| {
            panic!("no start command expected")
        })
        .await;
        assert!(exit.stop_requested());
        assert_eq!(stage.load(Ordering::Acquire), 2);

        let frontier = finish_embassy_wifi_active_role(
            &mut endpoint,
            crate::wifi::RadioSubsystemGeneration::INITIAL,
            exit,
            EmbassyWifiRoleFrontier::<_, ()>::Stopped,
            |_| unreachable!("the test role returned a stopped owner"),
        )
        .await;
        assert!(matches!(
            frontier,
            EmbassyWifiRoleFrontier::Stopped("returned-owner")
        ));
    };
    let (report, ()) = run(join(application, supervisor));
    assert_eq!(
        report.generation(),
        crate::wifi::RadioSubsystemGeneration::INITIAL
    );
    assert_eq!(stage.load(Ordering::Acquire), 2);
}

#[test]
fn faulted_role_never_produces_a_successful_stop_response() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let stop = Signal::<NoopRawMutex, ()>::new();
    let stage = AtomicU8::new(0);
    let mut control = TestRoleControl {
        stop: &stop,
        stage: &stage,
    };

    let application = async {
        let mut port = radio.into_wifi().into_port();
        port.stop().await
    };
    let supervisor = async {
        let role = async {
            stop.wait().await;
            "retained-faulted-owner"
        };
        let exit = drive_embassy_wifi_active_role(&mut endpoint, &mut control, role, |_| {
            unreachable!("no start command expected")
        })
        .await;
        finish_embassy_wifi_active_role(
            &mut endpoint,
            crate::wifi::RadioSubsystemGeneration::INITIAL,
            exit,
            EmbassyWifiRoleFrontier::<(), _>::Faulted,
            |owner| {
                assert_eq!(*owner, "retained-faulted-owner");
                "faulted"
            },
        )
        .await
    };

    let (result, frontier) = run(join(application, supervisor));
    assert_eq!(result, Err(EmbassyWifiSupervisorError::Service("faulted")));
    assert!(matches!(
        frontier,
        EmbassyWifiRoleFrontier::Faulted("retained-faulted-owner")
    ));
}

#[test]
fn active_role_rejects_a_new_start_with_the_untouched_request() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let stop = Signal::<NoopRawMutex, ()>::new();
    let finish = Signal::<NoopRawMutex, ()>::new();
    let stage = AtomicU8::new(0);
    let mut control = TestRoleControl {
        stop: &stop,
        stage: &stage,
    };

    let application = async { radio.into_wifi().start_station(station_request()).await };
    let supervisor = async {
        let role = async {
            finish.wait().await;
            "returned-owner"
        };
        let exit = drive_embassy_wifi_active_role(&mut endpoint, &mut control, role, |kind| {
            assert_eq!(kind, EmbassyWifiStartKind::Station);
            finish.signal(());
            "already-running"
        })
        .await;
        assert_eq!(exit.output(), &"returned-owner");
        assert!(!exit.stop_requested());
    };

    let (result, ()) = run(join(application, supervisor));
    match result {
        Err(crate::wifi::WifiRoleStartFailure::Rejected {
            wifi: _,
            request,
            error: EmbassyWifiSupervisorError::Service("already-running"),
        }) => assert_eq!(request.ssid().as_bytes(), b"mailbox"),
        _ => panic!("active role must return the exact rejected request"),
    }
}

#[test]
fn active_role_identifies_a_rejected_retained_cycle() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let stop = Signal::<NoopRawMutex, ()>::new();
    let finish = Signal::<NoopRawMutex, ()>::new();
    let stage = AtomicU8::new(0);
    let mut control = TestRoleControl {
        stop: &stop,
        stage: &stage,
    };

    let application = async {
        match radio.into_wifi().cycle_retained_radio().await {
            Ok(_) => panic!("an active role cannot enter a retained RF cycle"),
            Err(error) => error,
        }
    };
    let supervisor = async {
        let role = async {
            finish.wait().await;
            "returned-owner"
        };
        let exit = drive_embassy_wifi_active_role(&mut endpoint, &mut control, role, |kind| {
            assert_eq!(kind, EmbassyWifiStartKind::WholeRadioRetainedCycle);
            finish.signal(());
            "already-running"
        })
        .await;
        assert_eq!(exit.output(), &"returned-owner");
        assert!(!exit.stop_requested());
    };

    let (error, ()) = run(join(application, supervisor));
    assert_eq!(
        error,
        EmbassyWifiSupervisorError::Service("already-running")
    );
}

#[test]
fn mailbox_storage_cannot_be_split_twice() {
    let resources = EmbassyWifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let _ = resources.split().unwrap();
    assert!(matches!(
        resources.split(),
        Err(EmbassyWifiSupervisorControlError::InUse)
    ));
}
