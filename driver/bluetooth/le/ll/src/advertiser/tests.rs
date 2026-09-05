use crate::{LeDeviceAddress, LeDeviceAddressKind};

use super::*;
use crate::advertising::{
    AdvertisingInterval, LegacyAdvertisingData, LegacyNonconnectableAdvertisement,
    PrimaryAdvertisingChannelMap,
};

fn set<'a>(
    data: &'a [u8],
    channels: PrimaryAdvertisingChannelMap,
) -> LegacyNonconnectableAdvertisingSet<'a> {
    LegacyNonconnectableAdvertisingSet::new(
        LegacyNonconnectableAdvertisement::new(
            LeDeviceAddress::from_wire_bytes([6, 5, 4, 3, 2, 1], LeDeviceAddressKind::Public),
            LegacyAdvertisingData::new(data).unwrap(),
        ),
        channels,
        AdvertisingInterval::new(AdvertisingInterval::MIN_UNITS).unwrap(),
    )
}

#[test]
fn rejected_admission_returns_the_same_generation_event_and_channel_plan() {
    let enabled = LegacyAdvertiserStandby::new()
        .configure(set(&[1], PrimaryAdvertisingChannelMap::all()))
        .enable()
        .unwrap();
    let prepared = enabled.prepare_event();
    let identity = prepared.identity();

    assert_eq!(identity.generation().get(), 1);
    assert_eq!(identity.event().get(), 0);
    assert_eq!(prepared.channels(), PrimaryAdvertisingChannelMap::all());
    assert_eq!(prepared.cancel().prepare_event().identity(), identity);
}

#[test]
fn stale_completion_retains_in_flight_owner_and_exact_progress() {
    let in_flight = LegacyAdvertiserStandby::new()
        .configure(set(&[1], PrimaryAdvertisingChannelMap::all()))
        .enable()
        .unwrap()
        .prepare_event()
        .into_submitted();
    let expected = in_flight.identity();
    let stale = LegacyAdvertisingEventIdentity::from_parts(expected.generation().get(), 1);
    let LegacyAdvertiserEventCompletion::Mismatch { error, in_flight } = in_flight.complete(stale)
    else {
        panic!("stale completion must retain the in-flight owner");
    };
    assert_eq!(error.expected, expected);
    assert_eq!(error.observed, stale);
    let complete = in_flight.complete_exact();
    assert_eq!(complete.event_sequence.get(), 0);
}

#[test]
fn completed_event_requires_fresh_delay_and_advances_event_identity() {
    let in_flight = LegacyAdvertiserStandby::new()
        .configure(set(
            &[1, 2],
            PrimaryAdvertisingChannelMap::new(false, false, true).unwrap(),
        ))
        .enable()
        .unwrap()
        .prepare_event()
        .into_submitted();
    let identity = in_flight.identity();
    let LegacyAdvertiserEventCompletion::Completed(complete) = in_flight.complete(identity) else {
        panic!("the exact hardware event must close the portable event");
    };

    let scheduled = complete
        .schedule_next(AdvertisingDelay::from_micros(7_500).unwrap())
        .unwrap();
    assert_eq!(scheduled.generation().get(), 1);
    assert_eq!(scheduled.event_sequence().get(), 1);
    assert_eq!(scheduled.start_offset_micros(), 27_500);
    let next = scheduled.into_event().prepare_event();
    assert_eq!(next.identity().event().get(), 1);
    assert_eq!(
        next.channels(),
        PrimaryAdvertisingChannelMap::new(false, false, true).unwrap()
    );
}

#[test]
fn disable_during_in_flight_joins_exact_tx_and_mints_next_generation() {
    let stopping = LegacyAdvertiserStandby::new()
        .configure(set(&[1], PrimaryAdvertisingChannelMap::all()))
        .enable()
        .unwrap()
        .prepare_event()
        .into_submitted()
        .request_disable();
    let expected = stopping.identity();
    let stale = LegacyAdvertisingEventIdentity::from_parts(2, expected.event().get());
    let LegacyAdvertiserStopCompletion::Mismatch { error, stopping } = stopping.complete(stale)
    else {
        panic!("cross-generation completion must retain stopping");
    };
    assert_eq!(error.expected, expected);

    let LegacyAdvertiserStopCompletion::Configured(configured) = stopping.complete(expected) else {
        panic!("exact completion must close stopping");
    };
    assert_eq!(configured.enable().unwrap().generation().get(), 2);
}

#[test]
fn generation_and_event_sequence_exhaustion_retain_their_owners() {
    let standby = LegacyAdvertiserStandby {
        generations: LegacyAdvertisingGenerationAllocator::from_next_generation(Some(u32::MAX)),
    };
    let enabled = standby
        .configure(set(&[], PrimaryAdvertisingChannelMap::all()))
        .enable()
        .unwrap();
    assert_eq!(enabled.generation().get(), u32::MAX);
    let configured = enabled.disable();
    assert!(configured.enable().is_err());

    let in_flight = LegacyAdvertiserStandby::new()
        .configure(set(
            &[],
            PrimaryAdvertisingChannelMap::new(true, false, false).unwrap(),
        ))
        .enable()
        .unwrap()
        .prepare_event()
        .into_submitted();
    let identity = in_flight.identity();
    let LegacyAdvertiserEventCompletion::Completed(mut complete) = in_flight.complete(identity)
    else {
        panic!("the complete hardware event must close the portable event");
    };
    complete.event_sequence = LegacyAdvertisingEventIdentity::from_parts(1, u32::MAX).event();
    let exhausted = complete
        .schedule_next(AdvertisingDelay::from_micros(0).unwrap())
        .unwrap_err();
    let configured = exhausted.into_complete().disable();
    assert_eq!(configured.enable().unwrap().generation().get(), 2);
}
