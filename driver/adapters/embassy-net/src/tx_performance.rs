//! Diagnostic-only network TX phase accounting.
//!
//! The counters split the network-stack/device boundary into admission,
//! packet emission, publication and staged Core0 promotion without changing
//! packet ownership. They are deliberately absent from production builds
//! because each observation reads `mcycle` and `minstret` in the per-datagram
//! path.

use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxPerformanceSample {
    pub cycles: u32,
    pub instructions: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TxShadowGrantObservation {
    Matched,
    NoWindow,
    KeyMismatch,
    CreditExhausted,
    Unclassified,
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
    pub classification_calls: u32,
    pub classification_cycles: u32,
    pub classification_instructions: u32,
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
    pub egress_runs: u32,
    pub egress_run_31: u32,
    pub egress_run_32: u32,
    pub egress_run_other: u32,
    pub shadow_grant_checks: u32,
    pub shadow_grant_matches: u32,
    pub shadow_grant_no_window: u32,
    pub shadow_grant_key_mismatch: u32,
    pub shadow_grant_credit_exhausted: u32,
    pub shadow_grant_unclassified: u32,
    pub radio_returns: u32,
    pub radio_return_wakes: u32,
    pub publication_free_zero: u32,
    pub publication_free_one: u32,
    pub publication_free_two_plus: u32,
    pub publication_ready_le31: u32,
    pub publication_ready_32: u32,
    pub publication_ready_ge33: u32,
    pub promotion_attempts: u32,
    pub promotion_successes: u32,
    pub promotion_no_credit: u32,
    pub promotion_bytes: u32,
    pub promotion_cycles: u32,
    pub promotion_instructions: u32,
    pub promotion_credit_cycles: u32,
    pub promotion_credit_instructions: u32,
    pub promotion_destination_claim_cycles: u32,
    pub promotion_destination_claim_instructions: u32,
    pub promotion_copy_cycles: u32,
    pub promotion_copy_instructions: u32,
    pub promotion_publication_cycles: u32,
    pub promotion_publication_instructions: u32,
    pub promotion_source_release_cycles: u32,
    pub promotion_source_release_instructions: u32,
    pub promotion_radio_claim_cycles: u32,
    pub promotion_radio_claim_instructions: u32,
}

impl TxPerformanceSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            classification_calls: self
                .classification_calls
                .wrapping_sub(earlier.classification_calls),
            classification_cycles: self
                .classification_cycles
                .wrapping_sub(earlier.classification_cycles),
            classification_instructions: self
                .classification_instructions
                .wrapping_sub(earlier.classification_instructions),
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
            egress_runs: self.egress_runs.wrapping_sub(earlier.egress_runs),
            egress_run_31: self.egress_run_31.wrapping_sub(earlier.egress_run_31),
            egress_run_32: self.egress_run_32.wrapping_sub(earlier.egress_run_32),
            egress_run_other: self.egress_run_other.wrapping_sub(earlier.egress_run_other),
            shadow_grant_checks: self
                .shadow_grant_checks
                .wrapping_sub(earlier.shadow_grant_checks),
            shadow_grant_matches: self
                .shadow_grant_matches
                .wrapping_sub(earlier.shadow_grant_matches),
            shadow_grant_no_window: self
                .shadow_grant_no_window
                .wrapping_sub(earlier.shadow_grant_no_window),
            shadow_grant_key_mismatch: self
                .shadow_grant_key_mismatch
                .wrapping_sub(earlier.shadow_grant_key_mismatch),
            shadow_grant_credit_exhausted: self
                .shadow_grant_credit_exhausted
                .wrapping_sub(earlier.shadow_grant_credit_exhausted),
            shadow_grant_unclassified: self
                .shadow_grant_unclassified
                .wrapping_sub(earlier.shadow_grant_unclassified),
            radio_returns: self.radio_returns.wrapping_sub(earlier.radio_returns),
            radio_return_wakes: self
                .radio_return_wakes
                .wrapping_sub(earlier.radio_return_wakes),
            publication_free_zero: self
                .publication_free_zero
                .wrapping_sub(earlier.publication_free_zero),
            publication_free_one: self
                .publication_free_one
                .wrapping_sub(earlier.publication_free_one),
            publication_free_two_plus: self
                .publication_free_two_plus
                .wrapping_sub(earlier.publication_free_two_plus),
            publication_ready_le31: self
                .publication_ready_le31
                .wrapping_sub(earlier.publication_ready_le31),
            publication_ready_32: self
                .publication_ready_32
                .wrapping_sub(earlier.publication_ready_32),
            publication_ready_ge33: self
                .publication_ready_ge33
                .wrapping_sub(earlier.publication_ready_ge33),
            promotion_attempts: self
                .promotion_attempts
                .wrapping_sub(earlier.promotion_attempts),
            promotion_successes: self
                .promotion_successes
                .wrapping_sub(earlier.promotion_successes),
            promotion_no_credit: self
                .promotion_no_credit
                .wrapping_sub(earlier.promotion_no_credit),
            promotion_bytes: self.promotion_bytes.wrapping_sub(earlier.promotion_bytes),
            promotion_cycles: self.promotion_cycles.wrapping_sub(earlier.promotion_cycles),
            promotion_instructions: self
                .promotion_instructions
                .wrapping_sub(earlier.promotion_instructions),
            promotion_credit_cycles: self
                .promotion_credit_cycles
                .wrapping_sub(earlier.promotion_credit_cycles),
            promotion_credit_instructions: self
                .promotion_credit_instructions
                .wrapping_sub(earlier.promotion_credit_instructions),
            promotion_destination_claim_cycles: self
                .promotion_destination_claim_cycles
                .wrapping_sub(earlier.promotion_destination_claim_cycles),
            promotion_destination_claim_instructions: self
                .promotion_destination_claim_instructions
                .wrapping_sub(earlier.promotion_destination_claim_instructions),
            promotion_copy_cycles: self
                .promotion_copy_cycles
                .wrapping_sub(earlier.promotion_copy_cycles),
            promotion_copy_instructions: self
                .promotion_copy_instructions
                .wrapping_sub(earlier.promotion_copy_instructions),
            promotion_publication_cycles: self
                .promotion_publication_cycles
                .wrapping_sub(earlier.promotion_publication_cycles),
            promotion_publication_instructions: self
                .promotion_publication_instructions
                .wrapping_sub(earlier.promotion_publication_instructions),
            promotion_source_release_cycles: self
                .promotion_source_release_cycles
                .wrapping_sub(earlier.promotion_source_release_cycles),
            promotion_source_release_instructions: self
                .promotion_source_release_instructions
                .wrapping_sub(earlier.promotion_source_release_instructions),
            promotion_radio_claim_cycles: self
                .promotion_radio_claim_cycles
                .wrapping_sub(earlier.promotion_radio_claim_cycles),
            promotion_radio_claim_instructions: self
                .promotion_radio_claim_instructions
                .wrapping_sub(earlier.promotion_radio_claim_instructions),
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

    pub fn promotion_unattributed_cycles(self) -> u32 {
        self.promotion_cycles.wrapping_sub(
            self.promotion_credit_cycles
                .wrapping_add(self.promotion_destination_claim_cycles)
                .wrapping_add(self.promotion_copy_cycles)
                .wrapping_add(self.promotion_publication_cycles)
                .wrapping_add(self.promotion_source_release_cycles)
                .wrapping_add(self.promotion_radio_claim_cycles),
        )
    }

    pub fn promotion_unattributed_instructions(self) -> u32 {
        self.promotion_instructions.wrapping_sub(
            self.promotion_credit_instructions
                .wrapping_add(self.promotion_destination_claim_instructions)
                .wrapping_add(self.promotion_copy_instructions)
                .wrapping_add(self.promotion_publication_instructions)
                .wrapping_add(self.promotion_source_release_instructions)
                .wrapping_add(self.promotion_radio_claim_instructions),
        )
    }
}

pub struct TxPerformanceCounters {
    classification_calls: AtomicU32,
    classification_cycles: AtomicU32,
    classification_instructions: AtomicU32,
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
    egress_runs: AtomicU32,
    egress_run_31: AtomicU32,
    egress_run_32: AtomicU32,
    egress_run_other: AtomicU32,
    shadow_grant_checks: AtomicU32,
    shadow_grant_matches: AtomicU32,
    shadow_grant_no_window: AtomicU32,
    shadow_grant_key_mismatch: AtomicU32,
    shadow_grant_credit_exhausted: AtomicU32,
    shadow_grant_unclassified: AtomicU32,
    radio_returns: AtomicU32,
    radio_return_wakes: AtomicU32,
    publication_free_zero: AtomicU32,
    publication_free_one: AtomicU32,
    publication_free_two_plus: AtomicU32,
    publication_ready_le31: AtomicU32,
    publication_ready_32: AtomicU32,
    publication_ready_ge33: AtomicU32,
    promotion_attempts: AtomicU32,
    promotion_successes: AtomicU32,
    promotion_no_credit: AtomicU32,
    promotion_bytes: AtomicU32,
    promotion_cycles: AtomicU32,
    promotion_instructions: AtomicU32,
    promotion_credit_cycles: AtomicU32,
    promotion_credit_instructions: AtomicU32,
    promotion_destination_claim_cycles: AtomicU32,
    promotion_destination_claim_instructions: AtomicU32,
    promotion_copy_cycles: AtomicU32,
    promotion_copy_instructions: AtomicU32,
    promotion_publication_cycles: AtomicU32,
    promotion_publication_instructions: AtomicU32,
    promotion_source_release_cycles: AtomicU32,
    promotion_source_release_instructions: AtomicU32,
    promotion_radio_claim_cycles: AtomicU32,
    promotion_radio_claim_instructions: AtomicU32,
}

impl TxPerformanceCounters {
    pub const fn new() -> Self {
        Self {
            classification_calls: AtomicU32::new(0),
            classification_cycles: AtomicU32::new(0),
            classification_instructions: AtomicU32::new(0),
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
            egress_runs: AtomicU32::new(0),
            egress_run_31: AtomicU32::new(0),
            egress_run_32: AtomicU32::new(0),
            egress_run_other: AtomicU32::new(0),
            shadow_grant_checks: AtomicU32::new(0),
            shadow_grant_matches: AtomicU32::new(0),
            shadow_grant_no_window: AtomicU32::new(0),
            shadow_grant_key_mismatch: AtomicU32::new(0),
            shadow_grant_credit_exhausted: AtomicU32::new(0),
            shadow_grant_unclassified: AtomicU32::new(0),
            radio_returns: AtomicU32::new(0),
            radio_return_wakes: AtomicU32::new(0),
            publication_free_zero: AtomicU32::new(0),
            publication_free_one: AtomicU32::new(0),
            publication_free_two_plus: AtomicU32::new(0),
            publication_ready_le31: AtomicU32::new(0),
            publication_ready_32: AtomicU32::new(0),
            publication_ready_ge33: AtomicU32::new(0),
            promotion_attempts: AtomicU32::new(0),
            promotion_successes: AtomicU32::new(0),
            promotion_no_credit: AtomicU32::new(0),
            promotion_bytes: AtomicU32::new(0),
            promotion_cycles: AtomicU32::new(0),
            promotion_instructions: AtomicU32::new(0),
            promotion_credit_cycles: AtomicU32::new(0),
            promotion_credit_instructions: AtomicU32::new(0),
            promotion_destination_claim_cycles: AtomicU32::new(0),
            promotion_destination_claim_instructions: AtomicU32::new(0),
            promotion_copy_cycles: AtomicU32::new(0),
            promotion_copy_instructions: AtomicU32::new(0),
            promotion_publication_cycles: AtomicU32::new(0),
            promotion_publication_instructions: AtomicU32::new(0),
            promotion_source_release_cycles: AtomicU32::new(0),
            promotion_source_release_instructions: AtomicU32::new(0),
            promotion_radio_claim_cycles: AtomicU32::new(0),
            promotion_radio_claim_instructions: AtomicU32::new(0),
        }
    }

    #[inline(always)]
    pub(crate) fn record_classification(
        &self,
        started: TxPerformanceSample,
        ended: TxPerformanceSample,
    ) {
        let delta = ended.wrapping_delta_since(started);
        self.classification_calls.fetch_add(1, Ordering::Relaxed);
        self.classification_cycles
            .fetch_add(delta.cycles, Ordering::Relaxed);
        self.classification_instructions
            .fetch_add(delta.instructions, Ordering::Relaxed);
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

    #[inline(always)]
    pub(crate) fn record_egress_run(&self, length: u8) {
        if length == 0 {
            return;
        }
        self.egress_runs.fetch_add(1, Ordering::Relaxed);
        match length {
            31 => self.egress_run_31.fetch_add(1, Ordering::Relaxed),
            32 => self.egress_run_32.fetch_add(1, Ordering::Relaxed),
            _ => self.egress_run_other.fetch_add(1, Ordering::Relaxed),
        };
    }

    #[inline(always)]
    pub(crate) fn publish_shadow_grant(
        &self,
        observation: TxShadowGrantObservation,
        checks: u32,
        category: u32,
    ) {
        self.shadow_grant_checks.store(checks, Ordering::Relaxed);
        match observation {
            TxShadowGrantObservation::Matched => &self.shadow_grant_matches,
            TxShadowGrantObservation::NoWindow => &self.shadow_grant_no_window,
            TxShadowGrantObservation::KeyMismatch => &self.shadow_grant_key_mismatch,
            TxShadowGrantObservation::CreditExhausted => &self.shadow_grant_credit_exhausted,
            TxShadowGrantObservation::Unclassified => &self.shadow_grant_unclassified,
        }
        .store(category, Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn record_publication_geometry(&self, free: usize, ready: usize) {
        match free {
            0 => &self.publication_free_zero,
            1 => &self.publication_free_one,
            _ => &self.publication_free_two_plus,
        }
        .fetch_add(1, Ordering::Relaxed);
        match ready {
            0..=31 => &self.publication_ready_le31,
            32 => &self.publication_ready_32,
            _ => &self.publication_ready_ge33,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn record_radio_return(&self, woke_network: bool) {
        self.radio_returns.fetch_add(1, Ordering::Relaxed);
        self.radio_return_wakes
            .fetch_add(u32::from(woke_network), Ordering::Relaxed);
    }

    #[inline(always)]
    #[cfg(any(feature = "tx-staging-copy-probe", test))]
    pub(crate) fn record_promotion_no_credit(
        &self,
        started: TxPerformanceSample,
        ended: TxPerformanceSample,
    ) {
        let total = ended.wrapping_delta_since(started);
        self.promotion_attempts.fetch_add(1, Ordering::Relaxed);
        self.promotion_no_credit.fetch_add(1, Ordering::Relaxed);
        self.promotion_cycles
            .fetch_add(total.cycles, Ordering::Relaxed);
        self.promotion_instructions
            .fetch_add(total.instructions, Ordering::Relaxed);
        self.promotion_credit_cycles
            .fetch_add(total.cycles, Ordering::Relaxed);
        self.promotion_credit_instructions
            .fetch_add(total.instructions, Ordering::Relaxed);
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    #[cfg(any(feature = "tx-staging-copy-probe", test))]
    pub(crate) fn record_promotion(
        &self,
        bytes: usize,
        started: TxPerformanceSample,
        credit_acquired: TxPerformanceSample,
        destination_claimed: TxPerformanceSample,
        copy: TxPerformanceSample,
        publication_started: TxPerformanceSample,
        published: TxPerformanceSample,
        source_released: TxPerformanceSample,
        radio_claimed: TxPerformanceSample,
    ) {
        let total = radio_claimed.wrapping_delta_since(started);
        let credit = credit_acquired.wrapping_delta_since(started);
        let destination_claim = destination_claimed.wrapping_delta_since(credit_acquired);
        let publication_total = published.wrapping_delta_since(publication_started);
        let publication = TxPerformanceSample {
            cycles: publication_total.cycles.wrapping_sub(copy.cycles),
            instructions: publication_total
                .instructions
                .wrapping_sub(copy.instructions),
        };
        let source_release = source_released.wrapping_delta_since(published);
        let radio_claim = radio_claimed.wrapping_delta_since(source_released);

        self.promotion_attempts.fetch_add(1, Ordering::Relaxed);
        self.promotion_successes.fetch_add(1, Ordering::Relaxed);
        self.promotion_bytes
            .fetch_add(u32::try_from(bytes).unwrap_or(u32::MAX), Ordering::Relaxed);
        for (sample, cycles, instructions) in [
            (total, &self.promotion_cycles, &self.promotion_instructions),
            (
                credit,
                &self.promotion_credit_cycles,
                &self.promotion_credit_instructions,
            ),
            (
                destination_claim,
                &self.promotion_destination_claim_cycles,
                &self.promotion_destination_claim_instructions,
            ),
            (
                copy,
                &self.promotion_copy_cycles,
                &self.promotion_copy_instructions,
            ),
            (
                publication,
                &self.promotion_publication_cycles,
                &self.promotion_publication_instructions,
            ),
            (
                source_release,
                &self.promotion_source_release_cycles,
                &self.promotion_source_release_instructions,
            ),
            (
                radio_claim,
                &self.promotion_radio_claim_cycles,
                &self.promotion_radio_claim_instructions,
            ),
        ] {
            cycles.fetch_add(sample.cycles, Ordering::Relaxed);
            instructions.fetch_add(sample.instructions, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> TxPerformanceSnapshot {
        TxPerformanceSnapshot {
            classification_calls: self.classification_calls.load(Ordering::Relaxed),
            classification_cycles: self.classification_cycles.load(Ordering::Relaxed),
            classification_instructions: self.classification_instructions.load(Ordering::Relaxed),
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
            egress_runs: self.egress_runs.load(Ordering::Relaxed),
            egress_run_31: self.egress_run_31.load(Ordering::Relaxed),
            egress_run_32: self.egress_run_32.load(Ordering::Relaxed),
            egress_run_other: self.egress_run_other.load(Ordering::Relaxed),
            shadow_grant_checks: self.shadow_grant_checks.load(Ordering::Relaxed),
            shadow_grant_matches: self.shadow_grant_matches.load(Ordering::Relaxed),
            shadow_grant_no_window: self.shadow_grant_no_window.load(Ordering::Relaxed),
            shadow_grant_key_mismatch: self.shadow_grant_key_mismatch.load(Ordering::Relaxed),
            shadow_grant_credit_exhausted: self
                .shadow_grant_credit_exhausted
                .load(Ordering::Relaxed),
            shadow_grant_unclassified: self.shadow_grant_unclassified.load(Ordering::Relaxed),
            radio_returns: self.radio_returns.load(Ordering::Relaxed),
            radio_return_wakes: self.radio_return_wakes.load(Ordering::Relaxed),
            publication_free_zero: self.publication_free_zero.load(Ordering::Relaxed),
            publication_free_one: self.publication_free_one.load(Ordering::Relaxed),
            publication_free_two_plus: self.publication_free_two_plus.load(Ordering::Relaxed),
            publication_ready_le31: self.publication_ready_le31.load(Ordering::Relaxed),
            publication_ready_32: self.publication_ready_32.load(Ordering::Relaxed),
            publication_ready_ge33: self.publication_ready_ge33.load(Ordering::Relaxed),
            promotion_attempts: self.promotion_attempts.load(Ordering::Relaxed),
            promotion_successes: self.promotion_successes.load(Ordering::Relaxed),
            promotion_no_credit: self.promotion_no_credit.load(Ordering::Relaxed),
            promotion_bytes: self.promotion_bytes.load(Ordering::Relaxed),
            promotion_cycles: self.promotion_cycles.load(Ordering::Relaxed),
            promotion_instructions: self.promotion_instructions.load(Ordering::Relaxed),
            promotion_credit_cycles: self.promotion_credit_cycles.load(Ordering::Relaxed),
            promotion_credit_instructions: self
                .promotion_credit_instructions
                .load(Ordering::Relaxed),
            promotion_destination_claim_cycles: self
                .promotion_destination_claim_cycles
                .load(Ordering::Relaxed),
            promotion_destination_claim_instructions: self
                .promotion_destination_claim_instructions
                .load(Ordering::Relaxed),
            promotion_copy_cycles: self.promotion_copy_cycles.load(Ordering::Relaxed),
            promotion_copy_instructions: self.promotion_copy_instructions.load(Ordering::Relaxed),
            promotion_publication_cycles: self.promotion_publication_cycles.load(Ordering::Relaxed),
            promotion_publication_instructions: self
                .promotion_publication_instructions
                .load(Ordering::Relaxed),
            promotion_source_release_cycles: self
                .promotion_source_release_cycles
                .load(Ordering::Relaxed),
            promotion_source_release_instructions: self
                .promotion_source_release_instructions
                .load(Ordering::Relaxed),
            promotion_radio_claim_cycles: self.promotion_radio_claim_cycles.load(Ordering::Relaxed),
            promotion_radio_claim_instructions: self
                .promotion_radio_claim_instructions
                .load(Ordering::Relaxed),
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
    use super::{
        TxPerformanceCounters, TxPerformanceSample, TxPerformanceSnapshot, TxShadowGrantObservation,
    };

    const fn sample(cycles: u32, instructions: u32) -> TxPerformanceSample {
        TxPerformanceSample {
            cycles,
            instructions,
        }
    }

    #[test]
    fn interval_and_publication_use_wrapping_deltas() {
        let earlier = TxPerformanceSnapshot {
            classification_calls: u32::MAX,
            classification_cycles: 60,
            classification_instructions: 20,
            consume_calls: u32::MAX,
            consume_cycles: 80,
            consume_instructions: 40,
            emit_cycles: 50,
            emit_instructions: 30,
            ..TxPerformanceSnapshot::default()
        };
        let current = TxPerformanceSnapshot {
            classification_calls: 2,
            classification_cycles: 95,
            classification_instructions: 42,
            consume_calls: 2,
            consume_cycles: 150,
            consume_instructions: 90,
            emit_cycles: 90,
            emit_instructions: 55,
            ..TxPerformanceSnapshot::default()
        };
        let delta = current.wrapping_delta_since(earlier);
        assert_eq!(delta.classification_calls, 3);
        assert_eq!(delta.classification_cycles, 35);
        assert_eq!(delta.classification_instructions, 22);
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

    #[test]
    fn classification_accounting_records_the_measured_boundary() {
        let counters = TxPerformanceCounters::new();
        counters.record_classification(sample(100, 200), sample(117, 209));

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.classification_calls, 1);
        assert_eq!(snapshot.classification_cycles, 17);
        assert_eq!(snapshot.classification_instructions, 9);
    }

    #[test]
    fn promotion_accounting_separates_copy_from_owner_transitions() {
        let counters = TxPerformanceCounters::new();
        counters.record_promotion(
            1_536,
            sample(100, 200),
            sample(110, 205),
            sample(117, 209),
            sample(20, 10),
            sample(120, 210),
            sample(150, 225),
            sample(170, 235),
            sample(180, 240),
        );
        counters.record_promotion_no_credit(sample(1_000, 2_000), sample(1_012, 2_007));

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.promotion_attempts, 2);
        assert_eq!(snapshot.promotion_successes, 1);
        assert_eq!(snapshot.promotion_no_credit, 1);
        assert_eq!(snapshot.promotion_bytes, 1_536);
        assert_eq!(snapshot.promotion_cycles, 92);
        assert_eq!(snapshot.promotion_instructions, 47);
        assert_eq!(snapshot.promotion_credit_cycles, 22);
        assert_eq!(snapshot.promotion_credit_instructions, 12);
        assert_eq!(snapshot.promotion_destination_claim_cycles, 7);
        assert_eq!(snapshot.promotion_destination_claim_instructions, 4);
        assert_eq!(snapshot.promotion_copy_cycles, 20);
        assert_eq!(snapshot.promotion_copy_instructions, 10);
        assert_eq!(snapshot.promotion_publication_cycles, 10);
        assert_eq!(snapshot.promotion_publication_instructions, 5);
        assert_eq!(snapshot.promotion_source_release_cycles, 20);
        assert_eq!(snapshot.promotion_source_release_instructions, 10);
        assert_eq!(snapshot.promotion_radio_claim_cycles, 10);
        assert_eq!(snapshot.promotion_radio_claim_instructions, 5);
        assert_eq!(snapshot.promotion_unattributed_cycles(), 3);
        assert_eq!(snapshot.promotion_unattributed_instructions(), 1);
    }

    #[test]
    fn queue_geometry_counts_runs_returns_and_publication_buckets() {
        let counters = TxPerformanceCounters::new();
        counters.record_egress_run(0);
        counters.record_egress_run(31);
        counters.record_egress_run(32);
        counters.record_egress_run(7);
        counters.publish_shadow_grant(TxShadowGrantObservation::Matched, 1, 1);
        counters.publish_shadow_grant(TxShadowGrantObservation::NoWindow, 2, 1);
        counters.publish_shadow_grant(TxShadowGrantObservation::KeyMismatch, 3, 1);
        counters.publish_shadow_grant(TxShadowGrantObservation::CreditExhausted, 4, 1);
        counters.publish_shadow_grant(TxShadowGrantObservation::Unclassified, 5, 1);
        counters.record_radio_return(true);
        counters.record_publication_geometry(0, 31);
        counters.record_publication_geometry(1, 32);
        counters.record_publication_geometry(2, 33);

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.egress_runs, 3);
        assert_eq!(snapshot.egress_run_31, 1);
        assert_eq!(snapshot.egress_run_32, 1);
        assert_eq!(snapshot.egress_run_other, 1);
        assert_eq!(snapshot.shadow_grant_checks, 5);
        assert_eq!(snapshot.shadow_grant_matches, 1);
        assert_eq!(snapshot.shadow_grant_no_window, 1);
        assert_eq!(snapshot.shadow_grant_key_mismatch, 1);
        assert_eq!(snapshot.shadow_grant_credit_exhausted, 1);
        assert_eq!(snapshot.shadow_grant_unclassified, 1);
        assert_eq!(snapshot.radio_returns, 1);
        assert_eq!(snapshot.radio_return_wakes, 1);
        assert_eq!(snapshot.publication_free_zero, 1);
        assert_eq!(snapshot.publication_free_one, 1);
        assert_eq!(snapshot.publication_free_two_plus, 1);
        assert_eq!(snapshot.publication_ready_le31, 1);
        assert_eq!(snapshot.publication_ready_32, 1);
        assert_eq!(snapshot.publication_ready_ge33, 1);
    }
}
