//! Pure shared-modem-clock ownership planner for ESP32-S31.
//!
//! The reviewed IEEE 802.15.4 dependency set has seven logical dependencies.
//! Both acquisition and release visit them from the lowest vendor dependency
//! bit to the highest. A physical acquire edge is emitted only for a software
//! refcount transition from zero to one; a physical release edge is emitted
//! only for a transition from one to zero.
//!
//! This module deliberately performs no MMIO and issues no PLL, clock, PHY, or
//! radio-readiness proof. In particular, recording a physical edge as complete
//! is only transaction bookkeeping for a future sealed target executor. It is
//! not hardware evidence by itself.
//!
//! The current production baseline is externally retained and unknown: ESP-HAL
//! owns the upstream 160 MHz clock policy and existing Wi-Fi initialization may
//! already have enabled shared dependencies. Such a planner rejects acquisition
//! and release planning. The managed constructor remains test-only until every
//! radio client and the upstream PLL owner migrate to one common manager.

#![forbid(unsafe_code)]
#![allow(
    dead_code,
    reason = "the isolated planner is wired in a later iteration"
)]

use core::{fmt, ptr};

const DEPENDENCY_COUNT: usize = 7;
/// Maximum count representable by the reviewed vendor `int16_t` contract.
const MAX_REFCOUNT: u16 = i16::MAX as u16;

/// Internal allocation-free capacity, not a vendor hardware or ABI limit.
const MAX_ACTIVE_LEASES: usize = 16;

/// Exact private low-bit-first dependency identities.
///
/// Discriminants are internal planner indices, not public register masks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum Dependency {
    Pll160AndModemSource = 0,
    Coexistence = 1,
    WifiBb80x1 = 2,
    Etm = 3,
    BtApbAndSecurity = 4,
    BtIeee802154CommonBaseband = 5,
    Ieee802154ApbAndMac = 6,
}

impl Dependency {
    const LOW_BIT_FIRST: [Self; DEPENDENCY_COUNT] = [
        Self::Pll160AndModemSource,
        Self::Coexistence,
        Self::WifiBb80x1,
        Self::Etm,
        Self::BtApbAndSecurity,
        Self::BtIeee802154CommonBaseband,
        Self::Ieee802154ApbAndMac,
    ];

    const fn index(self) -> usize {
        self as usize
    }

    const fn bit(self) -> u8 {
        1 << self.index()
    }

    const fn acquire_edge(self) -> ModemClockAcquireEdge {
        match self {
            Self::Pll160AndModemSource => ModemClockAcquireEdge::Pll160AndModemSource,
            Self::Coexistence => ModemClockAcquireEdge::Coexistence,
            Self::WifiBb80x1 => ModemClockAcquireEdge::WifiBb80x1,
            Self::Etm => ModemClockAcquireEdge::Etm,
            Self::BtApbAndSecurity => ModemClockAcquireEdge::BtApbAndSecurity,
            Self::BtIeee802154CommonBaseband => ModemClockAcquireEdge::BtIeee802154CommonBaseband,
            Self::Ieee802154ApbAndMac => ModemClockAcquireEdge::Ieee802154ApbAndMac,
        }
    }

    const fn release_edge(self) -> ModemClockReleaseEdge {
        match self {
            Self::Pll160AndModemSource => ModemClockReleaseEdge::Pll160AndModemSource,
            Self::Coexistence => ModemClockReleaseEdge::Coexistence,
            Self::WifiBb80x1 => ModemClockReleaseEdge::WifiBb80x1,
            Self::Etm => ModemClockReleaseEdge::Etm,
            Self::BtApbAndSecurity => ModemClockReleaseEdge::BtApbAndSecurity,
            Self::BtIeee802154CommonBaseband => ModemClockReleaseEdge::BtIeee802154CommonBaseband,
            Self::Ieee802154ApbAndMac => ModemClockReleaseEdge::Ieee802154ApbAndMac,
        }
    }
}

/// Private dependency membership. No raw mask crosses the module boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DependencySet(u8);

impl DependencySet {
    const IEEE802154: Self = Self((1 << DEPENDENCY_COUNT) - 1);

    const fn contains(self, dependency: Dependency) -> bool {
        self.0 & dependency.bit() != 0
    }

    #[cfg(test)]
    fn from_dependencies(dependencies: &[Dependency]) -> Self {
        let mut mask = 0;
        for dependency in dependencies {
            mask |= dependency.bit();
        }
        Self(mask)
    }
}

/// One source-backed physical enable operation.
///
/// The variants are semantic operations rather than register images. The first
/// variant includes acquiring the upstream 160 MHz source and publishing the
/// reviewed modem-source configuration; this planner does not perform either.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModemClockAcquireEdge {
    Pll160AndModemSource,
    Coexistence,
    WifiBb80x1,
    Etm,
    BtApbAndSecurity,
    BtIeee802154CommonBaseband,
    Ieee802154ApbAndMac,
}

/// One source-backed physical disable operation.
///
/// Release deliberately uses the same low-bit-first order as acquisition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModemClockReleaseEdge {
    Pll160AndModemSource,
    Coexistence,
    WifiBb80x1,
    Etm,
    BtApbAndSecurity,
    BtIeee802154CommonBaseband,
    Ieee802154ApbAndMac,
}

/// Stable, non-zero-sized identity borrowed by one planner epoch.
///
/// A mutable borrow constructs the planner and remains live as long as any
/// lease from that epoch exists. Distinct live identities therefore have
/// distinct addresses without a global counter or unsafe code.
/// This is only a pure-model identity; it is not bound to a
/// `RegisteredPhyRadio` or any target hardware epoch.
pub(crate) struct ModemClockPlannerIdentity {
    _occupied: u8,
}

impl ModemClockPlannerIdentity {
    pub(crate) const fn new() -> Self {
        Self { _occupied: 0 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Baseline {
    ExternallyRetained,
    Managed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LeaseSlot {
    generation: u64,
    active: bool,
    dependencies: DependencySet,
}

impl LeaseSlot {
    const EMPTY: Self = Self {
        generation: 0,
        active: false,
        dependencies: DependencySet(0),
    };
}

/// Unique software owner of one shared-clock accounting epoch.
///
/// This type is intentionally neither `Clone` nor `Copy`. Counts and slots have
/// no public accessors or mutation API.
#[must_use = "dropping the planner abandons its complete software ownership epoch"]
pub(crate) struct ModemClockPlanner<'identity> {
    identity: &'identity ModemClockPlannerIdentity,
    baseline: Baseline,
    counts: [u16; DEPENDENCY_COUNT],
    slots: [LeaseSlot; MAX_ACTIVE_LEASES],
}

impl<'identity> ModemClockPlanner<'identity> {
    /// Adopt the only currently honest production baseline.
    ///
    /// Unknown externally retained counts cannot produce a managed plan.
    pub(crate) fn externally_retained(identity: &'identity mut ModemClockPlannerIdentity) -> Self {
        Self {
            identity,
            baseline: Baseline::ExternallyRetained,
            counts: [0; DEPENDENCY_COUNT],
            slots: [LeaseSlot::EMPTY; MAX_ACTIVE_LEASES],
        }
    }

    /// Construct a zero-client managed epoch only in unit tests.
    ///
    /// Production must not gain an equivalent constructor until all clients and
    /// the upstream PLL ownership contract migrate together.
    #[cfg(test)]
    fn managed_for_test(identity: &'identity mut ModemClockPlannerIdentity) -> Self {
        Self {
            identity,
            baseline: Baseline::Managed,
            counts: [0; DEPENDENCY_COUNT],
            slots: [LeaseSlot::EMPTY; MAX_ACTIVE_LEASES],
        }
    }

    /// Prepare acquisition of the exact reviewed IEEE 802.15.4 dependency set.
    ///
    /// Preparation is transactional: counts and lease slots remain unchanged
    /// until all emitted physical edges are acknowledged and commit is called.
    #[allow(
        clippy::result_large_err,
        reason = "allocation-free failure retains the exact planner owner"
    )]
    pub(crate) fn prepare_ieee802154_acquire(
        self,
    ) -> Result<PreparedModemClockAcquire<'identity>, ModemClockAcquirePreparationFailure<'identity>>
    {
        self.prepare_acquire(DependencySet::IEEE802154)
    }

    #[allow(
        clippy::result_large_err,
        reason = "allocation-free failure retains the exact planner owner"
    )]
    fn prepare_acquire(
        self,
        dependencies: DependencySet,
    ) -> Result<PreparedModemClockAcquire<'identity>, ModemClockAcquirePreparationFailure<'identity>>
    {
        if self.baseline != Baseline::Managed {
            return Err(ModemClockAcquirePreparationFailure {
                planner: self,
                error: ModemClockAcquirePreparationError::UnknownBaseline,
            });
        }

        for dependency in Dependency::LOW_BIT_FIRST {
            if dependencies.contains(dependency) {
                let Some(next_count) = self.counts[dependency.index()].checked_add(1) else {
                    return Err(ModemClockAcquirePreparationFailure {
                        planner: self,
                        error: ModemClockAcquirePreparationError::RefcountOverflow(
                            dependency.acquire_edge(),
                        ),
                    });
                };
                if next_count > MAX_REFCOUNT {
                    return Err(ModemClockAcquirePreparationFailure {
                        planner: self,
                        error: ModemClockAcquirePreparationError::RefcountOverflow(
                            dependency.acquire_edge(),
                        ),
                    });
                }
            }
        }

        let Some(slot) = self.slots.iter().position(|slot| !slot.active) else {
            return Err(ModemClockAcquirePreparationFailure {
                planner: self,
                error: ModemClockAcquirePreparationError::LeaseCapacityReached,
            });
        };
        let Some(generation) = self.slots[slot].generation.checked_add(1) else {
            return Err(ModemClockAcquirePreparationFailure {
                planner: self,
                error: ModemClockAcquirePreparationError::LeaseGenerationExhausted,
            });
        };

        Ok(PreparedModemClockAcquire {
            planner: self,
            dependencies,
            slot: slot as u8,
            generation,
            next_dependency: 0,
            completed_edges: 0,
        })
    }

    /// Validate an opaque lease and prepare its complete release transaction.
    ///
    /// Every validation failure returns both unchanged opaque owners. An unknown
    /// baseline is rejected before any lease interpretation or count change.
    #[allow(
        clippy::result_large_err,
        reason = "allocation-free failure retains the exact planner and lease owners"
    )]
    pub(crate) fn prepare_release<'lease>(
        self,
        lease: ModemClockLease<'lease>,
    ) -> Result<
        PreparedModemClockRelease<'identity, 'lease>,
        ModemClockReleasePreparationFailure<'identity, 'lease>,
    > {
        let error = if self.baseline != Baseline::Managed {
            Some(ModemClockReleasePreparationError::UnknownBaseline)
        } else if !ptr::eq(self.identity, lease.identity) {
            Some(ModemClockReleasePreparationError::CrossManagerLease)
        } else if usize::from(lease.slot) >= MAX_ACTIVE_LEASES {
            Some(ModemClockReleasePreparationError::InvalidLeaseSlot)
        } else {
            let slot = self.slots[usize::from(lease.slot)];
            if slot.generation != lease.generation {
                Some(ModemClockReleasePreparationError::StaleLease)
            } else if !slot.active {
                Some(ModemClockReleasePreparationError::DuplicateRelease)
            } else if slot.dependencies != lease.dependencies {
                Some(ModemClockReleasePreparationError::LeaseRecordMismatch)
            } else {
                let mut underflow = None;
                for dependency in Dependency::LOW_BIT_FIRST {
                    if lease.dependencies.contains(dependency)
                        && self.counts[dependency.index()] == 0
                    {
                        underflow = Some(ModemClockReleasePreparationError::RefcountUnderflow(
                            dependency.release_edge(),
                        ));
                        break;
                    }
                }
                underflow
            }
        };

        if let Some(error) = error {
            return Err(ModemClockReleasePreparationFailure {
                planner: self,
                lease,
                error,
            });
        }

        Ok(PreparedModemClockRelease {
            planner: self,
            lease,
            next_dependency: 0,
            completed_edges: 0,
        })
    }
}

/// Error found before an acquire transaction exposes any physical edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModemClockAcquirePreparationError {
    UnknownBaseline,
    RefcountOverflow(ModemClockAcquireEdge),
    LeaseCapacityReached,
    LeaseGenerationExhausted,
}

/// Failed acquire preparation retaining the unchanged manager.
#[must_use = "the failed preparation still owns the complete planner"]
pub(crate) struct ModemClockAcquirePreparationFailure<'identity> {
    planner: ModemClockPlanner<'identity>,
    error: ModemClockAcquirePreparationError,
}

impl fmt::Debug for ModemClockAcquirePreparationFailure<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModemClockAcquirePreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<'identity> ModemClockAcquirePreparationFailure<'identity> {
    pub(crate) const fn error(&self) -> ModemClockAcquirePreparationError {
        self.error
    }

    /// Recover the unchanged planner; preparation has exposed no MMIO edge.
    pub(crate) fn into_planner(self) -> ModemClockPlanner<'identity> {
        self.planner
    }
}

/// Prepared but uncommitted acquire transaction.
///
/// No count or slot is modified while this owner exists.
#[must_use = "an acquire transaction must advance, commit, or remain abandoned"]
pub(crate) struct PreparedModemClockAcquire<'identity> {
    planner: ModemClockPlanner<'identity>,
    dependencies: DependencySet,
    slot: u8,
    generation: u64,
    next_dependency: u8,
    completed_edges: u8,
}

/// Next ownership state of an acquire transaction.
#[must_use = "the next acquire transaction owner is unique"]
pub(crate) enum ModemClockAcquireStep<'identity> {
    Physical(PendingModemClockAcquireEdge<'identity>),
    CommitReady(ModemClockAcquireCommitReady<'identity>),
}

impl<'identity> PreparedModemClockAcquire<'identity> {
    /// Move to the next boundary edge or the explicit commit state.
    pub(crate) fn advance(mut self) -> ModemClockAcquireStep<'identity> {
        while usize::from(self.next_dependency) < DEPENDENCY_COUNT {
            let dependency = Dependency::LOW_BIT_FIRST[usize::from(self.next_dependency)];
            self.next_dependency += 1;
            if self.dependencies.contains(dependency)
                && self.planner.counts[dependency.index()] == 0
            {
                return ModemClockAcquireStep::Physical(PendingModemClockAcquireEdge {
                    transaction: self,
                    edge: dependency.acquire_edge(),
                });
            }
        }

        ModemClockAcquireStep::CommitReady(ModemClockAcquireCommitReady { transaction: self })
    }
}

/// Acquire transaction owner while one physical edge is outstanding.
///
/// It has no method which returns the planner. Failure becomes an opaque
/// poisoned owner, so partial physical execution cannot be represented as a
/// clean rollback.
#[must_use = "the outstanding physical edge retains the complete transaction"]
pub(crate) struct PendingModemClockAcquireEdge<'identity> {
    transaction: PreparedModemClockAcquire<'identity>,
    edge: ModemClockAcquireEdge,
}

impl<'identity> PendingModemClockAcquireEdge<'identity> {
    pub(crate) const fn edge(&self) -> ModemClockAcquireEdge {
        self.edge
    }

    /// Record bookkeeping completion and continue the same transaction.
    ///
    /// Only a future sealed target executor may treat this as hardware-backed.
    fn complete(mut self) -> PreparedModemClockAcquire<'identity> {
        self.transaction.completed_edges += 1;
        self.transaction
    }

    /// Retain the complete owner after an uncertain or failed physical edge.
    pub(crate) fn fail(self) -> PoisonedModemClockAcquire<'identity> {
        PoisonedModemClockAcquire { pending: self }
    }
}

/// Opaque owner after a physical acquire edge did not complete cleanly.
#[must_use = "a poisoned acquire still owns its planner and pending lease"]
pub(crate) struct PoisonedModemClockAcquire<'identity> {
    pending: PendingModemClockAcquireEdge<'identity>,
}

impl<'identity> PoisonedModemClockAcquire<'identity> {
    pub(crate) const fn edge(&self) -> ModemClockAcquireEdge {
        self.pending.edge
    }

    pub(crate) const fn completed_edges(&self) -> u8 {
        self.pending.transaction.completed_edges
    }

    /// Re-expose the same pure-model edge to unit tests.
    ///
    /// This is not a hardware retry contract. A production outcome-unknown
    /// composite edge remains terminal until a target executor can distinguish
    /// not-started from partially completed effects.
    #[cfg(test)]
    fn reexpose_for_test(self) -> PendingModemClockAcquireEdge<'identity> {
        self.pending
    }
}

/// Acquire transaction whose required physical edge sequence is complete.
#[must_use = "logical refcounts change only through explicit commit"]
pub(crate) struct ModemClockAcquireCommitReady<'identity> {
    transaction: PreparedModemClockAcquire<'identity>,
}

impl<'identity> ModemClockAcquireCommitReady<'identity> {
    /// Atomically publish logical counts and the new opaque lease.
    ///
    /// This is a software accounting commit, not a hardware-readiness witness.
    fn commit(mut self) -> (ModemClockPlanner<'identity>, ModemClockLease<'identity>) {
        for dependency in Dependency::LOW_BIT_FIRST {
            if self.transaction.dependencies.contains(dependency) {
                self.transaction.planner.counts[dependency.index()] += 1;
            }
        }

        let slot = &mut self.transaction.planner.slots[usize::from(self.transaction.slot)];
        slot.generation = self.transaction.generation;
        slot.active = true;
        slot.dependencies = self.transaction.dependencies;

        let lease = ModemClockLease {
            identity: self.transaction.planner.identity,
            slot: self.transaction.slot,
            generation: self.transaction.generation,
            dependencies: self.transaction.dependencies,
        };
        (self.transaction.planner, lease)
    }
}

/// Opaque ownership of one committed dependency acquisition.
///
/// This type is intentionally neither `Clone` nor `Copy` and exposes no slot,
/// generation, dependency set, or constructor.
#[must_use = "the shared-clock lease must remain owned until an explicit release"]
pub(crate) struct ModemClockLease<'identity> {
    identity: &'identity ModemClockPlannerIdentity,
    slot: u8,
    generation: u64,
    dependencies: DependencySet,
}

/// Error found before a release transaction exposes any physical edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModemClockReleasePreparationError {
    UnknownBaseline,
    CrossManagerLease,
    InvalidLeaseSlot,
    StaleLease,
    DuplicateRelease,
    LeaseRecordMismatch,
    RefcountUnderflow(ModemClockReleaseEdge),
}

/// Failed release preparation retaining the unchanged planner and exact lease.
#[must_use = "both opaque owners remain in this failed release"]
pub(crate) struct ModemClockReleasePreparationFailure<'planner, 'lease> {
    planner: ModemClockPlanner<'planner>,
    lease: ModemClockLease<'lease>,
    error: ModemClockReleasePreparationError,
}

impl fmt::Debug for ModemClockReleasePreparationFailure<'_, '_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModemClockReleasePreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<'planner, 'lease> ModemClockReleasePreparationFailure<'planner, 'lease> {
    pub(crate) const fn error(&self) -> ModemClockReleasePreparationError {
        self.error
    }

    /// Recover only the two unchanged opaque owners, never counts or IDs.
    pub(crate) fn into_owners(self) -> (ModemClockPlanner<'planner>, ModemClockLease<'lease>) {
        (self.planner, self.lease)
    }
}

/// Prepared but uncommitted release transaction.
///
/// The lease remains active and counts remain unchanged until explicit commit.
#[must_use = "a release transaction must advance, commit, or remain abandoned"]
pub(crate) struct PreparedModemClockRelease<'planner, 'lease> {
    planner: ModemClockPlanner<'planner>,
    lease: ModemClockLease<'lease>,
    next_dependency: u8,
    completed_edges: u8,
}

/// Next ownership state of a release transaction.
#[must_use = "the next release transaction owner is unique"]
pub(crate) enum ModemClockReleaseStep<'planner, 'lease> {
    Physical(PendingModemClockReleaseEdge<'planner, 'lease>),
    CommitReady(ModemClockReleaseCommitReady<'planner, 'lease>),
}

impl<'planner, 'lease> PreparedModemClockRelease<'planner, 'lease> {
    /// Move to the next one-to-zero edge or the explicit commit state.
    pub(crate) fn advance(mut self) -> ModemClockReleaseStep<'planner, 'lease> {
        while usize::from(self.next_dependency) < DEPENDENCY_COUNT {
            let dependency = Dependency::LOW_BIT_FIRST[usize::from(self.next_dependency)];
            self.next_dependency += 1;
            if self.lease.dependencies.contains(dependency)
                && self.planner.counts[dependency.index()] == 1
            {
                return ModemClockReleaseStep::Physical(PendingModemClockReleaseEdge {
                    transaction: self,
                    edge: dependency.release_edge(),
                });
            }
        }

        ModemClockReleaseStep::CommitReady(ModemClockReleaseCommitReady { transaction: self })
    }
}

/// Release transaction owner while one physical edge is outstanding.
#[must_use = "the outstanding physical edge retains the planner and lease"]
pub(crate) struct PendingModemClockReleaseEdge<'planner, 'lease> {
    transaction: PreparedModemClockRelease<'planner, 'lease>,
    edge: ModemClockReleaseEdge,
}

impl<'planner, 'lease> PendingModemClockReleaseEdge<'planner, 'lease> {
    pub(crate) const fn edge(&self) -> ModemClockReleaseEdge {
        self.edge
    }

    /// Record bookkeeping completion and continue the same transaction.
    fn complete(mut self) -> PreparedModemClockRelease<'planner, 'lease> {
        self.transaction.completed_edges += 1;
        self.transaction
    }

    /// Retain the complete owner after an uncertain or failed physical edge.
    pub(crate) fn fail(self) -> PoisonedModemClockRelease<'planner, 'lease> {
        PoisonedModemClockRelease { pending: self }
    }
}

/// Opaque owner after a physical release edge did not complete cleanly.
#[must_use = "a poisoned release still owns its planner and exact lease"]
pub(crate) struct PoisonedModemClockRelease<'planner, 'lease> {
    pending: PendingModemClockReleaseEdge<'planner, 'lease>,
}

impl<'planner, 'lease> PoisonedModemClockRelease<'planner, 'lease> {
    pub(crate) const fn edge(&self) -> ModemClockReleaseEdge {
        self.pending.edge
    }

    pub(crate) const fn completed_edges(&self) -> u8 {
        self.pending.transaction.completed_edges
    }

    /// Re-expose the same pure-model edge to unit tests.
    ///
    /// This is not a hardware retry contract. A production outcome-unknown
    /// composite edge remains terminal until a target executor can distinguish
    /// not-started from partially completed effects.
    #[cfg(test)]
    fn reexpose_for_test(self) -> PendingModemClockReleaseEdge<'planner, 'lease> {
        self.pending
    }
}

/// Release transaction whose required physical edge sequence is complete.
#[must_use = "logical refcounts change only through explicit commit"]
pub(crate) struct ModemClockReleaseCommitReady<'planner, 'lease> {
    transaction: PreparedModemClockRelease<'planner, 'lease>,
}

impl<'planner, 'lease> ModemClockReleaseCommitReady<'planner, 'lease> {
    /// Atomically decrement counts and retire the exact lease slot.
    fn commit(mut self) -> ModemClockPlanner<'planner> {
        for dependency in Dependency::LOW_BIT_FIRST {
            if self.transaction.lease.dependencies.contains(dependency) {
                self.transaction.planner.counts[dependency.index()] -= 1;
            }
        }

        let slot = &mut self.transaction.planner.slots[usize::from(self.transaction.lease.slot)];
        slot.active = false;
        slot.dependencies = DependencySet(0);
        self.transaction.planner
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    const ALL_ACQUIRE_EDGES: [ModemClockAcquireEdge; DEPENDENCY_COUNT] = [
        ModemClockAcquireEdge::Pll160AndModemSource,
        ModemClockAcquireEdge::Coexistence,
        ModemClockAcquireEdge::WifiBb80x1,
        ModemClockAcquireEdge::Etm,
        ModemClockAcquireEdge::BtApbAndSecurity,
        ModemClockAcquireEdge::BtIeee802154CommonBaseband,
        ModemClockAcquireEdge::Ieee802154ApbAndMac,
    ];

    const ALL_RELEASE_EDGES: [ModemClockReleaseEdge; DEPENDENCY_COUNT] = [
        ModemClockReleaseEdge::Pll160AndModemSource,
        ModemClockReleaseEdge::Coexistence,
        ModemClockReleaseEdge::WifiBb80x1,
        ModemClockReleaseEdge::Etm,
        ModemClockReleaseEdge::BtApbAndSecurity,
        ModemClockReleaseEdge::BtIeee802154CommonBaseband,
        ModemClockReleaseEdge::Ieee802154ApbAndMac,
    ];

    fn finish_acquire<'identity>(
        mut prepared: PreparedModemClockAcquire<'identity>,
    ) -> (
        ModemClockPlanner<'identity>,
        ModemClockLease<'identity>,
        Vec<ModemClockAcquireEdge>,
    ) {
        let mut edges = Vec::new();
        loop {
            match prepared.advance() {
                ModemClockAcquireStep::Physical(pending) => {
                    edges.push(pending.edge());
                    prepared = pending.complete();
                }
                ModemClockAcquireStep::CommitReady(ready) => {
                    let (planner, lease) = ready.commit();
                    return (planner, lease, edges);
                }
            }
        }
    }

    fn finish_release<'planner, 'lease>(
        mut prepared: PreparedModemClockRelease<'planner, 'lease>,
    ) -> (ModemClockPlanner<'planner>, Vec<ModemClockReleaseEdge>) {
        let mut edges = Vec::new();
        loop {
            match prepared.advance() {
                ModemClockReleaseStep::Physical(pending) => {
                    edges.push(pending.edge());
                    prepared = pending.complete();
                }
                ModemClockReleaseStep::CommitReady(ready) => {
                    return (ready.commit(), edges);
                }
            }
        }
    }

    fn duplicate_for_adversarial_test<'identity>(
        lease: &ModemClockLease<'identity>,
    ) -> ModemClockLease<'identity> {
        ModemClockLease {
            identity: lease.identity,
            slot: lease.slot,
            generation: lease.generation,
            dependencies: lease.dependencies,
        }
    }

    #[test]
    fn exact_ieee_set_uses_low_bit_first_order_for_acquire_and_release() {
        let mut identity = ModemClockPlannerIdentity::new();
        let planner = ModemClockPlanner::managed_for_test(&mut identity);
        let prepared = planner
            .prepare_ieee802154_acquire()
            .expect("known zero baseline");
        let (planner, lease, acquire_edges) = finish_acquire(prepared);
        assert_eq!(acquire_edges, ALL_ACQUIRE_EDGES);
        assert_eq!(planner.counts, [1; DEPENDENCY_COUNT]);

        let release = planner.prepare_release(lease).expect("valid exact lease");
        let (planner, release_edges) = finish_release(release);
        assert_eq!(release_edges, ALL_RELEASE_EDGES);
        assert_eq!(planner.counts, [0; DEPENDENCY_COUNT]);
    }

    #[test]
    fn overlapping_leases_emit_only_zero_one_and_one_zero_boundaries() {
        let mut identity = ModemClockPlannerIdentity::new();
        let planner = ModemClockPlanner::managed_for_test(&mut identity);
        let (planner, first, first_edges) = finish_acquire(
            planner
                .prepare_ieee802154_acquire()
                .expect("first acquisition"),
        );
        let (planner, second, second_edges) = finish_acquire(
            planner
                .prepare_ieee802154_acquire()
                .expect("overlapping acquisition"),
        );
        assert_eq!(first_edges, ALL_ACQUIRE_EDGES);
        assert!(second_edges.is_empty());
        assert_eq!(planner.counts, [2; DEPENDENCY_COUNT]);

        let (planner, first_release_edges) = finish_release(
            planner
                .prepare_release(first)
                .expect("first overlapping release"),
        );
        assert!(first_release_edges.is_empty());
        assert_eq!(planner.counts, [1; DEPENDENCY_COUNT]);

        let (planner, last_release_edges) = finish_release(
            planner
                .prepare_release(second)
                .expect("last overlapping release"),
        );
        assert_eq!(last_release_edges, ALL_RELEASE_EDGES);
        assert_eq!(planner.counts, [0; DEPENDENCY_COUNT]);
    }

    #[test]
    fn local_lease_capacity_fails_closed_and_preserves_all_owners() {
        let mut identity = ModemClockPlannerIdentity::new();
        let mut planner = ModemClockPlanner::managed_for_test(&mut identity);
        let mut leases = Vec::new();

        for _ in 0..MAX_ACTIVE_LEASES {
            let prepared = planner
                .prepare_ieee802154_acquire()
                .expect("one slot remains");
            let (next_planner, lease, _) = finish_acquire(prepared);
            planner = next_planner;
            leases.push(lease);
        }

        let failure = match planner.prepare_ieee802154_acquire() {
            Ok(_) => panic!("the local fixed-capacity table must reject a seventeenth lease"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            ModemClockAcquirePreparationError::LeaseCapacityReached
        );
        let mut planner = failure.into_planner();
        assert_eq!(planner.counts, [MAX_ACTIVE_LEASES as u16; DEPENDENCY_COUNT]);
        assert_eq!(
            planner.slots.iter().filter(|slot| slot.active).count(),
            MAX_ACTIVE_LEASES
        );

        for lease in leases {
            let prepared = planner.prepare_release(lease).expect("exact live lease");
            let (next_planner, _) = finish_release(prepared);
            planner = next_planner;
        }
        assert_eq!(planner.counts, [0; DEPENDENCY_COUNT]);
        assert!(planner.slots.iter().all(|slot| !slot.active));
    }

    #[test]
    fn exhausted_slot_generation_fails_closed_without_count_changes() {
        let mut identity = ModemClockPlannerIdentity::new();
        let mut planner = ModemClockPlanner::managed_for_test(&mut identity);
        planner.slots[0].generation = u64::MAX;

        let failure = match planner.prepare_ieee802154_acquire() {
            Ok(_) => panic!("an exhausted slot generation must not be reused"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            ModemClockAcquirePreparationError::LeaseGenerationExhausted
        );
        let planner = failure.into_planner();
        assert_eq!(planner.counts, [0; DEPENDENCY_COUNT]);
        assert_eq!(planner.slots[0].generation, u64::MAX);
        assert!(planner.slots.iter().all(|slot| !slot.active));
    }

    #[test]
    fn partial_dependency_overlap_preserves_order_and_independent_counts() {
        let mut identity = ModemClockPlannerIdentity::new();
        let planner = ModemClockPlanner::managed_for_test(&mut identity);
        let first_set = DependencySet::from_dependencies(&[
            Dependency::Pll160AndModemSource,
            Dependency::WifiBb80x1,
            Dependency::BtIeee802154CommonBaseband,
        ]);
        let second_set = DependencySet::from_dependencies(&[
            Dependency::WifiBb80x1,
            Dependency::Etm,
            Dependency::BtIeee802154CommonBaseband,
        ]);

        let (planner, first, first_edges) =
            finish_acquire(planner.prepare_acquire(first_set).expect("first set"));
        assert_eq!(
            first_edges,
            [
                ModemClockAcquireEdge::Pll160AndModemSource,
                ModemClockAcquireEdge::WifiBb80x1,
                ModemClockAcquireEdge::BtIeee802154CommonBaseband,
            ]
        );

        let (planner, second, second_edges) =
            finish_acquire(planner.prepare_acquire(second_set).expect("second set"));
        assert_eq!(second_edges, [ModemClockAcquireEdge::Etm]);
        assert_eq!(planner.counts, [1, 0, 2, 1, 0, 2, 0]);

        let (planner, first_release) =
            finish_release(planner.prepare_release(first).expect("first release"));
        assert_eq!(first_release, [ModemClockReleaseEdge::Pll160AndModemSource]);
        assert_eq!(planner.counts, [0, 0, 1, 1, 0, 1, 0]);

        let (planner, second_release) =
            finish_release(planner.prepare_release(second).expect("second release"));
        assert_eq!(
            second_release,
            [
                ModemClockReleaseEdge::WifiBb80x1,
                ModemClockReleaseEdge::Etm,
                ModemClockReleaseEdge::BtIeee802154CommonBaseband,
            ]
        );
        assert_eq!(planner.counts, [0; DEPENDENCY_COUNT]);
    }

    #[test]
    fn preparation_and_commit_are_transactionally_separate() {
        let mut identity = ModemClockPlannerIdentity::new();
        let planner = ModemClockPlanner::managed_for_test(&mut identity);
        let prepared = planner
            .prepare_ieee802154_acquire()
            .expect("managed baseline");
        assert_eq!(prepared.planner.counts, [0; DEPENDENCY_COUNT]);
        assert!(prepared.planner.slots.iter().all(|slot| !slot.active));

        let pending = match prepared.advance() {
            ModemClockAcquireStep::Physical(pending) => pending,
            ModemClockAcquireStep::CommitReady(_) => panic!("fresh acquire needs edges"),
        };
        assert_eq!(pending.edge(), ModemClockAcquireEdge::Pll160AndModemSource);
        assert_eq!(pending.transaction.planner.counts, [0; DEPENDENCY_COUNT]);

        let poisoned = pending.fail();
        assert_eq!(poisoned.completed_edges(), 0);
        assert_eq!(poisoned.edge(), ModemClockAcquireEdge::Pll160AndModemSource);
        let pending = poisoned.reexpose_for_test();
        assert_eq!(pending.transaction.planner.counts, [0; DEPENDENCY_COUNT]);

        let (planner, _lease, edges) = finish_acquire(pending.complete());
        assert_eq!(edges, ALL_ACQUIRE_EDGES[1..]);
        assert_eq!(planner.counts, [1; DEPENDENCY_COUNT]);
    }

    #[test]
    fn release_failure_is_poisoned_and_keeps_counts_and_lease_until_commit() {
        let mut identity = ModemClockPlannerIdentity::new();
        let planner = ModemClockPlanner::managed_for_test(&mut identity);
        let (planner, lease, _) = finish_acquire(
            planner
                .prepare_ieee802154_acquire()
                .expect("managed baseline"),
        );
        let prepared = planner.prepare_release(lease).expect("valid lease");
        assert_eq!(prepared.planner.counts, [1; DEPENDENCY_COUNT]);
        assert!(prepared.planner.slots[usize::from(prepared.lease.slot)].active);

        let pending = match prepared.advance() {
            ModemClockReleaseStep::Physical(pending) => pending,
            ModemClockReleaseStep::CommitReady(_) => panic!("last release needs edges"),
        };
        let poisoned = pending.fail();
        assert_eq!(poisoned.completed_edges(), 0);
        assert_eq!(poisoned.edge(), ModemClockReleaseEdge::Pll160AndModemSource);
        let pending = poisoned.reexpose_for_test();
        assert_eq!(pending.transaction.planner.counts, [1; DEPENDENCY_COUNT]);
        assert!(
            pending.transaction.planner.slots[usize::from(pending.transaction.lease.slot)].active
        );

        let (planner, edges) = finish_release(pending.complete());
        assert_eq!(edges, ALL_RELEASE_EDGES[1..]);
        assert_eq!(planner.counts, [0; DEPENDENCY_COUNT]);
    }

    #[test]
    fn source_refcount_boundary_accepts_max_then_rejects_overflow() {
        let mut identity = ModemClockPlannerIdentity::new();
        let mut planner = ModemClockPlanner::managed_for_test(&mut identity);
        planner.counts[Dependency::WifiBb80x1.index()] = MAX_REFCOUNT - 1;
        let identity_address = planner.identity as *const _;

        let prepared = planner
            .prepare_ieee802154_acquire()
            .expect("MAX_REFCOUNT - 1 may advance to the source maximum");
        let (planner, _lease, edges) = finish_acquire(prepared);
        assert!(!edges.contains(&ModemClockAcquireEdge::WifiBb80x1));
        assert_eq!(planner.counts[Dependency::WifiBb80x1.index()], MAX_REFCOUNT);

        let failure = match planner.prepare_ieee802154_acquire() {
            Ok(_) => panic!("MAX_REFCOUNT must not exceed the source contract"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            ModemClockAcquirePreparationError::RefcountOverflow(ModemClockAcquireEdge::WifiBb80x1)
        );
        let planner = failure.into_planner();
        assert_eq!(planner.identity as *const _, identity_address);
        assert_eq!(planner.counts[Dependency::WifiBb80x1.index()], MAX_REFCOUNT);
        assert_eq!(planner.slots.iter().filter(|slot| slot.active).count(), 1);
    }

    #[test]
    fn underflow_fails_transactionally_and_returns_both_opaque_owners() {
        let mut identity = ModemClockPlannerIdentity::new();
        let planner = ModemClockPlanner::managed_for_test(&mut identity);
        let (mut planner, lease, _) = finish_acquire(
            planner
                .prepare_ieee802154_acquire()
                .expect("managed baseline"),
        );
        planner.counts[Dependency::Etm.index()] = 0;
        let lease_slot = lease.slot;

        let failure = match planner.prepare_release(lease) {
            Ok(_) => panic!("underflow must fail"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            ModemClockReleasePreparationError::RefcountUnderflow(ModemClockReleaseEdge::Etm)
        );
        let (planner, lease) = failure.into_owners();
        assert_eq!(lease.slot, lease_slot);
        assert_eq!(planner.counts[Dependency::Etm.index()], 0);
        assert!(planner.slots[usize::from(lease.slot)].active);
    }

    #[test]
    fn duplicate_and_stale_leases_are_rejected_without_owner_loss() {
        let mut identity = ModemClockPlannerIdentity::new();
        let planner = ModemClockPlanner::managed_for_test(&mut identity);
        let (planner, first, _) = finish_acquire(
            planner
                .prepare_ieee802154_acquire()
                .expect("first acquisition"),
        );
        let duplicate = duplicate_for_adversarial_test(&first);
        let stale = duplicate_for_adversarial_test(&first);
        let (planner, _) = finish_release(planner.prepare_release(first).expect("first release"));

        let duplicate_failure = match planner.prepare_release(duplicate) {
            Ok(_) => panic!("duplicate release must fail"),
            Err(failure) => failure,
        };
        assert_eq!(
            duplicate_failure.error(),
            ModemClockReleasePreparationError::DuplicateRelease
        );
        let (planner, duplicate) = duplicate_failure.into_owners();
        assert_eq!(planner.counts, [0; DEPENDENCY_COUNT]);
        drop(duplicate);

        let (planner, current, _) = finish_acquire(
            planner
                .prepare_ieee802154_acquire()
                .expect("slot generation advances"),
        );
        let stale_failure = match planner.prepare_release(stale) {
            Ok(_) => panic!("stale generation must fail"),
            Err(failure) => failure,
        };
        assert_eq!(
            stale_failure.error(),
            ModemClockReleasePreparationError::StaleLease
        );
        let (planner, stale) = stale_failure.into_owners();
        assert_eq!(planner.counts, [1; DEPENDENCY_COUNT]);
        drop(stale);

        let (planner, edges) =
            finish_release(planner.prepare_release(current).expect("current lease"));
        assert_eq!(edges, ALL_RELEASE_EDGES);
        assert_eq!(planner.counts, [0; DEPENDENCY_COUNT]);
    }

    #[test]
    fn cross_manager_lease_is_rejected_and_both_epochs_are_retained() {
        let mut first_identity = ModemClockPlannerIdentity::new();
        let mut second_identity = ModemClockPlannerIdentity::new();
        let first_planner = ModemClockPlanner::managed_for_test(&mut first_identity);
        let second_planner = ModemClockPlanner::managed_for_test(&mut second_identity);
        let (first_planner, lease, _) = finish_acquire(
            first_planner
                .prepare_ieee802154_acquire()
                .expect("first manager"),
        );

        let failure = match second_planner.prepare_release(lease) {
            Ok(_) => panic!("cross-manager lease must fail"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            ModemClockReleasePreparationError::CrossManagerLease
        );
        let (second_planner, lease) = failure.into_owners();
        assert_eq!(second_planner.counts, [0; DEPENDENCY_COUNT]);
        assert_eq!(first_planner.counts, [1; DEPENDENCY_COUNT]);
        assert!(!ptr::eq(second_planner.identity, lease.identity));

        let (first_planner, edges) = finish_release(
            first_planner
                .prepare_release(lease)
                .expect("original manager accepts exact lease"),
        );
        assert_eq!(edges, ALL_RELEASE_EDGES);
        assert_eq!(first_planner.counts, [0; DEPENDENCY_COUNT]);
    }

    #[test]
    fn externally_retained_baseline_cannot_issue_acquire_or_release_plans() {
        let mut external_identity = ModemClockPlannerIdentity::new();
        let external = ModemClockPlanner::externally_retained(&mut external_identity);
        let failure = match external.prepare_ieee802154_acquire() {
            Ok(_) => panic!("unknown baseline cannot plan acquisition"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            ModemClockAcquirePreparationError::UnknownBaseline
        );
        let external = failure.into_planner();
        assert_eq!(external.baseline, Baseline::ExternallyRetained);
        assert_eq!(external.counts, [0; DEPENDENCY_COUNT]);

        let mut managed_identity = ModemClockPlannerIdentity::new();
        let managed = ModemClockPlanner::managed_for_test(&mut managed_identity);
        let (managed, lease, _) = finish_acquire(
            managed
                .prepare_ieee802154_acquire()
                .expect("test managed baseline"),
        );

        let failure = match external.prepare_release(lease) {
            Ok(_) => panic!("unknown baseline cannot plan release"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            ModemClockReleasePreparationError::UnknownBaseline
        );
        let (external, lease) = failure.into_owners();
        assert_eq!(external.baseline, Baseline::ExternallyRetained);

        let (managed, edges) = finish_release(
            managed
                .prepare_release(lease)
                .expect("managed owner retains release authority"),
        );
        assert_eq!(edges, ALL_RELEASE_EDGES);
        assert_eq!(managed.counts, [0; DEPENDENCY_COUNT]);
    }
}
