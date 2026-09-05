use core::{
    ptr::NonNull,
    sync::atomic::{AtomicU8, Ordering},
};

use super::*;

struct ReleaseProbe {
    slot: *const ExternalRxHandoffSlot,
    calls: AtomicU8,
}

#[allow(
    unsafe_code,
    reason = "test callback reconstructs the exact owner installed in ExternalRxBuffer"
)]
unsafe fn observe_release_state(owner: NonNull<()>, _owner_index: usize) {
    // SAFETY: the test installs a stable pointer to `ReleaseProbe` as the
    // callback owner and keeps both the probe and pool alive throughout
    // the complete affine lease transition.
    let probe = unsafe { owner.cast::<ReleaseProbe>().as_ref() };
    // SAFETY: the same lifetime proof keeps the referenced pool slot live.
    let state = unsafe { (*probe.slot).state.load(Ordering::Acquire) };
    assert_eq!(
        state, SLOT_RELEASING,
        "handoff credit became free before its DMA buffer was released"
    );
    probe.calls.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn dma_buffer_releases_before_handoff_credit_becomes_free() {
    let pool = ExternalRxHandoffPool::<64, 1>::new();
    let probe = ReleaseProbe {
        slot: core::ptr::addr_of!(pool.slots[0]),
        calls: AtomicU8::new(0),
    };
    let mut bytes = [0_u8; 64];
    let pointer = NonNull::new(bytes.as_mut_ptr()).unwrap();
    let owner = NonNull::from(&probe).cast::<()>();
    // SAFETY: the stack allocation remains stable and exclusively owned
    // until the network lease invokes the bound callback exactly once.
    #[allow(
        unsafe_code,
        reason = "test owns stable bytes and the exact callback context"
    )]
    let buffer = unsafe {
        ExternalRxBuffer::new(
            pointer,
            bytes.len(),
            bytes.len(),
            owner,
            0,
            observe_release_state,
        )
    };

    let radio = match pool.try_claim_radio(buffer, 0) {
        Ok(radio) => radio,
        Err(_) => panic!("fresh handoff slot must accept one buffer"),
    };
    let index = radio.republish(0, bytes.len());
    let network = pool.claim_network(index);
    assert_eq!(network.release(), 0);

    assert_eq!(probe.calls.load(Ordering::Relaxed), 1);
    assert_eq!(pool.claimed_slots(), 0);
}
