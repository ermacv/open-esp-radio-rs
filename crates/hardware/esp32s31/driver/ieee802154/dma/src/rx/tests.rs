use super::*;
use crate::{DMA_HIGH, DMA_LOW, MAX_PHR_LENGTH, MIN_PHR_LENGTH};

fn pool<const COUNT: usize>(base: u32) -> Result<PinnedRxPool<COUNT>, RxPoolError> {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(RxPoolStorage::new()));
    RxPoolStorage::pin_static_model(storage, DmaFrameAddress::try_new(base).unwrap())
        .map_err(|failure| failure.error())
}

#[test]
fn buffers_have_exact_geometry_and_contiguous_stub() {
    assert_eq!(core::mem::size_of::<RxFrameBuffer>(), FRAME_BUFFER_SIZE);
    assert_eq!(core::mem::align_of::<RxFrameBuffer>(), 4);
    let storage = RxPoolStorage::<2>::new();
    let first = (&storage.buffers[0] as *const RxFrameBuffer).addr();
    let second = (&storage.buffers[1] as *const RxFrameBuffer).addr();
    let stub = (&storage.stub as *const RxFrameBuffer).addr();
    assert_eq!(second - first, FRAME_BUFFER_SIZE);
    assert_eq!(stub - second, FRAME_BUFFER_SIZE);
}

#[test]
fn pool_span_includes_separate_stub() {
    let last_two_frames = DMA_HIGH - 2 * FRAME_BUFFER_SIZE as u32;
    assert!(pool::<1>(last_two_frames).is_ok());
    assert_eq!(
        pool::<2>(last_two_frames).err(),
        Some(RxPoolError::Address(DmaAddressError::OutOfRange))
    );
    assert_eq!(pool::<0>(DMA_LOW).err(), Some(RxPoolError::EmptyPool));
}

#[test]
fn failed_bind_returns_exact_unchanged_storage_for_corrected_retry() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(RxPoolStorage::<2>::new()));
    let identity = core::ptr::from_mut(storage);
    let last_two_frames = DMA_HIGH - 2 * FRAME_BUFFER_SIZE as u32;
    let failure = match RxPoolStorage::pin_static_model(
        storage,
        DmaFrameAddress::try_new(last_two_frames).unwrap(),
    ) {
        Ok(_) => panic!("three-frame pool span must not bind into two frames"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        RxPoolError::Address(DmaAddressError::OutOfRange)
    );

    let (storage, error) = failure.into_parts();
    assert_eq!(core::ptr::from_mut(storage), identity);
    assert_eq!(error, RxPoolError::Address(DmaAddressError::OutOfRange));
    assert_eq!(storage.states[0].load(Ordering::Acquire), STATE_FREE);
    assert_eq!(storage.states[1].load(Ordering::Acquire), STATE_FREE);
    assert!(!storage.active.load(Ordering::Acquire));
    assert!(!storage.poisoned.load(Ordering::Acquire));

    let rebound =
        RxPoolStorage::pin_static_model(storage, DmaFrameAddress::try_new(DMA_LOW).unwrap())
            .unwrap();
    assert_eq!(rebound.capacity(), 2);
}

#[test]
fn empty_pool_bind_failure_returns_exact_storage() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(RxPoolStorage::<0>::new()));
    let identity = core::ptr::from_mut(storage);
    let failure = match RxPoolStorage::pin_static_model(
        storage,
        DmaFrameAddress::try_new(DMA_LOW).unwrap(),
    ) {
        Ok(_) => panic!("empty pool must fail closed"),
        Err(failure) => failure,
    };

    assert_eq!(failure.error(), RxPoolError::EmptyPool);
    let (storage, error) = failure.into_parts();
    assert_eq!(core::ptr::from_mut(storage), identity);
    assert_eq!(error, RxPoolError::EmptyPool);
    assert!(!storage.active.load(Ordering::Acquire));
    assert!(!storage.poisoned.load(Ordering::Acquire));
}

#[test]
fn free_armed_delivered_free_transition_preserves_layout() {
    let pool = pool::<1>(DMA_LOW).unwrap();
    let RxArm::Buffer(mut armed) = pool.arm_next().unwrap() else {
        panic!("first arm must select the delivery slot");
    };
    assert_eq!(armed.index(), 0);
    assert_eq!(armed.dma_address().as_u32(), DMA_LOW);
    assert_eq!(pool.slot_state(0), Some(RxSlotState::Armed));

    let phr = MIN_PHR_LENGTH;
    let mut image = [0; FRAME_BUFFER_SIZE];
    image[0] = phr;
    image[1] = 0xa5;
    image[phr as usize - 1] = (-37_i8) as u8;
    image[phr as usize] = 199;
    armed.write_model(&image);
    let delivered = armed.complete_model().unwrap();
    assert_eq!(pool.slot_state(0), Some(RxSlotState::Delivered));
    let view = delivered.frame().unwrap();
    assert_eq!(view.mac_bytes(), &[0xa5]);
    assert_eq!(view.rssi(), -37);
    assert_eq!(view.lqi(), 199);
    delivered.release().unwrap();
    assert_eq!(pool.slot_state(0), Some(RxSlotState::Free));
}

#[test]
fn all_delivery_slots_are_used_before_stub() {
    let pool = pool::<2>(DMA_LOW).unwrap();
    let RxArm::Buffer(first) = pool.arm_next().unwrap() else {
        panic!();
    };
    let first = first.complete_model().unwrap();
    assert_eq!(pool.slot_state(0), Some(RxSlotState::Delivered));

    let RxArm::Buffer(second) = pool.arm_next().unwrap() else {
        panic!();
    };
    assert_eq!(first.index(), 0);
    assert_eq!(second.index(), 1);
    let second = second.complete_model().unwrap();
    assert_eq!(pool.slot_state(1), Some(RxSlotState::Delivered));

    let RxArm::Stub(stub) = pool.arm_next().unwrap() else {
        panic!("the third arm must use the separate stub");
    };
    assert_eq!(
        stub.dma_address().as_u32(),
        DMA_LOW + 2 * FRAME_BUFFER_SIZE as u32
    );
    assert_eq!(pool.stub_state(), RxSlotState::Armed);
    assert!(matches!(pool.arm_next(), Err(RxPoolError::HardwareBusy)));

    let stub = stub.complete_model().unwrap();
    assert_eq!(pool.stub_state(), RxSlotState::Delivered);
    // All ordinary buffers and the stub are retained. Exhaustion must
    // return the active gate, so a repeated attempt is Exhausted too.
    assert!(matches!(pool.arm_next(), Err(RxPoolError::Exhausted)));
    assert!(matches!(pool.arm_next(), Err(RxPoolError::Exhausted)));

    first.release().unwrap();
    second.release().unwrap();
    stub.discard().unwrap();
    assert_eq!(pool.stub_state(), RxSlotState::Free);
}

#[test]
fn reentrant_and_concurrent_arm_are_rejected_until_completion() {
    let pool = pool::<2>(DMA_LOW).unwrap();
    let RxArm::Buffer(armed) = pool.arm_next().unwrap() else {
        panic!();
    };

    assert!(matches!(pool.arm_next(), Err(RxPoolError::HardwareBusy)));
    std::thread::scope(|scope| {
        let rejected = scope
            .spawn(|| matches!(pool.arm_next(), Err(RxPoolError::HardwareBusy)))
            .join()
            .unwrap();
        assert!(rejected);
    });

    let delivered = armed.complete_model().unwrap();
    let RxArm::Buffer(next) = pool.arm_next().unwrap() else {
        panic!("successful completion must release the active gate");
    };
    assert_eq!(next.index(), 1);
    next.complete_model().unwrap().release().unwrap();
    delivered.release().unwrap();
}

#[test]
fn failed_completion_keeps_active_gate_closed() {
    let pool = pool::<1>(DMA_LOW).unwrap();
    let RxArm::Buffer(armed) = pool.arm_next().unwrap() else {
        panic!();
    };

    // Fault injection models corrupted lifecycle evidence at the external
    // completion boundary. Production callers cannot mutate this atomic.
    pool.storage.states[0].store(STATE_FREE, Ordering::Release);
    assert!(matches!(
        armed.complete_model(),
        Err(RxPoolError::State {
            expected: RxSlotState::Armed,
            observed: RxSlotState::Free,
        })
    ));
    assert!(matches!(pool.arm_next(), Err(RxPoolError::Poisoned)));
}

#[test]
fn aggregate_buffer_completion_failure_retains_terminal_owner() {
    let pool = pool::<1>(DMA_LOW).unwrap();
    let arm = pool.arm_next().unwrap();
    let terminal = DmaTerminalEvidence::for_native_model();

    // Fault injection stands in for inconsistent external lifecycle
    // evidence. The aggregate API must not return an Armed token to retry.
    pool.storage.states[0].store(STATE_FREE, Ordering::Release);
    let failure = match arm.complete(&terminal) {
        Ok(_) => panic!("mismatched ownership state must fail"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        RxPoolError::State {
            expected: RxSlotState::Armed,
            observed: RxSlotState::Free,
        }
    );
    assert!(matches!(pool.arm_next(), Err(RxPoolError::Poisoned)));
}

#[test]
fn aggregate_stub_completion_failure_retains_terminal_owner() {
    let pool = pool::<1>(DMA_LOW).unwrap();
    let RxArm::Buffer(frame) = pool.arm_next().unwrap() else {
        panic!();
    };
    let retained_frame = frame.complete_model().unwrap();
    let arm = pool.arm_next().unwrap();
    let terminal = DmaTerminalEvidence::for_native_model();
    assert!(matches!(&arm, RxArm::Stub(_)));

    pool.storage.stub_state.store(STATE_FREE, Ordering::Release);
    let failure = match arm.complete(&terminal) {
        Ok(_) => panic!("mismatched stub ownership state must fail"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        RxPoolError::State {
            expected: RxSlotState::Armed,
            observed: RxSlotState::Free,
        }
    );
    assert!(matches!(pool.arm_next(), Err(RxPoolError::Poisoned)));
    retained_frame.release().unwrap();
}

#[test]
fn aggregate_completion_and_recycle_cover_frame_and_stub() {
    let pool = pool::<1>(DMA_LOW).unwrap();
    let terminal = DmaTerminalEvidence::for_native_model();
    let completion = pool.arm_next().unwrap().complete(&terminal).unwrap();
    assert_eq!(completion.kind(), RxCompletionKind::Frame { index: 0 });
    assert!(completion.frame().is_some());

    let retained_frame = pool.arm_next().unwrap().complete(&terminal).unwrap();
    assert_eq!(retained_frame.kind(), RxCompletionKind::Stub);
    assert!(retained_frame.frame().is_none());
    retained_frame.recycle().unwrap();
    completion.recycle().unwrap();
    assert_eq!(pool.slot_state(0), Some(RxSlotState::Free));
    assert_eq!(pool.stub_state(), RxSlotState::Free);
}

#[test]
fn recycle_poison_survives_already_issued_arm_completion() {
    let pool = pool::<2>(DMA_LOW).unwrap();
    let terminal = DmaTerminalEvidence::for_native_model();
    let completion = pool.arm_next().unwrap().complete(&terminal).unwrap();
    let concurrent_arm = pool.arm_next().unwrap();
    pool.storage.states[0].store(STATE_ARMED, Ordering::Release);

    let failure = match completion.recycle() {
        Ok(()) => panic!("mismatched delivered state must fail"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        RxPoolError::State {
            expected: RxSlotState::Delivered,
            observed: RxSlotState::Armed,
        }
    );
    assert!(matches!(pool.arm_next(), Err(RxPoolError::Poisoned)));

    // This arm escaped before the mismatch was detected. Its completion
    // clears the transient active bit but must never clear poison.
    let concurrent_completion = concurrent_arm.complete(&terminal).unwrap();
    concurrent_completion.recycle().unwrap();
    assert!(!pool.storage.active.load(Ordering::Acquire));
    assert!(pool.storage.poisoned.load(Ordering::Acquire));
    assert!(matches!(pool.arm_next(), Err(RxPoolError::Poisoned)));
}

#[test]
fn dropped_delivery_is_quarantined_and_stub_absorbs_next_frame() {
    let pool = pool::<1>(DMA_LOW).unwrap();
    let RxArm::Buffer(armed) = pool.arm_next().unwrap() else {
        panic!();
    };
    {
        let _delivered = armed.complete_model().unwrap();
        assert_eq!(pool.slot_state(0), Some(RxSlotState::Delivered));
    }
    assert_eq!(pool.slot_state(0), Some(RxSlotState::Delivered));
    assert!(matches!(pool.arm_next(), Ok(RxArm::Stub(_))));
}

#[test]
fn delivered_view_rejects_invalid_phr_without_recycling() {
    for phr in [0, MIN_PHR_LENGTH - 1, MAX_PHR_LENGTH + 1, u8::MAX] {
        let pool = pool::<1>(DMA_LOW).unwrap();
        let RxArm::Buffer(mut armed) = pool.arm_next().unwrap() else {
            panic!();
        };
        armed.write_model(&[phr]);
        let delivered = armed.complete_model().unwrap();
        assert_eq!(
            delivered.frame().unwrap_err(),
            RxFrameError::PhrLengthOutOfRange { length: phr }
        );
        assert_eq!(pool.slot_state(0), Some(RxSlotState::Delivered));
        delivered.release().unwrap();
    }
}
