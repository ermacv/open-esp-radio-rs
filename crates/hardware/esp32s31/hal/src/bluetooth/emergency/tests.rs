use super::*;
use std::{
    collections::VecDeque,
    sync::{Arc, Barrier},
    thread, vec,
    vec::Vec,
};

struct Registers {
    samples: VecDeque<Result<bool, EmergencyUnconfirmed>>,
    tail: bool,
    commands: bool,
    operations: Vec<&'static str>,
}

impl StopControl for Registers {
    fn busy(&mut self) -> Result<bool, EmergencyUnconfirmed> {
        self.operations.push("busy");
        self.samples.pop_front().unwrap_or(Ok(self.tail))
    }
    fn preamble(&mut self) {
        self.operations.push("mask-run");
    }
    fn commands_ready(&mut self) -> bool {
        self.operations.push("command-zero-then-one");
        self.commands
    }
    fn request(&mut self) {
        self.operations.push("lifecycle-request");
    }
    fn fence(&mut self) {
        self.operations.push("fence");
    }
}

fn registers(samples: impl IntoIterator<Item = bool>, commands: bool) -> Registers {
    Registers {
        samples: samples.into_iter().map(Ok).collect(),
        tail: true,
        commands,
        operations: Vec::new(),
    }
}

#[test]
fn idle_skips_the_request_as_the_vendor_does() {
    let mut hardware = registers([false], false);
    assert_eq!(stop(&mut hardware, 3, 4), EmergencyOutcome::Stopped);
    assert_eq!(hardware.operations, ["busy", "fence"]);
}

#[test]
fn busy_stop_preserves_preamble_readiness_request_confirmation_order() {
    let mut hardware = registers([true, true, true, false], true);
    assert_eq!(stop(&mut hardware, 3, 4), EmergencyOutcome::Stopped);
    assert_eq!(
        hardware.operations,
        [
            "busy",
            "mask-run",
            "busy",
            "command-zero-then-one",
            "lifecycle-request",
            "busy",
            "busy",
            "fence"
        ]
    );
}

#[test]
fn idle_before_commands_skips_command_register_reads_but_still_requests() {
    let mut hardware = registers([true, false, false], false);
    assert_eq!(stop(&mut hardware, 3, 4), EmergencyOutcome::Stopped);
    assert_eq!(
        hardware.operations,
        [
            "busy",
            "mask-run",
            "busy",
            "lifecycle-request",
            "busy",
            "fence"
        ]
    );
}

#[test]
fn command_timeout_never_publishes_an_unadmitted_request() {
    let mut hardware = registers([], false);
    assert_eq!(
        stop(&mut hardware, 2, 4),
        EmergencyOutcome::Unconfirmed(EmergencyUnconfirmed::CommandDrain)
    );
    assert_eq!(
        hardware.operations,
        [
            "busy",
            "mask-run",
            "busy",
            "command-zero-then-one",
            "busy",
            "command-zero-then-one"
        ]
    );
}

#[test]
fn busy_timeout_is_finite_and_does_not_reset_or_recycle_anything() {
    let mut hardware = registers([], true);
    assert_eq!(
        stop(&mut hardware, 2, 3),
        EmergencyOutcome::Unconfirmed(EmergencyUnconfirmed::SchedulerBusy)
    );
    assert_eq!(
        hardware.operations,
        [
            "busy",
            "mask-run",
            "busy",
            "command-zero-then-one",
            "lifecycle-request",
            "busy",
            "busy",
            "busy"
        ]
    );
}

#[test]
fn unstable_diagnostic_is_unconfirmed_without_a_request() {
    let mut hardware = registers([], true);
    hardware
        .samples
        .push_back(Err(EmergencyUnconfirmed::Diagnostic));
    assert_eq!(
        stop(&mut hardware, 2, 3),
        EmergencyOutcome::Unconfirmed(EmergencyUnconfirmed::Diagnostic)
    );
    assert_eq!(hardware.operations, ["busy"]);
}

#[test]
fn entered_isr_on_another_core_must_leave_before_exclusion_is_proved() {
    let gate = Arc::new(EmergencyGate::new());
    let entered = Arc::new(Barrier::new(2));
    let leave = Arc::new(Barrier::new(2));
    let worker = {
        let (gate, entered, leave) = (gate.clone(), entered.clone(), leave.clone());
        thread::spawn(move || {
            let entry = gate.enter().unwrap();
            entered.wait();
            leave.wait();
            drop(entry);
        })
    };
    entered.wait();
    assert!(matches!(
        gate.close().drain(5),
        Err(EmergencyUnconfirmed::EnteredOperation)
    ));
    assert!(gate.enter().is_err());
    leave.wait();
    worker.join().unwrap();
    assert!(gate.close().drain(1).unwrap().belongs_to(&gate));
    assert!(gate.enter().is_err());
}

#[test]
fn late_interrupt_touches_no_installation_state() {
    let gate = EmergencyGate::new();
    let _drained = gate.close().drain(1).unwrap();
    let mut writes = vec![];
    if let Ok(_entry) = gate.enter() {
        writes.push("isr-publication");
    }
    assert!(writes.is_empty());
}

#[test]
#[allow(
    clippy::mem_forget,
    reason = "verify quarantine remains closed when an admitted entry is forgotten"
)]
fn forgotten_entry_prevents_exclusion_and_saturation_does_not_wrap() {
    let gate = EmergencyGate::new();
    core::mem::forget(gate.enter().unwrap());
    assert!(gate.close().drain(2).is_err());
    let saturated = EmergencyGate {
        entries: AtomicUsize::new(CLOSED - 1),
        runner: atomic_waker::AtomicWaker::new(),
    };
    assert!(saturated.enter().is_err());
    assert!(saturated.close().drain(1).is_err());
    assert!(saturated.enter().is_err());
    assert_eq!(saturated.entries.load(Ordering::Acquire), usize::MAX);
}

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl std::task::Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn a_pending_platform_runner_does_not_hold_admission_and_closure_wakes_it() {
    let gate = EmergencyGate::new();
    let wakes = Arc::new(WakeCount::default());
    let waker = core::task::Waker::from(wakes.clone());
    let mut cx = core::task::Context::from_waker(&waker);
    let mut waiting = core::pin::pin!(core::future::pending::<()>());
    assert!(gate.poll_runner(waiting.as_mut(), &mut cx).is_pending());
    let _drained = gate.close().drain(1).unwrap();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert_eq!(
        gate.poll_runner(waiting.as_mut(), &mut cx),
        core::task::Poll::Ready(Err(EmergencyClosed))
    );
}

#[test]
fn a_runner_first_polled_after_closure_never_polls_its_device_future() {
    let gate = EmergencyGate::new();
    let _drained = gate.close().drain(1).unwrap();
    let mut waiting = core::pin::pin!(core::future::poll_fn(|_| -> core::task::Poll<()> {
        panic!("a closed installation cannot poll device operations")
    }));
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    assert_eq!(
        gate.poll_runner(waiting.as_mut(), &mut cx),
        core::task::Poll::Ready(Err(EmergencyClosed))
    );
}

#[test]
fn vendor_thresholds_bound_the_number_of_failed_samples() {
    let mut commands = registers([], false);
    assert_eq!(
        stop(
            &mut commands,
            EMERGENCY_COMMAND_ATTEMPTS,
            EMERGENCY_BUSY_ATTEMPTS
        ),
        EmergencyOutcome::Unconfirmed(EmergencyUnconfirmed::CommandDrain)
    );
    assert_eq!(
        commands
            .operations
            .iter()
            .filter(|operation| **operation == "command-zero-then-one")
            .count(),
        10_000
    );
    let mut busy = registers([], true);
    assert_eq!(
        stop(
            &mut busy,
            EMERGENCY_COMMAND_ATTEMPTS,
            EMERGENCY_BUSY_ATTEMPTS
        ),
        EmergencyOutcome::Unconfirmed(EmergencyUnconfirmed::SchedulerBusy)
    );
    // Initial sample and one command-drain sample precede the bounded wait.
    assert_eq!(
        busy.operations
            .iter()
            .filter(|operation| **operation == "busy")
            .count(),
        65_538
    );
}
