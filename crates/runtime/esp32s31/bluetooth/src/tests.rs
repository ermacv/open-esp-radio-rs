use core::cell::RefCell;
use std::{rc::Rc, vec, vec::Vec};

use embassy_futures::{
    block_on,
    select::{Either, select},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::Timer;
use oer_bluetooth_radio::{
    AdvertisingChannel, AdvertisingChannels, AdvertisingConfiguration, AdvertisingEvent,
    AdvertisingPdu, AdvertisingSetId, EventId, EventResult, RadioDuration, RadioFault,
    RadioInstant, RadioOutcome, RadioRequest, ReceivedPdu, RequestError, TxPower,
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
use oer_esp32s31_bluetooth_memory::{ControllerSramLinkAddress, LeRxChain, SchedulerItemSpace};
use oer_esp32s31_bluetooth_radio::validation::model_memory;
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerHalInitConfig, BluetoothControllerTimeScale,
    BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
    BluetoothSchedulerStop, BluetoothSchedulerStopStep, BluetoothSchedulerStopped,
    BluetoothSchedulerWorkObservation,
};
use oer_esp32s31_hal::shared_radio::{QuiescentSpan, RadioClient};

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
    wake: SchedulerWakeCell,
}

/// Hardware that executes the head item at once when it starts.
#[derive(Clone, Default)]
struct Model(Rc<RefCell<State>>);

impl BluetoothRadioHardware for Model {
    type StartError = ();

    fn scheduler_config(&self) -> SchedulerSoftwareConfig {
        SchedulerSoftwareConfig::reviewed_standalone()
    }

    fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale()
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

    fn take_wake(&mut self) -> Option<SchedulerWakeBatch> {
        self.0.borrow().wake.take()
    }

    fn capture_finished_lists(
        &mut self,
        _wake: SchedulerWakeBatch,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        self.0.borrow_mut().finished =
            BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[0]);
        Ok(())
    }

    fn capture_stopped_finished_lists(
        &mut self,
        _stopped: &BluetoothSchedulerStopped,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
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
            work: BluetoothSchedulerWorkObservation::from_fields_for_validation(busy, false, 0),
            hardware_head: None,
        })
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
        state.started.push(insertion.head);
        if state.defer_execution {
            state.running = true;
        } else {
            items.record_status_for_validation(id, 0);
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
    ) -> Result<BluetoothSchedulerStopStep, BluetoothSchedulerStop> {
        let mut state = self.0.borrow_mut();
        state.stops += 1;
        state.running = false;
        Ok(BluetoothSchedulerStopStep::Stopped(
            BluetoothSchedulerStopped::for_validation(),
        ))
    }
}

type Runtime = BluetoothRuntime<NoopRawMutex, Model, 1, 1, 1, 1, 2, 3, 8, 4>;

const NONCONN: [u8; 8] = [0x02, 6, 1, 2, 3, 4, 5, 6];

fn configure() -> RadioRequest<'static> {
    RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
        set: AdvertisingSetId::new(0),
        pdu: AdvertisingPdu::new(&NONCONN).unwrap(),
        scan_response: None,
        tx_power: TxPower::from_dbm(0),
    })
}

fn advertise(id: u32, anchor: u64) -> RadioRequest<'static> {
    RadioRequest::Advertise(AdvertisingEvent {
        id: EventId::new(id),
        set: AdvertisingSetId::new(0),
        anchor: RadioInstant::from_micros(anchor),
        channels: AdvertisingChannels::single(AdvertisingChannel::Channel37),
        channel_spacing: RadioDuration::from_micros(1_000),
    })
}

fn installed(model: &Model) -> Runtime {
    let runtime = Runtime::new();
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
    let Err((error, _, _)) = block_on(Runtime::new().install(model_memory(), refusing)) else {
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
fn the_clock_reports_a_fresh_radio_time() {
    let model = Model::default();
    let runtime = installed(&model);
    model.0.borrow_mut().time = 2_000;
    let (now, timing) = block_on(runtime.clock()).unwrap();
    // Two raw ticks per microsecond.
    assert_eq!(now, RadioInstant::from_micros(1_000));
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
        block_on(Runtime::new().request(configure())),
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
    block_on(async {
        let Either::Second(()) = select(runtime.run(), Timer::after_millis(1)).await else {
            panic!("the runtime keeps running")
        };
    });
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
            captured_at: None,
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
            captured_at: None,
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
