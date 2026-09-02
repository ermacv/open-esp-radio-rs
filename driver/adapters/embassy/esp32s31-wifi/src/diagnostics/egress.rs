//! Low-overhead counters for the non-authoritative physical egress policy.
//!
//! The counters deliberately contain no queue identity or ownership. They
//! only make the shadow grant lifecycle visible to same-ELF HIL experiments.

use core::sync::atomic::{AtomicU32, Ordering};

pub const EGRESS_POLICY_VIF_COUNT: usize = 2;

/// Per-interface shadow accounting for Core0-issued burst grants.
///
/// Airtime is the conservative HT data-PPDU estimate used by the scheduler,
/// expressed in 100-nanosecond units. It is not measured medium occupancy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EgressPolicyVifShadowSnapshot {
    pub grants_issued: u32,
    pub issued_frame_credits: u32,
    pub issued_modeled_airtime_100ns: u32,
    pub grants_finished: u32,
    pub used_frames: u32,
    pub used_modeled_airtime_100ns: u32,
    pub grants_unused: u32,
}

impl EgressPolicyVifShadowSnapshot {
    pub const fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            grants_issued: self.grants_issued.wrapping_sub(earlier.grants_issued),
            issued_frame_credits: self
                .issued_frame_credits
                .wrapping_sub(earlier.issued_frame_credits),
            issued_modeled_airtime_100ns: self
                .issued_modeled_airtime_100ns
                .wrapping_sub(earlier.issued_modeled_airtime_100ns),
            grants_finished: self.grants_finished.wrapping_sub(earlier.grants_finished),
            used_frames: self.used_frames.wrapping_sub(earlier.used_frames),
            used_modeled_airtime_100ns: self
                .used_modeled_airtime_100ns
                .wrapping_sub(earlier.used_modeled_airtime_100ns),
            grants_unused: self.grants_unused.wrapping_sub(earlier.grants_unused),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EgressPolicyShadowSnapshot {
    pub grants_issued: u32,
    pub grants_finished: u32,
    pub grants_used: u32,
    pub grants_unused: u32,
    pub progress_without_grant: u32,
    pub rejected_updates: u32,
    pub rejected_progress: u32,
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
            grants_issued: self.grants_issued.wrapping_sub(earlier.grants_issued),
            grants_finished: self.grants_finished.wrapping_sub(earlier.grants_finished),
            grants_used: self.grants_used.wrapping_sub(earlier.grants_used),
            grants_unused: self.grants_unused.wrapping_sub(earlier.grants_unused),
            progress_without_grant: self
                .progress_without_grant
                .wrapping_sub(earlier.progress_without_grant),
            rejected_updates: self.rejected_updates.wrapping_sub(earlier.rejected_updates),
            rejected_progress: self
                .rejected_progress
                .wrapping_sub(earlier.rejected_progress),
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
    grants_issued: AtomicU32,
    issued_frame_credits: AtomicU32,
    issued_modeled_airtime_100ns: AtomicU32,
    grants_finished: AtomicU32,
    used_frames: AtomicU32,
    used_modeled_airtime_100ns: AtomicU32,
    grants_unused: AtomicU32,
}

impl EgressPolicyVifShadowCounters {
    const fn new() -> Self {
        Self {
            grants_issued: AtomicU32::new(0),
            issued_frame_credits: AtomicU32::new(0),
            issued_modeled_airtime_100ns: AtomicU32::new(0),
            grants_finished: AtomicU32::new(0),
            used_frames: AtomicU32::new(0),
            used_modeled_airtime_100ns: AtomicU32::new(0),
            grants_unused: AtomicU32::new(0),
        }
    }

    fn issued(&self, frames: u8, modeled_airtime_100ns: u32) {
        self.grants_issued.fetch_add(1, Ordering::Relaxed);
        self.issued_frame_credits
            .fetch_add(u32::from(frames), Ordering::Relaxed);
        self.issued_modeled_airtime_100ns
            .fetch_add(modeled_airtime_100ns, Ordering::Relaxed);
    }

    fn finished_used(&self, frames: u8, modeled_airtime_100ns: u32) {
        self.grants_finished.fetch_add(1, Ordering::Relaxed);
        self.used_frames
            .fetch_add(u32::from(frames), Ordering::Relaxed);
        self.used_modeled_airtime_100ns
            .fetch_add(modeled_airtime_100ns, Ordering::Relaxed);
    }

    fn finished_unused(&self) {
        self.grants_finished.fetch_add(1, Ordering::Relaxed);
        self.grants_unused.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> EgressPolicyVifShadowSnapshot {
        EgressPolicyVifShadowSnapshot {
            grants_issued: self.grants_issued.load(Ordering::Relaxed),
            issued_frame_credits: self.issued_frame_credits.load(Ordering::Relaxed),
            issued_modeled_airtime_100ns: self.issued_modeled_airtime_100ns.load(Ordering::Relaxed),
            grants_finished: self.grants_finished.load(Ordering::Relaxed),
            used_frames: self.used_frames.load(Ordering::Relaxed),
            used_modeled_airtime_100ns: self.used_modeled_airtime_100ns.load(Ordering::Relaxed),
            grants_unused: self.grants_unused.load(Ordering::Relaxed),
        }
    }
}

pub(crate) struct EgressPolicyShadowCounters {
    grants_issued: AtomicU32,
    grants_finished: AtomicU32,
    grants_used: AtomicU32,
    grants_unused: AtomicU32,
    progress_without_grant: AtomicU32,
    rejected_updates: AtomicU32,
    rejected_progress: AtomicU32,
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
            grants_issued: AtomicU32::new(0),
            grants_finished: AtomicU32::new(0),
            grants_used: AtomicU32::new(0),
            grants_unused: AtomicU32::new(0),
            progress_without_grant: AtomicU32::new(0),
            rejected_updates: AtomicU32::new(0),
            rejected_progress: AtomicU32::new(0),
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

    pub(crate) fn grant_issued(&self, vif: u8, frames: u8, modeled_airtime_100ns: u32) {
        self.grants_issued.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.vif(vif) {
            counters.issued(frames, modeled_airtime_100ns);
        }
    }

    pub(crate) fn grant_finished_used(&self, vif: u8, frames: u8, modeled_airtime_100ns: u32) {
        self.grants_finished.fetch_add(1, Ordering::Relaxed);
        self.grants_used.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.vif(vif) {
            counters.finished_used(frames, modeled_airtime_100ns);
        }
    }

    pub(crate) fn grant_finished_unused(&self, vif: u8) {
        self.grants_finished.fetch_add(1, Ordering::Relaxed);
        self.grants_unused.fetch_add(1, Ordering::Relaxed);
        if let Some(counters) = self.vif(vif) {
            counters.finished_unused();
        }
    }

    pub(crate) fn progress_without_grant(&self) {
        self.progress_without_grant.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn rejected_update(&self) {
        self.rejected_updates.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn rejected_progress(&self) {
        self.rejected_progress.fetch_add(1, Ordering::Relaxed);
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
            grants_issued: self.grants_issued.load(Ordering::Relaxed),
            grants_finished: self.grants_finished.load(Ordering::Relaxed),
            grants_used: self.grants_used.load(Ordering::Relaxed),
            grants_unused: self.grants_unused.load(Ordering::Relaxed),
            progress_without_grant: self.progress_without_grant.load(Ordering::Relaxed),
            rejected_updates: self.rejected_updates.load(Ordering::Relaxed),
            rejected_progress: self.rejected_progress.load(Ordering::Relaxed),
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

pub(crate) static EGRESS_POLICY_SHADOW_COUNTERS: EgressPolicyShadowCounters =
    EgressPolicyShadowCounters::new();

pub fn egress_policy_shadow_snapshot() -> EgressPolicyShadowSnapshot {
    EGRESS_POLICY_SHADOW_COUNTERS.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_vif_snapshot_delta_preserves_grant_lifecycle_identity() {
        let earlier = EgressPolicyShadowSnapshot {
            vifs: [
                EgressPolicyVifShadowSnapshot {
                    grants_issued: u32::MAX,
                    issued_frame_credits: 30,
                    issued_modeled_airtime_100ns: 1_000,
                    ..EgressPolicyVifShadowSnapshot::default()
                },
                EgressPolicyVifShadowSnapshot {
                    grants_finished: 7,
                    used_frames: 200,
                    used_modeled_airtime_100ns: 50_000,
                    ..EgressPolicyVifShadowSnapshot::default()
                },
            ],
            ..EgressPolicyShadowSnapshot::default()
        };
        let current = EgressPolicyShadowSnapshot {
            vifs: [
                EgressPolicyVifShadowSnapshot {
                    grants_issued: 2,
                    issued_frame_credits: 62,
                    issued_modeled_airtime_100ns: 3_000,
                    ..EgressPolicyVifShadowSnapshot::default()
                },
                EgressPolicyVifShadowSnapshot {
                    grants_finished: 9,
                    used_frames: 264,
                    used_modeled_airtime_100ns: 70_000,
                    ..EgressPolicyVifShadowSnapshot::default()
                },
            ],
            ..EgressPolicyShadowSnapshot::default()
        };

        let delta = current.wrapping_delta_since(earlier);
        assert_eq!(delta.vifs[0].grants_issued, 3);
        assert_eq!(delta.vifs[0].issued_frame_credits, 32);
        assert_eq!(delta.vifs[0].issued_modeled_airtime_100ns, 2_000);
        assert_eq!(delta.vifs[1].grants_finished, 2);
        assert_eq!(delta.vifs[1].used_frames, 64);
        assert_eq!(delta.vifs[1].used_modeled_airtime_100ns, 20_000);
    }
}
