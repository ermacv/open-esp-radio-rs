#![expect(
    clippy::manual_async_fn,
    reason = "test doubles implement the same explicit borrowed Future service contracts"
)]

use core::{
    future::{Future, ready},
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};
use oer_ieee80211_mac::sequence::SequenceNumber;

use crate::datapath::{
    DatapathRxServiceContext,
    network::{DatapathNetworkRx, DatapathNetworkRxSet},
    paired::DatapathPairedRxService,
    rx::{
        reorder::{
            RxBlockAckSnapshot, RxReorderCommand, RxReorderCommandResources,
            try_send_rx_reorder_command,
        },
        routed::StaApStagedRxQueue,
        staging::{StagedRxFrame, StagedRxQueue},
    },
};

use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::TryReceiveError};

use oer_esp32s31_ieee80211_dma::descriptor::{BIT_30, BIT_31, DESCRIPTOR_BYTES, LENGTH_SHIFT};

use oer_esp32s31_ieee80211_mac::rx::{
    PUBLIC_HEADER_SIZE, RxDmaBinding, RxDmaWalkerStopped, RxIngressConfig, RxRingStopped,
};

use oer_esp32s31_ieee80211_sta::connected_rx::{
    ConnectedRxConfig, ConnectedRxEvent, ConnectedRxSink,
};

use oer_ieee80211_mac::{
    data::EthernetFrameParts, security::LinkProtection, vif::StaApRxAddresses,
};

use oer_network_interface::RxEnqueueError;

use std::boxed::Box;

use super::*;

const BASE: u32 = 0x2f00_1000;

#[derive(Default)]
struct PairedNetworkRx {
    frames: usize,
}

impl DatapathNetworkRx for PairedNetworkRx {
    fn queue_len(&self) -> usize {
        self.frames
    }

    fn try_send(&mut self, _frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.frames += 1;
        Ok(())
    }

    fn try_send_parts(&mut self, _frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        self.frames += 1;
        Ok(())
    }

    fn poll_ready(&mut self, _context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        core::task::Poll::Ready(())
    }

    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        let result = self.try_send(frame);
        if result.is_ok() {
            before_publish();
        }
        result
    }

    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        let result = self.try_send_parts(frame);
        if result.is_ok() {
            before_publish();
        }
        result
    }
}

impl DatapathNetworkRxSet for PairedNetworkRx {
    fn primary_mut(&mut self) -> &mut dyn DatapathNetworkRx {
        self
    }

    fn get_mut(
        &mut self,
        _interface: oer_network_interface::NetworkInterfaceId,
    ) -> Option<&mut dyn DatapathNetworkRx> {
        None
    }

    fn pair_mut(
        &mut self,
        _first: oer_network_interface::NetworkInterfaceId,
        _second: oer_network_interface::NetworkInterfaceId,
    ) -> Option<(&mut dyn DatapathNetworkRx, &mut dyn DatapathNetworkRx)> {
        None
    }
}

#[derive(Default)]
struct PairedStationRole {
    frames: usize,
    publish_pending_calls: usize,
    pending: bool,
}

#[derive(Default)]
struct PairedAccessPointRole {
    frames: usize,
    tx_pending: bool,
    publish_pending_calls: usize,
}

#[derive(Default)]
struct RecordingRxObserver {
    stage_too_long_discards: AtomicU32,
    stage_chained_discards: AtomicU32,
    completed_descriptors: AtomicUsize,
    recycled_descriptors: AtomicUsize,
    overload_discarded_units: AtomicUsize,
    overload_recycled_descriptors: AtomicUsize,
    critical_reserve_admissions: AtomicUsize,
    critical_admission_blocked: AtomicBool,
    minimum_pool_credits: AtomicUsize,
    minimum_queue_credits: AtomicUsize,
}

impl RxPipelineObserver for RecordingRxObserver {
    fn now_micros(&self) -> u64 {
        0
    }

    fn observe(&self, observation: RxPipelineObservation) {
        match observation {
            RxPipelineObservation::StageDiscarded(RxStageDiscard::TooLong) => {
                self.stage_too_long_discards.fetch_add(1, Ordering::Relaxed);
            }
            RxPipelineObservation::StageDiscarded(RxStageDiscard::Chained) => {
                self.stage_chained_discards.fetch_add(1, Ordering::Relaxed);
            }
            RxPipelineObservation::ServiceCompleted(observation) => {
                self.completed_descriptors
                    .fetch_add(observation.completed_descriptors, Ordering::Relaxed);
                self.recycled_descriptors
                    .fetch_add(observation.recycled_descriptors, Ordering::Relaxed);
                self.overload_discarded_units
                    .fetch_add(observation.overload_discarded, Ordering::Relaxed);
                self.overload_recycled_descriptors
                    .fetch_add(observation.overload_recycled_descriptors, Ordering::Relaxed);
                self.critical_reserve_admissions
                    .fetch_add(observation.critical_reserve_admitted, Ordering::Relaxed);
                self.critical_admission_blocked
                    .store(observation.critical_admission_blocked, Ordering::Relaxed);
                self.minimum_pool_credits
                    .store(observation.minimum_pool_credits, Ordering::Relaxed);
                self.minimum_queue_credits
                    .store(observation.minimum_queue_credits, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct OneShotNarrowAdmission(AtomicU32);

impl Esp32s31RxStageAdmissionPolicy for OneShotNarrowAdmission {
    fn credit_policy(&self) -> RxStageCreditPolicy {
        // This length-admission fixture has a one-credit queue.
        RxStageCreditPolicy::new(0)
    }

    fn maximum_payload_length(
        &self,
        _unit: Esp32s31RxCompletedUnit,
        physical_capacity: usize,
    ) -> usize {
        if self
            .0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            0
        } else {
            physical_capacity
        }
    }

    fn observe(&self, observation: Esp32s31RxIngressObservation) {
        match observation {
            Esp32s31RxIngressObservation::DiscardRetained {
                reason: RxStageDiscard::TooLong,
                ..
            } => assert_eq!(
                self.0
                    .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire),
                Ok(1),
            ),
            Esp32s31RxIngressObservation::Staged(_) => assert_eq!(
                self.0
                    .compare_exchange(2, 3, Ordering::AcqRel, Ordering::Acquire),
                Ok(2),
            ),
            _ => panic!("unexpected admission observation: {observation:?}"),
        }
    }
}

#[derive(Default)]
struct CountingUnavailableAdmission(AtomicU32);

impl Esp32s31RxStageAdmissionPolicy for CountingUnavailableAdmission {
    fn credit_policy(&self) -> RxStageCreditPolicy {
        // This consumer-credit fixture intentionally has a one-credit queue.
        RxStageCreditPolicy::new(0)
    }

    fn unavailable_disposition(
        &self,
        preview: Esp32s31RxCompletedUnitPreview,
    ) -> RxStageUnavailableDisposition {
        self.0.fetch_add(1, Ordering::Relaxed);
        match preview.class {
            Esp32s31RxIngressClass::BulkProtectedData => {
                RxStageUnavailableDisposition::PreserveForCapacity
            }
            Esp32s31RxIngressClass::Critical | Esp32s31RxIngressClass::Unclassified => {
                RxStageUnavailableDisposition::PreserveForCriticalAdmission
            }
        }
    }
}

struct MockRxDma {
    walker: bool,
    descriptor_base: u32,
    fail_enable: bool,
    exhaust_on_reload: bool,
    last_descriptor_low: u32,
    next_descriptor_low: u32,
}

impl Default for MockRxDma {
    fn default() -> Self {
        Self {
            walker: false,
            descriptor_base: 0,
            fail_enable: false,
            exhaust_on_reload: false,
            last_descriptor_low: BASE & 0x000f_ffff,
            next_descriptor_low: (BASE + DESCRIPTOR_BYTES) & 0x000f_ffff,
        }
    }
}

impl MockRxDma {
    fn release_through(&mut self, last_index: usize, next_index: Option<usize>) {
        self.last_descriptor_low = (BASE + last_index as u32 * DESCRIPTOR_BYTES) & 0x000f_ffff;
        self.next_descriptor_low = next_index
            .map(|index| (BASE + index as u32 * DESCRIPTOR_BYTES) & 0x000f_ffff)
            .unwrap_or(0);
    }
}

impl RxDma for MockRxDma {
    fn last_descriptor_low(&mut self) -> u32 {
        if self.walker {
            self.last_descriptor_low
        } else {
            0
        }
    }
    fn next_descriptor_low(&mut self) -> u32 {
        if self.walker {
            self.next_descriptor_low
        } else {
            0
        }
    }

    fn next_descriptor(&mut self) -> oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor {
        oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor::validation(
            self.next_descriptor_low(),
            false,
        )
    }
    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaCursorObservation<'confirmation>,
        ) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        self.fence();
        let next = self.next_descriptor_low();
        self.fence();
        observed(oer_esp32s31_ieee80211_mac::rx::RxDmaCursorObservation::validation(last, next))
    }
    fn walker_enabled(&mut self) -> bool {
        self.walker
    }
    fn reload_pending(&mut self) -> bool {
        false
    }
    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaReloadSettled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        (!self.reload_pending())
            .then(|| settled(oer_esp32s31_ieee80211_mac::rx::RxDmaReloadSettled::validation()))
    }
    fn configure_descriptor_window(&mut self, _: &RxDmaBinding) {}
    fn write_descriptor_base(&mut self, _: &RxDmaBinding, address: u32) {
        self.descriptor_base = address;
    }
    fn publish_walker_enable(&mut self, _: &RxDmaBinding) {
        self.walker = true;
    }
    fn request_reload(&mut self, _: &RxDmaBinding) {
        if self.exhaust_on_reload {
            self.release_through(3, None);
        }
    }
    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        if self.fail_enable {
            return None;
        }
        self.walker = true;
        Some(enabled(
            oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled::validation(),
        ))
    }
    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.walker = false;
        Some(stopped(RxDmaWalkerStopped::validation()))
    }
    fn fence(&mut self) {}
}

/// Time that skips to every deadline, so a walker settle ends at once.
type NoDelay = oer_time_virtual::SkipClock;

#[derive(Default)]
struct Observer(u32);

impl ConnectedRxSink for Observer {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {
        self.0 += 1;
    }
}

#[derive(Default)]
struct OrderObserver(std::vec::Vec<u16>);

impl ConnectedRxSink for OrderObserver {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet { raw, .. } = event {
            let sequence_control =
                u16::from_le_bytes([raw[PUBLIC_HEADER_SIZE + 22], raw[PUBLIC_HEADER_SIZE + 23]]);
            self.0.push(sequence_control >> 4);
        }
    }
}

fn dispatcher_config() -> ConnectedRxConfig {
    ConnectedRxConfig {
        station_address: oer_ieee80211_mac::address::OwnAddress::new(
            oer_ieee80211_mac::address::IndividualAddress::new([2, 3, 4, 5, 6, 7]).unwrap(),
        ),
        bssid: oer_ieee80211_mac::address::Bssid::new(
            oer_ieee80211_mac::address::IndividualAddress::new([8, 9, 10, 11, 12, 13]).unwrap(),
        ),
        association_id: 1,
        ingress: RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        security: LinkProtection::Ccmp,
        peer_qos: true,
        management_protection: false,
    }
}

#[test]
fn connected_rx_stop_confirms_walker_off_and_preserves_static_resources() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    let pool = RxStagePool::<STAGED_DEPTH, ESP32S31_RX_BUFFER_SIZE>::new();
    let mut queue =
        StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, ESP32S31_RX_BUFFER_SIZE, STAGED_DEPTH>::new();
    let (sender, _receiver) = queue.split();
    let service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender);
    assert!(hardware.walker);

    let stopped = match service.try_stop(&mut hardware) {
        Ok(stopped) => stopped,
        Err(_) => panic!("mock walker must confirm the stop edge"),
    };

    assert!(!hardware.walker);
    assert_eq!(stopped.ring().descriptor_base(), BASE);
    assert_eq!(stopped.ring().buffer_addresses(), &addresses);
    assert_eq!(stopped.queued_frames(), 0);

    let (ring, epoch_resources) = stopped.into_epoch_parts();
    assert_eq!(ring.descriptor_base(), BASE);
    assert_eq!(epoch_resources.queued_frames(), 0);
    let stopped = epoch_resources.with_halted_ring(ring);

    let prepared = match stopped.prepare(&mut hardware) {
        Ok(prepared) => prepared,
        Err(_) => panic!("halted owner must rebuild the next descriptor epoch"),
    };
    assert!(!hardware.walker);
    assert_eq!(prepared.ring().initial_start(), 0);
    hardware.fail_enable = true;
    let prepared = match embassy_futures::block_on(prepared.start(&mut hardware)) {
        Ok(_) => panic!("rejected walker enable must not create a live owner"),
        Err((prepared, error)) => {
            assert_eq!(error, RxRingError::Busy);
            prepared
        }
    };
    assert!(!hardware.walker);
    assert_eq!(prepared.ring().initial_start(), 0);
    assert_eq!(prepared.queued_frames(), 0);
    hardware.fail_enable = false;
    let restarted = match embassy_futures::block_on(prepared.start(&mut hardware)) {
        Ok(restarted) => restarted,
        Err(_) => panic!("prepared owner must reopen the mock walker"),
    };
    assert!(hardware.walker);
    assert_eq!(restarted.ring().descriptor_base(), BASE);
    assert_eq!(restarted.queued_frames(), 0);

    let stopped = match restarted.try_stop(&mut hardware) {
        Ok(stopped) => stopped,
        Err(_) => panic!("restarted owner must stop before the split test"),
    };
    let (ring, epoch_resources) = stopped.into_epoch_parts();
    let prepared = match storage.prepare_halted(ring, &mut hardware) {
        Ok(prepared) => prepared,
        Err(_) => panic!("split halted ring must rebuild"),
    };
    let ring = prepared
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    let restarted = epoch_resources.with_live_ring(ring);
    assert!(hardware.walker);
    assert_eq!(restarted.ring().descriptor_base(), BASE);
    assert_eq!(restarted.queued_frames(), 0);
    restarted
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn logical_role_handoff_preserves_one_continuous_live_walker_epoch() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    let pool = RxStagePool::<STAGED_DEPTH, ESP32S31_RX_BUFFER_SIZE>::new();
    let mut queue =
        StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, ESP32S31_RX_BUFFER_SIZE, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let first = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender);

    let (ring, resources) = first
        .try_into_live_epoch_parts()
        .unwrap_or_else(|_| panic!("empty logical role must release its live producer"));
    assert!(hardware.walker);
    assert_eq!(ring.descriptor_base(), BASE);
    assert_eq!(resources.queued_frames(), 0);

    let second = resources.with_live_ring(ring);
    assert!(hardware.walker);
    assert_eq!(second.ring().descriptor_base(), BASE);
    drop(receiver);
    second
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test cleanup must stop the physical walker"));
    assert!(!hardware.walker);
}

#[test]
fn logical_role_handoff_recycles_released_prefix_before_discarding_old_completion() {
    const COUNT: usize = 8;
    const STAGED_DEPTH: usize = 2;
    const STAGE_CAPACITY: usize = 16;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    let mut addresses = [0_u32; COUNT];
    for (index, address) in addresses.iter_mut().enumerate() {
        *address = 0x2f00_2000 + index as u32 * 0x800;
        storage.buffer_mut(index).unwrap()[..4].copy_from_slice(&[1, 2, 3, 4]);
        storage.descriptors()[index]
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    }
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    for index in 0..4 {
        storage.descriptors()[index]
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    }
    hardware.release_through(3, Some(4));

    let pool = RxStagePool::<STAGED_DEPTH, STAGE_CAPACITY>::new();
    let mut queue =
        StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, STAGE_CAPACITY, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut first = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender);
    assert_eq!(
        embassy_futures::block_on(first.service(&mut hardware)),
        Ok(DatapathRxProgress::StageCapacityBlocked),
    );
    assert_eq!(pool.claimed_slots(), 2);
    drop(receiver.try_receive().unwrap());
    drop(receiver.try_receive().unwrap());
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(storage.released_buffer_count(), 2);

    let (ring, resources) = first
        .try_into_live_epoch_parts()
        .unwrap_or_else(|_| panic!("drained logical role must release its live producer"));
    // Model a stale pre-handoff completion which the new role rejects by
    // length. The released prefix advances first; only then may the new role
    // bind this ring-owned discard to its fresh frozen cursor.
    storage.descriptors()[2]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (32 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    let mut second = resources.with_live_ring(ring);
    assert!(matches!(
        embassy_futures::block_on(second.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending | DatapathRxProgress::RecycledAppendPending)
    ));
    assert_eq!(storage.released_buffer_count(), 0);
    assert_eq!(second.ring().recycle_start(), 3);
    while let Ok(frame) = receiver.try_receive() {
        drop(frame);
    }
    assert_eq!(pool.claimed_slots(), 0);
    second
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test cleanup must stop the physical walker"));
}

#[test]
fn exhausted_cursor_keeps_service_live_until_terminal_writeback_arrives() {
    const COUNT: usize = 4;
    const STAGE_CAPACITY: usize = 16;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    let addresses = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    for index in 0..3 {
        storage.descriptors()[index]
            .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    }
    hardware.release_through(3, None);
    let pool = RxStagePool::<COUNT, STAGE_CAPACITY>::new();
    let mut queue = StagedRxQueue::<NoopRawMutex, COUNT, STAGE_CAPACITY, COUNT>::new();
    let (sender, receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
        "NEXT=0/LAST=tail must survive a delayed terminal descriptor writeback"
    );
    assert_eq!(receiver.len(), 3);

    storage.descriptors()[3]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    assert_eq!(receiver.len(), 4);
    while receiver.try_receive().is_ok() {}
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    assert!(service.ring().observed_mask().is_empty());
    hardware.next_descriptor_low = BASE & 0x000f_ffff;
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn finite_service_discards_a_descriptor_chain_without_copying_it() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    const STAGE_CAPACITY: usize = 16;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    storage.buffer_mut(0).unwrap()[..4].copy_from_slice(&[1, 2, 3, 4]);
    storage.buffer_mut(1).unwrap()[..4].copy_from_slice(&[5, 6, 7, 8]);
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    storage.descriptors()[0]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_31);
    storage.descriptors()[1]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    hardware.release_through(1, None);
    let observer = RecordingRxObserver::default();
    let pool = RxStagePool::<STAGED_DEPTH, STAGE_CAPACITY>::new();
    let mut queue =
        StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, STAGE_CAPACITY, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_pipeline_observer(&observer)
        .with_stage_admission_policy(UnreservedRxStageAdmission);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    assert!(matches!(
        receiver.try_receive(),
        Err(TryReceiveError::Empty)
    ));
    assert_eq!(observer.stage_chained_discards.load(Ordering::Relaxed), 1);
    assert_eq!(service.ring().recycle_start(), 0);
    // The frozen LAST releases its complete chained unit inclusively, exactly
    // as the vendor worker processes the descriptor equal to saved LAST.
    assert_ne!(storage.descriptors()[0].word0() & BIT_31, 0);
    assert_eq!(storage.descriptors()[1].word0() & BIT_30, 0);
    assert_eq!(pool.claimed_slots(), 0);
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn frozen_last_reclaims_chained_and_empty_discards_through_its_frontier() {
    const COUNT: usize = 4;
    const STAGE_CAPACITY: usize = 16;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    storage.buffer_mut(0).unwrap()[..4].copy_from_slice(&[1, 2, 3, 4]);
    storage.buffer_mut(1).unwrap()[..4].copy_from_slice(&[5, 6, 7, 8]);
    let addresses = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    storage.descriptors()[0]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_31);
    storage.descriptors()[1]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    storage.descriptors()[2].write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | BIT_30 | BIT_31);
    storage.descriptors()[3].write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | BIT_30 | BIT_31);
    hardware.release_through(3, None);
    let observer = RecordingRxObserver::default();
    let pool = RxStagePool::<COUNT, STAGE_CAPACITY>::new();
    let mut queue = StagedRxQueue::<NoopRawMutex, COUNT, STAGE_CAPACITY, COUNT>::new();
    let (sender, receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_pipeline_observer(&observer);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    hardware.next_descriptor_low = BASE & 0x000f_ffff;
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
    assert_eq!(storage.descriptors()[1].word0() & BIT_30, 0);
    assert_eq!(storage.descriptors()[2].word0() & BIT_30, 0);
    assert_eq!(storage.descriptors()[3].word0() & BIT_30, 0);
    assert!(!service.ring().exhausted_republication_probe_pending());
    assert!(matches!(
        receiver.try_receive(),
        Err(TryReceiveError::Empty)
    ));
    assert_eq!(observer.stage_chained_discards.load(Ordering::Relaxed), 1);
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn production_ring_reclaims_before_a_32_slot_stage_pool_saturates() {
    const COUNT: usize = 64;
    const COMPLETED: usize = 40;
    const STAGED_DEPTH: usize = 32;
    const STAGE_CAPACITY: usize = 16;
    const DMA_BUFFER_SIZE: usize = 16;
    const DMA_STORAGE_SIZE: usize = 20;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >::new()));
    let mut addresses = [0_u32; COUNT];
    for (index, address) in addresses.iter_mut().enumerate() {
        *address = 0x2f00_2000 + index as u32 * 0x20;
        storage.buffer_mut(index).expect("test DMA buffer")[..4]
            .copy_from_slice(&(index as u32).to_le_bytes());
    }
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        DMA_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    for index in 0..COMPLETED {
        storage.descriptors()[index]
            .write_word0(DMA_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    }
    hardware.release_through(COMPLETED - 1, Some(COMPLETED));

    let pool = RxStagePool::<STAGED_DEPTH, STAGE_CAPACITY>::new();
    let observer = RecordingRxObserver::default();
    let mut queue =
        StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, STAGE_CAPACITY, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_pipeline_observer(&observer);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::BudgetExhausted),
    );
    assert_eq!(observer.completed_descriptors.load(Ordering::Relaxed), 32);
    assert_eq!(observer.recycled_descriptors.load(Ordering::Relaxed), 0);
    assert_eq!(service.ring().recycle_start(), 0);
    assert_eq!(pool.claimed_slots(), 32);
    for expected in 0_u32..32 {
        let frame = receiver.try_receive().expect("staged prefix frame");
        assert_eq!(frame.segment().buffer, expected.to_le_bytes());
    }
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::RecycledAppendPending),
    );
    assert_eq!(service.ring().recycle_start(), 32);
    for expected in 32_u32..40 {
        let frame = receiver.try_receive().expect("remaining staged frame");
        assert_eq!(frame.segment().buffer, expected.to_le_bytes());
    }
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn saturated_bulk_rx_discards_upper_copy_and_recycles_without_consuming_critical_credit() {
    const COUNT: usize = 40;
    const COMPLETED: usize = 34;
    const FRAME_LENGTH: usize = PUBLIC_HEADER_SIZE + 24;
    const STAGED: usize = VENDOR_LARGE_RX_SLOT_COUNT - 1;

    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    for index in 0..COMPLETED {
        storage.buffer_mut(index).unwrap()[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 2]
            .copy_from_slice(&0x4008_u16.to_le_bytes());
    }
    let addresses = core::array::from_fn(|index| 0x2f00_2000 + index as u32 * 0x1200);
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    for index in 0..COMPLETED {
        storage.descriptors()[index].write_word0(
            ESP32S31_RX_BUFFER_SIZE as u32
                | ((FRAME_LENGTH as u32) << LENGTH_SHIFT)
                | BIT_30
                | BIT_31,
        );
    }
    hardware.release_through(COMPLETED - 1, Some(COMPLETED));

    let pool = RxStagePool::<VENDOR_LARGE_RX_SLOT_COUNT, ESP32S31_RX_BUFFER_SIZE>::new();
    let observer = RecordingRxObserver::default();
    let mut queue = StagedRxQueue::<
        NoopRawMutex,
        VENDOR_LARGE_RX_SLOT_COUNT,
        ESP32S31_RX_BUFFER_SIZE,
        VENDOR_LARGE_RX_SLOT_COUNT,
    >::new();
    let (sender, receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_pipeline_observer(&observer);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::StageCapacityBlocked),
    );
    assert_eq!(pool.claimed_slots(), STAGED as u32);
    assert_eq!(observer.overload_discarded_units.load(Ordering::Relaxed), 0);
    assert_eq!(
        observer
            .overload_recycled_descriptors
            .load(Ordering::Relaxed),
        0
    );
    assert_eq!(
        observer.critical_reserve_admissions.load(Ordering::Relaxed),
        0
    );
    assert!(!observer.critical_admission_blocked.load(Ordering::Relaxed));
    assert_eq!(observer.minimum_pool_credits.load(Ordering::Relaxed), 1);
    assert_eq!(observer.minimum_queue_credits.load(Ordering::Relaxed), 1);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::RecycledAppendPending),
    );
    assert_eq!(observer.overload_discarded_units.load(Ordering::Relaxed), 3);
    assert_eq!(
        observer
            .overload_recycled_descriptors
            .load(Ordering::Relaxed),
        0
    );
    assert_eq!(service.ring().recycle_start(), 0);
    assert_eq!(pool.available_slots(), 1);

    for _ in 0..STAGED {
        drop(receiver.try_receive().expect("ordinary staged bulk frame"));
    }
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::RecycledAppendPending),
    );
    assert_eq!(service.ring().recycle_start(), COMPLETED);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
}

#[test]
fn burst_returns_to_consumer_before_treating_newly_filled_stage_as_overload() {
    const COUNT: usize = 40;
    const COMPLETED: usize = 34;
    const FRAME_LENGTH: usize = PUBLIC_HEADER_SIZE + 24;
    const STAGED: usize = VENDOR_LARGE_RX_SLOT_COUNT - 1;

    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    for index in 0..COMPLETED {
        storage.buffer_mut(index).unwrap()[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 2]
            .copy_from_slice(&0x4008_u16.to_le_bytes());
    }
    let addresses = core::array::from_fn(|index| 0x2f00_2000 + index as u32 * 0x1200);
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    for index in 0..COMPLETED {
        storage.descriptors()[index].write_word0(
            ESP32S31_RX_BUFFER_SIZE as u32
                | ((FRAME_LENGTH as u32) << LENGTH_SHIFT)
                | BIT_30
                | BIT_31,
        );
    }
    hardware.release_through(COMPLETED - 1, Some(COMPLETED));

    let pool = RxStagePool::<VENDOR_LARGE_RX_SLOT_COUNT, ESP32S31_RX_BUFFER_SIZE>::new();
    let observer = RecordingRxObserver::default();
    let mut queue = StagedRxQueue::<
        NoopRawMutex,
        VENDOR_LARGE_RX_SLOT_COUNT,
        ESP32S31_RX_BUFFER_SIZE,
        VENDOR_LARGE_RX_SLOT_COUNT,
    >::new();
    let (sender, receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_pipeline_observer(&observer);

    // A responsive consumer has not had a scheduling opportunity yet.
    // Filling this turn's available credits is not sustained backpressure.
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::StageCapacityBlocked),
    );
    assert_eq!(observer.overload_discarded_units.load(Ordering::Relaxed), 0);
    assert_eq!(pool.claimed_slots(), STAGED as u32);
    for _ in 0..STAGED {
        drop(receiver.try_receive().expect("first bounded publication"));
    }
    embassy_futures::block_on(service.service(&mut hardware)).unwrap();
    for _ in STAGED..COMPLETED {
        drop(
            receiver
                .try_receive()
                .expect("retained DMA tail must reach consumer"),
        );
    }
    assert!(receiver.try_receive().is_err());
    assert_eq!(
        observer.completed_descriptors.load(Ordering::Relaxed),
        COMPLETED
    );
    assert_eq!(observer.overload_discarded_units.load(Ordering::Relaxed), 0);
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn critical_frame_consumes_the_reserved_final_staging_credit() {
    const COUNT: usize = VENDOR_LARGE_RX_SLOT_COUNT;
    const FRAME_LENGTH: usize = PUBLIC_HEADER_SIZE + 24;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    for index in 0..COUNT {
        let frame_control: u16 = if index + 1 == COUNT { 0x0000 } else { 0x4008 };
        storage.buffer_mut(index).unwrap()[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 2]
            .copy_from_slice(&frame_control.to_le_bytes());
    }
    let addresses = core::array::from_fn(|index| 0x2f00_2000 + index as u32 * 0x1200);
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    for index in 0..COUNT {
        storage.descriptors()[index].write_word0(
            ESP32S31_RX_BUFFER_SIZE as u32
                | ((FRAME_LENGTH as u32) << LENGTH_SHIFT)
                | BIT_30
                | BIT_31,
        );
    }
    hardware.release_through(COUNT - 1, None);
    let pool = RxStagePool::<COUNT, ESP32S31_RX_BUFFER_SIZE>::new();
    let observer = RecordingRxObserver::default();
    let mut queue = StagedRxQueue::<NoopRawMutex, COUNT, ESP32S31_RX_BUFFER_SIZE, COUNT>::new();
    let (sender, _receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_pipeline_observer(&observer);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    assert_eq!(pool.claimed_slots(), COUNT as u32);
    assert_eq!(
        observer.critical_reserve_admissions.load(Ordering::Relaxed),
        1
    );
    assert_eq!(observer.overload_discarded_units.load(Ordering::Relaxed), 0);
    assert_eq!(observer.minimum_pool_credits.load(Ordering::Relaxed), 0);
    assert_eq!(observer.minimum_queue_credits.load(Ordering::Relaxed), 0);
}

#[test]
fn finite_service_discards_oversize_unit_and_keeps_the_ring_live() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    storage.descriptors()[0].write_word0(
        ESP32S31_RX_BUFFER_SIZE as u32
            | ((VENDOR_LARGE_RX_PAYLOAD_CAPACITY as u32 + 1) << LENGTH_SHIFT)
            | BIT_30
            | BIT_31,
    );
    storage.descriptors()[1].write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | BIT_30 | BIT_31);
    hardware.release_through(1, Some(0));
    let pool = RxStagePool::new();
    let observer = RecordingRxObserver::default();
    let mut queue = StagedRxQueue::<NoopRawMutex, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_pipeline_observer(&observer)
        .with_stage_admission_policy(UnreservedRxStageAdmission);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::RecycledAppendPending),
    );
    assert_eq!(service.ring().recycle_start(), 0);
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
    assert_ne!(storage.descriptors()[0].word0() & BIT_31, 0);
    assert_eq!(pool.claimed_slots(), 0);
    assert!(matches!(
        receiver.try_receive(),
        Err(TryReceiveError::Empty)
    ));
    assert_eq!(observer.stage_too_long_discards.load(Ordering::Relaxed), 1);
    hardware.next_descriptor_low = BASE & 0x000f_ffff;

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
    hardware.release_through(1, None);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);

    // The recovered discard path is not a reset frontier: the following
    // descriptor is still accepted, staged and returned to the caller.
    storage.descriptors()[0]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    hardware.release_through(0, None);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    let next = receiver.try_receive().expect("post-discard frame");
    assert_eq!(next.length(), 4);
    drop(next);
    hardware.next_descriptor_low = (BASE + DESCRIPTOR_BYTES) & 0x000f_ffff;
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    assert_eq!(pool.claimed_slots(), 0);
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn one_shot_admission_discards_before_staging_then_observes_same_live_ring() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    let pool = RxStagePool::new();
    let admission = OneShotNarrowAdmission::default();
    let mut queue = StagedRxQueue::<NoopRawMutex, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_stage_admission_policy(&admission);

    storage.descriptors()[0]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    storage.descriptors()[1]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    hardware.release_through(1, Some(0));
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::RecycledAppendPending),
    );
    assert_eq!(admission.0.load(Ordering::Acquire), 3);
    assert_eq!(
        receiver
            .try_receive()
            .expect("following staged unit")
            .length(),
        4
    );

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    assert_eq!(admission.0.load(Ordering::Acquire), 3);
    assert!(matches!(
        receiver.try_receive(),
        Err(TryReceiveError::Empty)
    ));

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    assert_eq!(service.ring().recycle_start(), 1);
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn finite_service_accepts_a_unit_within_a_wider_negotiated_stage() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    const WIDE_STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY + 1;
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    storage.descriptors()[0].write_word0(
        ESP32S31_RX_BUFFER_SIZE as u32
            | ((WIDE_STAGE_CAPACITY as u32) << LENGTH_SHIFT)
            | BIT_30
            | BIT_31,
    );
    storage.descriptors()[1].write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | BIT_30 | BIT_31);
    hardware.release_through(1, Some(0));
    let pool = RxStagePool::<VENDOR_LARGE_RX_SLOT_COUNT, WIDE_STAGE_CAPACITY>::new();
    let mut queue = StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, WIDE_STAGE_CAPACITY>::new();
    let (sender, receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_stage_admission_policy(UnreservedRxStageAdmission);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::RecycledAppendPending),
    );
    let frame = receiver.try_receive().expect("wide staged frame");
    assert_eq!(frame.length(), WIDE_STAGE_CAPACITY);
    drop(frame);
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::RecycledAppendPending),
    );
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

mod credit_policy;
mod transaction;
