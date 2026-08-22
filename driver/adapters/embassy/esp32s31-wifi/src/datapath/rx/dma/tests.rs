#![expect(
    clippy::manual_async_fn,
    reason = "test doubles implement the same explicit borrowed Future service contracts"
)]

use core::{
    future::{Future, ready},
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

use embassy_sync::channel::TryReceiveError;
use open_esp_radio_embassy_net::NoopRawMutex;
use open_esp_radio_embassy_net::RxEnqueueError;
use open_esp_radio_esp32s31_wifi_dma::descriptor::{
    BIT_30, BIT_31, DESCRIPTOR_BYTES, LENGTH_SHIFT,
};
use open_esp_radio_esp32s31_wifi_mac::rx::{
    PUBLIC_HEADER_SIZE, RxDmaBinding, RxDmaWalkerStopped, RxIngressConfig, RxRingStopped,
};
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{
    ConnectedRxConfig, ConnectedRxDispatcher, ConnectedRxEvent, ConnectedRxSink,
};
use open_esp_radio_ieee80211::data::EthernetFrameParts;
use open_esp_radio_ieee80211::vif::StaApRxAddresses;
use std::boxed::Box;

use super::*;
use crate::{
    datapath::irq::EmbassyMacIrqRuntime,
    datapath::rx::reorder::{
        RxBlockAckSnapshot, RxReorderCommand, RxReorderCommandResources,
        try_send_rx_reorder_command,
    },
    datapath::rx::staging::{Esp32s31StagedRxFrame, Esp32s31StagedRxQueue},
    datapath::{
        DatapathRxServiceContext, network::DatapathNetworkRx, paired::DatapathPairedRxService,
    },
    roles::concurrent::Esp32s31StaApStagedRxQueue,
    roles::station::rx_protocol::{
        AlwaysReadyConnectedRxSink, Esp32s31ConnectedRxProcessor, Esp32s31ConnectedRxProtocol,
        Esp32s31ConnectedRxProtocolStorage,
    },
};

const BASE: u32 = 0x2f00_1000;

#[test]
fn one_physical_producer_routes_one_ordered_lease_into_station_processor() {
    const COUNT: usize = 2;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const MPDU_LENGTH: usize = 24;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const RECEIVED: usize = FRAME_OFFSET + MPDU_LENGTH;
    const STA: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
    const UPLINK: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
    const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 3];
    const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 4];

    let mut storage = Esp32s31RxDmaStorage::<COUNT>::new();
    let initialize_frame = |buffer: &mut [u8],
                            frame_control: u16,
                            receiver: [u8; 6],
                            transmitter: [u8; 6],
                            third: [u8; 6]| {
        buffer[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
            &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
        );
        buffer[FRAME_OFFSET..FRAME_OFFSET + 2].copy_from_slice(&frame_control.to_le_bytes());
        buffer[FRAME_OFFSET + 4..FRAME_OFFSET + 10].copy_from_slice(&receiver);
        buffer[FRAME_OFFSET + 10..FRAME_OFFSET + 16].copy_from_slice(&transmitter);
        buffer[FRAME_OFFSET + 16..FRAME_OFFSET + 22].copy_from_slice(&third);
    };
    initialize_frame(
        storage.buffer_mut(0).expect("station DMA buffer"),
        0x0208,
        STA,
        UPLINK,
        PEER,
    );
    initialize_frame(
        storage.buffer_mut(1).expect("AP DMA buffer"),
        0x0108,
        AP,
        PEER,
        STA,
    );

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
        ESP32S31_RX_BUFFER_SIZE as u32 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
    );
    storage.descriptors()[1].write_word0(
        ESP32S31_RX_BUFFER_SIZE as u32 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
    );
    hardware.release_through(1, None);

    let pool = RxStagePool::<2, ESP32S31_RX_BUFFER_SIZE>::new();
    let queue = Esp32s31StaApStagedRxQueue::<NoopRawMutex, 2, ESP32S31_RX_BUFFER_SIZE, 2>::new();
    let (sender, mut receiver) = queue.split();
    let mut service = Esp32s31StagedRxProducer::new_sta_ap(
        ring,
        &storage,
        &pool,
        NoDelay,
        sender,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        StaApRxAddresses {
            station: STA,
            station_bssid: UPLINK,
            access_point: AP,
        },
    );

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    assert_eq!(pool.claimed_slots(), 2);

    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0; ESP32S31_RX_BUFFER_SIZE];
    let mut ethernet = [0; ESP32S31_RX_BUFFER_SIZE];
    let runtime = Box::leak(Box::new(Esp32s31ConnectedRxProtocolStorage::new()));
    let mut processor = Esp32s31ConnectedRxProcessor::new(
        &irq,
        dispatcher(),
        AlwaysReadyConnectedRxSink(Observer::default()),
        &mut mpdu,
        &mut ethernet,
        runtime,
    );
    let turn = embassy_futures::block_on(receiver.service_next(
        |frame| processor.dispatch_frame(frame),
        |frame| -> Result<
            crate::roles::concurrent::Esp32s31RoutedRxDisposition<_>,
            core::convert::Infallible,
        > {
            drop(frame);
            panic!("from-DS unit must never reach the AP processor")
        },
    ))
    .expect("station dispatch is infallible");
    assert!(matches!(
        turn,
        crate::roles::concurrent::Esp32s31StaApRxTurn::Station(Some(_))
    ));
    assert_eq!(pool.claimed_slots(), 1);

    let deferred = embassy_futures::block_on(receiver.service_next(
        |frame| {
            drop(frame);
            ready(())
        },
        |frame| {
            Ok::<_, core::convert::Infallible>(
                crate::roles::concurrent::Esp32s31RoutedRxDisposition::Deferred(frame),
            )
        },
    ))
    .expect("AP deferral is infallible");
    assert_eq!(
        deferred,
        crate::roles::concurrent::Esp32s31StaApRxTurn::DeferredAccessPoint
    );
    assert_eq!(receiver.queued_frames(), 1);
    assert_eq!(pool.claimed_slots(), 1);

    let processed = embassy_futures::block_on(receiver.service_next(
        |frame| {
            drop(frame);
            ready(())
        },
        |frame| {
            drop(frame);
            Ok::<_, core::convert::Infallible>(
                crate::roles::concurrent::Esp32s31RoutedRxDisposition::Processed,
            )
        },
    ))
    .expect("AP processing is infallible");
    assert_eq!(
        processed,
        crate::roles::concurrent::Esp32s31StaApRxTurn::AccessPoint
    );
    assert_eq!(pool.claimed_slots(), 0);
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

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

    #[cfg(feature = "diagnostics")]
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

    #[cfg(feature = "diagnostics")]
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

#[derive(Default)]
struct PairedStationRole {
    frames: usize,
}

impl<'pool, const CAPACITY: usize, const SLOTS: usize>
    crate::roles::concurrent::Esp32s31StaApStationRxRole<'pool, CAPACITY, SLOTS>
    for PairedStationRole
{
    type Dispatch = ();
    type Error = core::convert::Infallible;

    fn publish_pending_rx(
        &mut self,
        _network: &mut dyn DatapathNetworkRx,
    ) -> Result<DatapathRxProgress, Self::Error> {
        Ok(DatapathRxProgress::Drained)
    }

    fn service_station_rx<'a>(
        &'a mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
        _network: &'a mut dyn DatapathNetworkRx,
    ) -> impl Future<Output = Result<Self::Dispatch, Self::Error>> + 'a
    where
        'pool: 'a,
    {
        async move {
            self.frames += 1;
            drop(frame);
            Ok(())
        }
    }

    fn has_pending_rx(&self) -> bool {
        false
    }
}

#[derive(Default)]
struct PairedAccessPointRole {
    frames: usize,
    tx_pending: bool,
}

impl<'pool, H, PhysicalTx, const CAPACITY: usize, const SLOTS: usize>
    crate::roles::concurrent::Esp32s31StaApAccessPointRxRole<'pool, H, PhysicalTx, CAPACITY, SLOTS>
    for PairedAccessPointRole
{
    type Error = core::convert::Infallible;

    fn publish_pending_rx(
        &mut self,
        _physical_tx: &mut PhysicalTx,
        _network: &mut dyn DatapathNetworkRx,
    ) -> Result<DatapathRxProgress, Self::Error> {
        Ok(DatapathRxProgress::Drained)
    }

    fn service_access_point_rx(
        &mut self,
        _hardware: &mut H,
        _physical_tx: &mut PhysicalTx,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Result<
        crate::roles::concurrent::Esp32s31RoutedRxDisposition<
            Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
        >,
        Self::Error,
    > {
        self.frames += 1;
        self.tx_pending = true;
        drop(frame);
        Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Processed)
    }

    fn service_access_point_rx_during_tx(
        &mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Result<
        crate::roles::concurrent::Esp32s31RoutedRxDisposition<
            Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
        >,
        Self::Error,
    > {
        self.frames += 1;
        drop(frame);
        Ok(crate::roles::concurrent::Esp32s31RoutedRxDisposition::Processed)
    }

    fn has_pending_rx(&self) -> bool {
        false
    }

    fn tx_pending(&self) -> bool {
        self.tx_pending
    }
}

#[test]
fn paired_datapath_rx_uses_one_dma_epoch_and_two_narrow_role_capabilities() {
    const COUNT: usize = 2;
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = TAIL_OFFSET + 8;
    const MPDU_LENGTH: usize = 24;
    const SIGNAL_LENGTH: usize = MPDU_LENGTH + 4;
    const RECEIVED: usize = FRAME_OFFSET + MPDU_LENGTH;
    const STA: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
    const UPLINK: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
    const AP: [u8; 6] = [0x02, 0, 0, 0, 0, 3];
    const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 4];

    let mut storage = Esp32s31RxDmaStorage::<COUNT>::new();
    for (index, (frame_control, receiver, transmitter, third)) in
        [(0x0208_u16, STA, UPLINK, PEER), (0x0108_u16, AP, PEER, STA)]
            .into_iter()
            .enumerate()
    {
        let buffer = storage.buffer_mut(index).expect("paired DMA buffer");
        buffer[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
            &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
        );
        buffer[FRAME_OFFSET..FRAME_OFFSET + 2].copy_from_slice(&frame_control.to_le_bytes());
        buffer[FRAME_OFFSET + 4..FRAME_OFFSET + 10].copy_from_slice(&receiver);
        buffer[FRAME_OFFSET + 10..FRAME_OFFSET + 16].copy_from_slice(&transmitter);
        buffer[FRAME_OFFSET + 16..FRAME_OFFSET + 22].copy_from_slice(&third);
    }

    let addresses = [0x2f00_2000, 0x2f00_3200];
    let mut hardware = MockRxDma::default();
    let halted = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap()
    .into_halted();
    let pool = RxStagePool::<2, ESP32S31_RX_BUFFER_SIZE>::new();
    let queue = Esp32s31StaApStagedRxQueue::<NoopRawMutex, 2, ESP32S31_RX_BUFFER_SIZE, 2>::new();
    let (sender, consumer) = queue.split();
    let epoch = Esp32s31StagedRxEpoch::from_halted_sta_ap(
        halted,
        &storage,
        &pool,
        sender,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        StaApRxAddresses {
            station: STA,
            station_bssid: UPLINK,
            access_point: AP,
        },
        NoDelay,
    );
    let mut service = crate::roles::concurrent::Esp32s31StaApRxService::new(epoch, consumer);
    embassy_futures::block_on(service.start(&mut hardware)).unwrap();

    for descriptor in storage.descriptors() {
        descriptor.write_word0(
            ESP32S31_RX_BUFFER_SIZE as u32 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        );
    }
    hardware.release_through(1, None);

    let mut station = PairedStationRole::default();
    let mut access_point = PairedAccessPointRole::default();
    let mut station_network = PairedNetworkRx::default();
    let mut access_point_network = PairedNetworkRx::default();
    let progress = embassy_futures::block_on(DatapathPairedRxService::service(
        &mut service,
        &mut hardware,
        &mut (),
        &mut station,
        &mut access_point,
        &mut station_network,
        &mut access_point_network,
        DatapathRxServiceContext {
            maximum_protocol_frames: Some(2),
        },
    ));

    assert_eq!(
        progress,
        Ok(
            crate::datapath::paired::DatapathPairedRxProgress::TxPending(
                crate::datapath::paired::DatapathPairRole::Second
            )
        )
    );
    assert_eq!(station.frames, 1);
    assert_eq!(access_point.frames, 1);
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(service.serviced_frames(), 2);

    let (mut epoch, consumer) = service.into_parts();
    assert_eq!(consumer.queued_frames(), 0);
    epoch.stop(&mut hardware).unwrap();
    drop(consumer);

    let standalone = Esp32s31StagedRxQueue::<NoopRawMutex, 2, ESP32S31_RX_BUFFER_SIZE, 2>::new();
    let (standalone_sender, _standalone_receiver) = standalone.split();
    let stopped = match epoch.try_into_standalone_stopped(standalone_sender) {
        Ok(stopped) => stopped,
        Err(_) => panic!("a stopped and drained paired epoch must return to standalone STA"),
    };
    let (paired_sender, _paired_consumer) = queue.split();
    let epoch = match Esp32s31StagedRxEpoch::try_from_stopped_sta_ap(
        stopped,
        paired_sender,
        RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        StaApRxAddresses {
            station: STA,
            station_bssid: UPLINK,
            access_point: AP,
        },
    ) {
        Ok(epoch) => epoch,
        Err(_) => panic!("an empty standalone frontier must enter paired routing"),
    };
    assert!(epoch.try_into_halted().is_ok());
}

#[derive(Default)]
struct RecordingRxObserver {
    stage_too_long_discards: AtomicU32,
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

    fn next_descriptor_word(&mut self) -> u32 {
        self.next_descriptor_low()
    }
    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaCursorObservation<'confirmation>,
        ) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        self.fence();
        let next = self.next_descriptor_low();
        self.fence();
        observed(
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaCursorObservation::validation(last, next),
        )
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
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaReloadSettled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        (!self.reload_pending()).then(|| {
            settled(open_esp_radio_esp32s31_wifi_mac::rx::RxDmaReloadSettled::validation())
        })
    }
    fn set_descriptor_high_window(&mut self, _: &RxDmaBinding, _address_high: u16) {}
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
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        if self.fail_enable {
            return None;
        }
        self.walker = true;
        Some(enabled(
            open_esp_radio_esp32s31_wifi_mac::rx::RxDmaWalkerEnabled::validation(),
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

struct NoDelay;

impl RxDmaObservationDelay for NoDelay {
    fn after_micros(&mut self, _micros: u32) -> impl Future<Output = ()> + '_ {
        ready(())
    }
}

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

fn dispatcher() -> ConnectedRxDispatcher {
    ConnectedRxDispatcher::new(ConnectedRxConfig {
        station_address: [2, 3, 4, 5, 6, 7],
        bssid: [8, 9, 10, 11, 12, 13],
        association_id: 1,
        ingress: RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
    })
}

#[test]
fn finite_service_uses_queue_credits_and_protocol_dispatch_returns_ownership() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    let storage = Esp32s31RxDmaStorage::<COUNT>::new();
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
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (8 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    storage.descriptors()[1]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (8 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    hardware.release_through(1, Some(0));
    let pool = RxStagePool::new();
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0; ESP32S31_RX_BUFFER_SIZE];
    let mut ethernet = [0; ESP32S31_RX_BUFFER_SIZE];
    let protocol_runtime = Box::leak(Box::new(Esp32s31ConnectedRxProtocolStorage::new()));
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender);
    let mut protocol = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        dispatcher(),
        AlwaysReadyConnectedRxSink(Observer::default()),
        &mut mpdu,
        &mut ethernet,
        protocol_runtime,
    );

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::CriticalAdmissionBlocked),
    );
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(pool.network_slots(), 1);
    assert_eq!(protocol.queue_len(), 1);
    embassy_futures::block_on(protocol.dispatch_next());
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(pool.network_slots(), 0);
    assert_eq!(service.ring().recycle_start(), 1);
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
    assert_ne!(storage.descriptors()[0].word0() & BIT_31, 0);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    assert_eq!(service.ring().recycle_start(), 0);
    assert_eq!(protocol.queue_len(), 1);
    embassy_futures::block_on(protocol.dispatch_next());
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(pool.network_slots(), 0);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn connected_rx_stop_confirms_walker_off_and_preserves_static_resources() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    let storage = Esp32s31RxDmaStorage::<COUNT>::new();
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
    let queue = Esp32s31StagedRxQueue::<
        NoopRawMutex,
        STAGED_DEPTH,
        ESP32S31_RX_BUFFER_SIZE,
        STAGED_DEPTH,
    >::new();
    let (sender, _receiver) = queue.split();
    let service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender);
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
fn exhausted_cursor_keeps_service_live_until_terminal_writeback_arrives() {
    const COUNT: usize = 4;
    const STAGE_CAPACITY: usize = 16;
    let storage = Esp32s31RxDmaStorage::<COUNT>::new();
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
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, COUNT, STAGE_CAPACITY, COUNT>::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender);

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
        Ok(DatapathRxProgress::ProbePending),
    );
    assert_eq!(receiver.len(), 4);
    assert_eq!(service.ring().observed_mask(), 0);
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn finite_service_stages_a_descriptor_chain_as_one_contiguous_unit() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    const STAGE_CAPACITY: usize = 16;
    let mut storage = Esp32s31RxDmaStorage::<COUNT>::new();
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
    let pool = RxStagePool::<STAGED_DEPTH, STAGE_CAPACITY>::new();
    let queue =
        Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, STAGE_CAPACITY, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    let frame = receiver.try_receive().expect("one chained staged unit");
    assert_eq!(frame.segment().buffer, &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(service.ring().recycle_start(), 0);
    // The frozen LAST releases its complete chained unit inclusively, exactly
    // as the vendor worker processes the descriptor equal to saved LAST.
    assert_ne!(storage.descriptors()[0].word0() & BIT_31, 0);
    assert_eq!(storage.descriptors()[1].word0() & BIT_30, 0);
    drop(frame);
    assert_eq!(pool.claimed_slots(), 0);
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn frozen_last_reclaims_every_complete_observed_unit_through_its_frontier() {
    const COUNT: usize = 4;
    const STAGE_CAPACITY: usize = 16;
    let mut storage = Esp32s31RxDmaStorage::<COUNT>::new();
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
    let pool = RxStagePool::<COUNT, STAGE_CAPACITY>::new();
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, COUNT, STAGE_CAPACITY, COUNT>::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender);

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
    let frame = receiver.try_receive().expect("one staged unit");
    assert_eq!(frame.segment().buffer, &[1, 2, 3, 4, 5, 6, 7, 8]);
    drop(frame);
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
    let mut storage = Esp32s31RxDmaStorage::<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>::new();
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
    let queue =
        Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, STAGE_CAPACITY, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender)
        .with_pipeline_observer(&observer);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::BudgetExhausted),
    );
    assert_eq!(observer.completed_descriptors.load(Ordering::Relaxed), 32);
    assert_eq!(observer.recycled_descriptors.load(Ordering::Relaxed), 32);
    assert_eq!(service.ring().recycle_start(), 32);
    assert_eq!(pool.claimed_slots(), 32);
    for expected in 0_u32..32 {
        let frame = receiver.try_receive().expect("staged prefix frame");
        assert_eq!(frame.segment().buffer, expected.to_le_bytes());
    }
    assert_eq!(pool.claimed_slots(), 0);
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

    let mut storage = Esp32s31RxDmaStorage::<COUNT>::new();
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
    let queue = Esp32s31StagedRxQueue::<
        NoopRawMutex,
        VENDOR_LARGE_RX_SLOT_COUNT,
        ESP32S31_RX_BUFFER_SIZE,
        VENDOR_LARGE_RX_SLOT_COUNT,
    >::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender)
        .with_pipeline_observer(&observer);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::BudgetExhausted),
    );
    assert_eq!(pool.claimed_slots(), STAGED as u32);
    assert_eq!(observer.overload_discarded_units.load(Ordering::Relaxed), 1);
    assert_eq!(
        observer
            .overload_recycled_descriptors
            .load(Ordering::Relaxed),
        1
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
        Ok(DatapathRxProgress::ProbePending),
    );
    assert_eq!(observer.overload_discarded_units.load(Ordering::Relaxed), 3);
    assert_eq!(
        observer
            .overload_recycled_descriptors
            .load(Ordering::Relaxed),
        3
    );
    assert_eq!(service.ring().recycle_start(), COMPLETED);
    assert_eq!(pool.available_slots(), 1);

    for _ in 0..STAGED {
        drop(receiver.try_receive().expect("ordinary staged bulk frame"));
    }
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn critical_frame_consumes_the_reserved_final_staging_credit() {
    const COUNT: usize = VENDOR_LARGE_RX_SLOT_COUNT;
    const FRAME_LENGTH: usize = PUBLIC_HEADER_SIZE + 24;
    let mut storage = Esp32s31RxDmaStorage::<COUNT>::new();
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
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, COUNT, ESP32S31_RX_BUFFER_SIZE, COUNT>::new();
    let (sender, _receiver) = queue.split();
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender)
        .with_pipeline_observer(&observer);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
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
fn negotiated_rx_block_ack_releases_staged_leases_in_sequence_order() {
    const COUNT: usize = 4;
    const STAGED_DEPTH: usize = 3;
    const STAGE_CAPACITY: usize = 192;
    const MPDU: usize = 26 + 8 + 8 + 4 + 8;
    const SIGNAL: usize = MPDU + 4;
    const RECEIVED: usize = PUBLIC_HEADER_SIZE + SIGNAL;

    let mut storage = Esp32s31RxDmaStorage::<COUNT>::new();
    let addresses = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut hardware = MockRxDma::default();

    let mut buffer = [0_u8; ESP32S31_RX_BUFFER_SIZE];
    for (index, sequence) in [102_u16, 100, 101].into_iter().enumerate() {
        buffer.fill(0);
        buffer[0x38..0x3c]
            .copy_from_slice(&(((SIGNAL + 4) as u32) << 16 | SIGNAL as u32).to_le_bytes());
        let frame = &mut buffer[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + MPDU];
        frame[..2].copy_from_slice(&0x4288_u16.to_le_bytes());
        frame[4..10].copy_from_slice(&[2, 3, 4, 5, 6, 7]);
        frame[10..16].copy_from_slice(&[8, 9, 10, 11, 12, 13]);
        frame[16..22].copy_from_slice(&[14, 15, 16, 17, 18, 19]);
        frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
        frame[24] = 0;
        frame[26..34].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
        frame[34..42].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
        frame[42..46].copy_from_slice(&sequence.to_be_bytes().repeat(2));
        storage
            .buffer_mut(index)
            .expect("test RX buffer exists")
            .copy_from_slice(&buffer);
    }

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
        storage.descriptors()[index].write_word0(
            ESP32S31_RX_BUFFER_SIZE as u32 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        );
    }
    storage.descriptors()[3].write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | BIT_30 | BIT_31);
    hardware.release_through(3, Some(0));

    let pool = RxStagePool::<STAGED_DEPTH, STAGE_CAPACITY>::new();
    // Declared before the queue because protocol frame types borrow both
    // pools and Rust drops local owners in reverse declaration order.
    let reorder_storage =
        crate::datapath::rx::reorder::RxReorderFrameStorage::<STAGE_CAPACITY>::new();
    let queue =
        Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, STAGE_CAPACITY, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let reorder_resources = RxReorderCommandResources::<NoopRawMutex>::new();
    let (reorder_sender, reorder_receiver) = reorder_resources.split();
    try_send_rx_reorder_command(
        &reorder_sender,
        RxReorderCommand::Start(RxBlockAckSnapshot {
            hardware_index: 0,
            interface: open_esp_radio_esp32s31_wifi_mac::MacInterface::Station,
            peer: [8, 9, 10, 11, 12, 13],
            tid: 0,
            starting_sequence: 100,
            window: 8,
        }),
    )
    .unwrap();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0; STAGE_CAPACITY];
    let mut ethernet = [0; STAGE_CAPACITY];
    let mut reorder_scratch = [0; STAGE_CAPACITY];
    let protocol_runtime = Box::leak(Box::new(Esp32s31ConnectedRxProtocolStorage::new()));
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender);
    let mut protocol = Esp32s31ConnectedRxProtocol::new(
        receiver,
        &irq,
        dispatcher(),
        AlwaysReadyConnectedRxSink(OrderObserver::default()),
        &mut mpdu,
        &mut ethernet,
        protocol_runtime,
    )
    .with_rx_reorder_commands(reorder_receiver)
    .with_rx_reorder_storage(&reorder_storage)
    .with_rx_reorder_scratch(&mut reorder_scratch);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    assert_eq!(pool.claimed_slots(), 3);
    embassy_futures::block_on(protocol.dispatch_next());
    assert_eq!(protocol.sink().0.0, [100]);
    // Sequence 102 is retained in the cold reorder backing while 100 has
    // been dispatched. The former implementation retained both staging
    // leases here and therefore reported two claimed SRAM slots.
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(
        reorder_storage.available_slots(),
        crate::datapath::rx::reorder::RX_REORDER_BACKING_SLOT_COUNT - 1
    );
    embassy_futures::block_on(protocol.dispatch_next());
    assert_eq!(protocol.sink().0.0, [100, 101, 102]);
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(
        reorder_storage.available_slots(),
        crate::datapath::rx::reorder::RX_REORDER_BACKING_SLOT_COUNT
    );
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
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
fn finite_service_discards_oversize_unit_and_keeps_the_ring_live() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    let storage = Esp32s31RxDmaStorage::<COUNT>::new();
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
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender)
        .with_pipeline_observer(&observer);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
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
        Ok(DatapathRxProgress::ProbePending),
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
    let storage = Esp32s31RxDmaStorage::<COUNT>::new();
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
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH>::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender)
        .with_stage_admission_policy(&admission);

    storage.descriptors()[0]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    storage.descriptors()[1]
        .write_word0(ESP32S31_RX_BUFFER_SIZE as u32 | (4 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    hardware.release_through(1, Some(0));
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
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
    assert_eq!(service.ring().recycle_start(), 0);
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}

#[test]
fn finite_service_accepts_a_unit_within_a_wider_negotiated_stage() {
    const COUNT: usize = 2;
    const STAGED_DEPTH: usize = 1;
    const WIDE_STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY + 1;
    let storage = Esp32s31RxDmaStorage::<COUNT>::new();
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
    let queue = Esp32s31StagedRxQueue::<NoopRawMutex, STAGED_DEPTH, WIDE_STAGE_CAPACITY>::new();
    let (sender, receiver) = queue.split();
    let mut service = Esp32s31StagedRxProducer::new(ring, &storage, &pool, NoDelay, sender);

    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::ProbePending),
    );
    let frame = receiver.try_receive().expect("wide staged frame");
    assert_eq!(frame.length(), WIDE_STAGE_CAPACITY);
    drop(frame);
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    assert_eq!(
        embassy_futures::block_on(service.service(&mut hardware)),
        Ok(DatapathRxProgress::Drained),
    );
    service
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}
