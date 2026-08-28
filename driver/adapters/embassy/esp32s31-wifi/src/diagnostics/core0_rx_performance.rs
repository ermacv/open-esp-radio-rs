//! Paired Core0 cycle and retired-instruction accounting.
//!
//! `mcycle` alone measures executor residence, including cache and memory
//! stalls. Pairing it with `minstret` over the same coarse ownership intervals
//! exposes whether a busy Core0 is executing instructions or waiting. The
//! reads deliberately stay coarse: per-frame CSR sampling would perturb the
//! datapath which this diagnostic image is intended to measure.

use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0PerformanceSample {
    pub cycles: u32,
    pub instructions: u32,
}

impl Core0PerformanceSample {
    #[inline(always)]
    pub fn read() -> Self {
        Self {
            cycles: cycle_count(),
            instructions: instruction_count(),
        }
    }

    #[inline(always)]
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            cycles: self.cycles.wrapping_sub(earlier.cycles),
            instructions: self.instructions.wrapping_sub(earlier.instructions),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0PerformanceSnapshot {
    pub rx_interrupt_posts: u32,
    pub radio_polls: u32,
    pub radio_cycles: u32,
    pub radio_instructions: u32,
    pub poll_to_runner_cycles: u32,
    pub poll_to_runner_instructions: u32,
    pub runner_to_poll_exit_cycles: u32,
    pub runner_to_poll_exit_instructions: u32,
    pub runner_calls: u32,
    pub runner_cycles: u32,
    pub runner_instructions: u32,
    pub protocol_polls: u32,
    pub protocol_cycles: u32,
    pub protocol_instructions: u32,
    pub dma_calls: u32,
    pub dma_units: u32,
    pub dma_cycles: u32,
    pub dma_instructions: u32,
    pub protocol_frames: u32,
    pub protocol_frame_cycles: u32,
    pub protocol_frame_instructions: u32,
}

impl Core0PerformanceSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            rx_interrupt_posts: self
                .rx_interrupt_posts
                .wrapping_sub(earlier.rx_interrupt_posts),
            radio_polls: self.radio_polls.wrapping_sub(earlier.radio_polls),
            radio_cycles: self.radio_cycles.wrapping_sub(earlier.radio_cycles),
            radio_instructions: self
                .radio_instructions
                .wrapping_sub(earlier.radio_instructions),
            poll_to_runner_cycles: self
                .poll_to_runner_cycles
                .wrapping_sub(earlier.poll_to_runner_cycles),
            poll_to_runner_instructions: self
                .poll_to_runner_instructions
                .wrapping_sub(earlier.poll_to_runner_instructions),
            runner_to_poll_exit_cycles: self
                .runner_to_poll_exit_cycles
                .wrapping_sub(earlier.runner_to_poll_exit_cycles),
            runner_to_poll_exit_instructions: self
                .runner_to_poll_exit_instructions
                .wrapping_sub(earlier.runner_to_poll_exit_instructions),
            runner_calls: self.runner_calls.wrapping_sub(earlier.runner_calls),
            runner_cycles: self.runner_cycles.wrapping_sub(earlier.runner_cycles),
            runner_instructions: self
                .runner_instructions
                .wrapping_sub(earlier.runner_instructions),
            protocol_polls: self.protocol_polls.wrapping_sub(earlier.protocol_polls),
            protocol_cycles: self.protocol_cycles.wrapping_sub(earlier.protocol_cycles),
            protocol_instructions: self
                .protocol_instructions
                .wrapping_sub(earlier.protocol_instructions),
            dma_calls: self.dma_calls.wrapping_sub(earlier.dma_calls),
            dma_units: self.dma_units.wrapping_sub(earlier.dma_units),
            dma_cycles: self.dma_cycles.wrapping_sub(earlier.dma_cycles),
            dma_instructions: self.dma_instructions.wrapping_sub(earlier.dma_instructions),
            protocol_frames: self.protocol_frames.wrapping_sub(earlier.protocol_frames),
            protocol_frame_cycles: self
                .protocol_frame_cycles
                .wrapping_sub(earlier.protocol_frame_cycles),
            protocol_frame_instructions: self
                .protocol_frame_instructions
                .wrapping_sub(earlier.protocol_frame_instructions),
        }
    }
}

pub struct Core0PerformanceCounters {
    rx_interrupt_posts: AtomicU32,
    radio_polls: AtomicU32,
    radio_cycles: AtomicU32,
    radio_instructions: AtomicU32,
    poll_to_runner_cycles: AtomicU32,
    poll_to_runner_instructions: AtomicU32,
    runner_to_poll_exit_cycles: AtomicU32,
    runner_to_poll_exit_instructions: AtomicU32,
    runner_calls: AtomicU32,
    runner_cycles: AtomicU32,
    runner_instructions: AtomicU32,
    protocol_polls: AtomicU32,
    protocol_cycles: AtomicU32,
    protocol_instructions: AtomicU32,
    dma_calls: AtomicU32,
    dma_units: AtomicU32,
    dma_cycles: AtomicU32,
    dma_instructions: AtomicU32,
    protocol_frames: AtomicU32,
    protocol_frame_cycles: AtomicU32,
    protocol_frame_instructions: AtomicU32,
    active_radio_cycles: AtomicU32,
    active_radio_instructions: AtomicU32,
    active_radio_saw_runner: AtomicU32,
    active_runner_end_cycles: AtomicU32,
    active_runner_end_instructions: AtomicU32,
    active_protocol_cycles: AtomicU32,
    active_protocol_instructions: AtomicU32,
}

impl Core0PerformanceCounters {
    pub const fn new() -> Self {
        Self {
            rx_interrupt_posts: AtomicU32::new(0),
            radio_polls: AtomicU32::new(0),
            radio_cycles: AtomicU32::new(0),
            radio_instructions: AtomicU32::new(0),
            poll_to_runner_cycles: AtomicU32::new(0),
            poll_to_runner_instructions: AtomicU32::new(0),
            runner_to_poll_exit_cycles: AtomicU32::new(0),
            runner_to_poll_exit_instructions: AtomicU32::new(0),
            runner_calls: AtomicU32::new(0),
            runner_cycles: AtomicU32::new(0),
            runner_instructions: AtomicU32::new(0),
            protocol_polls: AtomicU32::new(0),
            protocol_cycles: AtomicU32::new(0),
            protocol_instructions: AtomicU32::new(0),
            dma_calls: AtomicU32::new(0),
            dma_units: AtomicU32::new(0),
            dma_cycles: AtomicU32::new(0),
            dma_instructions: AtomicU32::new(0),
            protocol_frames: AtomicU32::new(0),
            protocol_frame_cycles: AtomicU32::new(0),
            protocol_frame_instructions: AtomicU32::new(0),
            active_radio_cycles: AtomicU32::new(0),
            active_radio_instructions: AtomicU32::new(0),
            active_radio_saw_runner: AtomicU32::new(0),
            active_runner_end_cycles: AtomicU32::new(0),
            active_runner_end_instructions: AtomicU32::new(0),
            active_protocol_cycles: AtomicU32::new(0),
            active_protocol_instructions: AtomicU32::new(0),
        }
    }

    #[inline(always)]
    pub(crate) fn record_rx_interrupt_post(&self) {
        self.rx_interrupt_posts.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn begin_radio_poll(&self, started: Core0PerformanceSample) {
        self.active_radio_cycles
            .store(started.cycles, Ordering::Relaxed);
        self.active_radio_instructions
            .store(started.instructions, Ordering::Relaxed);
        self.active_radio_saw_runner.store(0, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn record_radio_poll(
        &self,
        started: Core0PerformanceSample,
        ended: Core0PerformanceSample,
    ) {
        let delta = ended.wrapping_delta_since(started);
        self.radio_polls.fetch_add(1, Ordering::Relaxed);
        self.radio_cycles.fetch_add(delta.cycles, Ordering::Relaxed);
        self.radio_instructions
            .fetch_add(delta.instructions, Ordering::Relaxed);
        if self.active_radio_saw_runner.load(Ordering::Relaxed) != 0 {
            self.runner_to_poll_exit_cycles.fetch_add(
                ended
                    .cycles
                    .wrapping_sub(self.active_runner_end_cycles.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
            self.runner_to_poll_exit_instructions.fetch_add(
                ended
                    .instructions
                    .wrapping_sub(self.active_runner_end_instructions.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        }
    }

    #[inline(always)]
    pub(crate) fn record_runner(
        &self,
        started: Core0PerformanceSample,
        ended: Core0PerformanceSample,
    ) {
        let delta = ended.wrapping_delta_since(started);
        self.runner_calls.fetch_add(1, Ordering::Relaxed);
        self.runner_cycles
            .fetch_add(delta.cycles, Ordering::Relaxed);
        self.runner_instructions
            .fetch_add(delta.instructions, Ordering::Relaxed);
        self.poll_to_runner_cycles.fetch_add(
            started
                .cycles
                .wrapping_sub(self.active_radio_cycles.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        self.poll_to_runner_instructions.fetch_add(
            started
                .instructions
                .wrapping_sub(self.active_radio_instructions.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        self.active_runner_end_instructions
            .store(ended.instructions, Ordering::Relaxed);
        self.active_runner_end_cycles
            .store(ended.cycles, Ordering::Relaxed);
        self.active_radio_saw_runner.store(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn begin_protocol_poll(&self, started: Core0PerformanceSample) {
        self.active_protocol_cycles
            .store(started.cycles, Ordering::Relaxed);
        self.active_protocol_instructions
            .store(started.instructions, Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn end_protocol_poll(&self, ended: Core0PerformanceSample) {
        let started = Core0PerformanceSample {
            cycles: self.active_protocol_cycles.load(Ordering::Relaxed),
            instructions: self.active_protocol_instructions.load(Ordering::Relaxed),
        };
        let delta = ended.wrapping_delta_since(started);
        self.protocol_polls.fetch_add(1, Ordering::Relaxed);
        self.protocol_cycles
            .fetch_add(delta.cycles, Ordering::Relaxed);
        self.protocol_instructions
            .fetch_add(delta.instructions, Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn record_dma(
        &self,
        units: usize,
        started: Core0PerformanceSample,
        ended: Core0PerformanceSample,
    ) {
        let delta = ended.wrapping_delta_since(started);
        self.dma_calls.fetch_add(1, Ordering::Relaxed);
        self.dma_units
            .fetch_add(u32::try_from(units).unwrap_or(u32::MAX), Ordering::Relaxed);
        self.dma_cycles.fetch_add(delta.cycles, Ordering::Relaxed);
        self.dma_instructions
            .fetch_add(delta.instructions, Ordering::Relaxed);
    }

    #[inline(always)]
    #[cfg(feature = "task-poll-telemetry")]
    pub(crate) fn record_protocol_frame(
        &self,
        started: Core0PerformanceSample,
        ended: Core0PerformanceSample,
    ) {
        let delta = ended.wrapping_delta_since(started);
        self.protocol_frames.fetch_add(1, Ordering::Relaxed);
        self.protocol_frame_cycles
            .fetch_add(delta.cycles, Ordering::Relaxed);
        self.protocol_frame_instructions
            .fetch_add(delta.instructions, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Core0PerformanceSnapshot {
        Core0PerformanceSnapshot {
            rx_interrupt_posts: self.rx_interrupt_posts.load(Ordering::Relaxed),
            radio_polls: self.radio_polls.load(Ordering::Relaxed),
            radio_cycles: self.radio_cycles.load(Ordering::Relaxed),
            radio_instructions: self.radio_instructions.load(Ordering::Relaxed),
            poll_to_runner_cycles: self.poll_to_runner_cycles.load(Ordering::Relaxed),
            poll_to_runner_instructions: self.poll_to_runner_instructions.load(Ordering::Relaxed),
            runner_to_poll_exit_cycles: self.runner_to_poll_exit_cycles.load(Ordering::Relaxed),
            runner_to_poll_exit_instructions: self
                .runner_to_poll_exit_instructions
                .load(Ordering::Relaxed),
            runner_calls: self.runner_calls.load(Ordering::Relaxed),
            runner_cycles: self.runner_cycles.load(Ordering::Relaxed),
            runner_instructions: self.runner_instructions.load(Ordering::Relaxed),
            protocol_polls: self.protocol_polls.load(Ordering::Relaxed),
            protocol_cycles: self.protocol_cycles.load(Ordering::Relaxed),
            protocol_instructions: self.protocol_instructions.load(Ordering::Relaxed),
            dma_calls: self.dma_calls.load(Ordering::Relaxed),
            dma_units: self.dma_units.load(Ordering::Relaxed),
            dma_cycles: self.dma_cycles.load(Ordering::Relaxed),
            dma_instructions: self.dma_instructions.load(Ordering::Relaxed),
            protocol_frames: self.protocol_frames.load(Ordering::Relaxed),
            protocol_frame_cycles: self.protocol_frame_cycles.load(Ordering::Relaxed),
            protocol_frame_instructions: self.protocol_frame_instructions.load(Ordering::Relaxed),
        }
    }
}

impl Default for Core0PerformanceCounters {
    fn default() -> Self {
        Self::new()
    }
}

pub static CORE0_PERFORMANCE: Core0PerformanceCounters = Core0PerformanceCounters::new();

/// Low-overhead measurement of one complete RX runner call.
///
/// Unlike the deep phase profiler this owner performs no intermediate reads.
/// The terminal update happens before the mandatory cooperative yield, so
/// sleeping executor time is not charged to the runner.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub(crate) struct Core0PerformanceRunnerProfile {
    started: Core0PerformanceSample,
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
impl Core0PerformanceRunnerProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        Self {
            started: Core0PerformanceSample::read(),
        }
    }

    #[inline(always)]
    pub(crate) fn begin_driver(&mut self) {}

    #[inline(always)]
    pub(crate) fn end_driver(&mut self) {}

    #[inline(always)]
    pub(crate) fn finish_before_yield(self) {
        CORE0_PERFORMANCE.record_runner(self.started, Core0PerformanceSample::read());
    }
}

/// Low-overhead measurement of one complete DMA service transaction.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub(crate) struct Core0PerformanceDmaProfile {
    started: Core0PerformanceSample,
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
impl Core0PerformanceDmaProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        Self {
            started: Core0PerformanceSample::read(),
        }
    }

    #[inline(always)]
    pub(crate) fn finish(self, units: usize) {
        CORE0_PERFORMANCE.record_dma(units, self.started, Core0PerformanceSample::read());
    }
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
fn cycle_count() -> u32 {
    riscv::register::mcycle::read() as u32
}

#[cfg(not(target_arch = "riscv32"))]
#[inline(always)]
fn cycle_count() -> u32 {
    0
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
fn instruction_count() -> u32 {
    riscv::register::minstret::read() as u32
}

#[cfg(not(target_arch = "riscv32"))]
#[inline(always)]
fn instruction_count() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::Core0PerformanceSnapshot;

    #[test]
    fn interval_snapshot_uses_wrapping_deltas() {
        let earlier = Core0PerformanceSnapshot {
            rx_interrupt_posts: u32::MAX,
            radio_polls: u32::MAX,
            radio_cycles: 80,
            radio_instructions: 90,
            poll_to_runner_cycles: u32::MAX,
            poll_to_runner_instructions: u32::MAX,
            dma_calls: u32::MAX,
            ..Core0PerformanceSnapshot::default()
        };
        let current = Core0PerformanceSnapshot {
            rx_interrupt_posts: 2,
            radio_polls: 2,
            radio_cycles: 130,
            radio_instructions: 120,
            poll_to_runner_cycles: 3,
            poll_to_runner_instructions: 4,
            dma_calls: 2,
            ..Core0PerformanceSnapshot::default()
        };
        let delta = current.wrapping_delta_since(earlier);
        assert_eq!(delta.rx_interrupt_posts, 3);
        assert_eq!(delta.radio_polls, 3);
        assert_eq!(delta.radio_cycles, 50);
        assert_eq!(delta.radio_instructions, 30);
        assert_eq!(delta.poll_to_runner_cycles, 4);
        assert_eq!(delta.poll_to_runner_instructions, 5);
        assert_eq!(delta.dma_calls, 3);
    }
}
