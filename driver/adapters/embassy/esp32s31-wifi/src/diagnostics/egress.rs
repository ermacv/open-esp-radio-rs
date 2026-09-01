//! Low-overhead counters for the non-authoritative physical egress policy.
//!
//! The counters deliberately contain no queue identity or ownership. They
//! only make the shadow policy's progress and agreement with the unchanged
//! production admission path visible to same-ELF HIL experiments.

use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EgressPolicyShadowSnapshot {
    pub recommendations: u32,
    pub exact_recommendations: u32,
    pub different_recommendations: u32,
    pub unavailable_actual: u32,
    pub rejected_updates: u32,
    pub rejected_observations: u32,
    pub snapshot_queries: u32,
    pub snapshot_ready: u32,
    pub key_rejected: u32,
    pub identity_rejected: u32,
    pub traffic_class_rejected: u32,
    pub role_unavailable: u32,
    pub non_ht_rate: u32,
    pub no_block_ack: u32,
    pub invalid_geometry: u32,
}

impl EgressPolicyShadowSnapshot {
    pub const fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            recommendations: self.recommendations.wrapping_sub(earlier.recommendations),
            exact_recommendations: self
                .exact_recommendations
                .wrapping_sub(earlier.exact_recommendations),
            different_recommendations: self
                .different_recommendations
                .wrapping_sub(earlier.different_recommendations),
            unavailable_actual: self
                .unavailable_actual
                .wrapping_sub(earlier.unavailable_actual),
            rejected_updates: self.rejected_updates.wrapping_sub(earlier.rejected_updates),
            rejected_observations: self
                .rejected_observations
                .wrapping_sub(earlier.rejected_observations),
            snapshot_queries: self.snapshot_queries.wrapping_sub(earlier.snapshot_queries),
            snapshot_ready: self.snapshot_ready.wrapping_sub(earlier.snapshot_ready),
            key_rejected: self.key_rejected.wrapping_sub(earlier.key_rejected),
            identity_rejected: self
                .identity_rejected
                .wrapping_sub(earlier.identity_rejected),
            traffic_class_rejected: self
                .traffic_class_rejected
                .wrapping_sub(earlier.traffic_class_rejected),
            role_unavailable: self.role_unavailable.wrapping_sub(earlier.role_unavailable),
            non_ht_rate: self.non_ht_rate.wrapping_sub(earlier.non_ht_rate),
            no_block_ack: self.no_block_ack.wrapping_sub(earlier.no_block_ack),
            invalid_geometry: self.invalid_geometry.wrapping_sub(earlier.invalid_geometry),
        }
    }
}

pub(crate) struct EgressPolicyShadowCounters {
    recommendations: AtomicU32,
    exact_recommendations: AtomicU32,
    different_recommendations: AtomicU32,
    unavailable_actual: AtomicU32,
    rejected_updates: AtomicU32,
    rejected_observations: AtomicU32,
    snapshot_queries: AtomicU32,
    snapshot_ready: AtomicU32,
    key_rejected: AtomicU32,
    identity_rejected: AtomicU32,
    traffic_class_rejected: AtomicU32,
    role_unavailable: AtomicU32,
    non_ht_rate: AtomicU32,
    no_block_ack: AtomicU32,
    invalid_geometry: AtomicU32,
}

impl EgressPolicyShadowCounters {
    const fn new() -> Self {
        Self {
            recommendations: AtomicU32::new(0),
            exact_recommendations: AtomicU32::new(0),
            different_recommendations: AtomicU32::new(0),
            unavailable_actual: AtomicU32::new(0),
            rejected_updates: AtomicU32::new(0),
            rejected_observations: AtomicU32::new(0),
            snapshot_queries: AtomicU32::new(0),
            snapshot_ready: AtomicU32::new(0),
            key_rejected: AtomicU32::new(0),
            identity_rejected: AtomicU32::new(0),
            traffic_class_rejected: AtomicU32::new(0),
            role_unavailable: AtomicU32::new(0),
            non_ht_rate: AtomicU32::new(0),
            no_block_ack: AtomicU32::new(0),
            invalid_geometry: AtomicU32::new(0),
        }
    }

    pub(crate) fn recommendation(&self) {
        self.recommendations.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn exact_recommendation(&self) {
        self.exact_recommendations.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn different_recommendation(&self) {
        self.different_recommendations
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn unavailable_actual(&self) {
        self.unavailable_actual.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn rejected_update(&self) {
        self.rejected_updates.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn rejected_observation(&self) {
        self.rejected_observations.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot_query(&self, ready: bool) {
        self.snapshot_queries.fetch_add(1, Ordering::Relaxed);
        if ready {
            self.snapshot_ready.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn snapshot_rejected(
        &self,
        reason: crate::datapath::egress::DatapathEgressSnapshotRejection,
    ) {
        use crate::datapath::egress::DatapathEgressSnapshotRejection;

        let counter = match reason {
            DatapathEgressSnapshotRejection::Key => &self.key_rejected,
            DatapathEgressSnapshotRejection::Identity => &self.identity_rejected,
            DatapathEgressSnapshotRejection::TrafficClass => &self.traffic_class_rejected,
            DatapathEgressSnapshotRejection::RoleUnavailable => &self.role_unavailable,
            DatapathEgressSnapshotRejection::NonHtRate => &self.non_ht_rate,
            DatapathEgressSnapshotRejection::NoBlockAck => &self.no_block_ack,
            DatapathEgressSnapshotRejection::InvalidGeometry => &self.invalid_geometry,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> EgressPolicyShadowSnapshot {
        EgressPolicyShadowSnapshot {
            recommendations: self.recommendations.load(Ordering::Relaxed),
            exact_recommendations: self.exact_recommendations.load(Ordering::Relaxed),
            different_recommendations: self.different_recommendations.load(Ordering::Relaxed),
            unavailable_actual: self.unavailable_actual.load(Ordering::Relaxed),
            rejected_updates: self.rejected_updates.load(Ordering::Relaxed),
            rejected_observations: self.rejected_observations.load(Ordering::Relaxed),
            snapshot_queries: self.snapshot_queries.load(Ordering::Relaxed),
            snapshot_ready: self.snapshot_ready.load(Ordering::Relaxed),
            key_rejected: self.key_rejected.load(Ordering::Relaxed),
            identity_rejected: self.identity_rejected.load(Ordering::Relaxed),
            traffic_class_rejected: self.traffic_class_rejected.load(Ordering::Relaxed),
            role_unavailable: self.role_unavailable.load(Ordering::Relaxed),
            non_ht_rate: self.non_ht_rate.load(Ordering::Relaxed),
            no_block_ack: self.no_block_ack.load(Ordering::Relaxed),
            invalid_geometry: self.invalid_geometry.load(Ordering::Relaxed),
        }
    }
}

pub(crate) static EGRESS_POLICY_SHADOW_COUNTERS: EgressPolicyShadowCounters =
    EgressPolicyShadowCounters::new();

pub fn egress_policy_shadow_snapshot() -> EgressPolicyShadowSnapshot {
    EGRESS_POLICY_SHADOW_COUNTERS.snapshot()
}
