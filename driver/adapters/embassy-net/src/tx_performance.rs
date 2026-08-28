//! Diagnostic-only Core1 TX phase accounting.
//!
//! The counters split the network-stack/device boundary into admission,
//! packet emission, and publication without changing packet ownership.  They
//! are deliberately absent from production builds because each observation
//! reads `mcycle` and `minstret` in the per-datagram path.

use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxPerformanceSample {
    pub cycles: u32,
    pub instructions: u32,
}

impl TxPerformanceSample {
    #[inline(always)]
    pub fn read() -> Self {
        Self {
            cycles: cycle_count(),
            instructions: instruction_count(),
        }
    }

    #[inline(always)]
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            cycles: self.cycles.wrapping_sub(earlier.cycles),
            instructions: self.instructions.wrapping_sub(earlier.instructions),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxPerformanceSnapshot {
    pub admission_attempts: u32,
    pub admission_successes: u32,
    pub admission_cycles: u32,
    pub admission_instructions: u32,
    pub consume_calls: u32,
    pub consume_bytes: u32,
    pub consume_cycles: u32,
    pub consume_instructions: u32,
    pub emit_cycles: u32,
    pub emit_instructions: u32,
}

impl TxPerformanceSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            admission_attempts: self
                .admission_attempts
                .wrapping_sub(earlier.admission_attempts),
            admission_successes: self
                .admission_successes
                .wrapping_sub(earlier.admission_successes),
            admission_cycles: self.admission_cycles.wrapping_sub(earlier.admission_cycles),
            admission_instructions: self
                .admission_instructions
                .wrapping_sub(earlier.admission_instructions),
            consume_calls: self.consume_calls.wrapping_sub(earlier.consume_calls),
            consume_bytes: self.consume_bytes.wrapping_sub(earlier.consume_bytes),
            consume_cycles: self.consume_cycles.wrapping_sub(earlier.consume_cycles),
            consume_instructions: self
                .consume_instructions
                .wrapping_sub(earlier.consume_instructions),
            emit_cycles: self.emit_cycles.wrapping_sub(earlier.emit_cycles),
            emit_instructions: self
                .emit_instructions
                .wrapping_sub(earlier.emit_instructions),
        }
    }

    /// Device-side work surrounding the stack's packet-emission callback.
    pub fn publication_cycles(self) -> u32 {
        self.consume_cycles.wrapping_sub(self.emit_cycles)
    }

    /// Retired instructions surrounding the stack's packet-emission callback.
    pub fn publication_instructions(self) -> u32 {
        self.consume_instructions
            .wrapping_sub(self.emit_instructions)
    }
}

pub struct TxPerformanceCounters {
    admission_attempts: AtomicU32,
    admission_successes: AtomicU32,
    admission_cycles: AtomicU32,
    admission_instructions: AtomicU32,
    consume_calls: AtomicU32,
    consume_bytes: AtomicU32,
    consume_cycles: AtomicU32,
    consume_instructions: AtomicU32,
    emit_cycles: AtomicU32,
    emit_instructions: AtomicU32,
}

impl TxPerformanceCounters {
    pub const fn new() -> Self {
        Self {
            admission_attempts: AtomicU32::new(0),
            admission_successes: AtomicU32::new(0),
            admission_cycles: AtomicU32::new(0),
            admission_instructions: AtomicU32::new(0),
            consume_calls: AtomicU32::new(0),
            consume_bytes: AtomicU32::new(0),
            consume_cycles: AtomicU32::new(0),
            consume_instructions: AtomicU32::new(0),
            emit_cycles: AtomicU32::new(0),
            emit_instructions: AtomicU32::new(0),
        }
    }

    #[inline(always)]
    pub(crate) fn record_admission(
        &self,
        started: TxPerformanceSample,
        ended: TxPerformanceSample,
        succeeded: bool,
    ) {
        let delta = ended.wrapping_delta_since(started);
        self.admission_attempts.fetch_add(1, Ordering::Relaxed);
        self.admission_successes
            .fetch_add(u32::from(succeeded), Ordering::Relaxed);
        self.admission_cycles
            .fetch_add(delta.cycles, Ordering::Relaxed);
        self.admission_instructions
            .fetch_add(delta.instructions, Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn record_consume(
        &self,
        bytes: usize,
        started: TxPerformanceSample,
        emitted: TxPerformanceSample,
        ended: TxPerformanceSample,
    ) {
        let total = ended.wrapping_delta_since(started);
        self.consume_calls.fetch_add(1, Ordering::Relaxed);
        self.consume_bytes
            .fetch_add(u32::try_from(bytes).unwrap_or(u32::MAX), Ordering::Relaxed);
        self.consume_cycles
            .fetch_add(total.cycles, Ordering::Relaxed);
        self.consume_instructions
            .fetch_add(total.instructions, Ordering::Relaxed);
        self.emit_cycles
            .fetch_add(emitted.cycles, Ordering::Relaxed);
        self.emit_instructions
            .fetch_add(emitted.instructions, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> TxPerformanceSnapshot {
        TxPerformanceSnapshot {
            admission_attempts: self.admission_attempts.load(Ordering::Relaxed),
            admission_successes: self.admission_successes.load(Ordering::Relaxed),
            admission_cycles: self.admission_cycles.load(Ordering::Relaxed),
            admission_instructions: self.admission_instructions.load(Ordering::Relaxed),
            consume_calls: self.consume_calls.load(Ordering::Relaxed),
            consume_bytes: self.consume_bytes.load(Ordering::Relaxed),
            consume_cycles: self.consume_cycles.load(Ordering::Relaxed),
            consume_instructions: self.consume_instructions.load(Ordering::Relaxed),
            emit_cycles: self.emit_cycles.load(Ordering::Relaxed),
            emit_instructions: self.emit_instructions.load(Ordering::Relaxed),
        }
    }
}

impl Default for TxPerformanceCounters {
    fn default() -> Self {
        Self::new()
    }
}

pub static TX_PERFORMANCE: TxPerformanceCounters = TxPerformanceCounters::new();

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
    use super::{TxPerformanceSample, TxPerformanceSnapshot};

    #[test]
    fn interval_and_publication_use_wrapping_deltas() {
        let earlier = TxPerformanceSnapshot {
            consume_calls: u32::MAX,
            consume_cycles: 80,
            consume_instructions: 40,
            emit_cycles: 50,
            emit_instructions: 30,
            ..TxPerformanceSnapshot::default()
        };
        let current = TxPerformanceSnapshot {
            consume_calls: 2,
            consume_cycles: 150,
            consume_instructions: 90,
            emit_cycles: 90,
            emit_instructions: 55,
            ..TxPerformanceSnapshot::default()
        };
        let delta = current.wrapping_delta_since(earlier);
        assert_eq!(delta.consume_calls, 3);
        assert_eq!(delta.publication_cycles(), 30);
        assert_eq!(delta.publication_instructions(), 25);

        assert_eq!(
            TxPerformanceSample {
                cycles: 5,
                instructions: 7,
            }
            .wrapping_delta_since(TxPerformanceSample {
                cycles: u32::MAX,
                instructions: u32::MAX,
            }),
            TxPerformanceSample {
                cycles: 6,
                instructions: 8,
            }
        );
    }
}
