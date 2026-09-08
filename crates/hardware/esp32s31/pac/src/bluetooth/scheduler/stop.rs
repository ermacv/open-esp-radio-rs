//! Finite scheduler lifecycle sequence from the reviewed common-library stop.
//!
//! Every pending result retains the sequence. The caller owns the absolute
//! deadline and must serialize each step with interrupt service. Neither an
//! idle sample nor this sequence alone releases descriptor/software ownership.

use crate::{BluetoothInterruptRegisters, BluetoothTaskRegisters, device_fence};

#[derive(Debug)]
enum Phase {
    Initial,
    Preamble,
    Requested,
}

/// Affine progress of the common scheduler stop transaction.
#[derive(Debug)]
#[must_use]
pub struct BluetoothSchedulerStop {
    phase: Phase,
}

impl Default for BluetoothSchedulerStop {
    fn default() -> Self {
        Self {
            phase: Phase::Initial,
        }
    }
}

/// The reviewed lifecycle sequence reached BUSY clear and a device fence.
/// This is global scheduler evidence, not descriptor reclamation permission.
#[derive(Debug)]
#[must_use]
pub struct BluetoothSchedulerStopped {
    _private: (),
}

/// Exact stopped item after fenced hardware-head retirement. This affine
/// capability authorizes one bound memory status sample, not CPU recycling.
#[derive(Debug)]
#[must_use]
pub struct BluetoothSchedulerStoppedItem {
    head: crate::BluetoothSchedulerHardwareListHeadEmptyObserved,
}
impl BluetoothSchedulerStoppedItem {
    pub const fn head(&self) -> &crate::BluetoothSchedulerHardwareListHeadEmptyObserved {
        &self.head
    }
    pub fn into_head(self) -> crate::BluetoothSchedulerHardwareListHeadEmptyObserved {
        self.head
    }
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn from_head_for_validation(
        head: crate::BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> Self {
        Self { head }
    }
}

#[must_use]
pub enum BluetoothSchedulerStoppedHeadRetirement {
    Retired(BluetoothSchedulerStoppedItem),
    Rejected(crate::BluetoothSchedulerHardwareListHeadRetirementObservation),
}

#[derive(Debug)]
#[must_use]
pub enum BluetoothSchedulerStopStep {
    Pending(BluetoothSchedulerStop),
    Stopped(BluetoothSchedulerStopped),
}

trait Control {
    fn busy(&mut self) -> bool;
    fn mask_dynamic(&mut self);
    fn disable_run(&mut self);
    fn command_0_ready(&mut self) -> bool;
    fn command_1_ready(&mut self) -> bool;
    fn request(&mut self);
    fn fence(&mut self);
}

fn step(mut stop: BluetoothSchedulerStop, hw: &mut impl Control) -> BluetoothSchedulerStopStep {
    if matches!(stop.phase, Phase::Initial) {
        if !hw.busy() {
            hw.fence();
            return BluetoothSchedulerStopStep::Stopped(BluetoothSchedulerStopped { _private: () });
        }
        hw.mask_dynamic();
        hw.disable_run();
        hw.fence();
        stop.phase = Phase::Preamble;
    }
    if matches!(stop.phase, Phase::Preamble) {
        // B8f: an idle scheduler bypasses command reads; while busy both
        // positional statuses are required, in this short-circuit order.
        if hw.busy() && !(hw.command_0_ready() && hw.command_1_ready()) {
            return BluetoothSchedulerStopStep::Pending(stop);
        }
        hw.request();
        hw.fence();
        stop.phase = Phase::Requested;
    }
    if hw.busy() {
        BluetoothSchedulerStopStep::Pending(stop)
    } else {
        hw.fence();
        BluetoothSchedulerStopStep::Stopped(BluetoothSchedulerStopped { _private: () })
    }
}

struct Hardware<'a> {
    task: &'a mut BluetoothTaskRegisters,
    interrupts: &'a mut BluetoothInterruptRegisters,
}

impl Control for Hardware<'_> {
    fn busy(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_scheduler_software_list_busy(
            &self
                .interrupts
                .peripherals
                .bluetooth_scheduler_interrupt_runtime,
        )
    }
    fn mask_dynamic(&mut self) {
        let bank = &self.interrupts.peripherals.bluetooth_interrupt_bank;
        crate::generated::mask_bluetooth_scheduler_run_interrupts_bank_0(bank);
        crate::generated::mask_bluetooth_scheduler_run_interrupts_bank_1(bank);
    }
    fn disable_run(&mut self) {
        crate::generated::disable_ble_scheduler_run_event_source(
            &self.task.bluetooth.btmac_ble_phy_init,
        );
    }
    fn command_0_ready(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_scheduler_software_list_command_0_status_26(
            &self.task.bluetooth.bluetooth_controller_core,
        )
    }
    fn command_1_ready(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_scheduler_software_list_command_1_status_18(
            &self.task.bluetooth.bluetooth_controller_core,
        )
    }
    fn request(&mut self) {
        crate::svd::fixed_register_image::publish_bluetooth_scheduler_lifecycle_request(
            &self.task.bluetooth.bluetooth_controller_core,
        );
    }
    fn fence(&mut self) {
        device_fence();
    }
}

trait HeadControl {
    /// Every observation includes its trailing device fence.
    fn observe(
        &mut self,
        index: crate::BluetoothSchedulerHardwareListIndex,
    ) -> crate::BluetoothSchedulerHardwareListHead;
    /// Clear only the pointer field, preserve unrelated fields, then fence.
    fn clear(&mut self, index: crate::BluetoothSchedulerHardwareListIndex);
}

impl HeadControl for BluetoothTaskRegisters {
    fn observe(
        &mut self,
        index: crate::BluetoothSchedulerHardwareListIndex,
    ) -> crate::BluetoothSchedulerHardwareListHead {
        super::execute_scheduler_hardware_list_head_observation(self, index)
    }
    fn clear(&mut self, index: crate::BluetoothSchedulerHardwareListIndex) {
        crate::generated::clear_bluetooth_scheduler_hardware_list_head(
            &self.bluetooth.btdm_scheduler_table,
            index.get() as usize,
        );
        device_fence();
    }
}

fn retire_head(
    control: &mut impl HeadControl,
    index: crate::BluetoothSchedulerHardwareListIndex,
    expected: crate::BluetoothSchedulerHardwareListHead,
) -> (
    super::BluetoothSchedulerHardwareListHeadRetirementDisposition,
    crate::BluetoothSchedulerHardwareListHead,
) {
    let observed = control.observe(index);
    if observed.address().is_some() && observed != expected {
        return (
            super::BluetoothSchedulerHardwareListHeadRetirementDisposition::UnexpectedHeadChanged,
            observed,
        );
    }
    if observed == expected {
        control.clear(index);
    }
    let observed = control.observe(index);
    (
        super::classify_scheduler_hardware_list_head_retirement(expected, observed),
        observed,
    )
}

impl BluetoothTaskRegisters {
    /// Advance one finite common-stop step while both register owners are held.
    pub fn step_scheduler_stop(
        &mut self,
        interrupts: &mut BluetoothInterruptRegisters,
        stop: BluetoothSchedulerStop,
    ) -> BluetoothSchedulerStopStep {
        step(
            stop,
            &mut Hardware {
                task: self,
                interrupts,
            },
        )
    }

    /// Detach only the exact RUN head after common stop. A foreign head is
    /// retained as an invariant failure. The final fresh fenced read is still
    /// required, and software unlink/recycle remain separate transitions.
    pub fn retire_stopped_scheduler_head(
        &mut self,
        _stopped: BluetoothSchedulerStopped,
        run: crate::BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothSchedulerStoppedHeadRetirement {
        use super::BluetoothSchedulerHardwareListHeadRetirementDisposition as Disposition;
        let (disposition, observed) = retire_head(self, run.index(), run.head());
        match disposition {
            Disposition::Empty => BluetoothSchedulerStoppedHeadRetirement::Retired(BluetoothSchedulerStoppedItem {
                head: crate::BluetoothSchedulerHardwareListHeadEmptyObserved { index: run.index(), completed_head: run.head() },
            }),
            Disposition::ExpectedHeadStillPublished => BluetoothSchedulerStoppedHeadRetirement::Rejected(
                crate::BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished { run, observed }),
            Disposition::UnexpectedHeadChanged => BluetoothSchedulerStoppedHeadRetirement::Rejected(
                crate::BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged { run, observed }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, vec, vec::Vec};
    struct Model {
        busy: VecDeque<bool>,
        c0: bool,
        c1: bool,
        trace: Vec<&'static str>,
    }
    impl Control for Model {
        fn busy(&mut self) -> bool {
            self.trace.push("busy");
            self.busy.pop_front().unwrap()
        }
        fn mask_dynamic(&mut self) {
            self.trace.push("mask");
        }
        fn disable_run(&mut self) {
            self.trace.push("disable-run");
        }
        fn command_0_ready(&mut self) -> bool {
            self.trace.push("command-0");
            self.c0
        }
        fn command_1_ready(&mut self) -> bool {
            self.trace.push("command-1");
            self.c1
        }
        fn request(&mut self) {
            self.trace.push("request");
        }
        fn fence(&mut self) {
            self.trace.push("fence");
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
            BluetoothSchedulerStopStep::Stopped(_)
        ));
        assert_eq!(hw.trace, ["busy", "fence"]);
    }
    #[test]
    fn silence_stop_waits_for_preamble_and_publishes_only_once() {
        let mut hw = model(&[true, true], false, false);
        let BluetoothSchedulerStopStep::Pending(stop) =
            step(BluetoothSchedulerStop::default(), &mut hw)
        else {
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
        let BluetoothSchedulerStopStep::Pending(stop) = step(stop, &mut hw) else {
            panic!()
        };
        assert_eq!(
            hw.trace,
            ["busy", "command-0", "command-1", "request", "fence", "busy"]
        );
        hw.busy.push_back(false);
        hw.trace.clear();
        assert!(matches!(
            step(stop, &mut hw),
            BluetoothSchedulerStopStep::Stopped(_)
        ));
        assert_eq!(hw.trace, ["busy", "fence"]);
    }
    #[test]
    fn completion_racing_preamble_skips_command_reads() {
        let mut hw = model(&[true, false, false], false, false);
        assert!(matches!(
            step(BluetoothSchedulerStop::default(), &mut hw),
            BluetoothSchedulerStopStep::Stopped(_)
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
            BluetoothSchedulerStopStep::Pending(_)
        ));
        assert!(!hw.trace.contains(&"request"));
    }
    struct Heads {
        reads: VecDeque<crate::BluetoothSchedulerHardwareListHead>,
        clears: usize,
    }
    impl HeadControl for Heads {
        fn observe(
            &mut self,
            _: crate::BluetoothSchedulerHardwareListIndex,
        ) -> crate::BluetoothSchedulerHardwareListHead {
            self.reads.pop_front().unwrap()
        }
        fn clear(&mut self, _: crate::BluetoothSchedulerHardwareListIndex) {
            self.clears += 1;
        }
    }
    #[test]
    fn stop_detaches_only_its_exact_head_and_verifies_retirement() {
        use super::super::BluetoothSchedulerHardwareListHeadRetirementDisposition as D;
        let expected = crate::BluetoothSchedulerHardwareListHead::from_address(
            crate::BluetoothControllerSramAddress::new(0x2f00_0400).unwrap(),
        )
        .unwrap();
        let foreign = crate::BluetoothSchedulerHardwareListHead::from_address(
            crate::BluetoothControllerSramAddress::new(0x2f00_0800).unwrap(),
        )
        .unwrap();
        let empty = crate::BluetoothSchedulerHardwareListHead::empty();
        let index = crate::BluetoothSchedulerHardwareListIndex::ZERO;
        let mut model = Heads {
            reads: [expected, empty].into(),
            clears: 0,
        };
        assert!(matches!(
            retire_head(&mut model, index, expected).0,
            D::Empty
        ));
        assert_eq!(model.clears, 1);
        let mut model = Heads {
            reads: [foreign].into(),
            clears: 0,
        };
        assert!(matches!(
            retire_head(&mut model, index, expected).0,
            D::UnexpectedHeadChanged
        ));
        assert_eq!(model.clears, 0);
        let mut model = Heads {
            reads: [expected, expected].into(),
            clears: 0,
        };
        assert!(matches!(
            retire_head(&mut model, index, expected).0,
            D::ExpectedHeadStillPublished
        ));
        let mut model = Heads {
            reads: [empty, empty].into(),
            clears: 0,
        };
        assert!(matches!(
            retire_head(&mut model, index, expected).0,
            D::Empty
        ));
        assert_eq!(model.clears, 0);
    }
}
