use super::*;
use core::cell::Cell;
use open_esp_radio_wifi_datapath::{FillStopReason, SelectedTxSource};
use std::rc::Rc;

#[derive(Debug)]
struct Lease {
    bytes: [u8; 4],
    length: Rc<Cell<usize>>,
    returned: Rc<Cell<usize>>,
}

impl AsRef<[u8]> for Lease {
    fn as_ref(&self) -> &[u8] {
        &self.bytes[..self.length.get()]
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.returned.set(self.returned.get() + 1);
    }
}

type Engine = ResearchNetworkEngine<2, 4, 8, Lease>;

fn engine() -> Engine {
    Engine::new(ResearchNetworkConfig {
        interface: radio_key().interface(),
        mac: MacAddress::new([2, 0, 0, 0, 0, 1]),
        ipv4: Ipv4Address::new([192, 168, 1, 1]),
    })
}

fn enqueue(
    engine: &mut Engine,
    key: RadioEgressKey,
    marker: u8,
    returned: &Rc<Cell<usize>>,
) -> Rc<Cell<usize>> {
    let length = Rc::new(Cell::new(4));
    engine
        .enqueue_udp_owned(
            1,
            ResolvedIpv4Route {
                radio: key,
                destination_mac: MacAddress::new([2, 0, 0, 0, 0, 2]),
                destination_ip: Ipv4Address::new([192, 168, 1, 2]),
            },
            1000,
            2000,
            AdmissionClass::Bulk,
            Lease {
                bytes: [marker; 4],
                length: length.clone(),
                returned: returned.clone(),
            },
        )
        .unwrap();
    length
}

fn selection(frames: u16, bytes: u32) -> EgressSelection {
    EgressSelection {
        key: EgressFlowKey {
            radio: radio_key(),
            admission: AdmissionClass::Bulk,
        },
        max_frames: NonZeroU16::new(frames).unwrap(),
        max_bytes: NonZeroU32::new(bytes).unwrap(),
    }
}

#[test]
fn radio_consumes_only_requested_payload_and_keeps_physical_credit_until_completion() {
    let resources = PinnedBatchResources::<3>::new();
    let allocator = resources.bind(TestPool::pin_static(Box::leak(Box::new(TestPool::new()))));
    let returned = Rc::new(Cell::new(0));
    let mut engine = engine();
    for marker in 1..=3 {
        enqueue(&mut engine, radio_key(), marker, &returned);
    }
    let batch = allocator
        .try_reserve::<3>(radio_key().interface(), 3)
        .unwrap();
    let source = SelectedTxSource::new(&mut engine, selection(3, 4800), batch);
    assert_eq!(source.pending_frames(), 3);
    assert_eq!(returned.get(), 0);
    let frame = source.try_take_physical().unwrap();
    assert_eq!(&frame.ethernet()[42..], &[1; 4]);
    assert_eq!(returned.get(), 1);
    assert_eq!(source.pending_frames(), 2);
    let report = source.finish();
    assert_eq!((report.frames, report.bytes), (1, 46));
    assert_eq!(report.stop, None);
    assert_eq!(engine.queued_work(), 2);
    assert_eq!(allocator.free_credits(), 2);
    drop(frame);
    assert_eq!(allocator.free_credits(), 3);
    drop(engine);
    assert_eq!(returned.get(), 3);
}

#[test]
fn dropping_an_unpolled_selection_keeps_every_source_owner() {
    let resources = PinnedBatchResources::<3>::new();
    let allocator = resources.bind(TestPool::pin_static(Box::leak(Box::new(TestPool::new()))));
    let returned = Rc::new(Cell::new(0));
    let mut engine = engine();
    enqueue(&mut engine, radio_key(), 1, &returned);
    let batch = allocator
        .try_reserve::<1>(radio_key().interface(), 1)
        .unwrap();
    drop(SelectedTxSource::new(
        &mut engine,
        selection(1, 1600),
        batch,
    ));
    assert_eq!(engine.queued_work(), 1);
    assert_eq!(returned.get(), 0);
    assert_eq!(allocator.free_credits(), 3);
}

#[test]
fn byte_frame_and_destination_limits_leave_the_tail_with_its_original_owner() {
    for (frames, bytes, stop) in [
        (1, 1600, FillStopReason::SelectionSatisfied),
        (3, 46, FillStopReason::ByteBudget),
        (3, 60, FillStopReason::ByteBudget),
        (3, 1600, FillStopReason::DestinationExhausted),
    ] {
        let resources = PinnedBatchResources::<3>::new();
        let allocator = resources.bind(TestPool::pin_static(Box::leak(Box::new(TestPool::new()))));
        let returned = Rc::new(Cell::new(0));
        let mut engine = engine();
        for marker in 1..=3 {
            enqueue(&mut engine, radio_key(), marker, &returned);
        }
        let batch = allocator
            .try_reserve::<1>(radio_key().interface(), 1)
            .unwrap();
        let source = SelectedTxSource::new(&mut engine, selection(frames, bytes), batch);
        let frame = source.try_take_physical().unwrap();
        assert!(source.try_take_physical().is_none());
        assert!(source.try_take_physical().is_none());
        let report = source.finish();
        assert_eq!((report.frames, report.bytes), (1, 46));
        assert_eq!(report.stop, Some(Ok(stop)));
        assert_eq!(engine.queued_work(), 2);
        assert_eq!(returned.get(), 1);
        drop(frame);
        assert_eq!(allocator.free_credits(), 3);
    }
}

#[test]
fn writer_error_retains_payload_and_is_reported_without_repeated_retries() {
    let resources = PinnedBatchResources::<3>::new();
    let allocator = resources.bind(TestPool::pin_static(Box::leak(Box::new(TestPool::new()))));
    let returned = Rc::new(Cell::new(0));
    let mut engine = engine();
    enqueue(&mut engine, radio_key(), 1, &returned);
    let length = enqueue(&mut engine, radio_key(), 2, &returned);
    length.set(3);
    let batch = allocator
        .try_reserve::<3>(radio_key().interface(), 3)
        .unwrap();
    let source = SelectedTxSource::new(&mut engine, selection(3, 4800), batch);
    drop(source.try_take_physical().unwrap());
    assert!(source.try_take_physical().is_none());
    length.set(4);
    assert!(source.try_take_physical().is_none());
    let report = source.finish();
    assert_eq!((report.frames, report.bytes), (1, 46));
    assert_eq!(
        report.stop,
        Some(Err(crate::FrameWriteError::PayloadLengthChanged))
    );
    assert_eq!(engine.queued_work(), 1);
    assert_eq!(returned.get(), 1);
    assert_eq!(allocator.free_credits(), 3);
}

#[test]
fn different_radio_epoch_is_never_consumed_by_this_selection() {
    let resources = PinnedBatchResources::<3>::new();
    let allocator = resources.bind(TestPool::pin_static(Box::leak(Box::new(TestPool::new()))));
    let returned = Rc::new(Cell::new(0));
    let mut engine = engine();
    let key = radio_key();
    let other_epoch = RadioEgressKey::new(
        key.interface(),
        key.link_epoch() + 1,
        key.peer(),
        key.traffic_identifier(),
    );
    enqueue(&mut engine, other_epoch, 7, &returned);
    enqueue(&mut engine, key, 1, &returned);
    let batch = allocator.try_reserve::<3>(key.interface(), 3).unwrap();
    let source = SelectedTxSource::new(&mut engine, selection(3, 4800), batch);
    assert_eq!(source.pending_frames(), 1);
    let frame = source.try_take_physical().unwrap();
    assert_eq!(&frame.ethernet()[42..], &[1; 4]);
    assert!(source.try_take_physical().is_none());
    assert_eq!(
        source.finish().stop,
        Some(Ok(FillStopReason::SourceDrained))
    );
    assert_eq!(engine.queued_work(), 1);
    assert_eq!(returned.get(), 1);
    drop(frame);
    assert_eq!(allocator.free_credits(), 3);
}

#[test]
fn oversized_frame_keeps_both_payload_and_reserved_credit_unconsumed() {
    type SmallPool = PinnedDmaTxPool<45, 64, 32, 3>;
    let resources = PinnedBatchResources::<3>::new();
    let allocator = resources.bind(SmallPool::pin_static(Box::leak(Box::new(SmallPool::new()))));
    let returned = Rc::new(Cell::new(0));
    let mut engine = engine();
    enqueue(&mut engine, radio_key(), 1, &returned);
    let batch = allocator
        .try_reserve::<1>(radio_key().interface(), 1)
        .unwrap();
    let source = SelectedTxSource::new(&mut engine, selection(1, 1600), batch);
    assert!(source.try_take_physical().is_none());
    let report = source.finish();
    assert_eq!((report.frames, report.bytes), (0, 0));
    assert_eq!(
        report.stop,
        Some(Ok(FillStopReason::FrameTooLong { capacity: 45 }))
    );
    assert_eq!(engine.queued_work(), 1);
    assert_eq!(returned.get(), 0);
    assert_eq!(allocator.free_credits(), 3);
}

#[test]
fn probing_an_empty_batch_before_selection_does_not_hide_newly_published_frames() {
    let resources = PinnedBatchResources::<3>::new();
    let allocator = resources.bind(TestPool::pin_static(Box::leak(Box::new(TestPool::new()))));
    let returned = Rc::new(Cell::new(0));
    let mut engine = engine();
    enqueue(&mut engine, radio_key(), 1, &returned);
    let batch = allocator
        .try_reserve::<1>(radio_key().interface(), 1)
        .unwrap();
    assert!(batch.try_take_physical().is_none());
    let source = SelectedTxSource::new(&mut engine, selection(1, 1600), batch);
    let frame = source.try_take_physical().unwrap();
    assert_eq!(&frame.ethernet()[42..], &[1; 4]);
    assert_eq!(source.finish().frames, 1);
    assert_eq!(engine.queued_work(), 0);
    assert_eq!(returned.get(), 1);
    drop(frame);
    assert_eq!(allocator.free_credits(), 3);
}
