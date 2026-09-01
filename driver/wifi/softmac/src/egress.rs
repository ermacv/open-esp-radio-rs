//! Portable, bounded Wi-Fi egress airtime policy.
//!
//! This module owns no packet bytes, DMA descriptors, executor wake or
//! association table. Callers mirror their active egress lifetimes into the
//! scheduler, provide one radio-valid opportunity for each active demand, and
//! reconcile an admitted transaction when its modeled or measured airtime is
//! known. The separation keeps software backlog, radio scheduling and physical
//! DMA credits as distinct resources.

use core::num::{NonZeroU8, NonZeroU16, NonZeroU32};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct WifiAirtimeUnits(NonZeroU32);

impl WifiAirtimeUnits {
    pub const fn new(hundred_nanoseconds: NonZeroU32) -> Self {
        Self(hundred_nanoseconds)
    }

    pub const fn hundred_nanoseconds(self) -> u32 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiEgressDemandId {
    schedule_epoch: u32,
    activation: NonZeroU32,
}

impl WifiEgressDemandId {
    pub const fn new(schedule_epoch: u32, activation: NonZeroU32) -> Self {
        Self {
            schedule_epoch,
            activation,
        }
    }

    pub const fn schedule_epoch(self) -> u32 {
        self.schedule_epoch
    }

    pub const fn activation(self) -> NonZeroU32 {
        self.activation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiEgressDemandLevel {
    ready_frames: NonZeroU16,
    aggregate_ready: bool,
}

impl WifiEgressDemandLevel {
    pub const fn new(ready_frames: NonZeroU16, aggregate_ready: bool) -> Self {
        Self {
            ready_frames,
            aggregate_ready,
        }
    }

    pub const fn ready_frames(self) -> NonZeroU16 {
        self.ready_frames
    }

    pub const fn aggregate_ready(self) -> bool {
        self.aggregate_ready
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiEgressDemand<K> {
    vif: u8,
    id: WifiEgressDemandId,
    key: K,
    level: WifiEgressDemandLevel,
}

impl<K> WifiEgressDemand<K> {
    pub const fn new(
        vif: u8,
        id: WifiEgressDemandId,
        key: K,
        level: WifiEgressDemandLevel,
    ) -> Self {
        Self {
            vif,
            id,
            key,
            level,
        }
    }

    pub const fn vif(&self) -> u8 {
        self.vif
    }

    pub const fn id(&self) -> WifiEgressDemandId {
        self.id
    }

    pub const fn key(&self) -> &K {
        &self.key
    }

    pub const fn level(&self) -> WifiEgressDemandLevel {
        self.level
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiEgressOpportunity {
    frame_limit: NonZeroU8,
    estimated_airtime: WifiAirtimeUnits,
    queue_pending_limit: WifiAirtimeUnits,
    queue_weight: NonZeroU8,
}

impl WifiEgressOpportunity {
    pub const fn new(
        frame_limit: NonZeroU8,
        estimated_airtime: WifiAirtimeUnits,
        queue_pending_limit: WifiAirtimeUnits,
        queue_weight: NonZeroU8,
    ) -> Self {
        Self {
            frame_limit,
            estimated_airtime,
            queue_pending_limit,
            queue_weight,
        }
    }

    pub const fn frame_limit(self) -> NonZeroU8 {
        self.frame_limit
    }

    pub const fn estimated_airtime(self) -> WifiAirtimeUnits {
        self.estimated_airtime
    }

    pub const fn queue_pending_limit(self) -> WifiAirtimeUnits {
        self.queue_pending_limit
    }

    pub const fn queue_weight(self) -> NonZeroU8 {
        self.queue_weight
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiEgressAirtimeConfig<const VIFS: usize> {
    vif_quantum: [WifiAirtimeUnits; VIFS],
    queue_quantum: WifiAirtimeUnits,
    global_pending_limit: WifiAirtimeUnits,
    vif_pending_limit: [WifiAirtimeUnits; VIFS],
}

impl<const VIFS: usize> WifiEgressAirtimeConfig<VIFS> {
    pub const fn new(
        vif_quantum: [WifiAirtimeUnits; VIFS],
        queue_quantum: WifiAirtimeUnits,
        global_pending_limit: WifiAirtimeUnits,
        vif_pending_limit: [WifiAirtimeUnits; VIFS],
    ) -> Self {
        assert!(
            VIFS != 0,
            "a Wi-Fi airtime policy must own at least one VIF"
        );
        Self {
            vif_quantum,
            queue_quantum,
            global_pending_limit,
            vif_pending_limit,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiEgressAirtimeError {
    VifOutOfRange,
    EpochUnavailable,
    EpochMismatch,
    LifetimeConflict,
    Full,
    SelectionOutstanding,
    SelectionUnavailable,
    StaleSelection,
    OpportunityExceedsDemand,
    ActualDemandUnavailable,
    CounterOverflow,
}

#[derive(Clone, Copy)]
struct DemandState {
    id: WifiEgressDemandId,
    level: WifiEgressDemandLevel,
}

#[derive(Clone, Copy)]
struct QueueState<K> {
    key: K,
    vif: u8,
    demand: Option<DemandState>,
    deficit: i64,
    pending_airtime: u64,
    version: u32,
}

#[derive(Clone, Copy)]
struct VifState {
    schedule_epoch: Option<u32>,
    deficit: i64,
    pending_airtime: u64,
    queue_cursor: usize,
}

impl VifState {
    const EMPTY: Self = Self {
        schedule_epoch: None,
        deficit: 0,
        pending_airtime: 0,
        queue_cursor: 0,
    };
}

#[must_use = "a selection must be committed or explicitly cancelled"]
pub struct WifiEgressSelection<K> {
    serial: u32,
    slot: usize,
    slot_version: u32,
    demand: WifiEgressDemand<K>,
    opportunity: WifiEgressOpportunity,
}

impl<K> WifiEgressSelection<K> {
    pub const fn demand(&self) -> &WifiEgressDemand<K> {
        &self.demand
    }

    pub const fn opportunity(&self) -> WifiEgressOpportunity {
        self.opportunity
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiEgressAdmissionObservation {
    ExactRecommendation,
    DifferentQueue,
}

#[derive(Debug)]
#[must_use = "an admission must return its pending airtime at terminal completion"]
pub struct WifiEgressAdmission<K> {
    key: K,
    demand_id: WifiEgressDemandId,
    vif: u8,
    slot: usize,
    estimated_airtime: WifiAirtimeUnits,
}

impl<K> WifiEgressAdmission<K> {
    pub const fn key(&self) -> &K {
        &self.key
    }

    pub const fn demand_id(&self) -> WifiEgressDemandId {
        self.demand_id
    }

    pub const fn estimated_airtime(&self) -> WifiAirtimeUnits {
        self.estimated_airtime
    }
}

pub struct WifiEgressAirtimeScheduler<K: Copy + Eq, const VIFS: usize, const QUEUES: usize> {
    config: WifiEgressAirtimeConfig<VIFS>,
    vifs: [VifState; VIFS],
    queues: [Option<QueueState<K>>; QUEUES],
    vif_cursor: usize,
    global_pending_airtime: u64,
    selection_serial: u32,
    selection_outstanding: bool,
}

impl<K: Copy + Eq, const VIFS: usize, const QUEUES: usize>
    WifiEgressAirtimeScheduler<K, VIFS, QUEUES>
{
    pub const fn new(config: WifiEgressAirtimeConfig<VIFS>) -> Self {
        assert!(QUEUES != 0, "a Wi-Fi airtime policy must own queue state");
        Self {
            config,
            vifs: [VifState::EMPTY; VIFS],
            queues: [None; QUEUES],
            vif_cursor: 0,
            global_pending_airtime: 0,
            selection_serial: 0,
            selection_outstanding: false,
        }
    }

    pub fn reset_vif(
        &mut self,
        vif: u8,
        schedule_epoch: u32,
    ) -> Result<(), WifiEgressAirtimeError> {
        let vif_index = self.vif_index(vif)?;
        self.vifs[vif_index].schedule_epoch = Some(schedule_epoch);
        self.vifs[vif_index].deficit = 0;
        self.vifs[vif_index].queue_cursor = 0;
        for slot in 0..QUEUES {
            let Some(mut queue) = self.queues[slot] else {
                continue;
            };
            if usize::from(queue.vif) != vif_index {
                continue;
            }
            queue.demand = None;
            queue.deficit = 0;
            queue.version = next_version(queue.version);
            self.queues[slot] = (queue.pending_airtime != 0).then_some(queue);
        }
        Ok(())
    }

    pub fn upsert_demand(
        &mut self,
        demand: WifiEgressDemand<K>,
    ) -> Result<(), WifiEgressAirtimeError> {
        let vif = self.vif_index(demand.vif)?;
        let Some(schedule_epoch) = self.vifs[vif].schedule_epoch else {
            return Err(WifiEgressAirtimeError::EpochUnavailable);
        };
        if demand.id.schedule_epoch != schedule_epoch {
            return Err(WifiEgressAirtimeError::EpochMismatch);
        }

        if let Some(slot) = self.queue_slot(demand.vif, demand.key) {
            let mut queue = self.queues[slot].expect("located queue remains owned");
            if let Some(current) = queue.demand
                && current.id != demand.id
            {
                return Err(WifiEgressAirtimeError::LifetimeConflict);
            }
            if queue.demand.is_none() {
                queue.deficit = 0;
            }
            queue.demand = Some(DemandState {
                id: demand.id,
                level: demand.level,
            });
            queue.version = next_version(queue.version);
            self.queues[slot] = Some(queue);
            return Ok(());
        }

        let slot = self
            .queues
            .iter()
            .position(Option::is_none)
            .ok_or(WifiEgressAirtimeError::Full)?;
        self.queues[slot] = Some(QueueState {
            key: demand.key,
            vif: demand.vif,
            demand: Some(DemandState {
                id: demand.id,
                level: demand.level,
            }),
            deficit: 0,
            pending_airtime: 0,
            version: 1,
        });
        Ok(())
    }

    pub fn remove_demand(&mut self, demand: WifiEgressDemand<K>) {
        let Some(slot) = self.queue_slot(demand.vif, demand.key) else {
            return;
        };
        let mut queue = self.queues[slot].expect("located queue remains owned");
        if !queue.demand.is_some_and(|current| current.id == demand.id) {
            return;
        }
        queue.demand = None;
        queue.deficit = 0;
        queue.version = next_version(queue.version);
        self.queues[slot] = (queue.pending_airtime != 0).then_some(queue);
    }

    pub fn select_next(
        &mut self,
        mut opportunity_for: impl FnMut(WifiEgressDemand<K>) -> Option<WifiEgressOpportunity>,
    ) -> Result<Option<WifiEgressSelection<K>>, WifiEgressAirtimeError> {
        if self.selection_outstanding {
            return Err(WifiEgressAirtimeError::SelectionOutstanding);
        }

        let mut opportunities = [None; QUEUES];
        let mut eligible_vifs = [false; VIFS];
        for (slot, queue) in self.queues.iter().copied().enumerate() {
            let Some(queue) = queue else { continue };
            let Some(demand) = queue.demand else { continue };
            let demand = WifiEgressDemand::new(queue.vif, demand.id, queue.key, demand.level);
            let Some(opportunity) = opportunity_for(demand) else {
                continue;
            };
            if u16::from(opportunity.frame_limit.get()) > demand.level.ready_frames.get() {
                return Err(WifiEgressAirtimeError::OpportunityExceedsDemand);
            }
            let vif = usize::from(queue.vif);
            if !can_admit(
                self.global_pending_airtime,
                opportunity.estimated_airtime,
                self.config.global_pending_limit,
            ) || !can_admit(
                self.vifs[vif].pending_airtime,
                opportunity.estimated_airtime,
                self.config.vif_pending_limit[vif],
            ) || !can_admit(
                queue.pending_airtime,
                opportunity.estimated_airtime,
                opportunity.queue_pending_limit,
            ) {
                continue;
            }
            opportunities[slot] = Some(opportunity);
            eligible_vifs[vif] = true;
        }
        if !eligible_vifs.iter().any(|eligible| *eligible) {
            return Ok(None);
        }

        let vif = self.select_vif(&eligible_vifs);
        let slot = self.select_queue(&opportunities, vif);
        let opportunity = opportunities[slot].expect("selected queue retains an opportunity");
        let queue = self.queues[slot].expect("selected queue remains owned");
        let demand = queue.demand.expect("selected demand remains active");
        self.selection_serial = self
            .selection_serial
            .checked_add(1)
            .ok_or(WifiEgressAirtimeError::CounterOverflow)?;
        self.selection_outstanding = true;
        Ok(Some(WifiEgressSelection {
            serial: self.selection_serial,
            slot,
            slot_version: queue.version,
            demand: WifiEgressDemand::new(queue.vif, demand.id, queue.key, demand.level),
            opportunity,
        }))
    }

    pub fn cancel_selection(
        &mut self,
        selection: WifiEgressSelection<K>,
    ) -> Result<(), WifiEgressAirtimeError> {
        self.take_selection(&selection)?;
        Ok(())
    }

    pub fn commit_selected(
        &mut self,
        selection: WifiEgressSelection<K>,
    ) -> Result<WifiEgressAdmission<K>, WifiEgressAirtimeError> {
        let actual = selection.demand;
        let opportunity = selection.opportunity;
        self.observe_admission(selection, actual, opportunity)
            .map(|(_, admission)| admission)
    }

    pub fn observe_admission(
        &mut self,
        selection: WifiEgressSelection<K>,
        actual: WifiEgressDemand<K>,
        actual_opportunity: WifiEgressOpportunity,
    ) -> Result<(WifiEgressAdmissionObservation, WifiEgressAdmission<K>), WifiEgressAirtimeError>
    {
        self.take_selection(&selection)?;
        let actual_slot = self
            .queue_slot(actual.vif, actual.key)
            .ok_or(WifiEgressAirtimeError::ActualDemandUnavailable)?;
        let mut queue = self.queues[actual_slot].expect("located queue remains owned");
        if !queue.demand.is_some_and(|demand| demand.id == actual.id) {
            return Err(WifiEgressAirtimeError::ActualDemandUnavailable);
        }
        if u16::from(actual_opportunity.frame_limit.get()) > actual.level.ready_frames.get() {
            return Err(WifiEgressAirtimeError::OpportunityExceedsDemand);
        }
        let vif = self.vif_index(actual.vif)?;
        let cost = u64::from(actual_opportunity.estimated_airtime.hundred_nanoseconds());
        self.global_pending_airtime = self
            .global_pending_airtime
            .checked_add(cost)
            .ok_or(WifiEgressAirtimeError::CounterOverflow)?;
        self.vifs[vif].pending_airtime = self.vifs[vif]
            .pending_airtime
            .checked_add(cost)
            .ok_or(WifiEgressAirtimeError::CounterOverflow)?;
        queue.pending_airtime = queue
            .pending_airtime
            .checked_add(cost)
            .ok_or(WifiEgressAirtimeError::CounterOverflow)?;
        let signed_cost = i64::from(actual_opportunity.estimated_airtime.hundred_nanoseconds());
        self.vifs[vif].deficit = self.vifs[vif].deficit.saturating_sub(signed_cost);
        queue.deficit = queue.deficit.saturating_sub(signed_cost);
        if self.vifs[vif].deficit > 0 {
            self.vif_cursor = vif;
        } else {
            self.vif_cursor = (vif + 1) % VIFS;
        }
        if queue.deficit > 0 {
            self.vifs[vif].queue_cursor = actual_slot;
        } else {
            self.vifs[vif].queue_cursor = (actual_slot + 1) % QUEUES;
        }
        let admission = WifiEgressAdmission {
            key: actual.key,
            demand_id: actual.id,
            vif: actual.vif,
            slot: actual_slot,
            estimated_airtime: actual_opportunity.estimated_airtime,
        };
        self.queues[actual_slot] = Some(queue);
        let observation = if selection.demand.vif == actual.vif
            && selection.demand.id == actual.id
            && selection.demand.key == actual.key
        {
            WifiEgressAdmissionObservation::ExactRecommendation
        } else {
            WifiEgressAdmissionObservation::DifferentQueue
        };
        Ok((observation, admission))
    }

    pub fn reconcile_modeled_airtime(
        &mut self,
        admission: WifiEgressAdmission<K>,
        modeled_airtime: WifiAirtimeUnits,
    ) -> Result<(), WifiEgressAirtimeError> {
        let estimated = u64::from(admission.estimated_airtime.hundred_nanoseconds());
        self.global_pending_airtime = self
            .global_pending_airtime
            .checked_sub(estimated)
            .ok_or(WifiEgressAirtimeError::CounterOverflow)?;
        let vif = self.vif_index(admission.vif)?;
        self.vifs[vif].pending_airtime = self.vifs[vif]
            .pending_airtime
            .checked_sub(estimated)
            .ok_or(WifiEgressAirtimeError::CounterOverflow)?;

        let Some(mut queue) = self.queues.get(admission.slot).copied().flatten() else {
            return Ok(());
        };
        // An inactive queue with outstanding airtime remains as a tombstone,
        // so its slot cannot be reused for another key before every affine
        // admission receipt is reconciled. Demand-version changes invalidate
        // recommendations, but must not invalidate terminal credit return.
        if queue.key != admission.key {
            return Ok(());
        }
        queue.pending_airtime = queue
            .pending_airtime
            .checked_sub(estimated)
            .ok_or(WifiEgressAirtimeError::CounterOverflow)?;
        let correction = i64::from(modeled_airtime.hundred_nanoseconds())
            - i64::from(admission.estimated_airtime.hundred_nanoseconds());
        if self.vifs[vif].schedule_epoch == Some(admission.demand_id.schedule_epoch) {
            self.vifs[vif].deficit = self.vifs[vif].deficit.saturating_sub(correction);
            queue.deficit = queue.deficit.saturating_sub(correction);
        }
        self.queues[admission.slot] =
            (queue.demand.is_some() || queue.pending_airtime != 0).then_some(queue);
        Ok(())
    }

    pub fn global_pending_airtime(&self) -> u64 {
        self.global_pending_airtime
    }

    fn vif_index(&self, vif: u8) -> Result<usize, WifiEgressAirtimeError> {
        let vif = usize::from(vif);
        (vif < VIFS)
            .then_some(vif)
            .ok_or(WifiEgressAirtimeError::VifOutOfRange)
    }

    fn queue_slot(&self, vif: u8, key: K) -> Option<usize> {
        self.queues
            .iter()
            .position(|queue| queue.is_some_and(|queue| queue.vif == vif && queue.key == key))
    }

    fn take_selection(
        &mut self,
        selection: &WifiEgressSelection<K>,
    ) -> Result<(), WifiEgressAirtimeError> {
        if !self.selection_outstanding || selection.serial != self.selection_serial {
            return Err(WifiEgressAirtimeError::SelectionUnavailable);
        }
        self.selection_outstanding = false;
        let Some(queue) = self.queues.get(selection.slot).copied().flatten() else {
            return Err(WifiEgressAirtimeError::StaleSelection);
        };
        if queue.version != selection.slot_version
            || queue.key != selection.demand.key
            || !queue
                .demand
                .is_some_and(|demand| demand.id == selection.demand.id)
        {
            return Err(WifiEgressAirtimeError::StaleSelection);
        }
        Ok(())
    }

    fn select_vif(&mut self, eligible_vifs: &[bool; VIFS]) -> usize {
        let eligible_count = eligible_vifs.iter().filter(|eligible| **eligible).count();
        debug_assert_ne!(eligible_count, 0);

        let mut first_eligible = None;
        for offset in 0..VIFS {
            let vif = (self.vif_cursor + offset) % VIFS;
            if !eligible_vifs[vif] {
                continue;
            }
            first_eligible.get_or_insert(vif);
            if self.vifs[vif].deficit <= 0 {
                self.vifs[vif].deficit = self.vifs[vif].deficit.saturating_add(i64::from(
                    self.config.vif_quantum[vif].hundred_nanoseconds(),
                ));
            }
            if self.vifs[vif].deficit > 0 || eligible_count == 1 {
                return vif;
            }
        }

        // A transaction larger than one quantum can leave every VIF in debt.
        // Such an empty DRR round must advance credit without making an active
        // radio idle. Add only the number of complete rounds needed for the
        // first VIF to become serviceable.
        let rounds = eligible_vifs
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, eligible)| *eligible)
            .map(|(vif, _)| {
                rounds_until_positive(
                    self.vifs[vif].deficit,
                    u64::from(self.config.vif_quantum[vif].hundred_nanoseconds()),
                )
            })
            .min()
            .expect("an eligible VIF exists");
        for (vif, eligible) in eligible_vifs.iter().copied().enumerate() {
            if eligible {
                self.vifs[vif].deficit = saturating_add_quantum(
                    self.vifs[vif].deficit,
                    u64::from(self.config.vif_quantum[vif].hundred_nanoseconds()),
                    rounds,
                );
            }
        }
        (0..VIFS)
            .map(|offset| (self.vif_cursor + offset) % VIFS)
            .find(|vif| eligible_vifs[*vif] && self.vifs[*vif].deficit > 0)
            .unwrap_or_else(|| first_eligible.expect("an eligible VIF exists"))
    }

    fn select_queue(
        &mut self,
        opportunities: &[Option<WifiEgressOpportunity>; QUEUES],
        vif: usize,
    ) -> usize {
        let eligible_count = opportunities
            .iter()
            .copied()
            .enumerate()
            .filter(|(slot, opportunity)| {
                opportunity.is_some()
                    && self.queues[*slot].is_some_and(|queue| usize::from(queue.vif) == vif)
            })
            .count();
        debug_assert_ne!(eligible_count, 0);

        let cursor = self.vifs[vif].queue_cursor;
        let mut first_eligible = None;
        for offset in 0..QUEUES {
            let slot = (cursor + offset) % QUEUES;
            let Some(opportunity) = opportunities[slot] else {
                continue;
            };
            let Some(queue) = self.queues[slot].as_mut() else {
                continue;
            };
            if usize::from(queue.vif) != vif {
                continue;
            }
            first_eligible.get_or_insert(slot);
            if queue.deficit <= 0 {
                queue.deficit = queue.deficit.saturating_add(weighted_queue_quantum(
                    self.config.queue_quantum,
                    opportunity,
                ));
            }
            if queue.deficit > 0 || eligible_count == 1 {
                return slot;
            }
        }

        let rounds = opportunities
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(slot, opportunity)| {
                let opportunity = opportunity?;
                let queue = self.queues[slot]?;
                (usize::from(queue.vif) == vif).then(|| {
                    rounds_until_positive(
                        queue.deficit,
                        u64::try_from(weighted_queue_quantum(
                            self.config.queue_quantum,
                            opportunity,
                        ))
                        .unwrap_or(u64::MAX),
                    )
                })
            })
            .min()
            .expect("an eligible queue exists");
        for (slot, opportunity) in opportunities.iter().copied().enumerate() {
            let Some(opportunity) = opportunity else {
                continue;
            };
            let Some(queue) = self.queues[slot].as_mut() else {
                continue;
            };
            if usize::from(queue.vif) == vif {
                queue.deficit = saturating_add_quantum(
                    queue.deficit,
                    u64::try_from(weighted_queue_quantum(
                        self.config.queue_quantum,
                        opportunity,
                    ))
                    .unwrap_or(u64::MAX),
                    rounds,
                );
            }
        }
        (0..QUEUES)
            .map(|offset| (cursor + offset) % QUEUES)
            .find(|slot| {
                opportunities[*slot].is_some()
                    && self.queues[*slot]
                        .is_some_and(|queue| usize::from(queue.vif) == vif && queue.deficit > 0)
            })
            .unwrap_or_else(|| first_eligible.expect("an eligible queue exists"))
    }
}

fn next_version(current: u32) -> u32 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

fn can_admit(pending: u64, cost: WifiAirtimeUnits, limit: WifiAirtimeUnits) -> bool {
    pending == 0
        || pending.saturating_add(u64::from(cost.hundred_nanoseconds()))
            <= u64::from(limit.hundred_nanoseconds())
}

fn weighted_queue_quantum(base: WifiAirtimeUnits, opportunity: WifiEgressOpportunity) -> i64 {
    i64::from(base.hundred_nanoseconds()) * i64::from(opportunity.queue_weight.get())
}

fn saturating_add_quantum(current: i64, quantum: u64, rounds: u64) -> i64 {
    let addition = quantum.saturating_mul(rounds).min(i64::MAX as u64) as i64;
    current.saturating_add(addition)
}

fn rounds_until_positive(deficit: i64, quantum: u64) -> u64 {
    if deficit > 0 {
        return 0;
    }
    let debt = deficit.unsigned_abs();
    debt.saturating_add(1).saturating_add(quantum - 1) / quantum
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    fn airtime(value: u32) -> WifiAirtimeUnits {
        WifiAirtimeUnits::new(NonZeroU32::new(value).unwrap())
    }

    fn config<const VIFS: usize>() -> WifiEgressAirtimeConfig<VIFS> {
        WifiEgressAirtimeConfig::new(
            [airtime(40_000); VIFS],
            airtime(40_000),
            airtime(120_000),
            [airtime(120_000); VIFS],
        )
    }

    fn demand(vif: u8, epoch: u32, activation: u32, key: u8, ready: u16) -> WifiEgressDemand<u8> {
        WifiEgressDemand::new(
            vif,
            WifiEgressDemandId::new(epoch, NonZeroU32::new(activation).unwrap()),
            key,
            WifiEgressDemandLevel::new(NonZeroU16::new(ready).unwrap(), ready >= 32),
        )
    }

    fn opportunity(cost: u32) -> WifiEgressOpportunity {
        WifiEgressOpportunity::new(
            NonZeroU8::new(32).unwrap(),
            airtime(cost),
            airtime(80_000),
            NonZeroU8::new(1).unwrap(),
        )
    }

    fn serve<const VIFS: usize, const QUEUES: usize>(
        scheduler: &mut WifiEgressAirtimeScheduler<u8, VIFS, QUEUES>,
        cost_for: impl Fn(u8) -> u32,
    ) -> u8 {
        let selected = scheduler
            .select_next(|demand| Some(opportunity(cost_for(*demand.key()))))
            .unwrap()
            .unwrap();
        let key = *selected.demand().key();
        let cost = selected.opportunity().estimated_airtime();
        let admission = scheduler.commit_selected(selected).unwrap();
        scheduler
            .reconcile_modeled_airtime(admission, cost)
            .unwrap();
        key
    }

    #[test]
    fn sparse_nonaggregate_demand_is_selected_immediately() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 1, 4>::new(config());
        scheduler.reset_vif(0, 7).unwrap();
        scheduler.upsert_demand(demand(0, 7, 1, 9, 1)).unwrap();

        let selected = scheduler
            .select_next(|demand| {
                assert!(!demand.level().aggregate_ready());
                Some(WifiEgressOpportunity::new(
                    NonZeroU8::new(1).unwrap(),
                    airtime(800),
                    airtime(80_000),
                    NonZeroU8::new(1).unwrap(),
                ))
            })
            .unwrap()
            .unwrap();

        assert_eq!(*selected.demand().key(), 9);
        assert_eq!(selected.opportunity().frame_limit().get(), 1);
    }

    #[test]
    fn hierarchy_fairs_one_sta_vif_against_two_ap_peers_by_airtime() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 2, 6>::new(config());
        scheduler.reset_vif(0, 1).unwrap();
        scheduler.reset_vif(1, 1).unwrap();
        scheduler.upsert_demand(demand(0, 1, 1, 1, 32)).unwrap();
        scheduler.upsert_demand(demand(1, 1, 2, 2, 32)).unwrap();
        scheduler.upsert_demand(demand(1, 1, 3, 3, 32)).unwrap();

        let mut vif_airtime = [0_u64; 2];
        let mut queue_airtime = [0_u64; 4];
        for _ in 0..120 {
            let key = serve(&mut scheduler, |_| 20_000);
            let vif = usize::from(key != 1);
            vif_airtime[vif] += 20_000;
            queue_airtime[usize::from(key)] += 20_000;
        }

        assert_eq!(vif_airtime, [1_200_000, 1_200_000]);
        assert_eq!(queue_airtime[2], queue_airtime[3]);
    }

    #[test]
    fn slow_peer_gets_fewer_frames_but_equal_airtime() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 1, 4>::new(config());
        scheduler.reset_vif(0, 1).unwrap();
        scheduler.upsert_demand(demand(0, 1, 1, 1, 32)).unwrap();
        scheduler.upsert_demand(demand(0, 1, 2, 2, 32)).unwrap();

        let mut frames = [0_u32; 3];
        let mut used = [0_u64; 3];
        for _ in 0..120 {
            let key = serve(&mut scheduler, |key| if key == 1 { 10_000 } else { 40_000 });
            let cost = if key == 1 { 10_000 } else { 40_000 };
            frames[usize::from(key)] += 1;
            used[usize::from(key)] += cost;
        }

        assert!(frames[1] > frames[2] * 3);
        assert!(used[1].abs_diff(used[2]) <= 40_000);
    }

    #[test]
    fn aql_pending_blocks_refill_without_blocking_another_queue() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 1, 4>::new(config());
        scheduler.reset_vif(0, 1).unwrap();
        scheduler.upsert_demand(demand(0, 1, 1, 1, 32)).unwrap();
        scheduler.upsert_demand(demand(0, 1, 2, 2, 32)).unwrap();

        let first = scheduler
            .select_next(|_| Some(opportunity(50_000)))
            .unwrap()
            .unwrap();
        let first_key = *first.demand().key();
        let first_admission = scheduler.commit_selected(first).unwrap();
        let second = scheduler
            .select_next(|_| Some(opportunity(50_000)))
            .unwrap()
            .unwrap();
        assert_ne!(*second.demand().key(), first_key);
        scheduler.cancel_selection(second).unwrap();
        scheduler
            .reconcile_modeled_airtime(first_admission, airtime(50_000))
            .unwrap();
        assert_eq!(scheduler.global_pending_airtime(), 0);
    }

    #[test]
    fn inactive_queue_retains_pending_credit_until_terminal_reconciliation() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 1, 1>::new(config());
        scheduler.reset_vif(0, 3).unwrap();
        let active = demand(0, 3, 1, 7, 32);
        scheduler.upsert_demand(active).unwrap();
        let selected = scheduler
            .select_next(|_| Some(opportunity(20_000)))
            .unwrap()
            .unwrap();
        let admission = scheduler.commit_selected(selected).unwrap();
        scheduler.remove_demand(active);

        assert_eq!(
            scheduler.upsert_demand(demand(0, 3, 2, 8, 1)),
            Err(WifiEgressAirtimeError::Full)
        );
        scheduler
            .reconcile_modeled_airtime(admission, airtime(30_000))
            .unwrap();
        scheduler.upsert_demand(demand(0, 3, 2, 8, 1)).unwrap();
    }

    #[test]
    fn stale_lifetime_cannot_spend_a_new_activation() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 1, 2>::new(config());
        scheduler.reset_vif(0, 1).unwrap();
        let old = demand(0, 1, 1, 4, 32);
        scheduler.upsert_demand(old).unwrap();
        let selected = scheduler
            .select_next(|_| Some(opportunity(10_000)))
            .unwrap()
            .unwrap();
        scheduler.remove_demand(old);
        scheduler.upsert_demand(demand(0, 1, 2, 4, 32)).unwrap();

        assert_eq!(
            scheduler.commit_selected(selected).unwrap_err(),
            WifiEgressAirtimeError::StaleSelection
        );
    }

    #[test]
    fn one_selection_must_finish_before_another_is_issued() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 1, 2>::new(config());
        scheduler.reset_vif(0, 1).unwrap();
        scheduler.upsert_demand(demand(0, 1, 1, 4, 32)).unwrap();
        let selected = scheduler
            .select_next(|_| Some(opportunity(10_000)))
            .unwrap()
            .unwrap();

        assert!(matches!(
            scheduler.select_next(|_| Some(opportunity(10_000))),
            Err(WifiEgressAirtimeError::SelectionOutstanding)
        ));
        scheduler.cancel_selection(selected).unwrap();
        assert!(
            scheduler
                .select_next(|_| Some(opportunity(10_000)))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn opportunity_cost_and_frame_limit_remain_one_indivisible_profile() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 1, 2>::new(config());
        scheduler.reset_vif(0, 1).unwrap();
        scheduler.upsert_demand(demand(0, 1, 1, 4, 1)).unwrap();

        assert!(matches!(
            scheduler.select_next(|_| Some(opportunity(10_000))),
            Err(WifiEgressAirtimeError::OpportunityExceedsDemand)
        ));
    }

    #[test]
    fn shadow_observation_charges_the_actual_queue_not_the_recommendation() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 1, 2>::new(config());
        scheduler.reset_vif(0, 1).unwrap();
        let first = demand(0, 1, 1, 4, 32);
        let second = demand(0, 1, 2, 5, 32);
        scheduler.upsert_demand(first).unwrap();
        scheduler.upsert_demand(second).unwrap();
        let selected = scheduler
            .select_next(|_| Some(opportunity(20_000)))
            .unwrap()
            .unwrap();
        let actual = if selected.demand().key() == first.key() {
            second
        } else {
            first
        };

        let (observation, admission) = scheduler
            .observe_admission(selected, actual, opportunity(30_000))
            .unwrap();
        assert_eq!(observation, WifiEgressAdmissionObservation::DifferentQueue);
        assert_eq!(admission.key(), actual.key());
        assert_eq!(scheduler.global_pending_airtime(), 30_000);
        scheduler
            .reconcile_modeled_airtime(admission, airtime(35_000))
            .unwrap();
        assert_eq!(scheduler.global_pending_airtime(), 0);
    }

    #[test]
    fn queue_weight_changes_airtime_share_without_changing_cost_units() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 1, 2>::new(config());
        scheduler.reset_vif(0, 1).unwrap();
        scheduler.upsert_demand(demand(0, 1, 1, 1, 32)).unwrap();
        scheduler.upsert_demand(demand(0, 1, 2, 2, 32)).unwrap();
        let mut frames = [0_u32; 3];
        for _ in 0..120 {
            let selected = scheduler
                .select_next(|demand| {
                    Some(WifiEgressOpportunity::new(
                        NonZeroU8::new(32).unwrap(),
                        airtime(20_000),
                        airtime(80_000),
                        NonZeroU8::new(if *demand.key() == 1 { 2 } else { 1 }).unwrap(),
                    ))
                })
                .unwrap()
                .unwrap();
            let key = *selected.demand().key();
            frames[usize::from(key)] += 1;
            let admission = scheduler.commit_selected(selected).unwrap();
            scheduler
                .reconcile_modeled_airtime(admission, airtime(20_000))
                .unwrap();
        }

        assert_eq!(frames[1], frames[2] * 2);
    }

    #[test]
    fn oversize_transactions_accumulate_debt_without_stalling() {
        let mut scheduler = WifiEgressAirtimeScheduler::<u8, 1, 2>::new(config());
        scheduler.reset_vif(0, 1).unwrap();
        scheduler.upsert_demand(demand(0, 1, 1, 1, 32)).unwrap();
        scheduler.upsert_demand(demand(0, 1, 2, 2, 32)).unwrap();
        let mut used = [0_u64; 3];
        for _ in 0..60 {
            let key = serve(&mut scheduler, |_| 70_000);
            used[usize::from(key)] += 70_000;
        }

        assert!(used[1].abs_diff(used[2]) <= 70_000);
    }

    #[test]
    fn dual_vif_policy_state_is_bounded_core0_metadata() {
        type PhysicalPolicy = WifiEgressAirtimeScheduler<[u32; 4], 2, 32>;

        assert!(core::mem::size_of::<PhysicalPolicy>() <= 4096);
        assert!(core::mem::size_of::<WifiEgressSelection<[u32; 4]>>() <= 64);
        assert!(core::mem::size_of::<WifiEgressAdmission<[u32; 4]>>() <= 48);
    }
}
