//! Lossless bounded synchronization of stack-owned egress demand.
//!
//! The stack callback is synchronous, while the radio owner runs on another
//! core and may temporarily stop consuming its SPSC queue. Retaining an
//! unbounded FIFO of lifecycle events would therefore be the wrong resource
//! model. The network side instead keeps the latest desired key set and the
//! state already published into the ordered stream. It can always reconstruct
//! the minimal `Reset`, `Inactive`, and `Active` suffix after capacity returns.

use embassy_net_driver::{EgressDemand, EgressDemandUpdate, EgressKey};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EgressDemandStateError {
    EpochUnavailable,
    EpochMismatch,
    Full,
}

struct EgressDemandSet<const CAPACITY: usize> {
    schedule_epoch: Option<u32>,
    entries: [Option<EgressDemand>; CAPACITY],
}

impl<const CAPACITY: usize> EgressDemandSet<CAPACITY> {
    const fn new() -> Self {
        assert!(CAPACITY != 0, "egress demand state must not be empty");
        Self {
            schedule_epoch: None,
            entries: [None; CAPACITY],
        }
    }

    fn reset(&mut self, schedule_epoch: u32) {
        self.schedule_epoch = Some(schedule_epoch);
        self.entries.fill(None);
    }

    fn active_slot(&self, key: EgressKey) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.key() == key))
    }

    fn exact_slot(&self, demand: EgressDemand) -> Option<usize> {
        self.entries.iter().position(|entry| {
            entry.is_some_and(|entry| entry.key() == demand.key() && entry.id() == demand.id())
        })
    }

    fn record_active(&mut self, demand: EgressDemand) -> Result<(), EgressDemandStateError> {
        let Some(schedule_epoch) = self.schedule_epoch else {
            return Err(EgressDemandStateError::EpochUnavailable);
        };
        if demand.id().schedule_epoch() != schedule_epoch {
            return Err(EgressDemandStateError::EpochMismatch);
        }
        let slot = self
            .active_slot(demand.key())
            .or_else(|| self.entries.iter().position(Option::is_none))
            .ok_or(EgressDemandStateError::Full)?;
        self.entries[slot] = Some(demand);
        Ok(())
    }

    fn record_inactive(&mut self, key: EgressKey, id: embassy_net_driver::EgressDemandId) {
        if let Some(slot) = self
            .entries
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.key() == key && entry.id() == id))
        {
            self.entries[slot] = None;
        }
    }

    fn apply(&mut self, update: EgressDemandUpdate) -> Result<(), EgressDemandStateError> {
        match update {
            EgressDemandUpdate::Reset { schedule_epoch } => {
                self.reset(schedule_epoch);
                Ok(())
            }
            EgressDemandUpdate::Active(demand) => self.record_active(demand),
            EgressDemandUpdate::Inactive { id, key } => {
                self.record_inactive(key, id);
                Ok(())
            }
        }
    }
}

/// Core1 state which coalesces arbitrary lifecycle churn into a lossless
/// bounded stream suffix.
pub(crate) struct EgressDemandOutbox<const CAPACITY: usize> {
    desired: EgressDemandSet<CAPACITY>,
    sent: EgressDemandSet<CAPACITY>,
}

impl<const CAPACITY: usize> EgressDemandOutbox<CAPACITY> {
    pub(crate) const fn new() -> Self {
        Self {
            desired: EgressDemandSet::new(),
            sent: EgressDemandSet::new(),
        }
    }

    pub(crate) fn record(
        &mut self,
        update: EgressDemandUpdate,
    ) -> Result<(), EgressDemandStateError> {
        self.desired.apply(update)
    }

    /// Return the next transition required to make `sent == desired`.
    ///
    /// Call [`Self::commit`] only after the transition entered the affine
    /// transport. A full queue therefore changes no synchronization state.
    pub(crate) fn next(&self) -> Option<EgressDemandUpdate> {
        if self.desired.schedule_epoch != self.sent.schedule_epoch {
            return self
                .desired
                .schedule_epoch
                .map(|schedule_epoch| EgressDemandUpdate::Reset { schedule_epoch });
        }

        for sent in self.sent.entries.iter().flatten().copied() {
            match self.desired.exact_slot(sent) {
                None => {
                    return Some(EgressDemandUpdate::Inactive {
                        id: sent.id(),
                        key: sent.key(),
                    });
                }
                Some(slot) => {
                    let desired = self.desired.entries[slot].expect("exact demand remains live");
                    if desired.level() != sent.level() {
                        return Some(EgressDemandUpdate::Active(desired));
                    }
                }
            }
        }

        self.desired
            .entries
            .iter()
            .flatten()
            .copied()
            .find(|demand| self.sent.exact_slot(*demand).is_none())
            .map(EgressDemandUpdate::Active)
    }

    pub(crate) fn commit(&mut self, update: EgressDemandUpdate) {
        self.sent
            .apply(update)
            .expect("an outbox transition is valid for its sent frontier");
    }

    #[cfg(test)]
    fn synchronized(&self) -> bool {
        self.next().is_none()
    }
}

/// Core0 mirror of all demand transitions already consumed from one
/// interface stream.
pub(crate) struct EgressRadioDemandState<const CAPACITY: usize> {
    active: EgressDemandSet<CAPACITY>,
}

impl<const CAPACITY: usize> EgressRadioDemandState<CAPACITY> {
    pub(crate) const fn new() -> Self {
        Self {
            active: EgressDemandSet::new(),
        }
    }

    pub(crate) fn apply(
        &mut self,
        update: EgressDemandUpdate,
    ) -> Result<(), EgressDemandStateError> {
        self.active.apply(update)
    }

    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        self.active.entries.iter().flatten().count()
    }

    #[cfg(test)]
    pub(crate) fn demand_for(&self, key: EgressKey) -> Option<EgressDemand> {
        self.active
            .active_slot(key)
            .and_then(|slot| self.active.entries[slot])
    }
}

#[cfg(test)]
mod tests {
    use core::num::{NonZeroU16, NonZeroU32};

    use embassy_net_driver::{EgressDemandId, EgressDemandLevel};

    use super::*;

    fn key(value: u32) -> EgressKey {
        EgressKey::from_words([value, 0, 0, 0])
    }

    fn demand(epoch: u32, activation: u32, key: EgressKey, ready: u16) -> EgressDemand {
        EgressDemand::new(
            EgressDemandId::new(epoch, NonZeroU32::new(activation).unwrap()),
            key,
            EgressDemandLevel::new(NonZeroU16::new(ready).unwrap(), ready >= 32),
        )
    }

    fn transfer_one<const CAPACITY: usize>(
        outbox: &mut EgressDemandOutbox<CAPACITY>,
        radio: &mut EgressRadioDemandState<CAPACITY>,
    ) -> bool {
        let Some(update) = outbox.next() else {
            return false;
        };
        outbox.commit(update);
        radio.apply(update).unwrap();
        true
    }

    #[test]
    fn initial_snapshot_is_reset_then_active_demands() {
        let mut outbox = EgressDemandOutbox::<2>::new();
        let mut radio = EgressRadioDemandState::<2>::new();
        outbox
            .record(EgressDemandUpdate::Reset { schedule_epoch: 7 })
            .unwrap();
        outbox
            .record(EgressDemandUpdate::Active(demand(7, 1, key(1), 1)))
            .unwrap();
        outbox
            .record(EgressDemandUpdate::Active(demand(7, 2, key(2), 32)))
            .unwrap();

        assert!(matches!(
            outbox.next(),
            Some(EgressDemandUpdate::Reset { .. })
        ));
        while transfer_one(&mut outbox, &mut radio) {}
        assert!(outbox.synchronized());
        assert_eq!(radio.active_count(), 2);
        assert_eq!(
            radio
                .demand_for(key(2))
                .unwrap()
                .level()
                .ready_units()
                .get(),
            32
        );
    }

    #[test]
    fn queue_stall_coalesces_levels_without_losing_terminal_identity() {
        let mut outbox = EgressDemandOutbox::<1>::new();
        let mut radio = EgressRadioDemandState::<1>::new();
        outbox
            .record(EgressDemandUpdate::Reset { schedule_epoch: 9 })
            .unwrap();
        outbox
            .record(EgressDemandUpdate::Active(demand(9, 1, key(1), 1)))
            .unwrap();
        transfer_one(&mut outbox, &mut radio);
        transfer_one(&mut outbox, &mut radio);

        // Core0 now stalls. Intermediate queue levels and an entire second
        // lifetime never need their own retained FIFO entries.
        outbox
            .record(EgressDemandUpdate::Active(demand(9, 1, key(1), 32)))
            .unwrap();
        outbox
            .record(EgressDemandUpdate::Inactive {
                id: demand(9, 1, key(1), 32).id(),
                key: key(1),
            })
            .unwrap();
        outbox
            .record(EgressDemandUpdate::Active(demand(9, 2, key(1), 4)))
            .unwrap();

        assert!(matches!(
            outbox.next(),
            Some(EgressDemandUpdate::Inactive { .. })
        ));
        while transfer_one(&mut outbox, &mut radio) {}
        assert!(outbox.synchronized());
        assert_eq!(radio.active_count(), 1);
        assert_eq!(radio.demand_for(key(1)).unwrap().id().activation().get(), 2);
        assert_eq!(
            radio
                .demand_for(key(1))
                .unwrap()
                .level()
                .ready_units()
                .get(),
            4
        );
    }

    #[test]
    fn newer_reset_supersedes_every_unsent_old_epoch_transition() {
        let mut outbox = EgressDemandOutbox::<1>::new();
        let mut radio = EgressRadioDemandState::<1>::new();
        outbox
            .record(EgressDemandUpdate::Reset { schedule_epoch: 1 })
            .unwrap();
        outbox
            .record(EgressDemandUpdate::Active(demand(1, 1, key(1), 8)))
            .unwrap();
        outbox
            .record(EgressDemandUpdate::Reset { schedule_epoch: 2 })
            .unwrap();
        outbox
            .record(EgressDemandUpdate::Active(demand(2, 2, key(2), 3)))
            .unwrap();

        while transfer_one(&mut outbox, &mut radio) {}
        assert!(radio.demand_for(key(1)).is_none());
        assert_eq!(radio.demand_for(key(2)).unwrap().id().schedule_epoch(), 2);
    }

    #[test]
    fn distinct_key_capacity_fails_closed() {
        let mut outbox = EgressDemandOutbox::<1>::new();
        outbox
            .record(EgressDemandUpdate::Reset { schedule_epoch: 1 })
            .unwrap();
        outbox
            .record(EgressDemandUpdate::Active(demand(1, 1, key(1), 1)))
            .unwrap();
        assert_eq!(
            outbox.record(EgressDemandUpdate::Active(demand(1, 2, key(2), 1))),
            Err(EgressDemandStateError::Full)
        );
    }
}
