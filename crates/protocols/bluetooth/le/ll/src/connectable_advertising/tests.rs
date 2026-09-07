use super::*;
use crate::{
    advertising::LEGACY_ADVERTISING_PDU_CAPACITY,
    connection::{LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES},
};

const ADVERTISER_BYTES: [u8; 6] = [7, 8, 9, 10, 11, 12];

fn advertiser(
    support: LeChannelSelectionAlgorithmTwoSupport,
) -> LegacyConnectableAdvertisingSet<'static> {
    LegacyConnectableAdvertisingSet::new(
        LegacyConnectableAdvertisement::new(
            LeDeviceAddress::from_wire_bytes(ADVERTISER_BYTES, LeDeviceAddressKind::Random),
            LegacyAdvertisingData::new(&[2, 1, 6]).unwrap(),
            support,
        ),
        LegacyScanResponseData::new(&[3, 9, 8, 7]).unwrap(),
        PrimaryAdvertisingChannelMap::all(),
        AdvertisingInterval::new(32).unwrap(),
    )
}

fn first_event(
    support: LeChannelSelectionAlgorithmTwoSupport,
) -> LegacyConnectableAdvertisingEvent<'static> {
    LegacyConnectableAdvertiserStandby::new()
        .configure(advertiser(support))
        .enable()
        .unwrap()
}

fn connection_request(
    advertiser: [u8; 6],
    channel_selection_two: bool,
) -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
    let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
    pdu[0] = 0b0101 | (1 << 7) | if channel_selection_two { 1 << 5 } else { 0 };
    pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
    pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    pdu[8..14].copy_from_slice(&advertiser);
    pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
    pdu[21] = 2;
    pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
    pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
    pdu[26..28].copy_from_slice(&0u16.to_le_bytes());
    pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
    pdu[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
    pdu[35] = 5 | (4 << 5);
    pdu
}

#[test]
fn adv_ind_roundtrips_with_channel_selection_capability() {
    let advertisement =
        advertiser(LeChannelSelectionAlgorithmTwoSupport::Supported).advertisement();
    let mut encoded = [0; LEGACY_ADVERTISING_PDU_CAPACITY];
    let length = advertisement.encode(&mut encoded).unwrap();

    assert_eq!(
        LegacyConnectableAdvertisement::decode(&encoded[..length]),
        Ok(advertisement)
    );
    assert_eq!(encoded[0], 0x60);
    assert_eq!(&encoded[2..8], &ADVERTISER_BYTES);
}

#[test]
fn prepared_event_owns_matching_adv_ind_and_scan_response_pdus() {
    let prepared = first_event(LeChannelSelectionAlgorithmTwoSupport::Supported).prepare();
    let identity = prepared.identity();
    let adv_ind = prepared.adv_ind_pdu();
    let scan_response = prepared.scan_response_pdu();

    assert_eq!(
        LegacyConnectableAdvertisement::decode(adv_ind.as_bytes()),
        Ok(prepared.advertisement())
    );
    assert_eq!(scan_response.as_bytes()[0], 0x44);
    assert_eq!(&scan_response.as_bytes()[2..8], &ADVERTISER_BYTES);
    assert_eq!(&scan_response.as_bytes()[8..], &[3, 9, 8, 7]);
    assert_eq!(scan_response.payload_length(), 10);

    let event = prepared.cancel();
    assert_eq!(event.identity(), identity);
    assert_eq!(event.prepare().scan_response().as_bytes(), &[3, 9, 8, 7]);
}

#[test]
fn rejected_response_retains_the_exact_in_flight_event_for_a_later_packet() {
    let in_flight = first_event(LeChannelSelectionAlgorithmTwoSupport::Supported)
        .prepare()
        .into_submitted();
    let identity = in_flight.identity();
    let LegacyConnectableConnectionRequestAdmission::Rejected(rejected) =
        in_flight.admit_connection_request(&connection_request([9; 6], true))
    else {
        panic!("a request for another advertiser must be rejected");
    };
    assert_eq!(
        rejected.error(),
        LegacyConnectableConnectionRequestRejection::DifferentAdvertiser
    );
    assert_eq!(rejected.in_flight.identity(), identity);

    let accepted = rejected
        .into_in_flight()
        .admit_connection_request(&connection_request(ADVERTISER_BYTES, true));
    let LegacyConnectableConnectionRequestAdmission::Accepted(accepted) = accepted else {
        panic!("the retained event must admit a valid request");
    };
    assert_eq!(
        accepted.request().advertiser().wire_bytes(),
        ADVERTISER_BYTES
    );
    assert_eq!(accepted.identity(), identity);
    let (configured, accepted_identity, connection) = accepted.into_parts();
    assert_eq!(accepted_identity, identity);
    assert_eq!(
        configured.enable().unwrap().prepare().channels(),
        PrimaryAdvertisingChannelMap::all()
    );
    assert_eq!(connection.event_counter(), 0);
}

#[test]
fn algorithm_two_rejection_keeps_the_submitted_event_in_flight() {
    let in_flight = first_event(LeChannelSelectionAlgorithmTwoSupport::Unsupported)
        .prepare()
        .into_submitted();
    let identity = in_flight.identity();
    let LegacyConnectableConnectionRequestAdmission::Rejected(rejected) =
        in_flight.admit_connection_request(&connection_request(ADVERTISER_BYTES, true))
    else {
        panic!("algorithm two cannot be negotiated without advertised support");
    };
    assert_eq!(
        rejected.error(),
        LegacyConnectableConnectionRequestRejection::UnsupportedChannelSelectionAlgorithmTwo
    );
    let retained = rejected.into_in_flight();
    assert_eq!(retained.identity(), identity);
    let configured = retained.complete_without_connection().disable();
    assert_eq!(
        configured.enable().unwrap().prepare().channels(),
        PrimaryAdvertisingChannelMap::all()
    );
}

#[test]
fn no_request_completion_requires_a_fresh_bounded_delay() {
    let first = first_event(LeChannelSelectionAlgorithmTwoSupport::Supported);
    let first_identity = first.identity();
    let scheduled = first
        .prepare()
        .into_submitted()
        .complete_without_connection()
        .schedule_next(AdvertisingDelay::from_micros(7_500).unwrap())
        .unwrap();
    assert_eq!(scheduled.start_offset_micros(), 27_500);
    assert_eq!(
        scheduled.identity().generation(),
        first_identity.generation()
    );
    assert_eq!(scheduled.identity().event().get(), 1);
    assert_eq!(
        scheduled
            .into_event()
            .prepare()
            .advertisement()
            .advertiser()
            .wire_bytes(),
        ADVERTISER_BYTES
    );
}

#[test]
fn disable_and_reenable_mints_a_new_generation_at_event_zero() {
    let first = first_event(LeChannelSelectionAlgorithmTwoSupport::Supported);
    let first_identity = first.identity();
    let second = first.disable().enable().unwrap();

    assert_eq!(second.identity().generation().get(), 2);
    assert_eq!(second.identity().event().get(), 0);
    assert_ne!(second.identity().generation(), first_identity.generation());
}

#[test]
fn generation_and_event_exhaustion_return_lossless_connectable_owners() {
    let standby = LegacyConnectableAdvertiserStandby {
        generations: LegacyAdvertisingGenerationAllocator::from_next_generation(Some(u32::MAX)),
    };
    let last_generation = standby
        .configure(advertiser(LeChannelSelectionAlgorithmTwoSupport::Supported))
        .enable()
        .unwrap();
    assert_eq!(last_generation.identity().generation().get(), u32::MAX);
    let exhausted = last_generation.disable().enable().unwrap_err();
    let configured = exhausted.into_configured().reconfigure(advertiser(
        LeChannelSelectionAlgorithmTwoSupport::Unsupported,
    ));
    assert!(configured.enable().is_err());

    let mut last_event = first_event(LeChannelSelectionAlgorithmTwoSupport::Supported);
    last_event.identity = LegacyAdvertisingEventIdentity::from_parts(1, u32::MAX);
    let exhausted = last_event
        .prepare()
        .into_submitted()
        .complete_without_connection()
        .schedule_next(AdvertisingDelay::from_micros(0).unwrap())
        .unwrap_err();
    let next_generation = exhausted.into_complete().disable().enable().unwrap();
    assert_eq!(next_generation.identity().generation().get(), 2);
    assert_eq!(next_generation.identity().event().get(), 0);
}
