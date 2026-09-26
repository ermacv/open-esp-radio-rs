use std::{collections::VecDeque, vec, vec::Vec};

use super::{BluetoothSchedulerStop, Control, Progress, step};

struct Model {
    busy: VecDeque<bool>,
    c0: bool,
    c1: bool,
    trace: Vec<&'static str>,
}

impl Model {
    fn read_busy(&mut self) -> bool {
        self.trace.push("busy");
        self.busy.pop_front().unwrap()
    }
}

impl Control for Model {
    type Stopped = ();

    fn busy(&mut self) -> bool {
        self.read_busy()
    }
    fn preamble(&mut self) {
        self.trace.extend(["mask", "disable-run", "fence"]);
    }
    fn commands_ready(&mut self) -> bool {
        self.trace.push("command-0");
        if !self.c0 {
            return false;
        }
        self.trace.push("command-1");
        self.c1
    }
    fn request(&mut self) {
        self.trace.extend(["request", "fence"]);
    }
    fn confirm_stopped(&mut self) -> Option<()> {
        if self.read_busy() {
            return None;
        }
        self.trace.push("fence");
        Some(())
    }
}

fn model(busy: &[bool], c0: bool, c1: bool) -> Model {
    Model {
        busy: busy.iter().copied().collect(),
        c0,
        c1,
        trace: vec![],
    }
}

#[test]
fn idle_has_no_lifecycle_side_effect() {
    let mut hw = model(&[false], false, false);
    assert!(matches!(
        step(BluetoothSchedulerStop::default(), &mut hw),
        Progress::Stopped(())
    ));
    assert_eq!(hw.trace, ["busy", "fence"]);
}

#[test]
fn silence_stop_waits_for_preamble_and_publishes_only_once() {
    let mut hw = model(&[true, true], false, false);
    let Progress::Pending(stop) = step(BluetoothSchedulerStop::default(), &mut hw) else {
        panic!()
    };
    assert_eq!(
        hw.trace,
        ["busy", "mask", "disable-run", "fence", "busy", "command-0"]
    );
    hw.c0 = true;
    hw.c1 = true;
    hw.busy.extend([true, true]);
    hw.trace.clear();
    let Progress::Pending(stop) = step(stop, &mut hw) else {
        panic!()
    };
    assert_eq!(
        hw.trace,
        ["busy", "command-0", "command-1", "request", "fence", "busy"]
    );
    hw.busy.push_back(false);
    hw.trace.clear();
    assert!(matches!(step(stop, &mut hw), Progress::Stopped(())));
    assert_eq!(hw.trace, ["busy", "fence"]);
}

#[test]
fn completion_racing_preamble_skips_command_reads() {
    let mut hw = model(&[true, false, false], false, false);
    assert!(matches!(
        step(BluetoothSchedulerStop::default(), &mut hw),
        Progress::Stopped(())
    ));
    assert_eq!(
        hw.trace,
        [
            "busy",
            "mask",
            "disable-run",
            "fence",
            "busy",
            "request",
            "fence",
            "busy",
            "fence"
        ]
    );
}

#[test]
fn second_command_not_ready_keeps_owner_without_request() {
    let mut hw = model(&[true, true], true, false);
    assert!(matches!(
        step(BluetoothSchedulerStop::default(), &mut hw),
        Progress::Pending(_)
    ));
    assert!(!hw.trace.contains(&"request"));
}
