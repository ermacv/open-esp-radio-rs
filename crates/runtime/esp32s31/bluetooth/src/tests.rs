use core::cell::RefCell;
use std::{boxed::Box, rc::Rc, vec, vec::Vec};

use embassy_futures::{
    block_on,
    select::{Either, select},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_bluetooth_radio::{
    AcceptListChange, AcceptListDevice, AdvertisingChannel, AdvertisingChannels,
    AdvertisingConfiguration, AdvertisingEvent, AdvertisingPdu, AdvertisingReception,
    AdvertisingSetId, CoexistenceLevel, EventId, EventResult, LeInstant, LePhy, LeWindow,
    RadioDuration, RadioFault, RadioOutcome, RadioRequest, ReceivedPdu, RequestError,
    ScanFilterPolicy, ScanType, ScannerConfiguration, ScannerId, TestChannel, TestPhy, TestReceive,
    TxPower,
};
use oer_esp32s31_bluetooth::{
    ControllerTimeSample,
    controller_time::{
        ControllerTimeEventError, ControllerTimeEventStep, ControllerTimeRequest,
        ControllerTimeRequestError,
    },
    interrupt::{SchedulerWakeBatch, SchedulerWakeCell, SchedulerWorkerWakeClass},
    scheduler::{
        SchedulerFinishedListCaptureError, SchedulerFinishedListWorkerStep, SchedulerHardwareError,
        SchedulerHardwareView, SchedulerIdleInsertion, SchedulerObservation,
        SchedulerSoftwareConfig, SchedulerStartError, SchedulerStep, SchedulerTransactionFault,
        SchedulerWait,
    },
};
use oer_esp32s31_bluetooth_memory::{
    BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY, ControllerSramLinkAddress, LeDeviceTablePublication,
    LeRxChain, SchedulerItemSpace,
};
use oer_esp32s31_bluetooth_radio::validation::model_memory;
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerHalInitConfig, BluetoothControllerTimeScale,
    BluetoothSchedulerBusyObservation, BluetoothSchedulerFinishedListObservation,
    BluetoothSchedulerFinishedListPop, BluetoothSchedulerStop, BluetoothSchedulerStopStep,
    BluetoothSchedulerStopped,
};
use oer_esp32s31_hal::shared_radio::{QuiescentSpan, RadioClient};
use oer_time::Clock as _;
use oer_time_virtual::VirtualClock;

use crate::{
    BluetoothInstallError, BluetoothOutcome, BluetoothRadioHardware, BluetoothRuntime,
    BluetoothRuntimeError, RxChainPublicationError,
};

#[derive(Default)]
struct State {
    chains_published: bool,
    refuse_chains: bool,
    time: u32,
    started: Vec<ControllerSramLinkAddress>,
    stops: usize,
    defer_execution: bool,
    running: bool,
    finished: Option<BluetoothSchedulerFinishedListObservation>,
    capture_on_start: bool,
    wake: SchedulerWakeCell,
    routes_disabled: usize,
    routes_restored: usize,
    refuse_start: bool,
    scan_starts: usize,
    device_tables: Vec<LeDeviceTablePublication>,
}

/// The model's owner of a disabled BLE PHY ETM route.
#[must_use]
struct ModelRoute;

/// Hardware that executes the head item at once when it starts.
#[derive(Clone, Default)]
struct Model(Rc<RefCell<State>>);

impl BluetoothRadioHardware for Model {
    type StartError = ();
    type DisabledPhyRoute = ModelRoute;

    fn scheduler_config(&self) -> SchedulerSoftwareConfig {
        SchedulerSoftwareConfig::reviewed_standalone()
    }

    fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale()
    }

    fn local_sleep_clock_ppm(&self) -> u16 {
        500
    }

    fn request_time(&mut self) -> Result<ControllerTimeRequest, ControllerTimeRequestError> {
        Ok(ControllerTimeRequest::for_validation(1))
    }

    fn recheck_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
        Ok(ControllerTimeEventStep::Sample {
            request,
            sample: ControllerTimeSample::for_validation(self.0.borrow().time),
        })
    }

    fn drain_time(&mut self) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
        Ok(ControllerTimeEventStep::Idle)
    }

    fn publish_rx_chains<const SCANNING: usize, const NON_SCANNING: usize>(
        &mut self,
        _scanning: &LeRxChain<SCANNING>,
        _non_scanning: &LeRxChain<NON_SCANNING>,
    ) -> Result<(), RxChainPublicationError> {
        let mut state = self.0.borrow_mut();
        if state.refuse_chains {
            return Err(RxChainPublicationError::SchedulerStarted);
        }
        state.chains_published = true;
        Ok(())
    }

    fn publish_scan_start(&mut self) {
        self.0.borrow_mut().scan_starts += 1;
    }

    fn publish_device_table(&mut self, publication: LeDeviceTablePublication) {
        self.0.borrow_mut().device_tables.push(publication);
    }

    fn take_wake(&mut self) -> Option<SchedulerWakeBatch> {
        self.0.borrow().wake.take()
    }

    fn capture_finished_lists(
        &mut self,
        _wake: SchedulerWakeBatch,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        if self.0.borrow().finished.is_some() {
            return Err(SchedulerFinishedListCaptureError::DrainAlreadyActive);
        }
        self.0.borrow_mut().finished =
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0]);
        Ok(())
    }

    fn capture_stopped_finished_lists(
        &mut self,
        _stopped: &BluetoothSchedulerStopped,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        if self.0.borrow().finished.is_some() {
            return Err(SchedulerFinishedListCaptureError::DrainAlreadyActive);
        }
        self.0.borrow_mut().finished =
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[]);
        Ok(())
    }

    fn next_finished_list(&mut self) -> SchedulerFinishedListWorkerStep {
        let Some(observation) = self.0.borrow_mut().finished.take() else {
            return SchedulerFinishedListWorkerStep::Idle;
        };
        match observation.pop_lowest() {
            BluetoothSchedulerFinishedListPop::Complete => {
                SchedulerFinishedListWorkerStep::Complete
            }
            BluetoothSchedulerFinishedListPop::List {
                observed,
                remaining,
            } => {
                let more = !remaining.is_empty();
                if more {
                    self.0.borrow_mut().finished = Some(remaining);
                }
                SchedulerFinishedListWorkerStep::List { observed, more }
            }
        }
    }

    fn observe(&mut self) -> Result<SchedulerHardwareView, SchedulerHardwareError> {
        // A started item whose execution is deferred keeps the scheduler busy.
        let busy = self.0.borrow().running;
        Ok(SchedulerHardwareView {
            busy: BluetoothSchedulerBusyObservation::from_busy_for_validation(busy),
            hardware_head: None,
        })
    }

    fn disable_phy_etm_route(&mut self) -> ModelRoute {
        self.0.borrow_mut().routes_disabled += 1;
        ModelRoute
    }

    fn restore_phy_etm_route(&mut self, route: ModelRoute) {
        let ModelRoute = route;
        self.0.borrow_mut().routes_restored += 1;
    }

    fn start(
        &mut self,
        items: &SchedulerItemSpace<'_>,
        insertion: SchedulerIdleInsertion,
    ) -> Result<(), SchedulerStartError<()>> {
        let id = items
            .listed_item(insertion.head)
            .ok_or(SchedulerStartError::ForeignItem(insertion.head))?;
        let mut state = self.0.borrow_mut();
        if state.refuse_start {
            return Err(SchedulerStartError::Interrupts(()));
        }
        state.started.push(insertion.head);
        if state.defer_execution {
            state.running = true;
        } else {
            items.record_status_for_validation(id, 0);
            if state.capture_on_start {
                state.finished =
                    BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0]);
            }
            let _ = state
                .wake
                .publish_from_interrupt(SchedulerWorkerWakeClass::Ordinary);
        }
        Ok(())
    }

    fn perform<I: Copy, const CAPACITY: usize>(
        &mut self,
        _items: &SchedulerItemSpace<'_>,
        _step: &SchedulerStep<I, CAPACITY>,
    ) -> Result<(), SchedulerHardwareError> {
        Ok(())
    }

    fn observe_wait(
        &mut self,
        wait: SchedulerWait,
    ) -> Result<SchedulerObservation, SchedulerHardwareError> {
        Err(SchedulerHardwareError::NotAwaiting(wait))
    }

    fn recover(&mut self, _fault: SchedulerTransactionFault) {}

    fn step_stop(
        &mut self,
        _stop: BluetoothSchedulerStop,
    ) -> Result<BluetoothSchedulerStopStep, oer_esp32s31_bluetooth::scheduler::SchedulerStopError>
    {
        let mut state = self.0.borrow_mut();
        state.stops += 1;
        state.running = false;
        Ok(BluetoothSchedulerStopStep::Stopped(
            BluetoothSchedulerStopped::for_validation(),
        ))
    }
}

type Runtime = BluetoothRuntime<NoopRawMutex, Model, &'static VirtualClock, 1, 1, 1, 1, 2, 3, 8, 4>;

std::thread_local! {
    /// The virtual time of the test running on this thread.
    static CLOCK: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
}

fn clock() -> &'static VirtualClock {
    CLOCK.with(|clock| *clock)
}

/// Run the runtime for one millisecond of virtual time: poll it, moving
/// time to each deadline it waits for within the millisecond.
fn run_for_a_millisecond(runtime: &Runtime) {
    let end = clock()
        .now()
        .checked_add(oer_time::Duration::from_millis(1))
        .unwrap();
    let mut run = core::pin::pin!(runtime.run());
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    for _ in 0..1_000 {
        assert!(
            run.as_mut().poll(&mut context).is_pending(),
            "the runtime keeps running"
        );
        match clock().next_deadline() {
            Some(deadline) if deadline <= end => clock().advance_to(deadline),
            _ => {}
        }
    }
    clock().advance_to(end);
}

const NONCONN: [u8; 8] = [0x02, 6, 1, 2, 3, 4, 5, 6];

fn configure() -> RadioRequest<'static> {
    RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
        set: AdvertisingSetId::new(0),
        pdu: AdvertisingPdu::new(&NONCONN).unwrap(),
        reception: AdvertisingReception::None,
        tx_power: TxPower::from_dbm(0),
        phy: LePhy::Le1M,
    })
}

fn advertise(id: u32, anchor: u64) -> RadioRequest<'static> {
    RadioRequest::Advertise(AdvertisingEvent {
        id: EventId::new(id),
        set: AdvertisingSetId::new(0),
        anchor: LeInstant::from_micros(anchor),
        channels: AdvertisingChannels::single(AdvertisingChannel::Channel37),
        channel_spacing: RadioDuration::from_micros(1_000),
        coexistence: CoexistenceLevel::Baseline,
    })
}

fn installed(model: &Model) -> Runtime {
    let runtime = Runtime::new(clock());
    block_on(runtime.install(model_memory(), model.clone()))
        .unwrap_or_else(|_| panic!("the first install succeeds"));
    runtime
}

#[test]
fn install_publishes_the_chains_once() {
    let model = Model::default();
    let runtime = installed(&model);
    assert!(model.0.borrow().chains_published);
    let Err((error, _, _)) = block_on(runtime.install(model_memory(), model.clone())) else {
        panic!("a second install is refused")
    };
    assert_eq!(error, BluetoothInstallError::AlreadyInstalled);

    let refusing = Model::default();
    refusing.0.borrow_mut().refuse_chains = true;
    let Err((error, _, _)) = block_on(Runtime::new(clock()).install(model_memory(), refusing))
    else {
        panic!("a refused chain publication fails the install")
    };
    assert_eq!(
        error,
        BluetoothInstallError::RxChains(RxChainPublicationError::SchedulerStarted)
    );
}

#[test]
fn an_admitted_event_runs_and_ends() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.request(configure()).await.unwrap();
        runtime.request(advertise(1, 10_000)).await.unwrap();
        let outcome = match select(runtime.run(), runtime.next_outcome()).await {
            Either::First(fault) => panic!("the runtime faulted: {fault:?}"),
            Either::Second(outcome) => outcome.unwrap(),
        };
        assert_eq!(
            outcome,
            BluetoothOutcome::EventEnded {
                id: EventId::new(1),
                result: EventResult::Executed { anchor: None },
            }
        );
    });
    assert_eq!(model.0.borrow().started.len(), 1);
}

#[test]
fn an_accepted_scanner_publishes_the_scan_start_once() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.request(configure())).unwrap();
    assert_eq!(model.0.borrow().scan_starts, 0);
    let scanner = || {
        RadioRequest::ConfigureScanner(ScannerConfiguration {
            scanner: ScannerId::new(0),
            scan_type: ScanType::Active,
            filter_policy: ScanFilterPolicy::AcceptAll,
            tx_power: TxPower::from_dbm(0),
            phy: LePhy::Le1M,
        })
    };
    block_on(runtime.request(scanner())).unwrap();
    assert_eq!(model.0.borrow().scan_starts, 1);
    // A refused configuration publishes nothing.
    assert!(block_on(runtime.request(scanner())).is_err());
    assert_eq!(model.0.borrow().scan_starts, 1);
}

#[test]
fn installation_and_every_accepted_list_change_publish_the_device_table() {
    let model = Model::default();
    let runtime = installed(&model);
    let counts = || {
        model
            .0
            .borrow()
            .device_tables
            .iter()
            .map(|publication| publication.count.get())
            .collect::<Vec<_>>()
    };
    // The empty table is published before any scanner can filter by it.
    assert_eq!(counts(), [0]);
    let device = |index: u8| AcceptListDevice {
        random: true,
        address: [index, 0, 0, 0, 0, 0xc0],
    };
    for index in 0..BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY as u8 {
        block_on(
            runtime.request(RadioRequest::FilterAcceptList(AcceptListChange::Add(
                device(index),
            ))),
        )
        .unwrap();
    }
    assert_eq!(
        counts().last().copied(),
        Some(BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY as u32)
    );
    let published = counts().len();
    // A refused addition publishes nothing.
    assert!(matches!(
        block_on(
            runtime.request(RadioRequest::FilterAcceptList(AcceptListChange::Add(
                device(0xff)
            )))
        ),
        Err(BluetoothRuntimeError::Rejected(RequestError::ListFull))
    ));
    assert_eq!(counts().len(), published);
    block_on(
        runtime.request(RadioRequest::FilterAcceptList(AcceptListChange::Remove(
            device(0),
        ))),
    )
    .unwrap();
    block_on(runtime.request(RadioRequest::FilterAcceptList(AcceptListChange::Clear))).unwrap();
    assert_eq!(
        counts()[published..],
        [BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY as u32 - 1, 0]
    );
    let tables = model.0.borrow();
    assert!(
        tables
            .device_tables
            .iter()
            .all(|publication| publication.first_entry == tables.device_tables[0].first_entry)
    );
}

#[test]
fn the_clock_reports_a_fresh_radio_time() {
    let model = Model::default();
    let runtime = installed(&model);
    model.0.borrow_mut().time = 2_000;
    let (now, timing) = block_on(runtime.clock()).unwrap();
    // Two raw ticks per microsecond.
    assert_eq!(now, LeInstant::from_micros(1_000));
    assert_eq!(timing.admission_guard, RadioDuration::from_micros(40));
}

#[test]
fn a_refused_request_reports_the_radio_error() {
    let model = Model::default();
    model.0.borrow_mut().time = 20_000;
    let runtime = installed(&model);
    block_on(async {
        runtime.request(configure()).await.unwrap();
        // The fresh sample places the anchor in the past.
        assert_eq!(
            runtime.request(advertise(1, 5_000)).await,
            Err(BluetoothRuntimeError::Rejected(RequestError::TooLate))
        );
    });
    assert_eq!(
        block_on(Runtime::new(clock()).request(configure())),
        Err(BluetoothRuntimeError::NotInstalled)
    );
}

#[test]
fn maintenance_runs_while_the_scheduler_is_stopped() {
    let model = Model::default();
    let runtime = installed(&model);
    let span = block_on(runtime.quiesce(|proof| {
        assert_eq!(proof.client(), RadioClient::Bluetooth);
        proof.span()
    }))
    .unwrap();
    assert_eq!(span, QuiescentSpan::Stopped);
    assert_eq!(model.0.borrow().stops, 1);
    // The radio resumed: it admits new requests.
    block_on(runtime.request(configure())).unwrap();
}

#[test]
fn a_listed_event_restarts_after_maintenance() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.request(configure()).await.unwrap();
        runtime.request(advertise(1, 10_000)).await.unwrap();
    });
    // The event starts but has not run when maintenance stops the scheduler.
    model.0.borrow_mut().defer_execution = true;
    run_for_a_millisecond(&runtime);
    assert_eq!(model.0.borrow().started.len(), 1);
    model.0.borrow_mut().defer_execution = false;
    block_on(runtime.quiesce(|_| ())).unwrap();
    // Resuming restarts the scheduler at the listed event, which now runs.
    block_on(async {
        let outcome = match select(runtime.run(), runtime.next_outcome()).await {
            Either::First(fault) => panic!("the runtime faulted: {fault:?}"),
            Either::Second(outcome) => outcome.unwrap(),
        };
        assert_eq!(
            outcome,
            BluetoothOutcome::EventEnded {
                id: EventId::new(1),
                result: EventResult::Executed { anchor: None },
            }
        );
    });
    assert_eq!(model.0.borrow().started.len(), 2);
}

#[test]
fn an_oversized_pdu_becomes_a_memory_fault() {
    let pdu = vec![0; crate::MAX_PDU_BYTES + 1];
    let outcome = BluetoothOutcome::copy(RadioOutcome::Received {
        id: EventId::new(1),
        pdu: ReceivedPdu {
            pdu: &pdu,
            rssi_dbm: -40,
            captured_at: Ok(None),
        },
    });
    assert_eq!(
        outcome,
        BluetoothOutcome::Fault(RadioFault::MemoryInconsistency)
    );
    let fits = BluetoothOutcome::copy(RadioOutcome::Received {
        id: EventId::new(1),
        pdu: ReceivedPdu {
            pdu: &NONCONN,
            rssi_dbm: -40,
            captured_at: Ok(None),
        },
    });
    assert_eq!(fits.portable().clone(), {
        let BluetoothOutcome::Received { pdu, .. } = &fits else {
            panic!("the PDU fits")
        };
        RadioOutcome::Received {
            id: EventId::new(1),
            pdu: pdu.portable(),
        }
    });
}

#[test]
fn as_a_radio_port_a_refusal_answers_and_a_missing_radio_ends_service() {
    use oer_bluetooth_radio::LeRadioPort;

    let model = Model::default();
    model.0.borrow_mut().time = 20_000;
    let runtime = installed(&model);
    block_on(async {
        assert_eq!(LeRadioPort::submit(&runtime, configure()).await, Ok(Ok(())));
        assert_eq!(
            LeRadioPort::submit(&runtime, advertise(1, 5_000)).await,
            Ok(Err(RequestError::TooLate))
        );
    });
    let empty = Runtime::new(clock());
    assert_eq!(
        block_on(LeRadioPort::submit(&empty, configure())),
        Err(BluetoothRuntimeError::NotInstalled)
    );
    assert_eq!(
        block_on(LeRadioPort::clock(&empty)),
        Err(BluetoothRuntimeError::NotInstalled)
    );
}

#[test]
fn as_a_radio_port_it_states_le_1m_and_hardware_acknowledgement() {
    use oer_bluetooth_radio::{LeRadioPort, LinkAcknowledgement};

    let model = Model::default();
    let runtime = installed(&model);
    let capabilities = LeRadioPort::capabilities(&runtime);
    assert!(capabilities.legacy_advertising && capabilities.active_scanning);
    assert!(capabilities.phys.contains(LePhy::Le1M));
    assert!(!capabilities.phys.contains(LePhy::Le2M));
    let connection = capabilities
        .peripheral_connection
        .expect("the runtime has a connection");
    assert_eq!(
        connection.link_acknowledgement,
        LinkAcknowledgement::Hardware
    );
    // Every PDU the radio can carry fits an owned outcome.
    assert_eq!(
        crate::MAX_PDU_BYTES,
        2 + usize::from(connection.max_data_payload)
    );

    let RadioRequest::ConfigureAdvertising(configuration) = configure() else {
        unreachable!()
    };
    let le_2m = RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
        phy: LePhy::Le2M,
        ..configuration
    });
    assert_eq!(
        block_on(LeRadioPort::submit(&runtime, le_2m)),
        Ok(Err(RequestError::Unsupported))
    );
    assert_eq!(
        block_on(LeRadioPort::submit(&runtime, configure())),
        Ok(Ok(()))
    );
}

#[test]
fn uninstall_stops_the_scheduler_and_a_reset_radio_installs_again() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.request(configure()).await.unwrap();
        runtime.request(advertise(1, 10_000)).await.unwrap();
    });
    // The event is listed and running when the epoch ends.
    model.0.borrow_mut().defer_execution = true;
    run_for_a_millisecond(&runtime);
    let (radio, hardware) = block_on(runtime.uninstall()).unwrap();
    assert_eq!(model.0.borrow().stops, 1);
    assert!(matches!(
        block_on(runtime.run()),
        crate::BluetoothRuntimeFault::NotInstalled
    ));
    assert_eq!(
        block_on(runtime.request(configure())),
        Err(BluetoothRuntimeError::NotInstalled)
    );

    let memory =
        radio.into_memory(&oer_esp32s31_hal::bluetooth::BluetoothControllerReset::for_validation());
    model.0.borrow_mut().chains_published = false;
    block_on(runtime.install(memory, hardware))
        .unwrap_or_else(|_| panic!("the next epoch installs"));
    assert!(model.0.borrow().chains_published);
    block_on(runtime.request(configure())).unwrap();
}

fn test_receive(id: u32) -> RadioRequest<'static> {
    RadioRequest::TestReceive(TestReceive {
        id: EventId::new(id),
        channel: TestChannel::new(19).unwrap(),
        phy: TestPhy::Le1M,
        window: LeWindow::new(
            LeInstant::from_micros(10_000),
            RadioDuration::from_micros(1_000),
        )
        .unwrap(),
        recurring: false,
        tx_power: TxPower::from_dbm(0),
    })
}

fn routes(model: &Model) -> (usize, usize) {
    let state = model.0.borrow();
    (state.routes_disabled, state.routes_restored)
}

/// Run the runtime until it is idle for a moment.
fn settle(runtime: &Runtime) {
    run_for_a_millisecond(runtime);
}

#[test]
fn a_test_session_holds_the_route_until_test_end() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.request(test_receive(4))).unwrap();
    settle(&runtime);
    assert_eq!(routes(&model), (1, 0));
    block_on(runtime.request(RadioRequest::EndTest)).unwrap();
    settle(&runtime);
    assert_eq!(routes(&model), (1, 1));
}

#[test]
fn uninstall_restores_the_route_of_an_open_test() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.request(test_receive(4))).unwrap();
    settle(&runtime);
    assert_eq!(routes(&model), (1, 0));
    let _ = block_on(runtime.uninstall()).unwrap();
    assert_eq!(routes(&model), (1, 1));
}

#[test]
fn a_fault_restores_the_route_of_an_open_test() {
    let model = Model::default();
    model.0.borrow_mut().refuse_start = true;
    let runtime = installed(&model);
    block_on(runtime.request(test_receive(4))).unwrap();
    assert!(matches!(
        block_on(runtime.run()),
        crate::BluetoothRuntimeFault::Start(_)
    ));
    assert_eq!(routes(&model), (1, 1));
}

#[test]
fn owned_outcomes_retain_failed_timing_and_correct_pdu_contents() {
    use oer_bluetooth_radio::{CaptureError, TimingError};
    let cause = CaptureError::PacketStartCorrection(TimingError::BeforeEpoch);
    let received = BluetoothOutcome::copy(RadioOutcome::Received {
        id: EventId::new(3),
        pdu: ReceivedPdu {
            pdu: &NONCONN,
            rssi_dbm: -40,
            captured_at: Err(cause),
        },
    });
    let BluetoothOutcome::Received { pdu, .. } = &received else {
        panic!("PDU is retained")
    };
    assert_eq!(pdu.pdu(), &NONCONN);
    assert_eq!(pdu.captured_at, Err(cause));
    assert_eq!(pdu.portable().captured_at, Err(cause));
    let result = EventResult::TimingFailed {
        cause,
        executed: true,
        anchor: Err(cause),
    };
    let ended = BluetoothOutcome::copy(RadioOutcome::EventEnded {
        id: EventId::new(3),
        result,
    });
    assert_eq!(
        ended.portable(),
        RadioOutcome::EventEnded {
            id: EventId::new(3),
            result
        }
    );
}

#[test]
fn stop_drains_the_prior_finished_list_before_capturing_the_final_snapshot() {
    let model = Model::default();
    let runtime = installed(&model);
    model.0.borrow_mut().finished =
        BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0]);
    block_on(runtime.quiesce(|_| ())).unwrap();
    assert!(model.0.borrow().finished.is_none());
    assert_eq!(model.0.borrow().stops, 1);
    block_on(runtime.request(configure())).unwrap();
}

#[test]
fn pass_drains_prior_finished_work_before_consuming_the_next_wake() {
    let model = Model::default();
    model.0.borrow_mut().capture_on_start = true;
    let runtime = installed(&model);
    block_on(runtime.request(configure())).unwrap();
    block_on(runtime.request(advertise(19, 10_000))).unwrap();
    // A prior captured list and the next interrupt coexist. Draining the
    // prior owner must precede consuming that interrupt's transfer.
    run_for_a_millisecond(&runtime);
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert_eq!(
        core::pin::pin!(runtime.next_outcome()).poll(&mut context),
        core::task::Poll::Ready(Ok(BluetoothOutcome::EventEnded {
            id: EventId::new(19),
            result: EventResult::Executed { anchor: None },
        }))
    );
    assert!(
        core::pin::pin!(runtime.next_outcome())
            .poll(&mut context)
            .is_pending()
    );
    assert!(model.0.borrow().finished.is_none());
    assert!(model.0.borrow().wake.take().is_none());
}
