use super::*;

const A: MacAddress = [2, 0, 0, 0, 0, 1];
const B: MacAddress = [2, 0, 0, 0, 0, 2];

fn qos(tid: u8) -> QosControl {
    QosControl {
        tid: Tid::new(tid).unwrap(),
        end_of_service_period: false,
        amsdu_present: false,
        high: 0,
    }
}

fn header(
    frame_control: u16,
    address4: Option<MacAddress>,
    qos: Option<QosControl>,
    ht_control: Option<[u8; 4]>,
) -> Option<HeaderSpec<IndividualAddress>> {
    HeaderSpec::new(
        FrameControl::new(frame_control)?,
        0,
        IndividualAddress::new(B).unwrap(),
        A,
        B,
        0,
        address4,
        qos,
        ht_control,
    )
}

#[test]
fn a_receiver_address_is_individual_or_group_by_its_group_bit() {
    assert!(IndividualAddress::new(A).is_some());
    assert!(IndividualAddress::new([0xff; 6]).is_none());
    assert!(GroupAddress::new([0xff; 6]).is_some());
    assert!(GroupAddress::new(A).is_none());
}

#[test]
fn a_frame_control_is_a_data_or_management_frame_of_version_zero() {
    assert!(FrameControl::new(0x0008).is_some()); // data
    assert!(FrameControl::new(0x0080).is_some()); // beacon
    assert!(FrameControl::new(0x0084).is_none()); // BlockAckReq, a control frame
    assert!(FrameControl::new(0x0009).is_none()); // protocol version 1
    assert!(Tid::new(15).is_some());
    assert!(Tid::new(16).is_none());
}

#[test]
fn a_header_agrees_with_its_frame_control() {
    // QoS data needs its QoS Control, and plain data has none.
    assert!(header(0x0088, None, Some(qos(1)), None).is_some());
    assert!(header(0x0088, None, None, None).is_none());
    assert!(header(0x0008, None, Some(qos(1)), None).is_none());
    // A fourth address exactly when To DS and From DS are both set.
    assert!(header(0x0308, Some(A), None, None).is_some());
    assert!(header(0x0308, None, None, None).is_none());
    assert!(header(0x0108, Some(A), None, None).is_none());
    // An HT Control exactly with the Order bit of a QoS data or
    // management frame; in non-QoS data the bit asks for strict order.
    assert!(header(0x8088, None, Some(qos(1)), Some([0; 4])).is_some());
    assert!(header(0x8088, None, Some(qos(1)), None).is_none());
    assert!(header(0x8080, None, None, Some([0; 4])).is_some());
    assert!(header(0x8008, None, None, None).is_some());
    assert!(header(0x8008, None, None, Some([0; 4])).is_none());
}

#[test]
fn the_qos_ack_policy_follows_the_frames_response() {
    let header = header(0x0388, Some(A), Some(qos(6)), None).unwrap();
    assert_eq!(header.len(), 32);
    let mut out = [0; 32];
    crate::HeaderLayout::new(AckHeader::new(header).unwrap(), &mut out)
        .unwrap()
        .encode();
    assert_eq!(out[30] & 0b0110_0000, 0, "Normal Ack");
    assert_eq!(out[30] & 0x0f, 6);
    crate::HeaderLayout::new(NoAckHeader::new(header).unwrap(), &mut out)
        .unwrap()
        .encode();
    assert_eq!(out[30] & 0b0110_0000, 0b0010_0000, "No Ack");
    assert_eq!(&out[4..10], &B);
}

#[test]
fn group_qos_frames_encode_no_ack_without_an_individual_response() {
    let group = GroupAddress::new([0xff; 6]).unwrap();
    let header = HeaderSpec::new(
        FrameControl::new(0x0288).unwrap(),
        0,
        group,
        A,
        B,
        0,
        None,
        Some(qos(3)),
        None,
    )
    .unwrap();
    let mut out = [0; 26];
    crate::HeaderLayout::new(header, &mut out).unwrap().encode();
    assert_eq!(out[24] & 0b0110_0000, 0b0010_0000);
    assert_eq!(out[24] & 0x0f, 3);
    assert_eq!(&out[4..10], &group.get());
}

#[test]
fn only_a_frame_that_says_so_goes_without_an_ack() {
    // QoS data carries its Ack Policy, and Action No Ack is unacknowledged.
    assert!(NoAckHeader::new(header(0x0088, None, Some(qos(1)), None).unwrap()).is_some());
    assert!(NoAckHeader::new(header(0x00e0, None, None, None).unwrap()).is_some());
    // Non-QoS data and an Action frame are acknowledged by their receiver.
    assert!(NoAckHeader::new(header(0x0008, None, None, None).unwrap()).is_none());
    assert!(NoAckHeader::new(header(0x00d0, None, None, None).unwrap()).is_none());
    // An Action No Ack frame never solicits an ACK; the others may.
    assert!(AckHeader::new(header(0x00e0, None, None, None).unwrap()).is_none());
    assert!(AckHeader::new(header(0x00d0, None, None, None).unwrap()).is_some());
    assert!(AckHeader::new(header(0x0088, None, Some(qos(1)), None).unwrap()).is_some());
}

#[test]
fn a_header_decodes_as_it_was_encoded() {
    for (control, address4, qos, ht) in [
        // Management.
        (0x00b0, None, None, None),
        // QoS data to the DS.
        (0x0188, None, Some(qos(5)), None),
        // Four-address QoS data with HT Control.
        (0x8388, Some(A), Some(qos(3)), Some([1, 2, 3, 4])),
    ] {
        let spec = header(control, address4, qos, ht).unwrap();
        let mut bytes = [0; 36];
        crate::HeaderLayout::new(AckHeader::new(spec).unwrap(), &mut bytes)
            .unwrap()
            .encode();
        let decoded = HeaderSpec::decode(&bytes[..spec.len()], IndividualAddress::new).unwrap();
        for len in 0..spec.len() {
            let mut short = [0xa5; 36];
            assert!(matches!(
                crate::HeaderLayout::new(AckHeader::new(spec).unwrap(), &mut short[..len]),
                Err(crate::LayoutError::StorageTooShort)
            ));
            assert_eq!(short, [0xa5; 36]);
            assert!(HeaderSpec::decode(&bytes[..len], IndividualAddress::new).is_none());
        }
        assert_eq!(decoded, spec);
    }
    // A group receiver is no individual header, and a control frame no
    // header at all.
    let mut group = [0; 24];
    group[4..10].copy_from_slice(&[0xff; 6]);
    assert!(HeaderSpec::<IndividualAddress>::decode(&group, IndividualAddress::new).is_none());
    assert!(HeaderSpec::<GroupAddress>::decode(&group, GroupAddress::new).is_some());
    assert!(
        HeaderSpec::<IndividualAddress>::decode(&[0xd4, 0, 0, 0], IndividualAddress::new).is_none()
    );
}
