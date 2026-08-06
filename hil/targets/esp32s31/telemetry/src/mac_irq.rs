//! HIL classification of one bounded ESP32-S31 MAC interrupt drain.

use core::sync::atomic::{AtomicU32, Ordering};

use open_esp_radio_esp32s31_wifi_mac::irq::{
    MAC_INT_COLLISION, MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK, MAC_INT_RX_SUCCESS,
    MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT,
};

const MAC_TX_IRQ_MASK: u32 = MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacIrqClassificationSnapshot {
    pub spurious_entries: u32,
    pub rx_only_entries: u32,
    pub rx_mixed_entries: u32,
    pub tx_only_entries: u32,
    pub tx_mixed_entries: u32,
    pub other_only_entries: u32,
    pub extra_nonzero_snapshots: u32,
    pub saturated_entries: u32,
}

impl MacIrqClassificationSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            spurious_entries: self.spurious_entries.wrapping_sub(earlier.spurious_entries),
            rx_only_entries: self.rx_only_entries.wrapping_sub(earlier.rx_only_entries),
            rx_mixed_entries: self.rx_mixed_entries.wrapping_sub(earlier.rx_mixed_entries),
            tx_only_entries: self.tx_only_entries.wrapping_sub(earlier.tx_only_entries),
            tx_mixed_entries: self.tx_mixed_entries.wrapping_sub(earlier.tx_mixed_entries),
            other_only_entries: self
                .other_only_entries
                .wrapping_sub(earlier.other_only_entries),
            extra_nonzero_snapshots: self
                .extra_nonzero_snapshots
                .wrapping_sub(earlier.extra_nonzero_snapshots),
            saturated_entries: self
                .saturated_entries
                .wrapping_sub(earlier.saturated_entries),
        }
    }
}

pub struct MacIrqClassificationCounters {
    spurious_entries: AtomicU32,
    rx_only_entries: AtomicU32,
    rx_mixed_entries: AtomicU32,
    tx_only_entries: AtomicU32,
    tx_mixed_entries: AtomicU32,
    other_only_entries: AtomicU32,
    extra_nonzero_snapshots: AtomicU32,
    saturated_entries: AtomicU32,
    auxiliary_status_or: AtomicU32,
    unknown_status_or: AtomicU32,
}

impl MacIrqClassificationCounters {
    pub const fn new() -> Self {
        Self {
            spurious_entries: AtomicU32::new(0),
            rx_only_entries: AtomicU32::new(0),
            rx_mixed_entries: AtomicU32::new(0),
            tx_only_entries: AtomicU32::new(0),
            tx_mixed_entries: AtomicU32::new(0),
            other_only_entries: AtomicU32::new(0),
            extra_nonzero_snapshots: AtomicU32::new(0),
            saturated_entries: AtomicU32::new(0),
            auxiliary_status_or: AtomicU32::new(0),
            unknown_status_or: AtomicU32::new(0),
        }
    }

    #[inline]
    pub fn record(&self, first_status: u32, observed_status: u32, nonzero_snapshots: u32) {
        let rx = first_status & MAC_INT_RX_SUCCESS != 0;
        let tx = first_status & MAC_TX_IRQ_MASK != 0;
        let auxiliary = first_status & MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK != 0;
        let unknown = first_status
            & !(MAC_INT_RX_SUCCESS | MAC_TX_IRQ_MASK | MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK)
            != 0;
        let counter = if first_status == 0 {
            &self.spurious_entries
        } else if rx && !tx && !unknown {
            &self.rx_only_entries
        } else if rx {
            &self.rx_mixed_entries
        } else if tx && !auxiliary && !unknown {
            &self.tx_only_entries
        } else if tx {
            &self.tx_mixed_entries
        } else {
            &self.other_only_entries
        };
        counter.fetch_add(1, Ordering::Relaxed);

        let extra = nonzero_snapshots.saturating_sub(1);
        if extra != 0 {
            self.extra_nonzero_snapshots
                .fetch_add(extra, Ordering::Relaxed);
        }
        if nonzero_snapshots == 32 {
            self.saturated_entries.fetch_add(1, Ordering::Relaxed);
        }
        let auxiliary_status = observed_status & MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK;
        if auxiliary_status != 0 {
            self.auxiliary_status_or
                .fetch_or(auxiliary_status, Ordering::Relaxed);
        }
        let unknown_status = observed_status
            & !(MAC_INT_RX_SUCCESS | MAC_TX_IRQ_MASK | MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK);
        if unknown_status != 0 {
            self.unknown_status_or
                .fetch_or(unknown_status, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> MacIrqClassificationSnapshot {
        MacIrqClassificationSnapshot {
            spurious_entries: self.spurious_entries.load(Ordering::Relaxed),
            rx_only_entries: self.rx_only_entries.load(Ordering::Relaxed),
            rx_mixed_entries: self.rx_mixed_entries.load(Ordering::Relaxed),
            tx_only_entries: self.tx_only_entries.load(Ordering::Relaxed),
            tx_mixed_entries: self.tx_mixed_entries.load(Ordering::Relaxed),
            other_only_entries: self.other_only_entries.load(Ordering::Relaxed),
            extra_nonzero_snapshots: self.extra_nonzero_snapshots.load(Ordering::Relaxed),
            saturated_entries: self.saturated_entries.load(Ordering::Relaxed),
        }
    }

    pub fn take_auxiliary_status_or(&self) -> u32 {
        self.auxiliary_status_or.swap(0, Ordering::Relaxed)
    }

    pub fn take_unknown_status_or(&self) -> u32 {
        self.unknown_status_or.swap(0, Ordering::Relaxed)
    }
}

impl Default for MacIrqClassificationCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_separates_rx_tx_and_extra_drain_work() {
        let counters = MacIrqClassificationCounters::new();
        counters.record(MAC_INT_RX_SUCCESS, MAC_INT_RX_SUCCESS, 1);
        counters.record(MAC_INT_TX_COMPLETE, MAC_INT_TX_COMPLETE, 2);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.rx_only_entries, 1);
        assert_eq!(snapshot.tx_only_entries, 1);
        assert_eq!(snapshot.extra_nonzero_snapshots, 1);
    }
}
