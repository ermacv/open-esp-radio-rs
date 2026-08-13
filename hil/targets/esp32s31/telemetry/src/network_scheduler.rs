//! Aggregate cooperative network scheduler measurements.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_net::{CooperativePollExit, CooperativePollReport};
use open_esp_radio_hil_protocol::NetworkSchedulerEvidence;

pub struct NetworkSchedulerCounters {
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
    pub const fn new() -> Self {
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

    #[inline]
    pub fn record(&self, report: CooperativePollReport) {
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
        self.egress_budget_exhausted
            .fetch_add(u32::from(report.egress_budget_exhausted), Ordering::Relaxed);
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

    pub fn snapshot(&self) -> NetworkSchedulerEvidence {
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

impl Default for NetworkSchedulerCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_direction_and_exit() {
        let counters = NetworkSchedulerCounters::new();
        counters.record(CooperativePollReport {
            ingress_calls: 5,
            ingress_packets: 4,
            egress_passes: 2,
            egress_tx_tokens: 3,
            egress_blocked: true,
            ingress_budget_exhausted: false,
            egress_budget_exhausted: false,
            started_with_ingress: true,
            exit: CooperativePollExit::EgressCredit,
        });
        let evidence = counters.snapshot();
        assert_eq!(evidence.polls, 1);
        assert_eq!(evidence.ingress_packets, 4);
        assert_eq!(evidence.egress_tx_tokens, 3);
        assert_eq!(evidence.egress_blocked, 1);
        assert_eq!(evidence.started_with_ingress, 1);
        assert_eq!(evidence.exit_egress_credit, 1);
    }
}
