//! Physical-radio egress policy and value-only TX cost models.

use core::num::{NonZeroU8, NonZeroU32};

use open_esp_radio_embassy_net::{EgressDemandUpdate, EgressKey};
use open_esp_radio_esp32s31_wifi_mac::{
    tx::HtRate,
    tx_ampdu::{HtAmpduLengthAccumulator, ModeledHtAmpduPpduDuration},
};
use open_esp_radio_wifi_softmac::{
    WifiAirtimeUnits, WifiEgressAdmissionObservation, WifiEgressAirtimeConfig,
    WifiEgressAirtimeError, WifiEgressAirtimeScheduler, WifiEgressDemand, WifiEgressDemandId,
    WifiEgressDemandLevel, WifiEgressOpportunity, WifiEgressSelection,
};

const PHYSICAL_EGRESS_VIFS: usize = 2;
const PHYSICAL_EGRESS_QUEUES: usize = 32;
const IEEE80211_FCS_BYTES: usize = 4;

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
            airtime_units(20_000),
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

    let mut length = HtAmpduLengthAccumulator::new(requested, maximum_aggregate_bytes).ok()?;
    let mut admitted = 0_u8;
    for _ in 0..requested {
        if length.push(u32::from(psdu_bytes), 0).is_err() {
            break;
        }
        admitted += 1;
    }
    let admitted = NonZeroU8::new(admitted)?;
    let aggregate = length.finish().ok()?;
    let duration = ModeledHtAmpduPpduDuration::from_published_ampdu(rate, aggregate).ok()?;
    Some(WifiEgressOpportunity::new(
        admitted,
        airtime_units(duration.hundred_nanoseconds()),
        queue_pending_limit,
        queue_weight,
    ))
}

/// Physical-radio shadow policy retained outside every async runner frame.
///
/// The two VIFs and 32 queue slots correspond to the two permanent network
/// interfaces and their independent 16-key lifecycle mirrors. The policy owns
/// only bounded metadata; packet bytes and DMA credits remain in the existing
/// network/radio owners.
pub struct DatapathEgressAirtimePolicy {
    scheduler: WifiEgressAirtimeScheduler<EgressKey, PHYSICAL_EGRESS_VIFS, PHYSICAL_EGRESS_QUEUES>,
    recommendation: Option<WifiEgressSelection<EgressKey>>,
    rejected_updates: u32,
    recommendations: u32,
    exact_recommendations: u32,
    different_recommendations: u32,
    cancelled_recommendations: u32,
    unavailable_actual: u32,
    rejected_observations: u32,
}

impl DatapathEgressAirtimePolicy {
    pub const fn new() -> Self {
        Self {
            scheduler: WifiEgressAirtimeScheduler::new(WifiEgressAirtimeConfig::new(
                [airtime_units(10_000); PHYSICAL_EGRESS_VIFS],
                airtime_units(10_000),
                airtime_units(40_000),
                [airtime_units(20_000); PHYSICAL_EGRESS_VIFS],
            )),
            recommendation: None,
            rejected_updates: 0,
            recommendations: 0,
            exact_recommendations: 0,
            different_recommendations: 0,
            cancelled_recommendations: 0,
            unavailable_actual: 0,
            rejected_observations: 0,
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
        self.cancel_recommendation();
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

    fn cancel_recommendation(&mut self) -> bool {
        let Some(recommendation) = self.recommendation.take() else {
            return false;
        };
        if self.scheduler.cancel_selection(recommendation).is_err() {
            self.rejected_observations = self.rejected_observations.saturating_add(1);
            #[cfg(feature = "tx-phase-telemetry")]
            crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.rejected_observation();
        }
        true
    }

    fn prepare_recommendation(
        &mut self,
        opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> bool {
        self.cancel_recommendation();
        match self.scheduler.select_next(|demand| {
            opportunity_for(demand)
                .and_then(|snapshot| snapshot.opportunity(demand.level().ready_frames().get()))
        }) {
            Ok(Some(recommendation)) => {
                self.recommendations = self.recommendations.saturating_add(1);
                #[cfg(feature = "tx-phase-telemetry")]
                crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.recommendation();
                self.recommendation = Some(recommendation);
                true
            }
            Ok(None) => false,
            Err(_) => {
                self.rejected_observations = self.rejected_observations.saturating_add(1);
                #[cfg(feature = "tx-phase-telemetry")]
                crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.rejected_observation();
                false
            }
        }
    }

    fn observe_actual(
        &mut self,
        interface: u8,
        key: Option<EgressKey>,
        opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> bool {
        let Some(recommendation) = self.recommendation.take() else {
            self.unavailable_actual = self.unavailable_actual.saturating_add(1);
            #[cfg(feature = "tx-phase-telemetry")]
            crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.unavailable_actual();
            return false;
        };
        let Some(key) = key else {
            let _ = self.scheduler.cancel_selection(recommendation);
            self.unavailable_actual = self.unavailable_actual.saturating_add(1);
            #[cfg(feature = "tx-phase-telemetry")]
            crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.unavailable_actual();
            return false;
        };
        // The selected demand and its role-derived opportunity are immutable
        // for the lifetime of one outstanding recommendation. Reusing both
        // on the overwhelmingly common exact path avoids a second role
        // snapshot query at every physical transaction boundary. A genuinely
        // different production choice must still be revalidated against its
        // own current BA/rate/power-save state.
        let (actual, opportunity) = if recommendation.demand().vif() == interface
            && *recommendation.demand().key() == key
        {
            (*recommendation.demand(), recommendation.opportunity())
        } else {
            let Some(actual) = self.scheduler.demand(interface, key) else {
                let _ = self.scheduler.cancel_selection(recommendation);
                self.unavailable_actual = self.unavailable_actual.saturating_add(1);
                #[cfg(feature = "tx-phase-telemetry")]
                crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.unavailable_actual();
                return false;
            };
            let Some(opportunity) = opportunity_for(actual)
                .and_then(|snapshot| snapshot.opportunity(actual.level().ready_frames().get()))
            else {
                let _ = self.scheduler.cancel_selection(recommendation);
                self.unavailable_actual = self.unavailable_actual.saturating_add(1);
                #[cfg(feature = "tx-phase-telemetry")]
                crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.unavailable_actual();
                return false;
            };
            (actual, opportunity)
        };
        let estimated = opportunity.estimated_airtime();
        match self
            .scheduler
            .observe_admission(recommendation, actual, opportunity)
        {
            Ok((observation, admission)) => {
                match observation {
                    WifiEgressAdmissionObservation::ExactRecommendation => {
                        self.exact_recommendations = self.exact_recommendations.saturating_add(1);
                        #[cfg(feature = "tx-phase-telemetry")]
                        crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS
                            .exact_recommendation();
                    }
                    WifiEgressAdmissionObservation::DifferentQueue => {
                        self.different_recommendations =
                            self.different_recommendations.saturating_add(1);
                        #[cfg(feature = "tx-phase-telemetry")]
                        crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS
                            .different_recommendation();
                    }
                }
                if self
                    .scheduler
                    .reconcile_modeled_airtime(admission, estimated)
                    .is_err()
                {
                    self.rejected_observations = self.rejected_observations.saturating_add(1);
                    #[cfg(feature = "tx-phase-telemetry")]
                    crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS
                        .rejected_observation();
                }
                true
            }
            Err(_) => {
                self.rejected_observations = self.rejected_observations.saturating_add(1);
                #[cfg(feature = "tx-phase-telemetry")]
                crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.rejected_observation();
                false
            }
        }
    }

    pub const fn shadow_observation(&self) -> DatapathEgressShadowObservation {
        DatapathEgressShadowObservation {
            recommendations: self.recommendations,
            exact_recommendations: self.exact_recommendations,
            different_recommendations: self.different_recommendations,
            cancelled_recommendations: self.cancelled_recommendations,
            unavailable_actual: self.unavailable_actual,
            rejected_observations: self.rejected_observations,
        }
    }
}

/// Monotonic comparison between the radio policy and unchanged production
/// packet admission.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DatapathEgressShadowObservation {
    pub recommendations: u32,
    pub exact_recommendations: u32,
    pub different_recommendations: u32,
    pub cancelled_recommendations: u32,
    pub unavailable_actual: u32,
    pub rejected_observations: u32,
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

    fn prepare_recommendation(
        &mut self,
        opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> bool;

    /// Return one unconsumed shadow selection to the scheduler.
    ///
    /// A role may accept or retain a queue head without publishing a physical
    /// radio transaction. That is not an unavailable actual decision: the
    /// provisional scheduler selection simply did not cross the hardware
    /// boundary and must be cancelled explicitly.
    fn cancel_recommendation(&mut self);

    fn observe_actual(
        &mut self,
        interface: u8,
        key: Option<EgressKey>,
        opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> bool;
}

impl DatapathEgressPolicyOwner for () {
    fn observe_update(&mut self, _vif: u8, _update: EgressDemandUpdate) {}

    fn prepare_recommendation(
        &mut self,
        _opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> bool {
        false
    }

    fn cancel_recommendation(&mut self) {}

    fn observe_actual(
        &mut self,
        _interface: u8,
        _key: Option<EgressKey>,
        _opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> bool {
        false
    }
}

impl DatapathEgressPolicyOwner for DatapathEgressAirtimePolicy {
    fn observe_update(&mut self, vif: u8, update: EgressDemandUpdate) {
        DatapathEgressAirtimePolicy::observe_update(self, vif, update);
    }

    fn prepare_recommendation(
        &mut self,
        opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> bool {
        DatapathEgressAirtimePolicy::prepare_recommendation(self, opportunity_for)
    }

    fn cancel_recommendation(&mut self) {
        if DatapathEgressAirtimePolicy::cancel_recommendation(self) {
            self.cancelled_recommendations = self.cancelled_recommendations.saturating_add(1);
            #[cfg(feature = "tx-phase-telemetry")]
            crate::diagnostics::egress::EGRESS_POLICY_SHADOW_COUNTERS.cancelled_recommendation();
        }
    }

    fn observe_actual(
        &mut self,
        interface: u8,
        key: Option<EgressKey>,
        opportunity_for: &mut dyn FnMut(
            WifiEgressDemand<EgressKey>,
        ) -> Option<DatapathHtEgressSnapshot>,
    ) -> bool {
        DatapathEgressAirtimePolicy::observe_actual(self, interface, key, opportunity_for)
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
    fn shadow_recommendation_observes_actual_queue_without_retaining_pending_airtime() {
        let mut policy = DatapathEgressAirtimePolicy::new();
        let first = key(1);
        let second = key(2);
        policy.observe_update(0, EgressDemandUpdate::Reset { schedule_epoch: 7 });
        policy.observe_update(1, EgressDemandUpdate::Reset { schedule_epoch: 11 });
        policy.observe_update(0, active(7, 1, first, 32));
        policy.observe_update(1, active(11, 1, second, 32));

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
        let mut prepare_queries = 0;
        assert!(policy.prepare_recommendation(&mut |_| {
            prepare_queries += 1;
            Some(snapshot)
        }));
        let mut actual_queries = 0;
        assert!(policy.observe_actual(0, Some(first), &mut |_| {
            actual_queries += 1;
            Some(snapshot)
        }));
        assert!(prepare_queries != 0);
        assert_eq!(actual_queries, 0, "exact admission reuses selected facts");
        assert_eq!(policy.scheduler.global_pending_airtime(), 0);

        // The exact first admission debits VIF 0 and advances the DRR cursor
        // to VIF 1. Feed the unchanged production FIFO's VIF-0 choice to the
        // second recommendation and prove that disagreement is observational.
        assert!(policy.prepare_recommendation(&mut |_| {
            prepare_queries += 1;
            Some(snapshot)
        }));
        assert!(policy.observe_actual(0, Some(first), &mut |_| {
            actual_queries += 1;
            Some(snapshot)
        }));
        assert_eq!(
            actual_queries, 1,
            "a different actual queue requires its own role snapshot"
        );
        assert_eq!(policy.scheduler.global_pending_airtime(), 0);
        assert_eq!(
            policy.shadow_observation(),
            DatapathEgressShadowObservation {
                recommendations: 2,
                exact_recommendations: 1,
                different_recommendations: 1,
                cancelled_recommendations: 0,
                unavailable_actual: 0,
                rejected_observations: 0,
            }
        );
    }

    #[test]
    fn unpublished_physical_transaction_cancels_without_forging_unavailable_actual() {
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

        assert!(policy.prepare_recommendation(&mut |_| Some(snapshot)));
        DatapathEgressPolicyOwner::cancel_recommendation(&mut policy);
        assert_eq!(
            policy.shadow_observation(),
            DatapathEgressShadowObservation {
                recommendations: 1,
                exact_recommendations: 0,
                different_recommendations: 0,
                cancelled_recommendations: 1,
                unavailable_actual: 0,
                rejected_observations: 0,
            }
        );
        assert!(policy.prepare_recommendation(&mut |_| Some(snapshot)));
    }
}
