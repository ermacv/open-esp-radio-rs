//! Real deferred UDP work enters the production STA TX owner lazily.

use core::{
    cell::Cell,
    num::{NonZeroU16, NonZeroU32},
};
use std::{boxed::Box, rc::Rc};

use open_esp_radio_dma::PinnedDmaTxPool;
use open_esp_radio_research_datapath::{
    Ipv4Address, MacAddress, PinnedBatchResources, ResearchNetworkConfig, ResearchNetworkEngine,
    ResolvedIpv4Route,
};
use open_esp_radio_wifi_datapath::{
    AdmissionClass, EgressSelection, FillStopReason, PhysicalTxSource, RadioEgressKey, RadioPeer,
    SelectedTxSource, TrafficIdentifier, TxRequest, TxRequestSource,
};

use super::*;

type Drops = Rc<[Cell<usize>; TEST_QUEUE_DEPTH]>;
type Engine = ResearchNetworkEngine<2, TEST_QUEUE_DEPTH, 8, Payload>;
type Pool = PinnedDmaTxPool<TEST_FRAME_CAPACITY, TEST_HEADROOM, TEST_TRAILER, TEST_QUEUE_DEPTH>;

// This one-shot fixture request carries identity only: there is no Ethernet
// frame and no SoftwareTxFrame implementation before radio admission.
#[derive(Debug, Eq, PartialEq)]
struct NativeRequest {
    interface: NetworkInterfaceId,
    identity: Rc<()>,
}

impl TxRequest for NativeRequest {
    fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }
}

struct NativeSource<S> {
    selected: S,
    remaining: Cell<usize>,
    reject_once: Cell<bool>,
    materialization_calls: Cell<usize>,
}

impl<S: PhysicalTxSource> PhysicalTxSource for NativeSource<S> {
    type Frame = S::Frame;

    fn pending_frames(&self) -> usize {
        self.selected.pending_frames()
    }

    fn try_take_physical(&self) -> Option<Self::Frame> {
        let frame = self.selected.try_take_physical()?;
        self.remaining.set(self.remaining.get() - 1);
        Some(frame)
    }
}

impl<S: PhysicalTxSource> TxRequestSource for NativeSource<S> {
    type Request = NativeRequest;

    fn interface(&self) -> NetworkInterfaceId {
        NetworkInterfaceId::new(0)
    }

    fn try_materialize(&self, request: NativeRequest) -> Result<Self::Frame, NativeRequest> {
        self.materialization_calls
            .set(self.materialization_calls.get() + 1);
        if request.interface() != self.interface() || self.reject_once.replace(false) {
            return Err(request);
        }
        self.try_take_physical().ok_or(request)
    }

    fn materialization_capacity(&self) -> usize {
        self.remaining.get()
    }

    #[cfg(feature = "tx-phase-telemetry")]
    fn ownership_snapshot(&self) -> open_esp_radio_wifi_datapath::MaterializationOwnershipSnapshot {
        // This fixture is used only by the aggregate test, which retains all
        // taken frames until terminal completion, after the source is dropped.
        open_esp_radio_wifi_datapath::MaterializationOwnershipSnapshot {
            free: self.remaining.get(),
            radio_owned: TEST_QUEUE_DEPTH - self.remaining.get(),
        }
    }
}

#[derive(Debug)]
struct Payload {
    bytes: [u8; 8],
    index: usize,
    drops: Drops,
}

impl AsRef<[u8]> for Payload {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for Payload {
    fn drop(&mut self) {
        self.drops[self.index].set(self.drops[self.index].get() + 1);
    }
}

fn queued_engine(drops: &Drops) -> Engine {
    let interface = NetworkInterfaceId::new(0);
    let mut engine = Engine::new(ResearchNetworkConfig {
        interface,
        mac: MacAddress::new(STATION),
        ipv4: Ipv4Address::new([192, 168, 1, 2]),
    });
    let route = ResolvedIpv4Route {
        destination_mac: MacAddress::new([0x30, 0x31, 0x32, 0x33, 0x34, 1]),
        destination_ip: Ipv4Address::new([192, 168, 1, 3]),
        radio: RadioEgressKey::new(
            interface,
            1,
            RadioPeer::Unicast {
                slot: 0,
                generation: 1,
            },
            TrafficIdentifier::new(0).unwrap(),
        ),
    };
    for index in 0..TEST_QUEUE_DEPTH {
        engine
            .enqueue_udp_owned(
                0,
                route,
                4323,
                4324,
                AdmissionClass::Bulk,
                Payload {
                    bytes: [index as u8; 8],
                    index,
                    drops: Rc::clone(drops),
                },
            )
            .unwrap();
    }
    engine
}

fn selection(engine: &Engine) -> EgressSelection {
    let mut demand = None;
    engine.visit_demands(|next| {
        assert!(demand.replace(next).is_none(), "one selected UDP flow");
    });
    EgressSelection {
        key: demand.unwrap().key,
        max_frames: NonZeroU16::new(TEST_QUEUE_DEPTH as u16).unwrap(),
        max_bytes: NonZeroU32::new((TEST_FRAME_CAPACITY * TEST_QUEUE_DEPTH) as u32).unwrap(),
    }
}

fn drop_counts(drops: &Drops) -> [usize; TEST_QUEUE_DEPTH] {
    core::array::from_fn(|index| drops[index].get())
}

#[test]
fn native_udp_selection_retries_partial_block_ack_and_returns_terminal_credits() {
    let drops = Rc::new(core::array::from_fn(|_| Cell::new(0)));
    let mut engine = queued_engine(&drops);
    let selected = selection(&engine);
    let pool = Pool::pin_static(Box::leak(Box::new(Pool::new())));
    let resources = PinnedBatchResources::<TEST_QUEUE_DEPTH>::new();
    let allocator = resources.bind(pool);
    let batch = allocator
        .try_reserve::<TEST_QUEUE_DEPTH>(NetworkInterfaceId::new(0), TEST_QUEUE_DEPTH)
        .unwrap();
    let source = NativeSource {
        selected: SelectedTxSource::new(&mut engine, selected, batch),
        remaining: Cell::new(TEST_QUEUE_DEPTH),
        reject_once: Cell::new(true),
        materialization_calls: Cell::new(0),
    };
    assert_eq!(drop_counts(&drops), [0, 0, 0]);

    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            &mut retention,
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
    let identity = Rc::new(());
    let request = NativeRequest {
        interface: NetworkInterfaceId::new(0),
        identity: Rc::clone(&identity),
    };
    let Err(RequestTxError::Unmaterialized(request)) =
        tx.start_request(&mut hardware, request, &source)
    else {
        panic!("rejected materialization returns the exact request");
    };
    assert!(Rc::ptr_eq(&request.identity, &identity));
    assert_eq!(drop_counts(&drops), [0, 0, 0]);
    assert_eq!(source.pending_frames(), TEST_QUEUE_DEPTH);
    assert!(!tx.active());
    assert_eq!(hardware.ht_publications, 0);
    assert_eq!(hardware.legacy_publications, 0);
    assert_eq!(
        tx.start_request(&mut hardware, request, &source),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(source.materialization_calls.get(), 2);
    let busy_identity = Rc::new(());
    let busy_request = NativeRequest {
        interface: NetworkInterfaceId::new(0),
        identity: Rc::clone(&busy_identity),
    };
    let Err(RequestTxError::Busy(busy_request)) =
        tx.start_request(&mut hardware, busy_request, &source)
    else {
        panic!("active TX returns the request before asking its source");
    };
    assert!(Rc::ptr_eq(&busy_request.identity, &busy_identity));
    assert!(!tx.can_prepare_network_tx());
    let standby_request = tx
        .prepare_request_standby(busy_request, &source)
        .unwrap_err();
    assert!(Rc::ptr_eq(&standby_request.identity, &busy_identity));
    assert_eq!(source.materialization_calls.get(), 2);
    assert_eq!(drop_counts(&drops), [1, 1, 1]);
    assert_eq!(hardware.ht_publications, 1);
    let report = source.selected.finish();
    assert_eq!(report.frames, TEST_QUEUE_DEPTH as u16);
    assert!(matches!(
        report.stop,
        Some(Ok(FillStopReason::SourceDrained))
    ));
    assert_eq!(engine.queued_work(), 0);
    assert_eq!(drop_counts(&drops), [1, 1, 1]);
    assert_eq!(allocator.free_credits(), 0);

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b001));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE
            }
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 2);
    assert_eq!(allocator.free_credits(), 0);
    assert_eq!(drop_counts(&drops), [1, 1, 1]);
    hardware.aggregate_completion = Some(aggregate_completion(8, 0b11));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE
            }
        ),
        Ok(WifiTxProgress::Complete)
    );
    let status = tx.take_last_aggregate_status().unwrap();
    assert_eq!(status.result, MacAmpduTxResult::Delivered);
    assert_eq!(status.original_subframes, 3);
    assert_eq!(status.aggregate_attempts, 2);
    assert_eq!(allocator.free_credits(), TEST_QUEUE_DEPTH);
    drop(engine);
    assert_eq!(drop_counts(&drops), [1, 1, 1]);
    let _parts = tx
        .try_into_teardown_parts()
        .unwrap_or_else(|_| panic!("completed native TX detaches"));
    assert!(
        allocator
            .try_reserve::<TEST_QUEUE_DEPTH>(NetworkInterfaceId::new(0), TEST_QUEUE_DEPTH)
            .is_some()
    );
}

#[test]
fn native_udp_ordinary_fallback_retains_unrequested_payloads_and_backlog() {
    let drops = Rc::new(core::array::from_fn(|_| Cell::new(0)));
    let mut engine = queued_engine(&drops);
    let selected = selection(&engine);
    let pool = Pool::pin_static(Box::leak(Box::new(Pool::new())));
    let resources = PinnedBatchResources::<TEST_QUEUE_DEPTH>::new();
    let allocator = resources.bind(pool);
    let batch = allocator
        .try_reserve::<TEST_QUEUE_DEPTH>(NetworkInterfaceId::new(0), TEST_QUEUE_DEPTH)
        .unwrap();
    let source = SelectedTxSource::new(&mut engine, selected, batch);
    let first = source.try_take_physical().unwrap();
    let mut hardware = Hardware::default();
    let observer = RecordingAggregateTxObserver::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut tx = Esp32s31ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            &mut retention,
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
    // No negotiated BA session: the ordinary production owner must ask for
    // only its first packet, despite the three-frame reservation and demand.
    assert_eq!(
        tx.start_network(&mut hardware, first, &source),
        Ok(WifiTxProgress::Pending)
    );
    assert!(
        observer.observed(AggregateTxObservation::NetworkSingleMpdu {
            reason: NetworkSingleMpduReason::BlockAckUnavailable,
            ethernet_length: 50,
        })
    );
    let report = source.finish();
    assert_eq!(report.frames, 1);
    assert!(report.stop.is_none());
    assert_eq!(engine.queued_work(), 2);
    assert_eq!(drop_counts(&drops), [1, 0, 0]);
    // Ordinary TX copies into its own slot; all selected SRAM credits have
    // returned, while untouched payload owners remain queued in the engine.
    assert_eq!(allocator.free_credits(), TEST_QUEUE_DEPTH);
    hardware.ordinary_completion = Some(aggregate_completion(0, 0).tx());
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE
            }
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert_eq!(engine.queued_work(), 2);
    assert_eq!(drop_counts(&drops), [1, 0, 0]);
    let batch = allocator
        .try_reserve::<TEST_QUEUE_DEPTH>(NetworkInterfaceId::new(0), TEST_QUEUE_DEPTH)
        .unwrap();
    let source = SelectedTxSource::new(&mut engine, selected, batch);
    let next = source.try_take_physical().unwrap();
    drop(next);
    assert_eq!(source.finish().frames, 1);
    assert_eq!(drop_counts(&drops), [1, 1, 0]);
    assert_eq!(engine.queued_work(), 1);
    assert_eq!(allocator.free_credits(), TEST_QUEUE_DEPTH);
    drop(engine);
    assert_eq!(drop_counts(&drops), [1, 1, 1]);
    let _parts = tx
        .try_into_teardown_parts()
        .unwrap_or_else(|_| panic!("completed ordinary TX detaches"));
}
