//! Optional lock-free observations of the staged connected RX pipeline.
//!
//! These counters never participate in ownership or scheduling. Production
//! users that do not attach them pay only one predictable `Option` branch at
//! each instrumented phase; HIL attaches one shared instance in internal SRAM
//! and prints interval deltas only after the traffic sample has ended.

use core::sync::atomic::{AtomicU32, Ordering};

/// Diagnostic observations spanning DMA staging, protocol dispatch and the
/// final `embassy-net` publication copy.
pub struct RxPipelineCounters {
    now_micros: fn() -> u64,
    service_calls: AtomicU32,
    completion_frontier_frames: AtomicU32,
    admitted_frames: AtomicU32,
    staged_bytes: AtomicU32,
    backpressured_services: AtomicU32,
    pool_credit_limited_services: AtomicU32,
    queue_credit_limited_services: AtomicU32,
    maximum_frontier: AtomicU32,
    maximum_admitted: AtomicU32,
    service_micros: AtomicU32,
    service_lifetime_max_micros: AtomicU32,
    protocol_frames: AtomicU32,
    protocol_data_frames: AtomicU32,
    network_ready_waits: AtomicU32,
    network_ready_wait_micros: AtomicU32,
    network_ready_wait_lifetime_max_micros: AtomicU32,
    dispatch_micros: AtomicU32,
    dispatch_lifetime_max_micros: AtomicU32,
    network_publications: AtomicU32,
    network_published_bytes: AtomicU32,
    network_publish_micros: AtomicU32,
    network_publish_lifetime_max_micros: AtomicU32,
}

impl RxPipelineCounters {
    pub const fn new(now_micros: fn() -> u64) -> Self {
        Self {
            now_micros,
            service_calls: AtomicU32::new(0),
            completion_frontier_frames: AtomicU32::new(0),
            admitted_frames: AtomicU32::new(0),
            staged_bytes: AtomicU32::new(0),
            backpressured_services: AtomicU32::new(0),
            pool_credit_limited_services: AtomicU32::new(0),
            queue_credit_limited_services: AtomicU32::new(0),
            maximum_frontier: AtomicU32::new(0),
            maximum_admitted: AtomicU32::new(0),
            service_micros: AtomicU32::new(0),
            service_lifetime_max_micros: AtomicU32::new(0),
            protocol_frames: AtomicU32::new(0),
            protocol_data_frames: AtomicU32::new(0),
            network_ready_waits: AtomicU32::new(0),
            network_ready_wait_micros: AtomicU32::new(0),
            network_ready_wait_lifetime_max_micros: AtomicU32::new(0),
            dispatch_micros: AtomicU32::new(0),
            dispatch_lifetime_max_micros: AtomicU32::new(0),
            network_publications: AtomicU32::new(0),
            network_published_bytes: AtomicU32::new(0),
            network_publish_micros: AtomicU32::new(0),
            network_publish_lifetime_max_micros: AtomicU32::new(0),
        }
    }

    pub(crate) fn now_micros(&self) -> u64 {
        (self.now_micros)()
    }

    pub(crate) fn elapsed_micros_since(&self, started: u64) -> u64 {
        self.now_micros().wrapping_sub(started)
    }

    pub fn snapshot(&self) -> RxPipelineCounterSnapshot {
        RxPipelineCounterSnapshot {
            service_calls: self.service_calls.load(Ordering::Relaxed),
            completion_frontier_frames: self.completion_frontier_frames.load(Ordering::Relaxed),
            admitted_frames: self.admitted_frames.load(Ordering::Relaxed),
            staged_bytes: self.staged_bytes.load(Ordering::Relaxed),
            backpressured_services: self.backpressured_services.load(Ordering::Relaxed),
            pool_credit_limited_services: self.pool_credit_limited_services.load(Ordering::Relaxed),
            queue_credit_limited_services: self
                .queue_credit_limited_services
                .load(Ordering::Relaxed),
            maximum_frontier: self.maximum_frontier.load(Ordering::Relaxed),
            maximum_admitted: self.maximum_admitted.load(Ordering::Relaxed),
            service_micros: self.service_micros.load(Ordering::Relaxed),
            service_lifetime_max_micros: self.service_lifetime_max_micros.load(Ordering::Relaxed),
            protocol_frames: self.protocol_frames.load(Ordering::Relaxed),
            protocol_data_frames: self.protocol_data_frames.load(Ordering::Relaxed),
            network_ready_waits: self.network_ready_waits.load(Ordering::Relaxed),
            network_ready_wait_micros: self.network_ready_wait_micros.load(Ordering::Relaxed),
            network_ready_wait_lifetime_max_micros: self
                .network_ready_wait_lifetime_max_micros
                .load(Ordering::Relaxed),
            dispatch_micros: self.dispatch_micros.load(Ordering::Relaxed),
            dispatch_lifetime_max_micros: self.dispatch_lifetime_max_micros.load(Ordering::Relaxed),
            network_publications: self.network_publications.load(Ordering::Relaxed),
            network_published_bytes: self.network_published_bytes.load(Ordering::Relaxed),
            network_publish_micros: self.network_publish_micros.load(Ordering::Relaxed),
            network_publish_lifetime_max_micros: self
                .network_publish_lifetime_max_micros
                .load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_service(
        &self,
        frontier: usize,
        pool_credits: usize,
        queue_credits: usize,
        admitted: usize,
        staged_bytes: usize,
        micros: u64,
    ) {
        self.service_calls.fetch_add(1, Ordering::Relaxed);
        Self::add_usize(&self.completion_frontier_frames, frontier);
        Self::add_usize(&self.admitted_frames, admitted);
        Self::add_usize(&self.staged_bytes, staged_bytes);
        self.maximum_frontier.fetch_max(
            u32::try_from(frontier).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        self.maximum_admitted.fetch_max(
            u32::try_from(admitted).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        if admitted < frontier {
            self.backpressured_services.fetch_add(1, Ordering::Relaxed);
            if pool_credits <= queue_credits {
                self.pool_credit_limited_services
                    .fetch_add(1, Ordering::Relaxed);
            }
            if queue_credits <= pool_credits {
                self.queue_credit_limited_services
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        Self::record_time(
            &self.service_micros,
            &self.service_lifetime_max_micros,
            micros,
        );
    }

    pub(crate) fn record_network_ready_wait(&self, micros: u64) {
        self.network_ready_waits.fetch_add(1, Ordering::Relaxed);
        Self::record_time(
            &self.network_ready_wait_micros,
            &self.network_ready_wait_lifetime_max_micros,
            micros,
        );
    }

    pub(crate) fn record_dispatch(&self, data: bool, micros: u64) {
        self.protocol_frames.fetch_add(1, Ordering::Relaxed);
        if data {
            self.protocol_data_frames.fetch_add(1, Ordering::Relaxed);
        }
        Self::record_time(
            &self.dispatch_micros,
            &self.dispatch_lifetime_max_micros,
            micros,
        );
    }

    pub(crate) fn record_network_publish(&self, bytes: usize, micros: u64) {
        self.network_publications.fetch_add(1, Ordering::Relaxed);
        Self::add_usize(&self.network_published_bytes, bytes);
        Self::record_time(
            &self.network_publish_micros,
            &self.network_publish_lifetime_max_micros,
            micros,
        );
    }

    fn add_usize(counter: &AtomicU32, value: usize) {
        counter.fetch_add(u32::try_from(value).unwrap_or(u32::MAX), Ordering::Relaxed);
    }

    fn record_time(total: &AtomicU32, maximum: &AtomicU32, micros: u64) {
        let micros = u32::try_from(micros).unwrap_or(u32::MAX);
        total.fetch_add(micros, Ordering::Relaxed);
        maximum.fetch_max(micros, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxPipelineCounterSnapshot {
    pub service_calls: u32,
    pub completion_frontier_frames: u32,
    pub admitted_frames: u32,
    pub staged_bytes: u32,
    pub backpressured_services: u32,
    pub pool_credit_limited_services: u32,
    pub queue_credit_limited_services: u32,
    /// Maximum observed since boot, not an interval delta.
    pub maximum_frontier: u32,
    /// Maximum observed since boot, not an interval delta.
    pub maximum_admitted: u32,
    pub service_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub service_lifetime_max_micros: u32,
    pub protocol_frames: u32,
    pub protocol_data_frames: u32,
    pub network_ready_waits: u32,
    pub network_ready_wait_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub network_ready_wait_lifetime_max_micros: u32,
    pub dispatch_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub dispatch_lifetime_max_micros: u32,
    pub network_publications: u32,
    pub network_published_bytes: u32,
    pub network_publish_micros: u32,
    /// Maximum observed since boot, not an interval delta.
    pub network_publish_lifetime_max_micros: u32,
}

impl RxPipelineCounterSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            service_calls: self.service_calls.wrapping_sub(earlier.service_calls),
            completion_frontier_frames: self
                .completion_frontier_frames
                .wrapping_sub(earlier.completion_frontier_frames),
            admitted_frames: self.admitted_frames.wrapping_sub(earlier.admitted_frames),
            staged_bytes: self.staged_bytes.wrapping_sub(earlier.staged_bytes),
            backpressured_services: self
                .backpressured_services
                .wrapping_sub(earlier.backpressured_services),
            pool_credit_limited_services: self
                .pool_credit_limited_services
                .wrapping_sub(earlier.pool_credit_limited_services),
            queue_credit_limited_services: self
                .queue_credit_limited_services
                .wrapping_sub(earlier.queue_credit_limited_services),
            maximum_frontier: self.maximum_frontier,
            maximum_admitted: self.maximum_admitted,
            service_micros: self.service_micros.wrapping_sub(earlier.service_micros),
            service_lifetime_max_micros: self.service_lifetime_max_micros,
            protocol_frames: self.protocol_frames.wrapping_sub(earlier.protocol_frames),
            protocol_data_frames: self
                .protocol_data_frames
                .wrapping_sub(earlier.protocol_data_frames),
            network_ready_waits: self
                .network_ready_waits
                .wrapping_sub(earlier.network_ready_waits),
            network_ready_wait_micros: self
                .network_ready_wait_micros
                .wrapping_sub(earlier.network_ready_wait_micros),
            network_ready_wait_lifetime_max_micros: self.network_ready_wait_lifetime_max_micros,
            dispatch_micros: self.dispatch_micros.wrapping_sub(earlier.dispatch_micros),
            dispatch_lifetime_max_micros: self.dispatch_lifetime_max_micros,
            network_publications: self
                .network_publications
                .wrapping_sub(earlier.network_publications),
            network_published_bytes: self
                .network_published_bytes
                .wrapping_sub(earlier.network_published_bytes),
            network_publish_micros: self
                .network_publish_micros
                .wrapping_sub(earlier.network_publish_micros),
            network_publish_lifetime_max_micros: self.network_publish_lifetime_max_micros,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RxPipelineCounters;

    fn test_clock() -> u64 {
        0
    }

    #[test]
    fn interval_delta_retains_totals_limits_and_phase_times() {
        let counters = RxPipelineCounters::new(test_clock);
        let before = counters.snapshot();
        counters.record_service(4, 3, 5, 3, 4_500, 70);
        counters.record_network_ready_wait(2);
        counters.record_network_publish(1_514, 13);
        counters.record_dispatch(true, 31);
        let delta = counters.snapshot().wrapping_delta_since(before);

        assert_eq!(delta.service_calls, 1);
        assert_eq!(delta.completion_frontier_frames, 4);
        assert_eq!(delta.admitted_frames, 3);
        assert_eq!(delta.staged_bytes, 4_500);
        assert_eq!(delta.backpressured_services, 1);
        assert_eq!(delta.pool_credit_limited_services, 1);
        assert_eq!(delta.queue_credit_limited_services, 0);
        assert_eq!(delta.maximum_frontier, 4);
        assert_eq!(delta.maximum_admitted, 3);
        assert_eq!(delta.service_micros, 70);
        assert_eq!(delta.network_ready_wait_micros, 2);
        assert_eq!(delta.network_published_bytes, 1_514);
        assert_eq!(delta.network_publish_micros, 13);
        assert_eq!(delta.protocol_data_frames, 1);
        assert_eq!(delta.dispatch_micros, 31);
    }
}
