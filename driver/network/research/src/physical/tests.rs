extern crate std;

mod selected;

use core::num::{NonZeroU16, NonZeroU32};
use std::{boxed::Box, vec::Vec};

use open_esp_radio_dma::PinnedDmaTxPool;
use open_esp_radio_wifi_datapath::{
    AdmissionClass, EgressDemand, EgressFlowKey, EgressSelection, MaterializedTxFrame,
    RadioEgressKey, RadioPeer, TrafficIdentifier,
};

use super::*;
use crate::{
    Ipv4Address, MacAddress, ResearchNetworkConfig, ResearchNetworkEngine, ResolvedIpv4Route,
};

type TestPool = PinnedDmaTxPool<1600, 64, 32, 3>;

fn radio_key() -> RadioEgressKey {
    RadioEgressKey::new(
        NetworkInterfaceId::new(1),
        2,
        RadioPeer::Unicast {
            slot: 3,
            generation: 4,
        },
        TrafficIdentifier::new(0).unwrap(),
    )
}

#[test]
fn research_engine_constructs_directly_in_the_pinned_sram_batch() {
    let storage = Box::leak(Box::new(TestPool::new()));
    let pool = TestPool::pin_static(storage);
    let resources = PinnedBatchResources::<3>::new();
    let allocator = resources.bind(pool);
    let config = ResearchNetworkConfig {
        interface: NetworkInterfaceId::new(1),
        mac: MacAddress::new([2, 0, 0, 0, 0, 1]),
        ipv4: Ipv4Address::new([192, 168, 1, 1]),
    };
    let mut payload = *b"final-sram";
    let mut engine = ResearchNetworkEngine::<2, 4, 32, &mut [u8]>::new(config);
    engine
        .enqueue_udp_owned(
            1,
            ResolvedIpv4Route {
                destination_mac: MacAddress::new([2, 0, 0, 0, 0, 3]),
                destination_ip: Ipv4Address::new([192, 168, 1, 3]),
                radio: radio_key(),
            },
            1000,
            2000,
            AdmissionClass::Bulk,
            &mut payload[..],
        )
        .unwrap();
    let mut demands = Vec::<EgressDemand>::new();
    engine.visit_demands(|demand| demands.push(demand));
    let mut batch = allocator.try_reserve::<2>(config.interface, 2).unwrap();
    let outcome = engine
        .fill_selected(
            EgressSelection {
                key: EgressFlowKey {
                    radio: radio_key(),
                    admission: AdmissionClass::Bulk,
                },
                max_frames: NonZeroU16::new(2).unwrap(),
                max_bytes: NonZeroU32::new(3200).unwrap(),
            },
            &mut batch,
        )
        .unwrap();
    assert_eq!(demands[0].ready_frames, 1);
    assert_eq!(outcome.frames, 1);
    assert_eq!(batch.prepared_len(), 1);
    assert_eq!(allocator.free_credits(), 1);

    let frame = batch.take_prepared().unwrap();
    assert_eq!(&frame.ethernet()[42..], b"final-sram");
    drop(frame);
    drop(batch);
    assert_eq!(allocator.free_credits(), 3);
}

#[test]
fn physical_source_tracks_partial_build_and_returns_unconsumed_frames() {
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let resources = PinnedBatchResources::<3>::new();
    let allocator = resources.bind(pool);
    let mut batch = allocator
        .try_reserve::<3>(NetworkInterfaceId::new(1), 3)
        .unwrap();
    assert!(batch.try_take_physical().is_none());
    batch
        .try_write(14, |frame| {
            frame.fill(1);
            Ok::<_, ()>(())
        })
        .unwrap();
    assert_eq!(batch.pending_frames(), 1);
    let first = batch.try_take_physical().unwrap();
    assert_eq!(first.ethernet(), &[1; 14]);
    assert_eq!(batch.pending_frames(), 0);
    assert!(batch.try_take_physical().is_none());
    assert_eq!(
        batch.try_write(14, |_| Err("writer failed")),
        Err(BatchWriteError::Write("writer failed"))
    );
    assert_eq!(batch.pending_frames(), 0);
    assert_eq!(allocator.free_credits(), 1);
    batch
        .try_write(14, |frame| {
            frame.fill(2);
            Ok::<_, ()>(())
        })
        .unwrap();
    assert_eq!(batch.pending_frames(), 1);
    // An earlier empty read must not hide newly prepared work. Dropping
    // the container releases only its unconsumed frame, not `first`.
    assert_eq!(allocator.free_credits(), 1);
    drop(batch);
    assert_eq!(allocator.free_credits(), 2);
    drop(first);
    assert_eq!(allocator.free_credits(), 3);
}

#[test]
fn failed_whole_batch_reservation_restores_every_credit() {
    let storage = Box::leak(Box::new(TestPool::new()));
    let pool = TestPool::pin_static(storage);
    let resources = PinnedBatchResources::<3>::new();
    let allocator = resources.bind(pool);
    let first = allocator
        .try_reserve::<2>(NetworkInterfaceId::new(1), 2)
        .unwrap();
    assert!(
        allocator
            .try_reserve::<2>(NetworkInterfaceId::new(1), 2)
            .is_none()
    );
    assert_eq!(allocator.free_credits(), 1);
    drop(first);
    assert_eq!(allocator.free_credits(), 3);
}

#[test]
fn oversized_payload_preserves_source_and_physical_reservation() {
    let pool = TestPool::pin_static(Box::leak(Box::new(TestPool::new())));
    let resources = PinnedBatchResources::<3>::new();
    let allocator = resources.bind(pool);
    let config = ResearchNetworkConfig {
        interface: NetworkInterfaceId::new(1),
        mac: MacAddress::new([2, 0, 0, 0, 0, 1]),
        ipv4: Ipv4Address::new([192, 168, 1, 1]),
    };
    let mut engine = ResearchNetworkEngine::<2, 4, 8, &[u8]>::new(config);
    let payload = [0; 1600];
    engine
        .enqueue_udp_owned(
            0,
            ResolvedIpv4Route {
                destination_mac: MacAddress::new([2, 0, 0, 0, 0, 3]),
                destination_ip: Ipv4Address::new([192, 168, 1, 3]),
                radio: radio_key(),
            },
            1,
            2,
            AdmissionClass::Bulk,
            b"small",
        )
        .unwrap();
    engine
        .enqueue_udp_owned(
            1,
            ResolvedIpv4Route {
                destination_mac: MacAddress::new([2, 0, 0, 0, 0, 3]),
                destination_ip: Ipv4Address::new([192, 168, 1, 3]),
                radio: radio_key(),
            },
            1,
            2,
            AdmissionClass::Bulk,
            &payload,
        )
        .unwrap();
    let mut batch = allocator.try_reserve::<2>(config.interface, 2).unwrap();
    let outcome = engine
        .fill_selected(
            EgressSelection {
                key: EgressFlowKey {
                    radio: radio_key(),
                    admission: AdmissionClass::Bulk,
                },
                max_frames: NonZeroU16::new(2).unwrap(),
                max_bytes: NonZeroU32::new(3200).unwrap(),
            },
            &mut batch,
        )
        .unwrap();
    assert_eq!(
        outcome.stop,
        open_esp_radio_wifi_datapath::FillStopReason::FrameTooLong { capacity: 1600 }
    );
    assert_eq!(outcome.frames, 1);
    assert_eq!(outcome.bytes, 47);
    assert_eq!(outcome.source_remaining, 1);
    assert_eq!(batch.remaining(), 1);
    assert_eq!(batch.prepared_len(), 1);
    // The same reserved slot remains usable; the failed write did not claim it.
    batch
        .try_write(14, |frame| {
            frame.fill(1);
            Ok::<_, ()>(())
        })
        .unwrap();
    drop(batch);
    assert_eq!(allocator.free_credits(), 3);
}
