use super::*;

#[test]
fn initial_request_reference_layout_and_strict_tail() {
    // Public Action Initial Request, ANQP Advertisement IE, empty Query List.
    let bytes = [4, 10, 7, 108, 2, 0, 0, 6, 0, 0, 1, 2, 0, 1, 1];
    let frame = Frame::parse(&bytes).unwrap();
    let Body::InitialRequest {
        advertisement,
        query,
    } = frame.body
    else {
        panic!()
    };
    assert_eq!(
        advertisement.protocol,
        ProtocolId::Standard(ANQP_PROTOCOL_ID)
    );
    assert_eq!(query, &[0, 1, 2, 0, 1, 1]);
    for length in 0..bytes.len() {
        assert!(Frame::parse(&bytes[..length]).is_err());
    }
    let mut padded = bytes.to_vec();
    padded.push(0);
    assert_eq!(Frame::parse(&padded), Err(WireError::InconsistentFields));
    let mut output = [0u8; 15];
    assert_eq!(frame.encode(&mut output), Ok(bytes.len()));
    assert_eq!(output, bytes);
}
#[test]
fn comeback_fragment_layout_and_delayed_constraints() {
    let bytes = [9, 13, 0, 0, 0, 0x81, 0, 0, 108, 2, 127, 0, 2, 0, 0xab, 0xcd];
    let frame = Frame::parse(&bytes).unwrap();
    let Body::ComebackResponse {
        fragment, response, ..
    } = frame.body
    else {
        panic!()
    };
    assert_eq!(fragment.id(), 1);
    assert!(fragment.more());
    assert_eq!(response, &[0xab, 0xcd]);
    let mut delayed = bytes;
    delayed[6] = 1;
    assert_eq!(Frame::parse(&delayed), Err(WireError::InconsistentFields));
    assert_eq!(
        Fragment::new(MAX_FRAGMENT_ID, true),
        Err(WireError::InvalidFragment)
    );
    let mut error = bytes;
    error[3] = Status::RESPONSE_OUTSTANDING.0 as u8;
    assert!(Frame::parse(&error).is_ok());
}
#[test]
fn vendor_protocol_identity_is_not_anqp_and_multiple_tuples_are_not_action_legal() {
    let vendor = [0x50, 0x6f, 0x9a, 0x1a, 1];
    let advertisement = AdvertisementProtocol {
        info: ResponseInfo::new(127, true).unwrap(),
        protocol: ProtocolId::Vendor(&vendor),
    };
    let mut buffer = [0u8; 32];
    let length = advertisement.encode(&mut buffer).unwrap();
    assert_eq!(
        &buffer[..length],
        &[108, 8, 255, 221, 5, 0x50, 0x6f, 0x9a, 0x1a, 1]
    );
    assert_eq!(
        AdvertisementProtocol::parse(&buffer[..length]).unwrap(),
        advertisement
    );
    let multiple = [108, 4, 0, 0, 0, 1];
    assert_eq!(
        AdvertisementProtocols::parse(&multiple)
            .unwrap()
            .iter()
            .count(),
        2
    );
    assert!(AdvertisementProtocol::parse(&multiple).is_err());
}
#[test]
fn failed_encoding_does_not_modify_output() {
    let advertisement = AdvertisementProtocol {
        info: ResponseInfo::new(1, false).unwrap(),
        protocol: ProtocolId::Standard(ANQP_PROTOCOL_ID),
    };
    let frame = Frame {
        category: Category::Public,
        dialog_token: 1,
        body: Body::InitialResponse {
            status: Status::SUCCESS,
            comeback_delay_tu: 1,
            advertisement,
            response: &[1],
        },
    };
    let mut output = [0x55; 16];
    assert_eq!(
        frame.encode(&mut output),
        Err(WireError::InconsistentFields)
    );
    assert_eq!(output, [0x55; 16]);
}
