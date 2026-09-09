use super::*;
use crate::rx_ring::RxResumeError;

const BASE: u32 = 0x2f00_1000;
const BUFFERS: [u32; 2] = [0x2f00_2000, 0x2f00_2200];

fn live(storage: &'static RxDmaStorage<2, 16, 20>, mmio: &mut MockRxDma) -> RxRingLive<'static, 2> {
    storage
        .prepare_ring(mmio, BASE, &BUFFERS)
        .unwrap()
        .try_start(mmio)
        .unwrap_or_else(|_| panic!("start"))
}

#[test]
fn pause_keeps_a_network_lease_and_reclaims_its_return_after_resume() {
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20>::new()));
    storage.buffer_mut(0).unwrap()[4..8].copy_from_slice(&[1, 2, 3, 4]);
    let mut mmio = MockRxDma::default();
    let mut ring = live(storage, &mut mmio);
    storage.descriptors()[0].write_word0(
        16 | (8 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31,
    );
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & 0x000f_ffff;
    let detached = storage
        .take_completed_unit(&mut ring, 1)
        .unwrap()
        .unwrap()
        .detach_single()
        .unwrap();
    let pool = oer_memory::ExternalRxHandoffPool::<16, 1>::new();
    let radio = pool
        .try_claim_radio(detached.into_buffer(), 0)
        .unwrap_or_else(|_| panic!("claim lease"));
    let len = radio.frame().len();
    let mut network = pool.claim_network(radio.republish(0, len));
    let publications = mmio.publications;
    let paused = ring
        .try_pause(&mut mmio)
        .unwrap_or_else(|_| panic!("pause"));
    assert!(!mmio.walker);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(matches!(
        storage.prepare_ring(&mut mmio, BASE, &BUFFERS),
        Err(RxRingError::Busy)
    ));
    assert_eq!(
        network.with_frame(|frame| frame[4..8].to_vec()),
        [1, 2, 3, 4]
    );
    drop(network);
    assert_eq!(storage.released_buffer_count(), 1);
    let mut ring = paused
        .try_resume(&mut mmio)
        .unwrap_or_else(|_| panic!("resume"));
    assert!(mmio.walker);
    assert_eq!(
        mmio.publications, publications,
        "pause/resume must not republish storage"
    );
    let append = storage
        .recycle_released_prefix::<2, _>(&mut ring, &mut mmio)
        .unwrap()
        .expect("return retained descriptor exactly once");
    assert_eq!(append.descriptor_count, 1);
    assert_eq!(storage.released_buffer_count(), 0);
    assert!(
        storage
            .recycle_released_prefix::<2, _>(&mut ring, &mut mmio)
            .unwrap()
            .is_none()
    );
    assert!(ring.try_stop(&mut mmio).is_ok());
}

#[test]
fn reload_defers_pause_without_disabling_the_walker() {
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let ring = live(storage, &mut mmio);
    mmio.reload = true;
    let calls = mmio.disable_calls;
    let (ring, error) = ring.try_pause(&mut mmio).err().expect("pending reload");
    assert_eq!(error, RxRingError::Busy);
    assert_eq!(mmio.disable_calls, calls);
    assert!(mmio.walker);
    mmio.reload = false;
    let paused = ring
        .try_pause(&mut mmio)
        .unwrap_or_else(|_| panic!("settled pause"));
    assert!(paused.try_stop(&mut mmio).is_ok());
}

#[test]
fn failed_pause_retains_owner_for_retry_and_drop_quarantines_paused_storage() {
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let ring = live(storage, &mut mmio);
    mmio.fail_disable = true;
    let (ring, error) = ring.try_pause(&mut mmio).err().expect("unconfirmed stop");
    assert_eq!(error, RxRingError::Busy);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    mmio.fail_disable = false;
    let paused = ring
        .try_pause(&mut mmio)
        .unwrap_or_else(|_| panic!("pause retry"));
    drop(paused);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn cursor_change_and_ambiguous_enable_require_terminal_recovery() {
    for change_cursor in [true, false] {
        let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20>::new()));
        let mut mmio = MockRxDma::default();
        let ring = live(storage, &mut mmio);
        let paused = ring
            .try_pause(&mut mmio)
            .unwrap_or_else(|_| panic!("pause"));
        if change_cursor {
            mmio.next_descriptor_low = 12;
        } else {
            mmio.ambiguous_enable = true;
        }
        let failure = paused.try_resume(&mut mmio).err().expect("unsafe resume");
        assert_eq!(
            failure.error(),
            if change_cursor {
                RxResumeError::CursorChanged
            } else {
                RxResumeError::EnableUnconfirmed
            }
        );
        assert_eq!(mmio.walker, !change_cursor);
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
        mmio.fail_disable = true;
        let (failure, error) = failure
            .try_stop(&mut mmio)
            .err()
            .expect("retain failed cleanup");
        assert_eq!(error, RxRingError::Busy);
        mmio.fail_disable = false;
        assert!(failure.try_stop(&mut mmio).is_ok());
        assert!(!mmio.walker);
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    }
}

#[test]
fn unexpected_walker_or_reload_activity_is_reported_separately() {
    for walker in [true, false] {
        let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20>::new()));
        let mut mmio = MockRxDma::default();
        let ring = live(storage, &mut mmio);
        let paused = ring
            .try_pause(&mut mmio)
            .unwrap_or_else(|_| panic!("pause"));
        mmio.walker = walker;
        mmio.reload = !walker;
        let failure = paused
            .try_resume(&mut mmio)
            .err()
            .expect("external activity");
        assert_eq!(
            failure.error(),
            if walker {
                RxResumeError::WalkerActive
            } else {
                RxResumeError::ReloadPending
            }
        );
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
        assert!(failure.try_stop(&mut mmio).is_ok());
    }
}
