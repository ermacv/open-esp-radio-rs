//! Diagnostic-only ESP32-S31 L1 cache performance counters.
//!
//! Bus identifiers are reported exactly as the CACHE peripheral names them.
//! This module deliberately does not relabel DBUS0/1 as CPU identities and
//! keeps MISS and access-CONFLICT as separate hardware events.

use esp_hal::peripherals::CACHE;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::product_hil) struct L1CacheBusSnapshot {
    pub hit: u32,
    pub miss: u32,
    pub conflict: u32,
    pub next_level_read: u32,
    pub next_level_write: u32,
}

impl L1CacheBusSnapshot {
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            hit: self.hit.wrapping_sub(earlier.hit),
            miss: self.miss.wrapping_sub(earlier.miss),
            conflict: self.conflict.wrapping_sub(earlier.conflict),
            next_level_read: self.next_level_read.wrapping_sub(earlier.next_level_read),
            next_level_write: self.next_level_write.wrapping_sub(earlier.next_level_write),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::product_hil) struct L1CacheCounterEnable {
    pub ibus0: bool,
    pub ibus1: bool,
    pub dbus0: bool,
    pub dbus1: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::product_hil) struct L1CachePerformanceSnapshot {
    pub trace_enabled: bool,
    pub counter_enable: L1CacheCounterEnable,
    pub ibus0: L1CacheBusSnapshot,
    pub ibus1: L1CacheBusSnapshot,
    pub dbus0: L1CacheBusSnapshot,
    pub dbus1: L1CacheBusSnapshot,
}

impl L1CachePerformanceSnapshot {
    pub(in crate::product_hil) fn read() -> Self {
        let cache = CACHE::regs();
        let counter_control = cache.l1_cache_acs_cnt_ctrl().read();
        Self {
            trace_enabled: cache.trace_ena().read().l1_cache_trace_ena().bit_is_set(),
            counter_enable: L1CacheCounterEnable {
                ibus0: counter_control.l1_ibus0_cnt_ena().bit_is_set(),
                ibus1: counter_control.l1_ibus1_cnt_ena().bit_is_set(),
                dbus0: counter_control.l1_dbus0_cnt_ena().bit_is_set(),
                dbus1: counter_control.l1_dbus1_cnt_ena().bit_is_set(),
            },
            ibus0: L1CacheBusSnapshot {
                hit: cache
                    .l1_ibus0_acs_hit_cnt()
                    .read()
                    .l1_ibus0_hit_cnt()
                    .bits(),
                miss: cache
                    .l1_ibus0_acs_miss_cnt()
                    .read()
                    .l1_ibus0_miss_cnt()
                    .bits(),
                conflict: cache
                    .l1_ibus0_acs_conflict_cnt()
                    .read()
                    .l1_ibus0_conflict_cnt()
                    .bits(),
                next_level_read: cache
                    .l1_ibus0_acs_nxtlvl_rd_cnt()
                    .read()
                    .l1_ibus0_nxtlvl_rd_cnt()
                    .bits(),
                next_level_write: 0,
            },
            ibus1: L1CacheBusSnapshot {
                hit: cache
                    .l1_ibus1_acs_hit_cnt()
                    .read()
                    .l1_ibus1_hit_cnt()
                    .bits(),
                miss: cache
                    .l1_ibus1_acs_miss_cnt()
                    .read()
                    .l1_ibus1_miss_cnt()
                    .bits(),
                conflict: cache
                    .l1_ibus1_acs_conflict_cnt()
                    .read()
                    .l1_ibus1_conflict_cnt()
                    .bits(),
                next_level_read: cache
                    .l1_ibus1_acs_nxtlvl_rd_cnt()
                    .read()
                    .l1_ibus1_nxtlvl_rd_cnt()
                    .bits(),
                next_level_write: 0,
            },
            dbus0: L1CacheBusSnapshot {
                hit: cache
                    .l1_dbus0_acs_hit_cnt()
                    .read()
                    .l1_dbus0_hit_cnt()
                    .bits(),
                miss: cache
                    .l1_dbus0_acs_miss_cnt()
                    .read()
                    .l1_dbus0_miss_cnt()
                    .bits(),
                conflict: cache
                    .l1_dbus0_acs_conflict_cnt()
                    .read()
                    .l1_dbus0_conflict_cnt()
                    .bits(),
                next_level_read: cache
                    .l1_dbus0_acs_nxtlvl_rd_cnt()
                    .read()
                    .l1_dbus0_nxtlvl_rd_cnt()
                    .bits(),
                next_level_write: cache
                    .l1_dbus0_acs_nxtlvl_wr_cnt()
                    .read()
                    .l1_dbus0_nxtlvl_wr_cnt()
                    .bits(),
            },
            dbus1: L1CacheBusSnapshot {
                hit: cache
                    .l1_dbus1_acs_hit_cnt()
                    .read()
                    .l1_dbus1_hit_cnt()
                    .bits(),
                miss: cache
                    .l1_dbus1_acs_miss_cnt()
                    .read()
                    .l1_dbus1_miss_cnt()
                    .bits(),
                conflict: cache
                    .l1_dbus1_acs_conflict_cnt()
                    .read()
                    .l1_dbus1_conflict_cnt()
                    .bits(),
                next_level_read: cache
                    .l1_dbus1_acs_nxtlvl_rd_cnt()
                    .read()
                    .l1_dbus1_nxtlvl_rd_cnt()
                    .bits(),
                next_level_write: cache
                    .l1_dbus1_acs_nxtlvl_wr_cnt()
                    .read()
                    .l1_dbus1_nxtlvl_wr_cnt()
                    .bits(),
            },
        }
    }

    pub(in crate::product_hil) fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            trace_enabled: self.trace_enabled,
            counter_enable: self.counter_enable,
            ibus0: self.ibus0.wrapping_delta_since(earlier.ibus0),
            ibus1: self.ibus1.wrapping_delta_since(earlier.ibus1),
            dbus0: self.dbus0.wrapping_delta_since(earlier.dbus0),
            dbus1: self.dbus1.wrapping_delta_since(earlier.dbus1),
        }
    }
}

pub(in crate::product_hil) fn enable_l1_cache_counters() {
    let cache = CACHE::regs();
    cache
        .trace_ena()
        .modify(|_, writer| writer.l1_cache_trace_ena().set_bit());
    cache.l1_cache_acs_cnt_ctrl().write(|writer| {
        writer
            .l1_ibus0_cnt_clr()
            .set_bit()
            .l1_ibus1_cnt_clr()
            .set_bit()
            .l1_dbus0_cnt_clr()
            .set_bit()
            .l1_dbus1_cnt_clr()
            .set_bit()
    });
    cache.l1_cache_acs_cnt_ctrl().write(|writer| {
        writer
            .l1_ibus0_cnt_ena()
            .set_bit()
            .l1_ibus1_cnt_ena()
            .set_bit()
            .l1_dbus0_cnt_ena()
            .set_bit()
            .l1_dbus1_cnt_ena()
            .set_bit()
    });
}

#[cfg(test)]
mod tests {
    use super::{L1CacheBusSnapshot, L1CacheCounterEnable, L1CachePerformanceSnapshot};

    #[test]
    fn interval_snapshot_keeps_status_and_wraps_events() {
        let earlier = L1CachePerformanceSnapshot {
            ibus0: L1CacheBusSnapshot {
                hit: u32::MAX,
                miss: 10,
                ..L1CacheBusSnapshot::default()
            },
            ..L1CachePerformanceSnapshot::default()
        };
        let current = L1CachePerformanceSnapshot {
            trace_enabled: true,
            counter_enable: L1CacheCounterEnable {
                ibus0: true,
                ibus1: true,
                dbus0: true,
                dbus1: true,
            },
            ibus0: L1CacheBusSnapshot {
                hit: 2,
                miss: 15,
                ..L1CacheBusSnapshot::default()
            },
            ..L1CachePerformanceSnapshot::default()
        };
        let delta = current.wrapping_delta_since(earlier);
        assert!(delta.trace_enabled);
        assert_eq!(delta.counter_enable, current.counter_enable);
        assert_eq!(delta.ibus0.hit, 3);
        assert_eq!(delta.ibus0.miss, 5);
    }
}
