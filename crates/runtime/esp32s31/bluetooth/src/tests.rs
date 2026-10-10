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
    AdvertisingSetId, ClockError, CoexistenceLevel, EventId, EventResult, LeInstant, LePhy,
    LeWindow, LifecycleCommand, LifecycleError, LifecycleEvent, Poisoned, RadioDuration,
    RadioOutcome, RadioRequest, ReceivedPdu, RequestError, ScanFilterPolicy, ScanType,
    ScannerConfiguration, ScannerId, TestChannel, TestPhy, TestReceive, TxPower,
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
    BluetoothFault, BluetoothInstallError, BluetoothOutcome, BluetoothRadioHardware,
    BluetoothRuntime, RxChainPublicationError,
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

/// The radio's first instant: its epoch starts 2^32 us above zero.
const EPOCH_START: u64 = 1 << 32;

/// The instant `micros` after the radio epoch's start.
fn at(micros: u64) -> LeInstant {
    LeInstant::from_micros(EPOCH_START + micros)
}

fn advertise(id: u32, anchor: u64) -> RadioRequest<'static> {
    RadioRequest::Advertise(AdvertisingEvent {
        id: EventId::new(id),
        set: AdvertisingSetId::new(0),
        anchor: at(anchor),
        channels: AdvertisingChannels::single(AdvertisingChannel::Channel37),
        channel_spacing: RadioDuration::from_micros(1_000),
        coexistence: CoexistenceLevel::Baseline,
    })
}

/// A runtime for the model's scheduler policy and sleep clock.
fn new_runtime() -> Runtime {
    Runtime::new(clock(), SchedulerSoftwareConfig::reviewed_standalone(), 500)
}

/// A runtime with the model's radio installed but its port disabled.
fn installed_disabled(model: &Model) -> Runtime {
    let runtime = new_runtime();
    let handles = block_on(runtime.install(model_memory(), model.clone()))
        .unwrap_or_else(|_| panic!("the first install succeeds"));
    // The tests reach the runtime's own operations; the port is rebuilt
    // where a test goes through the port traits.
    drop(handles);
    runtime
}

/// The port of an installed `runtime`.
fn port(runtime: &Runtime) -> crate::BluetoothPort<'_, Runtime> {
    crate::BluetoothPort::new(runtime)
}

/// A runtime with the model's radio installed and its port enabled.
fn installed(model: &Model) -> Runtime {
    let runtime = installed_disabled(model);
    enable(&runtime);
    runtime
}

fn enable(runtime: &Runtime) {
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Enable)),
        Ok(Ok(()))
    );
    assert_eq!(
        taken(runtime),
        Some(BluetoothOutcome::Lifecycle(LifecycleEvent::Enabled))
    );
}

/// The next outcome already queued, if any.
fn taken(runtime: &Runtime) -> Option<BluetoothOutcome> {
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    match core::pin::pin!(runtime.wait_event()).poll(&mut context) {
        core::task::Poll::Ready(Ok(Ok(outcome))) => Some(outcome),
        core::task::Poll::Ready(other) => panic!("an outcome expected: {other:?}"),
        core::task::Poll::Pending => None,
    }
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
    let Err((error, _, _)) = block_on(new_runtime().install(model_memory(), refusing)) else {
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
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
        let outcome = match select(runtime.run(), runtime.wait_event()).await {
            Either::First(fault) => panic!("the runtime faulted: {fault:?}"),
            Either::Second(outcome) => outcome.unwrap().unwrap(),
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
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
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
    block_on(runtime.submit_request(scanner()))
        .unwrap()
        .unwrap();
    assert_eq!(model.0.borrow().scan_starts, 1);
    // A refused configuration publishes nothing.
    assert!(matches!(
        block_on(runtime.submit_request(scanner())),
        Ok(Err(_))
    ));
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
            runtime.submit_request(RadioRequest::FilterAcceptList(AcceptListChange::Add(
                device(index),
            ))),
        )
        .unwrap()
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
            runtime.submit_request(RadioRequest::FilterAcceptList(AcceptListChange::Add(
                device(0xff)
            )))
        ),
        Ok(Err(RequestError::ListFull))
    ));
    assert_eq!(counts().len(), published);
    block_on(
        runtime.submit_request(RadioRequest::FilterAcceptList(AcceptListChange::Remove(
            device(0),
        ))),
    )
    .unwrap()
    .unwrap();
    block_on(runtime.submit_request(RadioRequest::FilterAcceptList(AcceptListChange::Clear)))
        .unwrap()
        .unwrap();
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
fn an_exhausted_epoch_admits_nothing_and_still_settles_admitted_work() {
    let model = Model::default();
    let runtime = installed(&model);
    assert_eq!(block_on(runtime.submit_request(configure())), Ok(Ok(())));
    assert_eq!(
        block_on(runtime.submit_request(advertise(1, 10_000))),
        Ok(Ok(()))
    );
    block_on(runtime.with_radio(|radio| radio.reach_epoch_end_for_validation()));
    model.0.borrow_mut().time = 2_000;
    assert_eq!(
        block_on(runtime.read_now()),
        Ok(Err(ClockError::EpochExhausted))
    );
    // It stays exhausted, whatever the next sample.
    model.0.borrow_mut().time = 0;
    assert_eq!(
        block_on(runtime.read_now()),
        Ok(Err(ClockError::EpochExhausted))
    );
    assert_eq!(
        block_on(runtime.submit_request(advertise(2, 20_000))),
        Ok(Err(RequestError::EpochExhausted))
    );
    // Disable still settles the admitted event.
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Disable)),
        Ok(Ok(()))
    );
    settle(&runtime);
    assert!(matches!(
        taken(&runtime),
        Some(BluetoothOutcome::EventEnded { id, .. }) if id == EventId::new(1)
    ));
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::Lifecycle(LifecycleEvent::Disabled))
    );
}

#[test]
fn the_clock_reports_a_fresh_radio_time() {
    let model = Model::default();
    let runtime = installed(&model);
    model.0.borrow_mut().time = 2_000;
    let now = block_on(runtime.read_now()).unwrap().unwrap();
    // Two raw ticks per microsecond.
    assert_eq!(now, at(1_000));
    // The timing is the runtime's own, read before any install.
    assert_eq!(
        new_runtime().capabilities().timing.admission_guard,
        RadioDuration::from_micros(40)
    );
}

#[test]
fn a_refused_request_reports_the_radio_error() {
    let model = Model::default();
    model.0.borrow_mut().time = 20_000;
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        // The fresh sample places the anchor in the past.
        assert_eq!(
            runtime.submit_request(advertise(1, 5_000)).await,
            Ok(Err(RequestError::TooLate))
        );
    });
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
    // Maintenance quiesced the port and enabled it again.
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::Lifecycle(LifecycleEvent::Quiesced))
    );
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::Lifecycle(LifecycleEvent::Enabled))
    );
    // The radio resumed: it admits new requests.
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
}

#[test]
fn maintenance_waits_for_the_admitted_events_to_end() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
    });
    // The event runs before the scheduler stops for maintenance.
    let stops_before = model.0.borrow().stops;
    let ended_first = block_on(async {
        match select(
            runtime.run(),
            runtime.quiesce(|_| model.0.borrow().started.len()),
        )
        .await
        {
            Either::First(fault) => panic!("the runtime faulted: {fault:?}"),
            Either::Second(started) => started.unwrap(),
        }
    });
    assert_eq!(ended_first, 1);
    assert_eq!(model.0.borrow().stops, stops_before + 1);
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::EventEnded {
            id: EventId::new(1),
            result: EventResult::Executed { anchor: None },
        })
    );
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::Lifecycle(LifecycleEvent::Quiesced))
    );
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::Lifecycle(LifecycleEvent::Enabled))
    );
}

#[test]
fn a_disabled_port_admits_nothing_and_enable_opens_it() {
    let model = Model::default();
    let runtime = installed_disabled(&model);
    assert_eq!(
        block_on(runtime.submit_request(configure())),
        Ok(Err(RequestError::Disabled))
    );
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Quiesce)),
        Ok(Err(LifecycleError::InvalidState))
    );
    enable(&runtime);
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Enable)),
        Ok(Err(LifecycleError::AlreadyInState))
    );
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
}

#[test]
fn disable_ends_every_admitted_event_before_its_terminal() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
    });
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Disable)),
        Ok(Ok(()))
    );
    // The waiting event ends at once, then the port is disabled.
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::EventEnded {
            id: EventId::new(1),
            result: EventResult::NotExecuted,
        })
    );
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::Lifecycle(LifecycleEvent::Disabled))
    );
    assert_eq!(
        block_on(runtime.submit_request(advertise(2, 20_000))),
        Ok(Err(RequestError::Disabled))
    );
}

#[test]
fn quiesce_reports_quiesced_after_the_last_admitted_event() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
    });
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Quiesce)),
        Ok(Ok(()))
    );
    // Admission is closed while the event is still admitted.
    assert_eq!(
        block_on(runtime.submit_request(advertise(2, 20_000))),
        Ok(Err(RequestError::Disabled))
    );
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Enable)),
        Ok(Err(LifecycleError::Busy))
    );
    assert_eq!(taken(&runtime), None);
    run_for_a_millisecond(&runtime);
    assert!(matches!(
        taken(&runtime),
        Some(BluetoothOutcome::EventEnded { .. })
    ));
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::Lifecycle(LifecycleEvent::Quiesced))
    );
    enable(&runtime);
}

#[test]
fn quiesce_returns_the_poison_instead_of_waiting_for_an_end_that_never_comes() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
    });
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    let mut quiesce = core::pin::pin!(runtime.quiesce(|_| ()));
    // The admitted event keeps the port quiescing.
    assert!(quiesce.as_mut().poll(&mut context).is_pending());
    // The runner then faults: the event's end never comes.
    model.0.borrow_mut().refuse_start = true;
    let fault = block_on(runtime.run());
    let core::task::Poll::Ready(result) = quiesce.as_mut().poll(&mut context) else {
        panic!("a poisoned port ends the wait")
    };
    assert_eq!(
        result,
        Err(Poisoned {
            cause: BluetoothFault::Runner(fault),
        })
    );
}

#[test]
fn quiesce_on_a_full_queue_proceeds_once_the_consumer_frees_a_slot() {
    let model = Model::default();
    let runtime = installed(&model);
    // Two unread lifecycle terminals and an admitted event's two reserved
    // slots fill the four-entry queue: Quiesce has no slot for its terminal.
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Disable)),
        Ok(Ok(()))
    );
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Enable)),
        Ok(Ok(()))
    );
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
    });
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Quiesce)),
        Ok(Err(LifecycleError::Busy))
    );
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    let mut quiesce = core::pin::pin!(runtime.quiesce(|_| ()));
    assert!(quiesce.as_mut().poll(&mut context).is_pending());
    // The consumer takes an outcome: the freed slot admits the quiesce.
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::Lifecycle(LifecycleEvent::Disabled))
    );
    assert!(quiesce.as_mut().poll(&mut context).is_pending());
    run_for_a_millisecond(&runtime);
    let mut done = None;
    for _ in 0..8 {
        if let core::task::Poll::Ready(result) = quiesce.as_mut().poll(&mut context) {
            done = Some(result);
            break;
        }
        let _ = taken(&runtime);
    }
    assert_eq!(done, Some(Ok(())));
    // The port is enabled again once the consumer has read its outcomes.
    while taken(&runtime).is_some() {}
    block_on(runtime.submit_request(advertise(2, 30_000)))
        .unwrap()
        .unwrap();
}

/// Fill the four-entry queue with unread terminals and leave the port in
/// the `Quiesced` of a quiesce whose generation is returned.
fn quiesced_on_a_full_queue(runtime: &Runtime) -> u32 {
    for command in [
        LifecycleCommand::Quiesce,
        LifecycleCommand::Disable,
        LifecycleCommand::Enable,
    ] {
        assert_eq!(block_on(runtime.run_lifecycle(command)), Ok(Ok(())));
    }
    block_on(runtime.start_lifecycle(LifecycleCommand::Quiesce))
        .unwrap()
        .unwrap()
}

#[test]
fn reopening_after_a_quiesce_never_overrides_another_owners_disable() {
    let model = Model::default();
    let runtime = installed(&model);
    let generation = quiesced_on_a_full_queue(&runtime);
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    let mut reopen = core::pin::pin!(runtime.reopen(generation));
    // No slot for Enabled: the reopen waits.
    assert!(reopen.as_mut().poll(&mut context).is_pending());
    // Another owner disables the port in the slot the consumer freed.
    assert!(taken(&runtime).is_some());
    assert_eq!(
        block_on(runtime.run_lifecycle(LifecycleCommand::Disable)),
        Ok(Ok(()))
    );
    // A slot is free again, but the port is no longer the quiesce's.
    assert!(taken(&runtime).is_some());
    assert_eq!(
        reopen.as_mut().poll(&mut context),
        core::task::Poll::Ready(Ok(()))
    );
    assert_eq!(
        block_on(runtime.submit_request(configure())),
        Ok(Err(RequestError::Disabled))
    );
}

#[test]
fn reopening_never_overrides_another_owners_later_quiesce() {
    let model = Model::default();
    let runtime = installed(&model);
    let generation = quiesced_on_a_full_queue(&runtime);
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    let mut reopen = core::pin::pin!(runtime.reopen(generation));
    assert!(reopen.as_mut().poll(&mut context).is_pending());
    // Another owner enables and quiesces again: the port is Quiesced, but
    // by that owner's command.
    while taken(&runtime).is_some() {}
    for command in [LifecycleCommand::Enable, LifecycleCommand::Quiesce] {
        assert_eq!(block_on(runtime.run_lifecycle(command)), Ok(Ok(())));
    }
    assert_eq!(
        reopen.as_mut().poll(&mut context),
        core::task::Poll::Ready(Ok(()))
    );
    assert_eq!(
        block_on(runtime.submit_request(configure())),
        Ok(Err(RequestError::Disabled))
    );
}

#[test]
fn cancellation_withdraws_an_admitted_event_and_its_end_follows() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
    });
    assert_eq!(
        block_on(runtime.cancel_event(EventId::new(2))),
        Ok(Err(oer_bluetooth_radio::CancelError::NotRunning))
    );
    assert_eq!(block_on(runtime.cancel_event(EventId::new(1))), Ok(Ok(())));
    assert_eq!(
        taken(&runtime),
        Some(BluetoothOutcome::EventEnded {
            id: EventId::new(1),
            result: EventResult::NotExecuted,
        })
    );
}

#[test]
fn a_runner_fault_poisons_the_port_with_its_cause() {
    let model = Model::default();
    model.0.borrow_mut().refuse_start = true;
    let runtime = installed(&model);
    block_on(runtime.submit_request(test_receive(4)))
        .unwrap()
        .unwrap();
    let fault = block_on(runtime.run());
    let poisoned = Poisoned {
        cause: BluetoothFault::Runner(fault),
    };
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert_eq!(
        core::pin::pin!(runtime.wait_event()).poll(&mut context),
        core::task::Poll::Ready(Err(poisoned))
    );
    assert_eq!(block_on(runtime.submit_request(configure())), Err(poisoned));
    assert_eq!(block_on(runtime.read_now()), Err(poisoned));
}

#[test]
fn an_oversized_pdu_becomes_a_memory_fault() {
    let pdu = vec![0; crate::MAX_PDU_BYTES + 1];
    let outcome = BluetoothOutcome::copy(RadioOutcome::Received {
        id: EventId::new(1),
        pdu: ReceivedPdu {
            pdu: &pdu,
            rssi_dbm: -40,
            captured_at: None,
        },
    });
    // The runtime's sink takes it as a memory-inconsistency fault.
    assert_eq!(outcome, None);
    let fits = BluetoothOutcome::copy(RadioOutcome::Received {
        id: EventId::new(1),
        pdu: ReceivedPdu {
            pdu: &NONCONN,
            rssi_dbm: -40,
            captured_at: None,
        },
    })
    .unwrap();
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
        assert_eq!(
            LeRadioPort::submit(&port(&runtime), configure()).await,
            Ok(Ok(()))
        );
        assert_eq!(
            LeRadioPort::submit(&port(&runtime), advertise(1, 5_000)).await,
            Ok(Err(RequestError::TooLate))
        );
    });
}

#[test]
fn as_a_radio_port_it_states_le_1m_and_hardware_acknowledgement() {
    use oer_bluetooth_radio::{LeRadioPort, LinkAcknowledgement};

    let model = Model::default();
    let runtime = installed(&model);
    let capabilities = LeRadioPort::capabilities(&port(&runtime));
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
        block_on(LeRadioPort::submit(&port(&runtime), le_2m)),
        Ok(Err(RequestError::Unsupported))
    );
    assert_eq!(
        block_on(LeRadioPort::submit(&port(&runtime), configure())),
        Ok(Ok(()))
    );
}

#[test]
fn uninstall_stops_the_scheduler_and_a_reset_radio_installs_again() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(async {
        runtime.submit_request(configure()).await.unwrap().unwrap();
        runtime
            .submit_request(advertise(1, 10_000))
            .await
            .unwrap()
            .unwrap();
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
    let memory =
        radio.into_memory(&oer_esp32s31_hal::bluetooth::BluetoothControllerReset::for_validation());
    model.0.borrow_mut().chains_published = false;
    let _handles = block_on(runtime.install(memory, hardware))
        .unwrap_or_else(|_| panic!("the next epoch installs"));
    assert!(model.0.borrow().chains_published);
    // The next radio starts disabled.
    assert_eq!(
        block_on(runtime.submit_request(configure())),
        Ok(Err(RequestError::Disabled))
    );
    enable(&runtime);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
}

fn test_receive(id: u32) -> RadioRequest<'static> {
    RadioRequest::TestReceive(TestReceive {
        id: EventId::new(id),
        channel: TestChannel::new(19).unwrap(),
        phy: TestPhy::Le1M,
        window: LeWindow::new(at(10_000), RadioDuration::from_micros(1_000)).unwrap(),
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
    block_on(runtime.submit_request(test_receive(4)))
        .unwrap()
        .unwrap();
    settle(&runtime);
    assert_eq!(routes(&model), (1, 0));
    block_on(runtime.submit_request(RadioRequest::EndTest))
        .unwrap()
        .unwrap();
    settle(&runtime);
    assert_eq!(routes(&model), (1, 1));
}

#[test]
fn uninstall_restores_the_route_of_an_open_test() {
    let model = Model::default();
    let runtime = installed(&model);
    block_on(runtime.submit_request(test_receive(4)))
        .unwrap()
        .unwrap();
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
    block_on(runtime.submit_request(test_receive(4)))
        .unwrap()
        .unwrap();
    assert!(matches!(
        block_on(runtime.run()),
        crate::BluetoothRuntimeFault::Start(_)
    ));
    assert_eq!(routes(&model), (1, 1));
}

#[test]
fn owned_outcomes_keep_the_pdu_its_stamp_and_an_aborted_end() {
    let captured_at = Some(at(0));
    let received = BluetoothOutcome::copy(RadioOutcome::Received {
        id: EventId::new(3),
        pdu: ReceivedPdu {
            pdu: &NONCONN,
            rssi_dbm: -40,
            captured_at,
        },
    })
    .unwrap();
    let BluetoothOutcome::Received { pdu, .. } = &received else {
        panic!("PDU is retained")
    };
    assert_eq!(pdu.pdu(), &NONCONN);
    assert_eq!(pdu.portable().captured_at, captured_at);
    let ended = BluetoothOutcome::copy(RadioOutcome::EventEnded {
        id: EventId::new(3),
        result: EventResult::Aborted,
    })
    .unwrap();
    assert_eq!(
        ended.portable(),
        RadioOutcome::EventEnded {
            id: EventId::new(3),
            result: EventResult::Aborted
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
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
}

#[test]
fn pass_drains_prior_finished_work_before_consuming_the_next_wake() {
    let model = Model::default();
    model.0.borrow_mut().capture_on_start = true;
    let runtime = installed(&model);
    block_on(runtime.submit_request(configure()))
        .unwrap()
        .unwrap();
    block_on(runtime.submit_request(advertise(19, 10_000)))
        .unwrap()
        .unwrap();
    // A prior captured list and the next interrupt coexist. Draining the
    // prior owner must precede consuming that interrupt's transfer.
    run_for_a_millisecond(&runtime);
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert_eq!(
        core::pin::pin!(runtime.wait_event()).poll(&mut context),
        core::task::Poll::Ready(Ok(Ok(BluetoothOutcome::EventEnded {
            id: EventId::new(19),
            result: EventResult::Executed { anchor: None },
        })))
    );
    assert!(
        core::pin::pin!(runtime.wait_event())
            .poll(&mut context)
            .is_pending()
    );
    assert!(model.0.borrow().finished.is_none());
    assert!(model.0.borrow().wake.take().is_none());
}

#[test]
fn a_wait_past_the_timer_range_is_an_error_not_a_forever_wait() {
    let clock: VirtualClock =
        VirtualClock::starting_at(oer_time::Instant::from_micros(u64::MAX - 5));
    assert_eq!(
        block_on(crate::runtime::wait_for(&clock, crate::HARDWARE_RECHECK)),
        Err(crate::BluetoothTimeError::Deadline)
    );
    let clock: VirtualClock = VirtualClock::new();
    let wait = crate::runtime::wait_for(&clock, crate::HARDWARE_RECHECK);
    let mut wait = core::pin::pin!(wait);
    assert!(embassy_futures::poll_once(wait.as_mut()).is_pending());
    clock.advance(crate::HARDWARE_RECHECK).unwrap();
    assert_eq!(block_on(wait), Ok(()));
}

/// Install returns the radio's two handles: the consumer serves through the
/// port, the composition maintains through the control, and the control's
/// uninstall consumes both, ends the port's stream and returns the owners
/// for the next install.
#[test]
fn install_returns_the_port_and_the_control_whose_uninstall_takes_both() {
    use oer_bluetooth_radio::{LeRadioPort, RadioPort};

    let model = Model::default();
    let runtime = new_runtime();
    let (port, control) = block_on(runtime.install(model_memory(), model.clone()))
        .unwrap_or_else(|_| panic!("the first install succeeds"));
    block_on(async {
        // The consumer borrows the port while it serves.
        assert_eq!(port.lifecycle(LifecycleCommand::Enable).await, Ok(Ok(())));
        assert_eq!(port.submit(configure()).await, Ok(Ok(())));
    });
    // The control's maintenance quiesces and reopens the port.
    assert_eq!(
        block_on(control.quiesce(|proof| proof.client())),
        Ok(RadioClient::Bluetooth)
    );
    // The control's uninstall consumes both handles; the owners install
    // again.
    let Ok((radio, hardware)) = block_on(control.uninstall(port)) else {
        panic!("the idle radio uninstalls")
    };
    let memory =
        radio.into_memory(&oer_esp32s31_hal::bluetooth::BluetoothControllerReset::for_validation());
    let (port, _control) = block_on(runtime.install(memory, hardware))
        .unwrap_or_else(|_| panic!("the next epoch installs"));
    // The first port's undelivered outcomes ended with its stream: the next
    // port starts with an empty queue (#457).
    assert_eq!(taken(&runtime), None);
    // The next radio starts disabled.
    assert_eq!(
        block_on(port.submit(configure())),
        Ok(Err(RequestError::Disabled))
    );
}
