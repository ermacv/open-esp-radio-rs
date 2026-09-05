extern crate std;

use core::cell::Cell;
use std::boxed::Box;

use super::*;

type TestPool = PinnedDmaTxPool<32, 8, 4, 1>;

fn mark_dma_read_prepared(storage: &mut [u8]) {
    storage[0] = 0x5a;
}

struct ReturnProbe<'pool> {
    pool: &'pool TestPool,
    returned: &'pool Cell<Option<u8>>,
}

impl DmaIndexReturn for ReturnProbe<'_> {
    fn return_index(&self, index: u8) {
        assert_eq!(
            self.pool.slots[usize::from(index)]
                .state
                .load(Ordering::Acquire),
            SLOT_FREE,
            "the backing must release its slot before queue publication"
        );
        self.returned.set(Some(index));
    }
}

fn prepared_radio(pool: &TestPool) -> PinnedDmaTxRadioLease<'_, 32, 8, 4> {
    let network = pool.claim_network(0);
    let (index, ()) = network.publish(4, |frame| frame.copy_from_slice(&[1, 2, 3, 4]));
    pool.claim_radio(index)
}

#[test]
fn dropped_stage_leases_restore_the_slot() {
    let pool = TestPool::new();
    drop(pool.claim_network(0));

    let radio = prepared_radio(&pool);
    assert_eq!(radio.ethernet(), &[1, 2, 3, 4]);
    drop(radio);

    assert_eq!(pool.claim_network(0).release(), 0);
}

#[test]
fn radio_backing_runs_target_prepare_only_at_dma_publication_edge() {
    let pool = Box::leak(Box::new(TestPool::new()));
    let pool = TestPool::pin_static_with_dma_read_prepare(pool, mark_dma_read_prepared);
    let pool = pool.as_ref().get_ref();
    let mut radio = prepared_radio(pool);

    assert_eq!(radio.storage_mut()[0], 0);
    StableDmaBacking::prepare_for_dma_read(&mut radio);
    assert_eq!(radio.storage_mut()[0], 0x5a);
}

#[test]
fn failed_transactional_writer_never_publishes_the_slot() {
    let pool = TestPool::new();
    let lease = pool.claim_network(0);
    let (lease, error) = lease
        .try_publish(4, |frame| {
            frame.copy_from_slice(&[1, 2, 3, 4]);
            Err::<(), _>(7)
        })
        .unwrap_err();
    assert_eq!(error, 7);
    assert_eq!(lease.release(), 0);
    assert_eq!(pool.claim_network(0).release(), 0);
}

#[test]
fn returning_backing_releases_before_publishing_its_index() {
    let pool = TestPool::new();
    let returned = Cell::new(None);
    let backing = ReturningStableDmaBacking::new(
        prepared_radio(&pool),
        ReturnProbe {
            pool: &pool,
            returned: &returned,
        },
    );

    assert_eq!(returned.get(), None);
    drop(backing);
    assert_eq!(returned.get(), Some(0));
    assert_eq!(pool.claim_network(0).release(), 0);
}

#[test]
fn forgotten_backing_remains_quarantined() {
    let pool = TestPool::new();
    let returned = Cell::new(None);
    let backing = ReturningStableDmaBacking::new(
        prepared_radio(&pool),
        ReturnProbe {
            pool: &pool,
            returned: &returned,
        },
    );

    core::mem::forget(backing);

    assert_eq!(returned.get(), None);
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(pool.slots[0].state.load(Ordering::Acquire), SLOT_RADIO);
}

#[test]
#[should_panic(expected = "pinned TX pool boundary changed")]
fn explicit_pool_audit_checks_quarantined_slots() {
    type TwoSlotPool = PinnedDmaTxPool<32, 8, 4, 2>;
    let mut pool = TwoSlotPool::new();
    // Tests own the unpinned allocation and may model a DMA overrun by
    // changing the otherwise CPU-read-only guard.
    pool.slots[1].dma_overrun_guard.get_mut()[7] = 0;
    let _ = pool.claimed_slots();
}
