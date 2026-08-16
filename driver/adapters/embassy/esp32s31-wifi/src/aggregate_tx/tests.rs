use core::{
    future::{Future, ready},
    pin::Pin,
    task::{Context, Waker},
};

use open_esp_radio_embassy_net::{
    Driver as _, NoopRawMutex, PinnedTxPool, SplitPinnedDevice, SplitPinnedResources, TxToken as _,
};
use open_esp_radio_esp32s31_hal::types::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHtAmpduCompletionRegisters, MacHtTxProgram,
    MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionRegisters, MacTxDetachOutcome,
    MacTxDetachReason, MacTxQueueDetached,
};
use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxPowerPair, WifiTxResources};
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::{CcmpKeyHardware, install_sta_pairwise_ccmp},
    rx::HeGuardIntervalAndLtf,
    tx::{
        HardwareOwnedTxDma, HeMcs, HeRate, HtChannelWidth, HtGuardInterval, HtMcs, HtRate,
        LegacyRate, PreparedTxDma, TxSlot,
    },
    tx_ampdu::HtAmpduTxStorage,
    tx_runtime::WifiTxRuntimePolicy,
};
use open_esp_radio_esp32s31_wifi_sta::{
    connected_control::ConnectedControlTx,
    single_mpdu_tx::{ActionTxConfig, ConnectedTxHandoff, SingleMpduTxConfig},
};
use open_esp_radio_ieee80211::station::{
    STA_PROTECTED_QOS_ETHERNET_HEADROOM, StaTxSequenceCounters,
};
use open_esp_radio_ieee80211::wmm::WmmAccessCategory;
use open_esp_radio_wifi_softmac::MacTxPlan;

use super::*;

#[derive(Default)]
struct RecordingAggregateTxObserver {
    observations: std::sync::Mutex<std::vec::Vec<AggregateTxObservation>>,
}

impl AggregateTxObserver for RecordingAggregateTxObserver {
    fn observe(&self, observation: AggregateTxObservation) {
        self.observations.lock().unwrap().push(observation);
    }
}

impl RecordingAggregateTxObserver {
    fn observed(&self, expected: AggregateTxObservation) -> bool {
        self.observations.lock().unwrap().contains(&expected)
    }

    fn count(&self, expected: AggregateTxObservation) -> usize {
        self.observations
            .lock()
            .unwrap()
            .iter()
            .filter(|observation| **observation == expected)
            .count()
    }
}

const STATION: [u8; 6] = [2, 3, 4, 5, 6, 7];
const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
const TEST_FRAME_CAPACITY: usize = 64;
const TEST_HEADROOM: usize = open_esp_radio_esp32s31_wifi_mac::tx_ampdu::TX_AMPDU_METADATA_SIZE
    + STA_PROTECTED_QOS_ETHERNET_HEADROOM;
const TEST_TRAILER: usize = 12;
const TEST_QUEUE_DEPTH: usize = 3;
const TEST_SLOTS: usize = 3;
const TEST_BUFFER_SIZE: usize = 256;
const TEST_RATE: HtRate = HtRate::new(
    HtMcs::Mcs7,
    HtGuardInterval::Short400Ns,
    HtChannelWidth::Mhz20,
);

type Resources = SplitPinnedResources<
    NoopRawMutex,
    TEST_FRAME_CAPACITY,
    TEST_HEADROOM,
    TEST_TRAILER,
    TEST_QUEUE_DEPTH,
    TEST_QUEUE_DEPTH,
>;
type Pool = PinnedTxPool<TEST_FRAME_CAPACITY, TEST_HEADROOM, TEST_TRAILER, TEST_QUEUE_DEPTH>;
type Device = SplitPinnedDevice<
    'static,
    NoopRawMutex,
    TEST_FRAME_CAPACITY,
    TEST_HEADROOM,
    TEST_TRAILER,
    TEST_QUEUE_DEPTH,
    TEST_QUEUE_DEPTH,
>;

#[derive(Default)]
struct Hardware {
    legacy_publications: usize,
    ht_publications: usize,
    he_publications: usize,
    ordinary_completion: Option<MacTxCompletionRegisters>,
    aggregate_completion: Option<MacHtAmpduCompletionRegisters>,
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(&mut self, _index: u8, _words: [u32; 6]) -> MacKeyInstallOutcome {
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, _index: u8) {}
}

impl open_esp_radio_esp32s31_wifi_mac::tx::TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacLegacyTxProgram,
    ) -> bool {
        self.legacy_publications += 1;
        true
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8, _plcp0: u32) {}

    fn prepare_bound_ht_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacHtTxProgram,
    ) -> bool {
        self.ht_publications += 1;
        true
    }

    fn start_bound_ht_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8, _plcp0: u32) {}

    fn prepare_bound_he_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacHeTxProgram,
    ) -> bool {
        self.he_publications += 1;
        true
    }

    fn start_bound_he_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8, _plcp0: u32) {}

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionRegisters> {
        self.ordinary_completion.take()
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        true
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        match reason {
            MacTxDetachReason::Timeout => MacTxDetachOutcome::Failed,
            MacTxDetachReason::Collision | MacTxDetachReason::Completed => {
                MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                    expected_descriptor_head,
                )))
            }
        }
    }
}

impl HtAmpduHardware for Hardware {
    fn take_ht_ampdu_completion(&mut self, _queue: u8) -> Option<MacHtAmpduCompletionRegisters> {
        self.aggregate_completion.take()
    }

    fn prepare_he_trigger_based_queue(
        &mut self,
        _policy: MacHeTbTidLimit,
        _reservation: MacHeTbLinkReservation,
        _tid: MacHeTid,
        _mpdu_lengths: &[u16],
        _queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        unreachable!("HT tests never publish a trigger-based HE queue")
    }

    fn clear_he_trigger_based_queue(&mut self, _reservation: MacHeTbLinkReservation) {}
}

struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 5,
            alternate: 6,
        }
    }
}

#[derive(Default)]
struct Timer {
    now: u64,
}

impl WifiTxTimer for Timer {
    fn now_micros(&self) -> u64 {
        self.now
    }

    fn wait_until(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        self.now = deadline_micros;
        ready(())
    }

    fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_ {
        self.now += micros;
        ready(())
    }
}

fn context() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

fn send_frame(device: &mut Device, marker: u8) {
    device
        .transmit(&mut context())
        .expect("free pinned network slot")
        .consume(17, |frame| {
            frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
            frame[6..12].copy_from_slice(&STATION);
            frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
            frame[14..].fill(marker);
        });
}

fn send_short_frame(device: &mut Device) {
    device
        .transmit(&mut context())
        .expect("free pinned network slot")
        .consume(8, |frame| frame.fill(0));
}

fn aggregate_completion(starting_sequence: u16, bitmap: u64) -> MacHtAmpduCompletionRegisters {
    MacHtAmpduCompletionRegisters {
        tx: MacTxCompletionRegisters {
            aux_a: 0,
            aux_b: 0,
            aux_c: 0,
            primary: 0,
            alternate: 0,
            trigger_flow: false,
        },
        block_ack_control_and_sequence: u32::from(starting_sequence & 0x0fff) << 4,
        block_ack_bitmap_low: bitmap as u32,
        block_ack_bitmap_high: (bitmap >> 32) as u32,
        block_ack_received: true,
    }
}

fn make_ordinary<'a, const BUFFER_SIZE: usize>(
    slot: Pin<&'a mut TxSlot<BUFFER_SIZE>>,
    hardware: &mut Hardware,
) -> Esp32s31SingleMpduTx<'a, Power, fn() -> u32, Timer, BUFFER_SIZE> {
    fn entropy() -> u32 {
        0x1234_5678
    }

    let key = install_sta_pairwise_ccmp(hardware, BSSID, &[0x5a; 16]).unwrap();
    Esp32s31SingleMpduTx::new(
        WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy,
            timer: Timer::default(),
        },
        ConnectedTxHandoff {
            key,
            sequences: StaTxSequenceCounters::new(7),
            config: SingleMpduTxConfig {
                station_address: STATION,
                bssid: BSSID,
                peer_qos: true,
                exchange: MacTxPlan {
                    access_category: WmmAccessCategory::BestEffort,
                    initial_rate: TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                    publication_limit: 2,
                    publication_timeout_micros: 250_000,
                },
            },
        },
    )
}

fn make_network() -> (
    Device,
    open_esp_radio_embassy_net::SplitPinnedRadioRunner<
        'static,
        NoopRawMutex,
        TEST_FRAME_CAPACITY,
        TEST_HEADROOM,
        TEST_TRAILER,
        TEST_QUEUE_DEPTH,
        TEST_QUEUE_DEPTH,
    >,
) {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    resources.split(pool, STATION)
}

#[test]
fn idle_aggregate_returns_ordinary_and_storage_for_station_teardown() {
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let tx = Esp32s31ConnectedTx::<
        NoopRawMutex,
        _,
        _,
        _,
        TEST_FRAME_CAPACITY,
        TEST_HEADROOM,
        TEST_TRAILER,
        TEST_QUEUE_DEPTH,
        TEST_SLOTS,
        0,
        TEST_BUFFER_SIZE,
    >::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    assert!(
        core::mem::size_of_val(&tx) < 2_048,
        "connected TX must remain a movable handle over external retention arenas"
    );

    let returned = match tx.try_into_station_parts() {
        Ok(parts) => parts,
        Err(_) => panic!("idle aggregate must decompose"),
    };
    assert_eq!(returned.aggregate.primary().state(), TxSlotState::Free);
    assert_eq!(returned.resources.slot.state(), TxSlotState::Free);
    assert_eq!(returned.sequences.peek_non_qos(), 7);
    returned.pairwise_key.clear(&mut hardware);
}

#[test]
fn first_frame_outside_fresh_aggregate_txop_falls_back_to_ordinary_tx() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);
    let first = network.try_receive_tx().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::He(HeRate::new(HeMcs::Mcs0, HeGuardIntervalAndLtf::TwoLtf800Ns)),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::from_units_32_us(5).unwrap(),
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    assert_eq!(
        observer.count(AggregateTxObservation::BlockAckOperational {
            tid: 0,
            operational: true,
        }),
        1,
    );

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending),
    );
    assert_eq!(hardware.legacy_publications, 1);
    assert_eq!(hardware.ht_publications, 0);
    assert!(
        observer.observed(AggregateTxObservation::NetworkSingleMpdu {
            reason: NetworkSingleMpduReason::FreshAggregateCapacity,
            ethernet_length: 17,
        })
    );
    hardware.ordinary_completion = Some(aggregate_completion(0, 0).tx);
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Complete)
    );
}

#[test]
fn production_sized_he_frame_fits_a_fresh_default_txop_aggregate() {
    const FRAME_CAPACITY: usize = 1_600;
    const HEADROOM: usize = TEST_HEADROOM;
    const TRAILER: usize = 12;
    const QUEUE_DEPTH: usize = 3;
    type LargeResources = SplitPinnedResources<
        NoopRawMutex,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        QUEUE_DEPTH,
    >;
    type LargePool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

    let resources = std::boxed::Box::leak(std::boxed::Box::new(LargeResources::new()));
    let pool = LargePool::pin_static(std::boxed::Box::leak(
        std::boxed::Box::new(LargePool::new()),
    ));
    let (mut device, network) = resources.split(pool, STATION);
    for marker in 1..=2 {
        device
            .transmit(&mut context())
            .expect("free production-sized pinned network slot")
            .consume(1_514, |frame| {
                frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
                frame[6..12].copy_from_slice(&STATION);
                frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
                frame[14..].fill(marker);
            });
    }
    let first = network.try_receive_tx().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<2_048>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::He(HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns)),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending),
    );
    assert_eq!(hardware.he_publications, 1);
    assert_eq!(hardware.legacy_publications, 0);
    assert!(observer.observed(AggregateTxObservation::Prepared {
        subframes: 2,
        stop: AggregateBuildStop::QueueEmpty,
    }));
    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Complete)
    );
}

#[test]
fn negotiated_amsdu_pairs_network_frames_inside_the_block_ack_window() {
    const FRAME_CAPACITY: usize = 1_600;
    const HEADROOM: usize = TEST_HEADROOM;
    const TRAILER: usize = 1_632;
    const QUEUE_DEPTH: usize = 4;
    type LargeResources = SplitPinnedResources<
        NoopRawMutex,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        QUEUE_DEPTH,
        QUEUE_DEPTH,
    >;
    type LargePool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

    let resources = std::boxed::Box::leak(std::boxed::Box::new(LargeResources::new()));
    let pool = LargePool::pin_static(std::boxed::Box::leak(
        std::boxed::Box::new(LargePool::new()),
    ));
    let (mut device, network) = resources.split(pool, STATION);
    for marker in 1..=4 {
        device
            .transmit(&mut context())
            .expect("free production-sized pinned network slot")
            .consume(1_514, |frame| {
                frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
                frame[6..12].copy_from_slice(&STATION);
                frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
                frame[14..].fill(marker);
            });
    }
    let first = network.try_receive_tx().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<4_096>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Short400Ns,
                HtChannelWidth::Mhz40,
            )),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_agreement(0, Some((TEST_SLOTS as u16, true)));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending),
    );
    assert_eq!(hardware.ht_publications, 1);
    assert_eq!(hardware.legacy_publications, 0);
    assert!(observer.observed(AggregateTxObservation::Prepared {
        subframes: 2,
        stop: AggregateBuildStop::QueueEmpty,
    }));
    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Complete)
    );
}

#[test]
fn aggregate_never_exceeds_the_peer_negotiated_block_ack_window() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);
    send_frame(&mut device, 2);
    send_frame(&mut device, 3);
    let first = network.try_receive_tx().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);

    // The local arena can hold three frames, but the peer accepted only two.
    tx.set_block_ack_window(0, Some(2));
    assert_eq!(tx.block_ack_window(0), Some(2));
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending),
    );
    assert!(observer.observed(AggregateTxObservation::Prepared {
        subframes: 2,
        stop: AggregateBuildStop::FrameLimit,
    }));
    assert_eq!(network.tx_queue_len(), 1);

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Complete),
    );
}

#[test]
fn pipelined_arena_survives_current_retry_and_publishes_at_next_boundary() {
    const PIPELINE_DEPTH: usize = 6;
    type PipelineResources = SplitPinnedResources<
        NoopRawMutex,
        TEST_FRAME_CAPACITY,
        TEST_HEADROOM,
        TEST_TRAILER,
        PIPELINE_DEPTH,
        PIPELINE_DEPTH,
    >;
    type PipelinePool =
        PinnedTxPool<TEST_FRAME_CAPACITY, TEST_HEADROOM, TEST_TRAILER, PIPELINE_DEPTH>;

    let resources = std::boxed::Box::leak(std::boxed::Box::new(PipelineResources::new()));
    let pool = PipelinePool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(
        PipelinePool::new(),
    )));
    let (mut device, network) = resources.split(pool, STATION);
    for marker in 1..=3 {
        device
            .transmit(&mut context())
            .expect("pipeline queue has one free slot")
            .consume(17, |frame| {
                frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
                frame[6..12].copy_from_slice(&STATION);
                frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
                frame[14..].fill(marker);
            });
    }
    let first = network.try_receive_tx().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut primary = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut standby = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::pipelined(
            HtAmpduTxResources::new_model(primary.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
            HtAmpduTxResources::new_model(standby.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 1);
    assert!(!tx.has_prepared_network_tx());
    assert_eq!(network.tx_queue_len(), 0);

    // The network producer may wake more than once while the current arena is
    // hardware-owned. Each wake extends the same software-owned standby arena
    // without publishing it or consuming another sequence space.
    for marker in 4..=5 {
        device
            .transmit(&mut context())
            .expect("pipeline queue has one free slot")
            .consume(17, |frame| {
                frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
                frame[6..12].copy_from_slice(&STATION);
                frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
                frame[14..].fill(marker);
            });
    }
    let standby_first = network.try_receive_tx().unwrap();
    tx.prepare_network_standby(standby_first, &network.tx_consumer());
    assert!(tx.has_prepared_network_tx());
    assert_eq!(network.tx_queue_len(), 0);
    assert_eq!(hardware.ht_publications, 1);

    device
        .transmit(&mut context())
        .expect("pipeline queue has one free slot")
        .consume(17, |frame| {
            frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, 6]);
            frame[6..12].copy_from_slice(&STATION);
            frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
            frame[14..].fill(6);
        });
    let extension = network.try_receive_tx().unwrap();
    tx.prepare_network_standby(extension, &network.tx_consumer());
    assert!(tx.has_prepared_network_tx());
    assert_eq!(hardware.ht_publications, 1);
    assert_eq!(observer.count(AggregateTxObservation::StandbyPrepared), 1);
    assert_eq!(
        observer.count(AggregateTxObservation::Prepared {
            subframes: 3,
            stop: AggregateBuildStop::FrameLimit,
        }),
        1,
        "standby preparation stays private until the publication boundary"
    );

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b001));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 2);
    assert!(tx.has_prepared_network_tx());

    hardware.aggregate_completion = Some(aggregate_completion(8, 0b11));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Complete)
    );
    assert!(tx.has_prepared_network_tx());
    assert_eq!(hardware.ht_publications, 2);
    assert_eq!(
        tx.start_prepared_network(&mut hardware, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 3);
    assert!(!tx.has_prepared_network_tx());
    assert_eq!(
        observer.count(AggregateTxObservation::Prepared {
            subframes: 3,
            stop: AggregateBuildStop::FrameLimit,
        }),
        2
    );

    hardware.aggregate_completion = Some(aggregate_completion(10, 0b111));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Complete)
    );

    let returned = tx
        .try_into_station_parts()
        .unwrap_or_else(|_| panic!("both aggregate arenas must return idle"));
    assert_eq!(returned.aggregate.primary().state(), TxSlotState::Free);
    assert_eq!(
        returned.aggregate.standby().map(|arena| arena.state()),
        Some(TxSlotState::Free)
    );
    returned.pairwise_key.clear(&mut hardware);
}

#[test]
fn ordinary_control_tx_cannot_admit_a_standby_aggregate() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, STATION);
    send_frame(&mut device, 1);

    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut primary = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut standby = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::pipelined(
            HtAmpduTxResources::new_model(primary.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
            HtAmpduTxResources::new_model(standby.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    assert_eq!(
        ConnectedControlTx::start_action(
            &mut tx,
            &mut hardware,
            &[3, 0],
            ActionTxConfig::VENDOR_MANAGEMENT,
        ),
        Ok(WdevControlProgress::TxPending),
    );
    assert!(tx.active());
    assert!(!tx.can_prepare_network_tx());

    let frame = network.try_receive_tx().unwrap();
    tx.prepare_network_standby(frame, &network.tx_consumer());
    assert!(!tx.has_prepared_network_tx());
}

#[test]
fn rejected_standby_preparation_preserves_the_hardware_owned_primary() {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let pool = Pool::pin_static(std::boxed::Box::leak(std::boxed::Box::new(Pool::new())));
    let (mut device, network) = resources.split(pool, STATION);
    send_frame(&mut device, 1);
    send_frame(&mut device, 2);

    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut primary = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut standby = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::pipelined(
            HtAmpduTxResources::new_model(primary.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
            HtAmpduTxResources::new_model(standby.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    let first = network.try_receive_tx().unwrap();
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending),
    );
    assert!(tx.active());
    assert_eq!(tx.aggregate_slot_state(), TxSlotState::HardwareOwned);

    // A malformed software-owned next batch must be cancelled in the standby
    // arena without erasing the independently live primary transaction.
    send_short_frame(&mut device);
    let short = network.try_receive_tx().unwrap();
    tx.prepare_network_standby(short, &network.tx_consumer());

    assert!(tx.active());
    assert_eq!(tx.aggregate_slot_state(), TxSlotState::HardwareOwned);
    assert_eq!(tx.standby_aggregate_is_fully_free(), Some(true));
}

#[test]
fn block_ack_completion_releases_all_referenced_network_leases() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);
    send_frame(&mut device, 2);
    let first = network.try_receive_tx().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 1);
    // Only the unused third slot can return to the producer while the two
    // submitted frames remain radio-owned.
    send_frame(&mut device, 3);
    assert!(device.transmit(&mut context()).is_none());

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Complete)
    );
    assert_eq!(
        tx.take_last_aggregate_status(),
        Some(MacAmpduTxStatus {
            result: MacAmpduTxResult::Delivered,
            original_subframes: 2,
            aggregate_attempts: 1,
            aggregate_rate: TxPhyRate::Ht(TEST_RATE),
            block_acknowledged_subframes: 2,
            ordinary_retry: None,
        })
    );
    send_frame(&mut device, 4);
    send_frame(&mut device, 5);
    assert!(device.transmit(&mut context()).is_none());
    assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);
    for _ in 0..TEST_QUEUE_DEPTH {
        drop(network.try_receive_tx().unwrap());
    }
}

#[test]
fn partial_block_ack_retains_missing_frames_across_one_republication() {
    let (mut device, network) = make_network();
    for marker in 1..=3 {
        send_frame(&mut device, marker);
    }
    let first = network.try_receive_tx().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b001));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 2);
    assert!(device.transmit(&mut context()).is_none());

    hardware.aggregate_completion = Some(aggregate_completion(8, 0b11));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Complete)
    );
    assert_eq!(
        tx.take_last_aggregate_status(),
        Some(MacAmpduTxStatus {
            result: MacAmpduTxResult::Delivered,
            original_subframes: 3,
            aggregate_attempts: 2,
            aggregate_rate: TxPhyRate::Ht(TEST_RATE),
            block_acknowledged_subframes: 3,
            ordinary_retry: None,
        })
    );
    send_frame(&mut device, 4);
    send_frame(&mut device, 5);
    send_frame(&mut device, 6);
    assert!(device.transmit(&mut context()).is_none());
    assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);
    for _ in 0..TEST_QUEUE_DEPTH {
        drop(network.try_receive_tx().unwrap());
    }
}

#[test]
fn one_missing_ht_mpdu_moves_to_ordinary_retry_without_new_sequence_or_pn() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);
    send_frame(&mut device, 2);
    let first = network.try_receive_tx().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            attempt_limit: 2,
            completion_timeout_us: 250_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b01));
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(tx.take_last_aggregate_status(), None);
    assert_eq!(tx.peek_qos_sequence(0), Some(9));

    // The individual retry uses the private ordinary descriptor. Both
    // referenced network allocations have already crossed the safe
    // detach/release edge and can be filled again while it is in flight.
    send_frame(&mut device, 3);
    send_frame(&mut device, 4);
    send_frame(&mut device, 5);
    assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);

    hardware.ordinary_completion = Some(MacTxCompletionRegisters {
        aux_a: 0,
        aux_b: 0,
        aux_c: 0,
        primary: 0,
        alternate: 0,
        trigger_flow: false,
    });
    assert_eq!(
        embassy_futures::block_on(tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: MAC_INT_TX_COMPLETE,
            },
        )),
        Ok(WifiTxProgress::Complete)
    );
    let aggregate = tx
        .take_last_aggregate_status()
        .expect("ordinary retry completes the logical aggregate exchange");
    assert_eq!(aggregate.result, MacAmpduTxResult::Delivered);
    assert_eq!(aggregate.original_subframes, 2);
    assert_eq!(aggregate.aggregate_attempts, 1);
    assert_eq!(aggregate.aggregate_rate, TxPhyRate::Ht(TEST_RATE));
    assert_eq!(aggregate.block_acknowledged_subframes, 1);
    assert_eq!(aggregate.delivered_subframes(), 2);
    assert_eq!(aggregate.total_publication_attempts(), 2);
    assert!(matches!(
        aggregate.ordinary_retry,
        Some(status) if status.result == MacTxResult::Transmitted
            && status.final_rate == TxPhyRate::Ht(TEST_RATE)
            && status.acknowledged == Some(true)
    ));
    assert!(matches!(
        tx.take_last_ordinary_outcome(),
        Some(SingleMpduTxOutcome::Success(_))
    ));
    for _ in 0..TEST_QUEUE_DEPTH {
        drop(network.try_receive_tx().unwrap());
    }
}
