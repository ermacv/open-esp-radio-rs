//! Coarse Core1 network-scheduler accounting for TX diagnostics.
//!
//! The Embassy observer reports one completed bounded scheduler poll. These
//! counters deliberately record only work quantities and exit causes; cycle
//! and wall-time accounting remains owned by the existing task wrappers.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_net::{CooperativePollExit, CooperativePollReport};
use open_esp_radio_hil_protocol::NetworkSchedulerEvidence;

struct NetworkSchedulerCounters {
    polls: AtomicU32,
    ingress_calls: AtomicU32,
    ingress_packets: AtomicU32,
    egress_passes: AtomicU32,
    egress_tx_tokens: AtomicU32,
    egress_blocked: AtomicU32,
    ingress_budget_exhausted: AtomicU32,
    egress_budget_exhausted: AtomicU32,
    started_with_ingress: AtomicU32,
    started_with_egress: AtomicU32,
    exit_drained: AtomicU32,
    exit_work_budget: AtomicU32,
    exit_egress_credit: AtomicU32,
}

impl NetworkSchedulerCounters {
    const fn new() -> Self {
        Self {
            polls: AtomicU32::new(0),
            ingress_calls: AtomicU32::new(0),
            ingress_packets: AtomicU32::new(0),
            egress_passes: AtomicU32::new(0),
            egress_tx_tokens: AtomicU32::new(0),
            egress_blocked: AtomicU32::new(0),
            ingress_budget_exhausted: AtomicU32::new(0),
            egress_budget_exhausted: AtomicU32::new(0),
            started_with_ingress: AtomicU32::new(0),
            started_with_egress: AtomicU32::new(0),
            exit_drained: AtomicU32::new(0),
            exit_work_budget: AtomicU32::new(0),
            exit_egress_credit: AtomicU32::new(0),
        }
    }

    fn record(&self, report: CooperativePollReport) {
        self.polls.fetch_add(1, Ordering::Relaxed);
        self.ingress_calls
            .fetch_add(u32::from(report.ingress_calls), Ordering::Relaxed);
        self.ingress_packets
            .fetch_add(u32::from(report.ingress_packets), Ordering::Relaxed);
        self.egress_passes
            .fetch_add(u32::from(report.egress_passes), Ordering::Relaxed);
        self.egress_tx_tokens
            .fetch_add(report.egress_tx_tokens, Ordering::Relaxed);
        self.egress_blocked
            .fetch_add(u32::from(report.egress_blocked), Ordering::Relaxed);
        self.ingress_budget_exhausted.fetch_add(
            u32::from(report.ingress_budget_exhausted),
            Ordering::Relaxed,
        );
        self.egress_budget_exhausted.fetch_add(
            u32::from(report.egress_budget_exhausted),
            Ordering::Relaxed,
        );
        if report.started_with_ingress {
            self.started_with_ingress.fetch_add(1, Ordering::Relaxed);
        } else {
            self.started_with_egress.fetch_add(1, Ordering::Relaxed);
        }
        match report.exit {
            CooperativePollExit::Drained => &self.exit_drained,
            CooperativePollExit::WorkBudget => &self.exit_work_budget,
            CooperativePollExit::EgressCredit => &self.exit_egress_credit,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> NetworkSchedulerEvidence {
        NetworkSchedulerEvidence {
            polls: self.polls.load(Ordering::Relaxed),
            ingress_calls: self.ingress_calls.load(Ordering::Relaxed),
            ingress_packets: self.ingress_packets.load(Ordering::Relaxed),
            egress_passes: self.egress_passes.load(Ordering::Relaxed),
            egress_tx_tokens: self.egress_tx_tokens.load(Ordering::Relaxed),
            egress_blocked: self.egress_blocked.load(Ordering::Relaxed),
            ingress_budget_exhausted: self.ingress_budget_exhausted.load(Ordering::Relaxed),
            egress_budget_exhausted: self.egress_budget_exhausted.load(Ordering::Relaxed),
            started_with_ingress: self.started_with_ingress.load(Ordering::Relaxed),
            started_with_egress: self.started_with_egress.load(Ordering::Relaxed),
            exit_drained: self.exit_drained.load(Ordering::Relaxed),
            exit_work_budget: self.exit_work_budget.load(Ordering::Relaxed),
            exit_egress_credit: self.exit_egress_credit.load(Ordering::Relaxed),
        }
    }
}

static NETWORK_SCHEDULER: NetworkSchedulerCounters = NetworkSchedulerCounters::new();

pub(in crate::product_hil) fn observe(report: CooperativePollReport) {
    NETWORK_SCHEDULER.record(report);
}

pub(in crate::product_hil) fn snapshot() -> NetworkSchedulerEvidence {
    NETWORK_SCHEDULER.snapshot()
}

pub(in crate::product_hil) fn interval_since(
    earlier: NetworkSchedulerEvidence,
) -> NetworkSchedulerEvidence {
    let current = snapshot();
    NetworkSchedulerEvidence {
        polls: current.polls.wrapping_sub(earlier.polls),
        ingress_calls: current.ingress_calls.wrapping_sub(earlier.ingress_calls),
        ingress_packets: current
            .ingress_packets
            .wrapping_sub(earlier.ingress_packets),
        egress_passes: current.egress_passes.wrapping_sub(earlier.egress_passes),
        egress_tx_tokens: current
            .egress_tx_tokens
            .wrapping_sub(earlier.egress_tx_tokens),
        egress_blocked: current.egress_blocked.wrapping_sub(earlier.egress_blocked),
        ingress_budget_exhausted: current
            .ingress_budget_exhausted
            .wrapping_sub(earlier.ingress_budget_exhausted),
        egress_budget_exhausted: current
            .egress_budget_exhausted
            .wrapping_sub(earlier.egress_budget_exhausted),
        started_with_ingress: current
            .started_with_ingress
            .wrapping_sub(earlier.started_with_ingress),
        started_with_egress: current
            .started_with_egress
            .wrapping_sub(earlier.started_with_egress),
        exit_drained: current.exit_drained.wrapping_sub(earlier.exit_drained),
        exit_work_budget: current
            .exit_work_budget
            .wrapping_sub(earlier.exit_work_budget),
        exit_egress_credit: current
            .exit_egress_credit
            .wrapping_sub(earlier.exit_egress_credit),
    }
}
