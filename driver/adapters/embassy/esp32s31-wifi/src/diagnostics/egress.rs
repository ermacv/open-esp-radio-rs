//! Low-overhead counters for the non-authoritative physical egress policy.
//!
//! The counters deliberately contain no queue identity or ownership. They
//! only make the shadow policy's progress and agreement with the unchanged
//! production admission path visible to same-ELF HIL experiments.

use core::sync::atomic::{AtomicU32, Ordering};

pub const EGRESS_POLICY_VIF_COUNT: usize = 2;

/// Per-interface shadow accounting at the physical TX transaction boundary.
///
/// Airtime is the conservative HT data-PPDU estimate used by the scheduler,
/// expressed in 100-nanosecond units. It is not measured medium occupancy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EgressPolicyVifShadowSnapshot {
    pub selected_transactions: u32,
    pub selected_frames: u32,
    pub selected_modeled_airtime_100ns: u32,
    pub actual_transactions: u32,
    pub actual_frames: u32,
    pub actual_modeled_airtime_100ns: u32,
    pub exact_recommendations: u32,
    pub different_selected: u32,
    pub different_actual: u32,
    pub cancelled_selected: u32,
    pub unavailable_selected: u32,
}

impl EgressPolicyVifShadowSnapshot {
    pub const fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            selected_transactions: self
                .selected_transactions
                .wrapping_sub(earlier.selected_transactions),
            selected_frames: self.selected_frames.wrapping_sub(earlier.selected_frames),
            selected_modeled_airtime_100ns: self
                .selected_modeled_airtime_100ns
                .wrapping_sub(earlier.selected_modeled_airtime_100ns),
            actual_transactions: self
                .actual_transactions
                .wrapping_sub(earlier.actual_transactions),
            actual_frames: self.actual_frames.wrapping_sub(earlier.actual_frames),
            actual_modeled_airtime_100ns: self
                .actual_modeled_airtime_100ns
                .wrapping_sub(earlier.actual_modeled_airtime_100ns),
            exact_recommendations: self
                .exact_recommendations
                .wrapping_sub(earlier.exact_recommendations),
            different_selected: self
                .different_selected
                .wrapping_sub(earlier.different_selected),
            different_actual: self.different_actual.wrapping_sub(earlier.different_actual),
            cancelled_selected: self
                .cancelled_selected
                .wrapping_sub(earlier.cancelled_selected),
            unavailable_selected: self
                .unavailable_selected
                .wrapping_sub(earlier.unavailable_selected),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EgressPolicyShadowSnapshot {
    pub recommendations: u32,
    pub exact_recommendations: u32,
    pub different_recommendations: u32,
    pub cancelled_recommendations: u32,
    pub unavailable_actual: u32,
    pub unavailable_no_recommendation: u32,
    pub unavailable_missing_key: u32,
    pub unavailable_demand: u32,
    pub unavailable_opportunity: u32,
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
    pub vifs: [EgressPolicyVifShadowSnapshot; EGRESS_POLICY_VIF_COUNT],
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
            cancelled_recommendations: self
                .cancelled_recommendations
                .wrapping_sub(earlier.cancelled_recommendations),
            unavailable_actual: self
                .unavailable_actual
                .wrapping_sub(earlier.unavailable_actual),
            unavailable_no_recommendation: self
                .unavailable_no_recommendation
                .wrapping_sub(earlier.unavailable_no_recommendation),
            unavailable_missing_key: self
                .unavailable_missing_key
                .wrapping_sub(earlier.unavailable_missing_key),
            unavailable_demand: self
                .unavailable_demand
                .wrapping_sub(earlier.unavailable_demand),
            unavailable_opportunity: self
                .unavailable_opportunity
                .wrapping_sub(earlier.unavailable_opportunity),
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
            vifs: [
                self.vifs[0].wrapping_delta_since(earlier.vifs[0]),
                self.vifs[1].wrapping_delta_since(earlier.vifs[1]),
            ],
        }
    }
}

struct EgressPolicyVifShadowCounters {
    selected_transactions: AtomicU32,
    selected_frames: AtomicU32,
    selected_modeled_airtime_100ns: AtomicU32,
    actual_transactions: AtomicU32,
    actual_frames: AtomicU32,
    actual_modeled_airtime_100ns: AtomicU32,
    exact_recommendations: AtomicU32,
    different_selected: AtomicU32,
    different_actual: AtomicU32,
    cancelled_selected: AtomicU32,
    unavailable_selected: AtomicU32,
}

impl EgressPolicyVifShadowCounters {
    const fn new() -> Self {
        Self {
            selected_transactions: AtomicU32::new(0),
            selected_frames: AtomicU32::new(0),
            selected_modeled_airtime_100ns: AtomicU32::new(0),
            actual_transactions: AtomicU32::new(0),
            actual_frames: AtomicU32::new(0),
            actual_modeled_airtime_100ns: AtomicU32::new(0),
            exact_recommendations: AtomicU32::new(0),
            different_selected: AtomicU32::new(0),
            different_actual: AtomicU32::new(0),
            cancelled_selected: AtomicU32::new(0),
            unavailable_selected: AtomicU32::new(0),
        }
    }

    fn selected(&self, frames: u8, modeled_airtime_100ns: u32) {
        self.selected_transactions.fetch_add(1, Ordering::Relaxed);
        self.selected_frames
            .fetch_add(u32::from(frames), Ordering::Relaxed);
        self.selected_modeled_airtime_100ns
            .fetch_add(modeled_airtime_100ns, Ordering::Relaxed);
    }

    fn actual(&self, frames: u8, modeled_airtime_100ns: u32) {
        self.actual_transactions.fetch_add(1, Ordering::Relaxed);
        self.actual_frames
            .fetch_add(u32::from(frames), Ordering::Relaxed);
        self.actual_modeled_airtime_100ns
            .fetch_add(modeled_airtime_100ns, Ordering::Relaxed);
    }

    fn snapshot(&self) -> EgressPolicyVifShadowSnapshot {
        EgressPolicyVifShadowSnapshot {
            selected_transactions: self.selected_transactions.load(Ordering::Relaxed),
            selected_frames: self.selected_frames.load(Ordering::Relaxed),
            selected_modeled_airtime_100ns: self
                .selected_modeled_airtime_100ns
                .load(Ordering::Relaxed),
            actual_transactions: self.actual_transactions.load(Ordering::Relaxed),
            actual_frames: self.actual_frames.load(Ordering::Relaxed),
            actual_modeled_airtime_100ns: self.actual_modeled_airtime_100ns.load(Ordering::Relaxed),
            exact_recommendations: self.exact_recommendations.load(Ordering::Relaxed),
            different_selected: self.different_selected.load(Ordering::Relaxed),
            different_actual: self.different_actual.load(Ordering::Relaxed),
            cancelled_selected: self.cancelled_selected.load(Ordering::Relaxed),
            unavailable_selected: self.unavailable_selected.load(Ordering::Relaxed),
        }
    }
}

pub(crate) struct EgressPolicyShadowCounters {
    recommendations: AtomicU32,
    exact_recommendations: AtomicU32,
    different_recommendations: AtomicU32,
    cancelled_recommendations: AtomicU32,
    unavailable_actual: AtomicU32,
    unavailable_no_recommendation: AtomicU32,
    unavailable_missing_key: AtomicU32,
    unavailable_demand: AtomicU32,
    unavailable_opportunity: AtomicU32,
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
    vifs: [EgressPolicyVifShadowCounters; EGRESS_POLICY_VIF_COUNT],
}

impl EgressPolicyShadowCounters {
    const fn new() -> Self {
        Self {
            recommendations: AtomicU32::new(0),
            exact_recommendations: AtomicU32::new(0),
            different_recommendations: AtomicU32::new(0),
            cancelled_recommendations: AtomicU32::new(0),
            unavailable_actual: AtomicU32::new(0),
            unavailable_no_recommendation: AtomicU32::new(0),
            unavailable_missing_key: AtomicU32::new(0),
            unavailable_demand: AtomicU32::new(0),
            unavailable_opportunity: AtomicU32::new(0),
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
            vifs: [
                EgressPolicyVifShadowCounters::new(),
                EgressPolicyVifShadowCounters::new(),
            ],
        }
    }

    fn vif(&self, vif: u8) -> Option<&EgressPolicyVifShadowCounters> {
        self.vifs.get(usize::from(vif))
    }

    pub(crate) fn recommendation(&self, vif: u8, frames: u8, modeled_airtime_100ns: u32) {
        self.recommendations.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.vif(vif) {
            counters.selected(frames, modeled_airtime_100ns);
        }
    }

    pub(crate) fn exact_recommendation(&self, vif: u8, frames: u8, modeled_airtime_100ns: u32) {
        self.exact_recommendations.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.vif(vif) {
            counters
                .exact_recommendations
                .fetch_add(1, Ordering::Relaxed);
            counters.actual(frames, modeled_airtime_100ns);
        }
    }

    pub(crate) fn different_recommendation(
        &self,
        selected_vif: u8,
        actual_vif: u8,
        actual_frames: u8,
        actual_modeled_airtime_100ns: u32,
    ) {
        self.different_recommendations
            .fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.vif(selected_vif) {
            counters.different_selected.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(counters) = self.vif(actual_vif) {
            counters.different_actual.fetch_add(1, Ordering::Relaxed);
            counters.actual(actual_frames, actual_modeled_airtime_100ns);
        }
    }

    pub(crate) fn cancelled_recommendation(&self, selected_vif: u8) {
        self.cancelled_recommendations
            .fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.vif(selected_vif) {
            counters.cancelled_selected.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn unavailable_actual(
        &self,
        selected_vif: Option<u8>,
        reason: EgressPolicyUnavailableActual,
    ) {
        self.unavailable_actual.fetch_add(1, Ordering::Relaxed);
        match reason {
            EgressPolicyUnavailableActual::NoRecommendation => &self.unavailable_no_recommendation,
            EgressPolicyUnavailableActual::MissingKey => &self.unavailable_missing_key,
            EgressPolicyUnavailableActual::Demand => &self.unavailable_demand,
            EgressPolicyUnavailableActual::Opportunity => &self.unavailable_opportunity,
        }
        .fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = selected_vif.and_then(|vif| self.vif(vif)) {
            counters
                .unavailable_selected
                .fetch_add(1, Ordering::Relaxed);
        }
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
            cancelled_recommendations: self.cancelled_recommendations.load(Ordering::Relaxed),
            unavailable_actual: self.unavailable_actual.load(Ordering::Relaxed),
            unavailable_no_recommendation: self
                .unavailable_no_recommendation
                .load(Ordering::Relaxed),
            unavailable_missing_key: self.unavailable_missing_key.load(Ordering::Relaxed),
            unavailable_demand: self.unavailable_demand.load(Ordering::Relaxed),
            unavailable_opportunity: self.unavailable_opportunity.load(Ordering::Relaxed),
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
            vifs: [self.vifs[0].snapshot(), self.vifs[1].snapshot()],
        }
    }
}

pub(crate) enum EgressPolicyUnavailableActual {
    NoRecommendation,
    MissingKey,
    Demand,
    Opportunity,
}

pub(crate) static EGRESS_POLICY_SHADOW_COUNTERS: EgressPolicyShadowCounters =
    EgressPolicyShadowCounters::new();

pub fn egress_policy_shadow_snapshot() -> EgressPolicyShadowSnapshot {
    EGRESS_POLICY_SHADOW_COUNTERS.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_vif_snapshot_delta_preserves_selection_and_actual_identity() {
        let earlier = EgressPolicyShadowSnapshot {
            vifs: [
                EgressPolicyVifShadowSnapshot {
                    selected_transactions: u32::MAX,
                    selected_frames: 30,
                    selected_modeled_airtime_100ns: 1_000,
                    ..EgressPolicyVifShadowSnapshot::default()
                },
                EgressPolicyVifShadowSnapshot {
                    actual_transactions: 7,
                    actual_frames: 200,
                    actual_modeled_airtime_100ns: 50_000,
                    ..EgressPolicyVifShadowSnapshot::default()
                },
            ],
            ..EgressPolicyShadowSnapshot::default()
        };
        let current = EgressPolicyShadowSnapshot {
            vifs: [
                EgressPolicyVifShadowSnapshot {
                    selected_transactions: 2,
                    selected_frames: 62,
                    selected_modeled_airtime_100ns: 3_000,
                    ..EgressPolicyVifShadowSnapshot::default()
                },
                EgressPolicyVifShadowSnapshot {
                    actual_transactions: 9,
                    actual_frames: 264,
                    actual_modeled_airtime_100ns: 70_000,
                    ..EgressPolicyVifShadowSnapshot::default()
                },
            ],
            ..EgressPolicyShadowSnapshot::default()
        };

        let delta = current.wrapping_delta_since(earlier);
        assert_eq!(delta.vifs[0].selected_transactions, 3);
        assert_eq!(delta.vifs[0].selected_frames, 32);
        assert_eq!(delta.vifs[0].selected_modeled_airtime_100ns, 2_000);
        assert_eq!(delta.vifs[1].actual_transactions, 2);
        assert_eq!(delta.vifs[1].actual_frames, 64);
        assert_eq!(delta.vifs[1].actual_modeled_airtime_100ns, 20_000);
    }
}
