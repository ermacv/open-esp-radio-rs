//! Isolate deliberate singleton-pool exhaustion from other test processes.
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_xarxa_upstream::{
    LinkState, NetworkInterfaceId, Resources,
    driver::{Driver, PacketBuf},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Wake, Waker},
};

struct AvailableOnWake(AtomicBool);
impl Wake for AvailableOnWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0
            .store(PacketBuf::try_new().is_some(), Ordering::SeqCst);
    }
}

#[test]
fn packet_release_notifies_after_the_global_slot_becomes_available() {
    let mut resources = Resources::<NoopRawMutex, 2, 2>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(1), [2; 6]);
    radio.link_controller().set_link_state(LinkState::Up);
    device.transmit(PacketBuf::try_new().unwrap()).ok().unwrap();
    let mut held = Vec::new();
    while let Some(packet) = PacketBuf::try_new() {
        held.push(packet);
    }
    let observed = Arc::new(AvailableOnWake(AtomicBool::new(false)));
    let waker = Waker::from(observed.clone());
    device.register_waker(&waker).unwrap();
    let selected = radio.try_receive_tx().unwrap();
    assert!(
        !observed.0.load(Ordering::SeqCst),
        "queue credit alone does not free a pool slot"
    );
    // Model Core1 running before Core0 finishes copying the selected owner.
    device.register_waker(&waker).unwrap();
    drop(selected);
    assert!(
        observed.0.load(Ordering::SeqCst),
        "releasing the actual owner must wake the starved stack again"
    );
}
