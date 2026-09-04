//! Physical-radio egress policy and value-only TX cost models.

use core::num::{NonZeroU8, NonZeroU32};

use open_esp_radio_embassy_net::{
    EgressBurstGrant, EgressDemand, EgressDemandId, EgressDemandLevel, EgressDemandUpdate,
    EgressGrantProgress, EgressKey,
};
use open_esp_radio_esp32s31_wifi_mac::{
    tx::HtRate,
    tx_ampdu::{HtAmpduLengthAccumulator, ModeledHtAmpduPpduDuration},
};
use open_esp_radio_wifi_softmac::{
    WIFI_EGRESS_GRANT_HORIZON, WifiAirtimeUnits, WifiEgressAdmission, WifiEgressAirtimeConfig,
    WifiEgressAirtimeError, WifiEgressAirtimeScheduler, WifiEgressBurstGrant, WifiEgressDemand,
    WifiEgressDemandId, WifiEgressDemandLevel, WifiEgressOpportunity,
};

const PHYSICAL_EGRESS_VIFS: usize = 2;
const PHYSICAL_EGRESS_QUEUES: usize = 32;
const IEEE80211_FCS_BYTES: usize = 4;
const MAXIMUM_HT_PPDU_AIRTIME_100NS: u32 = 54_840;
const PHYSICAL_EGRESS_HORIZON_AIRTIME_100NS: u32 =
    MAXIMUM_HT_PPDU_AIRTIME_100NS * WIFI_EGRESS_GRANT_HORIZON as u32;

const fn airtime_units(hundred_nanoseconds: u32) -> WifiAirtimeUnits {
    let Some(value) = NonZeroU32::new(hundred_nanoseconds) else {
        panic!("airtime policy units must be non-zero");
    };
    WifiAirtimeUnits::new(value)
}

/// Immutable radio facts behind one currently valid egress queue.
///
/// This value contains no scheduler credit and grants no packet or DMA
/// ownership. A role may return it only after revalidating the opaque demand
/// key against its current association, BlockAck, power-save and PHY state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatapathHtEgressSnapshot {
    rate: HtRate,
    maximum_frames: NonZeroU8,
    maximum_ethernet_bytes: usize,
    maximum_aggregate_bytes: u16,
}

/// Fail-closed reason why a mirrored queue cannot currently become a radio
/// opportunity. These values are diagnostic classifications, never fallback
/// policy: a rejected snapshot remains ineligible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DatapathEgressSnapshotRejection {
    Key,
    Identity,
    TrafficClass,
    RoleUnavailable,
    NonHtRate,
    NoBlockAck,
    InvalidGeometry,
}

pub(crate) fn rejected_ht_egress_snapshot(
    reason: DatapathEgressSnapshotRejection,
) -> Option<DatapathHtEgressSnapshot> {
    #[cfg(feature = "tx-phase-telemetry")]
    crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.snapshot_rejected(reason);
    #[cfg(not(feature = "tx-phase-telemetry"))]
    let _ = reason;
    None
}

pub(crate) fn record_ht_egress_snapshot_query(snapshot_ready: bool) {
    #[cfg(feature = "tx-phase-telemetry")]
    crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.snapshot_query(snapshot_ready);
    #[cfg(not(feature = "tx-phase-telemetry"))]
    let _ = snapshot_ready;
}

impl DatapathHtEgressSnapshot {
    pub const fn new(
        rate: HtRate,
        maximum_frames: NonZeroU8,
        maximum_ethernet_bytes: usize,
        maximum_aggregate_bytes: u16,
    ) -> Self {
        Self {
            rate,
            maximum_frames,
            maximum_ethernet_bytes,
            maximum_aggregate_bytes,
        }
    }

    pub const fn rate(self) -> HtRate {
        self.rate
    }

    pub const fn maximum_frames(self) -> NonZeroU8 {
        self.maximum_frames
    }

    pub const fn maximum_ethernet_bytes(self) -> usize {
        self.maximum_ethernet_bytes
    }

    pub const fn maximum_aggregate_bytes(self) -> u16 {
        self.maximum_aggregate_bytes
    }

    fn opportunity(self, ready_frames: u16) -> Option<WifiEgressOpportunity> {
        conservative_ht_data_ppdu_opportunity(
            self.rate,
            ready_frames,
            self.maximum_frames.get(),
            self.maximum_ethernet_bytes,
            self.maximum_aggregate_bytes,
            airtime_units(PHYSICAL_EGRESS_HORIZON_AIRTIME_100NS),
            NonZeroU8::MIN,
        )
    }
}

/// Estimate one saturated HT transaction using an explicit maximum Ethernet
/// frame size and the exact S31 aggregate byte ceilings.
///
/// Demand currently reports frame count but not queued byte geometry. This
/// model therefore charges every selected frame as `maximum_ethernet_bytes`.
/// It is deliberately conservative and must later be corrected with the
/// exact published/terminal aggregate geometry. It is not measured medium
/// airtime: the returned cost contains the data PPDU only and excludes
/// contention, protection, SIFS and BlockAck exchange time.
pub fn conservative_ht_data_ppdu_opportunity(
    rate: HtRate,
    ready_frames: u16,
    maximum_frames: u8,
    maximum_ethernet_bytes: usize,
    negotiated_maximum_aggregate_bytes: u16,
    queue_pending_limit: WifiAirtimeUnits,
    queue_weight: NonZeroU8,
) -> Option<WifiEgressOpportunity> {
    let requested = usize::from(maximum_frames).min(usize::from(ready_frames));
    let requested = u8::try_from(requested).ok().filter(|count| *count != 0)?;
    let psdu_bytes = maximum_ethernet_bytes
        .checked_add(open_esp_radio_ieee80211::station::STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
        .and_then(|bytes| {
            bytes.checked_add(open_esp_radio_esp32s31_wifi::ordinary_tx::TX_CCMP_MIC_SIZE)
        })
        .and_then(|bytes| bytes.checked_add(IEEE80211_FCS_BYTES))?;
    let psdu_bytes = u16::try_from(psdu_bytes)
        .ok()
        .filter(|bytes| *bytes <= 0x3fff)?;
    let maximum_aggregate_bytes = rate
        .vendor_ampdu_byte_limit()
        .map_or(negotiated_maximum_aggregate_bytes, |rate_limit| {
            rate_limit.min(negotiated_maximum_aggregate_bytes)
        });
    if maximum_aggregate_bytes == 0 {
        return None;
    }

    let aggregate = HtAmpduLengthAccumulator::largest_repeated_prefix(
        requested,
        maximum_aggregate_bytes,
        u32::from(psdu_bytes),
        0,
        false,
    )
    .ok()?;
    let admitted = NonZeroU8::new(aggregate.subframes)?;
    let duration = ModeledHtAmpduPpduDuration::from_published_ampdu(rate, aggregate).ok()?;
    Some(WifiEgressOpportunity::new(
        admitted,
        airtime_units(duration.hundred_nanoseconds()),
        queue_pending_limit,
        queue_weight,
    ))
}

/// Physical-radio egress policy retained outside every async runner frame.
///
/// The two VIFs and 32 queue slots correspond to the two permanent network
/// interfaces and their independent 16-key lifecycle mirrors. The policy owns
/// only bounded metadata; packet bytes and DMA credits remain in the existing
/// network/radio owners.
pub struct DatapathEgressAirtimePolicy {
    scheduler: WifiEgressAirtimeScheduler<EgressKey, PHYSICAL_EGRESS_VIFS, PHYSICAL_EGRESS_QUEUES>,
    grants: [Option<DatapathEgressGrantState>; WIFI_EGRESS_GRANT_HORIZON],
    rejected_updates: u32,
    grants_issued: u32,
    grants_finished: u32,
    grants_used: u32,
    grants_unused: u32,
    progress_without_grant: u32,
    rejected_progress: u32,
}

#[derive(Debug)]
struct DatapathEgressGrantState {
    grant: WifiEgressBurstGrant<EgressKey>,
    transported: bool,
    admission: WifiEgressAdmission<EgressKey>,
}

impl DatapathEgressAirtimePolicy {
    pub const fn new() -> Self {
        Self {
            scheduler: WifiEgressAirtimeScheduler::new(WifiEgressAirtimeConfig::new(
                [airtime_units(10_000); PHYSICAL_EGRESS_VIFS],
                airtime_units(10_000),
                airtime_units(PHYSICAL_EGRESS_HORIZON_AIRTIME_100NS),
                [airtime_units(PHYSICAL_EGRESS_HORIZON_AIRTIME_100NS); PHYSICAL_EGRESS_VIFS],
            )),
            grants: [None, None],
            rejected_updates: 0,
            grants_issued: 0,
            grants_finished: 0,
            grants_used: 0,
            grants_unused: 0,
            progress_without_grant: 0,
            rejected_progress: 0,
        }
    }

    fn apply_update(
        &mut self,
        vif: u8,
        update: EgressDemandUpdate,
    ) -> Result<(), WifiEgressAirtimeError> {
        match update {
            EgressDemandUpdate::Reset { schedule_epoch } => {
                self.scheduler.reset_vif(vif, schedule_epoch)
            }
            EgressDemandUpdate::Active(demand) => {
                self.scheduler.upsert_demand(WifiEgressDemand::new(
                    vif,
                    WifiEgressDemandId::new(demand.id().schedule_epoch(), demand.id().activation()),
                    demand.key(),
                    WifiEgressDemandLevel::new(
                        demand.level().ready_units(),
                        demand.level().horizon_ready(),
                    ),
                ))
            }
            EgressDemandUpdate::Inactive { id, key } => {
                self.scheduler.remove_demand(
                    vif,
                    WifiEgressDemandId::new(id.schedule_epoch(), id.activation()),
                    key,
                );
                Ok(())
            }
        }
    }

    fn observe_update(&mut self, vif: u8, update: EgressDemandUpdate) {
        if self.apply_update(vif, update).is_err() {
            self.rejected_updates = self.rejected_updates.saturating_add(1);
            #[cfg(feature = "tx-phase-telemetry")]
            crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.rejected_update();
        }
    }

    pub const fn rejected_updates(&self) -> u32 {
        self.rejected_updates
    }

    /// Convert current role facts into one scheduler opportunity.
    ///
    /// The role remains the authority for association/BA/rate validity. The
    /// physical policy supplies only its provisional queue budget and weight.
    pub fn opportunity_for(
        &self,
        demand: WifiEgressDemand<EgressKey>,
        snapshot: DatapathHtEgressSnapshot,
    ) -> Option<WifiEgressOpportunity> {
        snapshot.opportunity(demand.level().ready_frames().get())
    }

    fn prepare_grant(
        &mut self,
        opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> Option<(u8, EgressBurstGrant)> {
        if let Some(state) = self
            .grants
            .iter()
            .flatten()
            .find(|state| !state.transported)
        {
            return Some((
                state.grant.demand().vif(),
                Self::transport_grant(state.grant),
            ));
        }
        let grant_slot = self.grants.iter().position(Option::is_none)?;
        if self.grants[grant_slot].is_some() {
            return None;
        }
        let selection = match self.scheduler.select_next(|demand| {
            opportunity_for(demand)
                .and_then(|snapshot| snapshot.opportunity(demand.level().ready_frames().get()))
        }) {
            Ok(Some(selection)) => selection,
            Ok(None) => return None,
            Err(_) => {
                self.rejected_progress = self.rejected_progress.saturating_add(1);
                #[cfg(feature = "tx-phase-telemetry")]
                crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.rejected_progress();
                return None;
            }
        };
        let (grant, admission) = match self.scheduler.issue_selected(selection) {
            Ok(issued) => issued.into_parts(),
            Err(_) => {
                self.rejected_progress = self.rejected_progress.saturating_add(1);
                #[cfg(feature = "tx-phase-telemetry")]
                crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.rejected_progress();
                return None;
            }
        };
        self.grants_issued = self.grants_issued.saturating_add(1);
        #[cfg(feature = "tx-phase-telemetry")]
        open_esp_radio_embassy_net::EGRESS_GRANT_TIMELINE.record_issued(grant.serial());
        #[cfg(feature = "tx-phase-telemetry")]
        crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.grant_issued(
            grant.demand().vif(),
            grant.opportunity().frame_limit().get(),
            grant
                .opportunity()
                .estimated_airtime()
                .hundred_nanoseconds(),
        );
        self.grants[grant_slot] = Some(DatapathEgressGrantState {
            grant,
            transported: false,
            admission,
        });
        Some((grant.demand().vif(), Self::transport_grant(grant)))
    }

    fn transport_grant(grant: WifiEgressBurstGrant<EgressKey>) -> EgressBurstGrant {
        let demand = grant.demand();
        EgressBurstGrant::new(
            grant.serial(),
            EgressDemand::new(
                EgressDemandId::new(demand.id().schedule_epoch(), demand.id().activation()),
                *demand.key(),
                EgressDemandLevel::new(
                    demand.level().ready_frames(),
                    demand.level().aggregate_ready(),
                ),
            ),
            grant.opportunity().frame_limit(),
            NonZeroU32::new(
                grant
                    .opportunity()
                    .estimated_airtime()
                    .hundred_nanoseconds(),
            )
            .expect("Wi-Fi airtime units are non-zero"),
        )
    }

    fn mark_grant_transported(&mut self, serial: NonZeroU32) {
        if let Some(state) = self
            .grants
            .iter_mut()
            .flatten()
            .find(|state| state.grant.serial() == serial)
            && state.grant.serial() == serial
        {
            state.transported = true;
        }
    }

    fn reject_grant_progress(&mut self) {
        self.rejected_progress = self.rejected_progress.saturating_add(1);
        #[cfg(feature = "tx-phase-telemetry")]
        crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.rejected_progress();
    }

    fn observe_grant_progress(&mut self, vif: u8, progress: EgressGrantProgress) {
        let EgressGrantProgress::Finished { serial, .. } = progress;
        let Some(grant_slot) = self.grants.iter().position(|state| {
            state
                .as_ref()
                .is_some_and(|state| state.grant.serial() == serial)
        }) else {
            self.progress_without_grant = self.progress_without_grant.saturating_add(1);
            #[cfg(feature = "tx-phase-telemetry")]
            crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.progress_without_grant();
            self.reject_grant_progress();
            return;
        };
        if !self.grants[grant_slot]
            .as_ref()
            .is_some_and(|state| state.grant.demand().vif() == vif)
        {
            self.reject_grant_progress();
            return;
        }

        let EgressGrantProgress::Finished {
            used_frames,
            remaining,
            ..
        } = progress;
        let state = self.grants[grant_slot]
            .take()
            .expect("validated grant remains present");
        let remaining = remaining
            .map(|level| WifiEgressDemandLevel::new(level.ready_units(), level.horizon_ready()));
        if self
            .scheduler
            .finish_grant(state.grant, used_frames, remaining)
            .is_err()
        {
            self.grants[grant_slot] = Some(state);
            self.reject_grant_progress();
            return;
        }
        let reconciliation = if used_frames == 0 {
            self.scheduler.cancel_admission(state.admission)
        } else {
            self.scheduler.reconcile_modeled_airtime(
                state.admission,
                state.grant.opportunity().estimated_airtime(),
            )
        };
        if reconciliation.is_err() {
            self.reject_grant_progress();
            return;
        }
        #[cfg(feature = "tx-phase-telemetry")]
        open_esp_radio_embassy_net::EGRESS_GRANT_TIMELINE.record_radio_received(serial);
        self.grants_finished = self.grants_finished.saturating_add(1);
        if used_frames == 0 {
            self.grants_unused = self.grants_unused.saturating_add(1);
            #[cfg(feature = "tx-phase-telemetry")]
            crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.grant_finished_unused(vif);
        } else {
            self.grants_used = self.grants_used.saturating_add(1);
            #[cfg(feature = "tx-phase-telemetry")]
            crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.grant_finished_used(
                vif,
                used_frames,
                state
                    .grant
                    .opportunity()
                    .estimated_airtime()
                    .hundred_nanoseconds(),
            );
        }
    }

    pub const fn grant_observation(&self) -> DatapathEgressGrantObservation {
        DatapathEgressGrantObservation {
            grants_issued: self.grants_issued,
            grants_finished: self.grants_finished,
            grants_used: self.grants_used,
            grants_unused: self.grants_unused,
            progress_without_grant: self.progress_without_grant,
            rejected_progress: self.rejected_progress,
        }
    }
}

/// Monotonic lifecycle totals for Core0-issued grants.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DatapathEgressGrantObservation {
    pub grants_issued: u32,
    pub grants_finished: u32,
    pub grants_used: u32,
    pub grants_unused: u32,
    pub progress_without_grant: u32,
    pub rejected_progress: u32,
}

impl Default for DatapathEgressAirtimePolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Bounded policy state updated by the physical Core0 egress owner.
///
/// Updates passed here have already been accepted by the transport-side
/// lifecycle mirror. Implementations must remain synchronous and must not
/// claim packet or DMA ownership.
pub trait DatapathEgressPolicyOwner {
    fn observe_update(&mut self, vif: u8, update: EgressDemandUpdate);

    /// Select or retry one bounded radio grant without charging airtime.
    ///
    /// Returning the same value until [`Self::mark_grant_transported`] makes
    /// a full cross-core grant ring ordinary backpressure rather than a lost
    /// scheduler decision.
    fn prepare_grant(
        &mut self,
        opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> Option<(u8, EgressBurstGrant)>;

    fn mark_grant_transported(&mut self, serial: NonZeroU32);

    fn observe_grant_progress(&mut self, vif: u8, progress: EgressGrantProgress);
}

impl DatapathEgressPolicyOwner for () {
    fn observe_update(&mut self, _vif: u8, _update: EgressDemandUpdate) {}

    fn prepare_grant(
        &mut self,
        _opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> Option<(u8, EgressBurstGrant)> {
        None
    }

    fn mark_grant_transported(&mut self, _serial: NonZeroU32) {}

    fn observe_grant_progress(&mut self, _vif: u8, _progress: EgressGrantProgress) {}
}

impl DatapathEgressPolicyOwner for DatapathEgressAirtimePolicy {
    fn observe_update(&mut self, vif: u8, update: EgressDemandUpdate) {
        DatapathEgressAirtimePolicy::observe_update(self, vif, update);
    }

    fn prepare_grant(
        &mut self,
        opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> Option<(u8, EgressBurstGrant)> {
        DatapathEgressAirtimePolicy::prepare_grant(self, opportunity_for)
    }

    fn mark_grant_transported(&mut self, serial: NonZeroU32) {
        DatapathEgressAirtimePolicy::mark_grant_transported(self, serial);
    }

    fn observe_grant_progress(&mut self, vif: u8, progress: EgressGrantProgress) {
        DatapathEgressAirtimePolicy::observe_grant_progress(self, vif, progress);
    }
}

#[cfg(test)]
mod tests {
    use core::num::{NonZeroU16, NonZeroU32};

    use open_esp_radio_embassy_net::{EgressDemand, EgressDemandId, EgressDemandLevel};
    use open_esp_radio_esp32s31_wifi_mac::tx::{HtChannelWidth, HtGuardInterval, HtMcs, HtRate};

    use super::*;

    const fn key(value: u32) -> EgressKey {
        EgressKey::from_words([value, 0, 0, 0])
    }

    fn active(epoch: u32, activation: u32, key: EgressKey, ready: u16) -> EgressDemandUpdate {
        EgressDemandUpdate::Active(EgressDemand::new(
            EgressDemandId::new(epoch, NonZeroU32::new(activation).unwrap()),
            key,
            EgressDemandLevel::new(NonZeroU16::new(ready).unwrap(), ready >= 32),
        ))
    }

    #[test]
    fn physical_policy_mirrors_exact_vif_and_demand_lifetimes() {
        let mut policy = DatapathEgressAirtimePolicy::new();
        let first = key(1);
        let second = key(2);

        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 7 });
        policy.observe_update(1, EgressDemandUpdate::Reset { schedule_epoch: 11 });
        policy.observe_update(0, active(7, 1, first, 32));
        policy.observe_update(1, active(11, 4, second, 3));

        let first_demand = policy.scheduler.demand(0, first).unwrap();
        let second_demand = policy.scheduler.demand(1, second).unwrap();
        assert_eq!(first_demand.id().schedule_epoch(), 7);
        assert_eq!(first_demand.level().ready_frames().get(), 32);
        assert_eq!(second_demand.id().activation().get(), 4);
        assert_eq!(second_demand.level().ready_frames().get(), 3);

        policy.observe_update(
            0,
            EgressDemandUpdate::Inactive {
                id: EgressDemandId::new(7, NonZeroU32::new(2).unwrap()),
                key: first,
            },
        );
        assert!(policy.scheduler.demand(0, first).is_some());

        policy.observe_update(
            0,
            EgressDemandUpdate::Inactive {
                id: EgressDemandId::new(7, NonZeroU32::new(1).unwrap()),
                key: first,
            },
        );
        assert!(policy.scheduler.demand(0, first).is_none());
        assert!(policy.scheduler.demand(1, second).is_some());
        assert_eq!(policy.rejected_updates(), 0);
    }

    #[test]
    fn physical_policy_counts_invalid_or_stale_updates_without_corrupting_state() {
        let mut policy = DatapathEgressAirtimePolicy::new();
        let current = key(3);

        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 9 });
        policy.observe_update(0, active(9, 1, current, 8));
        policy.observe_update(0, active(8, 2, key(4), 32));
        policy.observe_update(2, EgressDemandUpdate::Reset { schedule_epoch: 1 });

        assert_eq!(policy.rejected_updates(), 2);
        assert!(policy.scheduler.demand(0, current).is_some());
        assert!(policy.scheduler.demand(0, key(4)).is_none());
    }

    #[test]
    fn conservative_ht_cost_uses_real_rate_and_byte_ceilings() {
        let ht40_mcs7 = HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz40,
        );
        let full = conservative_ht_data_ppdu_opportunity(
            ht40_mcs7,
            32,
            32,
            1_600,
            u16::MAX,
            airtime_units(20_000),
            NonZeroU8::MIN,
        )
        .unwrap();
        assert_eq!(full.frame_limit().get(), 32);
        assert_eq!(full.estimated_airtime().hundred_nanoseconds(), 31_600);

        let ht40_mcs0 = HtRate::new(
            HtMcs::Mcs0,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz40,
        );
        let rate_limited = conservative_ht_data_ppdu_opportunity(
            ht40_mcs0,
            32,
            32,
            1_600,
            u16::MAX,
            airtime_units(20_000),
            NonZeroU8::MIN,
        )
        .unwrap();
        assert_eq!(rate_limited.frame_limit().get(), 3);
    }

    #[test]
    fn burst_grant_is_reserved_before_transport_and_retried_as_one_value() {
        let mut policy = DatapathEgressAirtimePolicy::new();
        let first = key(1);
        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 7 });
        policy.observe_update(0, active(7, 1, first, 32));

        let snapshot = DatapathHtEgressSnapshot::new(
            HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz40,
            ),
            NonZeroU8::new(32).unwrap(),
            1_600,
            u16::MAX,
        );
        let (vif, grant) = policy.prepare_grant(&mut |_| Some(snapshot)).unwrap();
        assert_eq!(vif, 0);
        assert_ne!(policy.scheduler.global_pending_airtime(), 0);
        assert_eq!(
            policy.prepare_grant(&mut |_| panic!("retry must reuse the selected facts")),
            Some((vif, grant))
        );
        policy.mark_grant_transported(grant.serial());
        assert_eq!(policy.prepare_grant(&mut |_| Some(snapshot)), None);

        policy.observe_grant_progress(
            vif,
            EgressGrantProgress::Finished {
                serial: grant.serial(),
                used_frames: grant.frame_credits().get(),
                remaining: None,
            },
        );
        assert_eq!(policy.scheduler.global_pending_airtime(), 0);
        assert_eq!(
            policy.grant_observation(),
            DatapathEgressGrantObservation {
                grants_issued: 1,
                grants_finished: 1,
                grants_used: 1,
                grants_unused: 0,
                progress_without_grant: 0,
                rejected_progress: 0,
            }
        );
    }

    #[test]
    fn current_and_standby_grants_are_both_reserved_and_closed_by_serial() {
        let mut policy = DatapathEgressAirtimePolicy::new();
        let first = key(1);
        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 7 });
        policy.observe_update(0, active(7, 1, first, 64));

        let snapshot = DatapathHtEgressSnapshot::new(
            HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz40,
            ),
            NonZeroU8::new(32).unwrap(),
            1_600,
            u16::MAX,
        );
        let (vif, current) = policy.prepare_grant(&mut |_| Some(snapshot)).unwrap();
        policy.mark_grant_transported(current.serial());
        let (_, standby) = policy.prepare_grant(&mut |_| Some(snapshot)).unwrap();
        policy.mark_grant_transported(standby.serial());
        assert_ne!(current.serial(), standby.serial());
        assert_eq!(current.demand().level().ready_units().get(), 64);
        assert_eq!(standby.demand().level().ready_units().get(), 32);
        assert_eq!(policy.prepare_grant(&mut |_| Some(snapshot)), None);

        policy.observe_grant_progress(
            vif,
            EgressGrantProgress::Finished {
                serial: current.serial(),
                used_frames: 32,
                remaining: Some(EgressDemandLevel::new(NonZeroU16::new(32).unwrap(), true)),
            },
        );
        policy.observe_grant_progress(
            vif,
            EgressGrantProgress::Finished {
                serial: standby.serial(),
                used_frames: 32,
                remaining: None,
            },
        );
        assert_eq!(policy.scheduler.global_pending_airtime(), 0);
        assert_eq!(
            policy.grant_observation(),
            DatapathEgressGrantObservation {
                grants_issued: 2,
                grants_finished: 2,
                grants_used: 2,
                grants_unused: 0,
                progress_without_grant: 0,
                rejected_progress: 0,
            }
        );
    }

    #[test]
    fn standby_advances_to_the_other_backlogged_vif() {
        let mut policy = DatapathEgressAirtimePolicy::new();
        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 7 });
        policy.observe_update(1, EgressDemandUpdate::Reset { schedule_epoch: 9 });
        policy.observe_update(0, active(7, 1, key(1), 64));
        policy.observe_update(1, active(9, 1, key(2), 64));
        let snapshot = DatapathHtEgressSnapshot::new(
            HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz40,
            ),
            NonZeroU8::new(32).unwrap(),
            1_600,
            u16::MAX,
        );

        let (current_vif, current) = policy.prepare_grant(&mut |_| Some(snapshot)).unwrap();
        policy.mark_grant_transported(current.serial());
        let (standby_vif, standby) = policy.prepare_grant(&mut |_| Some(snapshot)).unwrap();
        policy.mark_grant_transported(standby.serial());

        assert_ne!(current_vif, standby_vif);
        assert_ne!(current.demand().key(), standby.demand().key());
        for (vif, grant) in [(current_vif, current), (standby_vif, standby)] {
            policy.observe_grant_progress(
                vif,
                EgressGrantProgress::Finished {
                    serial: grant.serial(),
                    used_frames: 32,
                    remaining: Some(EgressDemandLevel::new(NonZeroU16::new(32).unwrap(), true)),
                },
            );
        }
        assert_eq!(policy.scheduler.global_pending_airtime(), 0);
    }

    #[test]
    fn unused_grant_returns_its_issued_airtime_reservation() {
        let mut policy = DatapathEgressAirtimePolicy::new();
        let first = key(1);
        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 7 });
        policy.observe_update(0, active(7, 1, first, 32));
        let snapshot = DatapathHtEgressSnapshot::new(
            HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz40,
            ),
            NonZeroU8::new(32).unwrap(),
            1_600,
            u16::MAX,
        );

        let (vif, grant) = policy.prepare_grant(&mut |_| Some(snapshot)).unwrap();
        assert_ne!(policy.scheduler.global_pending_airtime(), 0);
        policy.mark_grant_transported(grant.serial());
        policy.observe_grant_progress(
            vif,
            EgressGrantProgress::Finished {
                serial: grant.serial(),
                used_frames: 0,
                remaining: Some(grant.demand().level()),
            },
        );
        assert_eq!(policy.scheduler.global_pending_airtime(), 0);
        assert_eq!(
            policy.grant_observation(),
            DatapathEgressGrantObservation {
                grants_issued: 1,
                grants_finished: 1,
                grants_used: 0,
                grants_unused: 1,
                progress_without_grant: 0,
                rejected_progress: 0,
            }
        );
        assert!(policy.prepare_grant(&mut |_| Some(snapshot)).is_some());
    }

    #[test]
    fn epoch_reset_revokes_unused_credit_but_preserves_its_terminal_close() {
        let mut policy = DatapathEgressAirtimePolicy::new();
        let first = key(1);
        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 7 });
        policy.observe_update(0, active(7, 1, first, 32));
        let snapshot = DatapathHtEgressSnapshot::new(
            HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz40,
            ),
            NonZeroU8::new(32).unwrap(),
            1_600,
            u16::MAX,
        );
        let (vif, grant) = policy.prepare_grant(&mut |_| Some(snapshot)).unwrap();
        policy.mark_grant_transported(grant.serial());
        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 8 });
        policy.observe_grant_progress(
            vif,
            EgressGrantProgress::Finished {
                serial: grant.serial(),
                used_frames: 0,
                remaining: None,
            },
        );
        assert_eq!(
            policy.grant_observation(),
            DatapathEgressGrantObservation {
                grants_issued: 1,
                grants_finished: 1,
                grants_unused: 1,
                ..DatapathEgressGrantObservation::default()
            }
        );

        policy.observe_update(0, active(8, 2, first, 32));
        assert!(policy.prepare_grant(&mut |_| Some(snapshot)).is_some());
    }

    #[test]
    fn issued_grant_receipt_survives_epoch_reset_until_finish() {
        let mut policy = DatapathEgressAirtimePolicy::new();
        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 7 });
        policy.observe_update(0, active(7, 1, key(1), 32));
        let snapshot = DatapathHtEgressSnapshot::new(
            HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz40,
            ),
            NonZeroU8::new(32).unwrap(),
            1_600,
            u16::MAX,
        );
        let (vif, grant) = policy.prepare_grant(&mut |_| Some(snapshot)).unwrap();
        policy.mark_grant_transported(grant.serial());
        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 8 });
        policy.observe_grant_progress(
            vif,
            EgressGrantProgress::Finished {
                serial: grant.serial(),
                used_frames: grant.frame_credits().get(),
                remaining: None,
            },
        );
        assert_eq!(policy.scheduler.global_pending_airtime(), 0);
        assert_eq!(policy.grant_observation().rejected_progress, 0);
        assert_eq!(policy.grant_observation().grants_used, 1);
    }

    #[test]
    fn stale_grant_progress_cannot_close_the_live_grant() {
        let mut policy = DatapathEgressAirtimePolicy::new();
        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 7 });
        policy.observe_update(0, active(7, 1, key(1), 32));
        let snapshot = DatapathHtEgressSnapshot::new(
            HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz40,
            ),
            NonZeroU8::new(32).unwrap(),
            1_600,
            u16::MAX,
        );
        let (vif, grant) = policy.prepare_grant(&mut |_| Some(snapshot)).unwrap();
        policy.mark_grant_transported(grant.serial());
        let stale = NonZeroU32::new(grant.serial().get().wrapping_add(1)).unwrap();
        policy.observe_grant_progress(
            vif,
            EgressGrantProgress::Finished {
                serial: stale,
                used_frames: 0,
                remaining: None,
            },
        );
        assert_eq!(policy.grant_observation().rejected_progress, 1);
        assert_eq!(policy.prepare_grant(&mut |_| Some(snapshot)), None);

        policy.observe_grant_progress(
            vif,
            EgressGrantProgress::Finished {
                serial: grant.serial(),
                used_frames: grant.frame_credits().get(),
                remaining: None,
            },
        );
        assert_eq!(policy.scheduler.global_pending_airtime(), 0);
    }
}
