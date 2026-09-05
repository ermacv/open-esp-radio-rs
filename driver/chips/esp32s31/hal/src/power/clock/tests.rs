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
    assert!(pending.transaction.planner.slots[usize::from(pending.transaction.lease.slot)].active);

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

    let (planner, edges) = finish_release(planner.prepare_release(current).expect("current lease"));
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
