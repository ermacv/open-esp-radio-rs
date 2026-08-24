use open_esp_radio_embassy_net::NoopRawMutex;
use open_esp_radio_esp32s31_wifi_mac::rx::RxIngressConfig;
use open_esp_radio_esp32s31_wifi_sta::connected_rx::ConnectedRxConfig;
use open_esp_radio_ieee80211::security::WifiSecurityMode;
use open_esp_radio_wifi_embassy::connected_tasks::{
    ConnectedTaskControlResources, ConnectedTaskGroup,
};
use std::boxed::Box;

use super::*;

struct Sink;

impl ConnectedRxSink for Sink {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {}
}

impl<const CAPACITY: usize, const SLOTS: usize> ConnectedRxProtocolSink<CAPACITY, SLOTS> for Sink {
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        ready(())
    }
}

fn open_config() -> ConnectedRxConfig {
    ConnectedRxConfig {
        station_address: [2, 3, 4, 5, 6, 7],
        bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        association_id: 1,
        ingress: RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        security: WifiSecurityMode::Open,
        peer_qos: false,
    }
}

#[test]
fn empty_standalone_queue_can_be_replaced_without_rebuilding_station_protocol() {
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    let runtime = Box::leak(Box::new(Esp32s31ConnectedRxProtocolStorage::new()));
    let protocol = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        ConnectedRxDispatcher::new(open_config()),
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
    );

    let processor = protocol
        .try_into_processor()
        .unwrap_or_else(|_| panic!("an empty standalone queue must detach"));
    let (_sender, receiver) = queue.split();
    let protocol = Esp32s31ConnectedRxProtocol::from_processor(receiver, processor);

    assert_eq!(protocol.queue_len(), 0);
    let stopped = embassy_futures::block_on(protocol.run_until_stopped(ready(())));
    assert_eq!(stopped.shutdown(), ConnectedRxProtocolShutdown::default());
}

#[test]
fn paired_processor_returns_the_same_stopped_protocol_owner() {
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    let runtime = Box::leak(Box::new(Esp32s31ConnectedRxProtocolStorage::new()));
    let runtime_ptr = runtime as *mut _;
    let protocol = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        ConnectedRxDispatcher::new(open_config()),
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
    );

    let processor = protocol
        .try_into_processor()
        .unwrap_or_else(|_| panic!("empty standalone queue must detach"));
    let stopped = processor.into_stopped();

    assert_eq!(stopped.shutdown(), ConnectedRxProtocolShutdown::default());
    let (_, _, returned_runtime) = stopped.into_parts();
    assert_eq!(returned_runtime as *mut _, runtime_ptr);
}

#[test]
fn stop_edge_returns_an_empty_reusable_protocol_epoch() {
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let dispatcher = ConnectedRxDispatcher::new(open_config());
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    // Host tests leak the arena to model the static lifetime required by an
    // embedded composition root without consuming thread-stack space.
    let runtime = Box::leak(Box::new(Esp32s31ConnectedRxProtocolStorage::new()));
    let runtime_ptr = runtime as *mut _;
    let runtime_size = core::mem::size_of_val(&*runtime);
    let mpdu_ptr = mpdu.as_mut_ptr();
    let ethernet_ptr = ethernet.as_mut_ptr();
    let protocol = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        dispatcher,
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
    );
    assert!(
        core::mem::size_of_val(&protocol) < runtime_size,
        "the movable protocol handle must not absorb its static reorder arena"
    );

    let stopped = embassy_futures::block_on(protocol.run_until_stopped(ready(())));
    assert_eq!(stopped.shutdown(), ConnectedRxProtocolShutdown::default());
    let (returned_mpdu, returned_ethernet, returned_runtime) = stopped.into_parts();
    assert_eq!(returned_mpdu.as_mut_ptr(), mpdu_ptr);
    assert_eq!(returned_mpdu.len(), 64);
    assert_eq!(returned_ethernet.as_mut_ptr(), ethernet_ptr);
    assert_eq!(returned_ethernet.len(), 64);
    assert_eq!(returned_runtime as *mut _, runtime_ptr);
}

#[test]
fn controlled_task_observes_shutdown_before_returning_exact_scratch() {
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let dispatcher = ConnectedRxDispatcher::new(open_config());
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    // Host tests leak the arena to model the static lifetime required by an
    // embedded composition root without consuming thread-stack space.
    let runtime = Box::leak(Box::new(Esp32s31ConnectedRxProtocolStorage::new()));
    let runtime_ptr = runtime as *mut _;
    let mpdu_ptr = mpdu.as_mut_ptr();
    let ethernet_ptr = ethernet.as_mut_ptr();
    let protocol = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        dispatcher,
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
    );
    let control = ConnectedTaskControlResources::<NoopRawMutex, _>::new();
    let (mut controller, endpoint) = control.split().expect("fresh task epoch starts");
    controller.request_stop();
    let mut observed = None;

    embassy_futures::block_on(protocol.run_controlled_task(endpoint, |shutdown| {
        observed = Some(shutdown);
    }));
    let stopped = embassy_futures::block_on(controller.wait_stopped());

    assert_eq!(observed, Some(ConnectedRxProtocolShutdown::default()));
    let (returned_mpdu, returned_ethernet, returned_runtime) = stopped.into_parts();
    assert_eq!(returned_mpdu.as_mut_ptr(), mpdu_ptr);
    assert_eq!(returned_ethernet.as_mut_ptr(), ethernet_ptr);
    assert_eq!(returned_runtime as *mut _, runtime_ptr);
}

#[test]
fn stopped_protocol_arena_starts_a_second_epoch_without_reinitialization() {
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    let runtime = Box::leak(Box::new(Esp32s31ConnectedRxProtocolStorage::new()));
    let runtime_ptr = runtime as *mut _;
    let scratch_ptrs = (mpdu.as_mut_ptr(), ethernet.as_mut_ptr());

    let first = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        ConnectedRxDispatcher::new(open_config()),
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
    );
    let stopped = embassy_futures::block_on(first.run_until_stopped(ready(())));
    let (mpdu, ethernet, runtime) = stopped.into_parts();

    let (_sender, receiver) = queue.split();
    let second = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        ConnectedRxDispatcher::new(open_config()),
        Sink,
        mpdu,
        ethernet,
        runtime,
    );
    let stopped = embassy_futures::block_on(second.run_until_stopped(ready(())));
    let (mpdu, ethernet, runtime) = stopped.into_parts();

    assert_eq!(mpdu.as_mut_ptr(), scratch_ptrs.0);
    assert_eq!(ethernet.as_mut_ptr(), scratch_ptrs.1);
    assert_eq!(runtime as *mut _, runtime_ptr);
}
