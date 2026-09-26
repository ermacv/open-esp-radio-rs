//! Finite scheduler lifecycle sequence from the reviewed common-library stop.
//!
//! Every pending result retains the sequence. The caller owns the absolute
//! deadline and must serialize each step with interrupt service. Neither an
//! idle sample nor this sequence alone releases descriptor/software ownership.

use oer_esp32s31_pac::{
    BluetoothInterruptRegisters, BluetoothSchedulerStopped, BluetoothTaskRegisters,
};

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

/// Result of one finite stop step.
#[derive(Debug)]
#[must_use]
pub enum BluetoothSchedulerStopStep {
    Pending(BluetoothSchedulerStop),
    Stopped(BluetoothSchedulerStopped),
}

enum Progress<Stopped> {
    Pending(BluetoothSchedulerStop),
    Stopped(Stopped),
}

trait Control {
    type Stopped;

    fn busy(&mut self) -> bool;
    /// Mask dynamic run interrupts, disable RUN, then fence.
    fn preamble(&mut self);
    /// Command-zero then, only if ready, command-one status.
    fn commands_ready(&mut self) -> bool;
    /// Publish the lifecycle request, then fence.
    fn request(&mut self);
    /// Observe BUSY once; when clear, fence and return the receipt.
    fn confirm_stopped(&mut self) -> Option<Self::Stopped>;
}

fn step<C: Control>(mut stop: BluetoothSchedulerStop, hw: &mut C) -> Progress<C::Stopped> {
    if matches!(stop.phase, Phase::Initial) {
        if let Some(stopped) = hw.confirm_stopped() {
            return Progress::Stopped(stopped);
        }
        hw.preamble();
        stop.phase = Phase::Preamble;
    }
    if matches!(stop.phase, Phase::Preamble) {
        // B8f: an idle scheduler bypasses command reads; while busy both
        // positional statuses are required, in this short-circuit order.
        if hw.busy() && !hw.commands_ready() {
            return Progress::Pending(stop);
        }
        hw.request();
        stop.phase = Phase::Requested;
    }
    match hw.confirm_stopped() {
        Some(stopped) => Progress::Stopped(stopped),
        None => Progress::Pending(stop),
    }
}

struct Hardware<'a> {
    task: &'a mut BluetoothTaskRegisters,
    interrupts: &'a mut BluetoothInterruptRegisters,
}

impl Control for Hardware<'_> {
    type Stopped = BluetoothSchedulerStopped;

    fn busy(&mut self) -> bool {
        self.task.scheduler_stop_busy(self.interrupts)
    }
    fn preamble(&mut self) {
        self.task.publish_scheduler_stop_preamble(self.interrupts);
    }
    fn commands_ready(&mut self) -> bool {
        self.task.scheduler_stop_commands_ready()
    }
    fn request(&mut self) {
        self.task.publish_scheduler_lifecycle_request();
    }
    fn confirm_stopped(&mut self) -> Option<BluetoothSchedulerStopped> {
        self.task.confirm_scheduler_stopped(self.interrupts)
    }
}

/// Advance one finite common-stop step while both register owners are held.
pub(crate) fn step_hardware(
    task: &mut BluetoothTaskRegisters,
    interrupts: &mut BluetoothInterruptRegisters,
    stop: BluetoothSchedulerStop,
) -> BluetoothSchedulerStopStep {
    match step(stop, &mut Hardware { task, interrupts }) {
        Progress::Pending(stop) => BluetoothSchedulerStopStep::Pending(stop),
        Progress::Stopped(stopped) => BluetoothSchedulerStopStep::Stopped(stopped),
    }
}

#[cfg(test)]
mod tests;
