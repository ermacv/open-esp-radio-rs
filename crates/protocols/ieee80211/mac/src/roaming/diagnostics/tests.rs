use super::*;
#[test]
fn diagnostic_tokens_timeout_and_nested_information_roundtrip() {
    let information = Elements::parse(&[
        2, 8, 2, 0, 0, 0, 0, 1, 81, 6, 15, 1, 3, 7, 8, 254, 0, 0, 1, 0, 0, 0, 2, 0, 1, 3,
    ])
    .unwrap();
    let r = DiagnosticRequest {
        token: 0,
        kind: DiagnosticType::IEEE8021X,
        timeout_seconds: 257,
        information,
    };
    let mut storage = [0; 256];
    let n = r.encode(&mut storage).unwrap();
    assert_eq!(DiagnosticRequest::parse(&storage[2..n]).unwrap(), r);
    assert_eq!(r.target().unwrap().unwrap().channel, 6);
    let elements = Elements::parse(&storage[..n]).unwrap();
    let frame = DiagnosticRequestFrame {
        dialog_token: 1,
        elements,
    };
    let mut out = [0; 260];
    let n = frame.encode(&mut out).unwrap();
    assert_eq!(DiagnosticRequestFrame::parse(&out[..n]).unwrap(), frame);
}
#[test]
fn malformed_expanded_eap_and_public_values_fail_before_output_writes() {
    assert!(validate_diagnostic_information(Elements::parse(&[7, 1, 254]).unwrap()).is_err());
    let r = DiagnosticRequest {
        token: 1,
        kind: DiagnosticType::ASSOCIATION,
        timeout_seconds: 1,
        information: Elements::EMPTY,
    };
    let mut out = [0xaa; 16];
    assert!(r.encode(&mut out).is_err());
    assert_eq!(out, [0xaa; 16]);
    assert!(DiagnosticRequestFrame::parse(&[10, 2, 0, 80, 4, 1, 1, 1, 0]).is_err());
}
#[test]
fn diagnostic_frames_keep_unknown_values_and_validate_uri_placement() {
    let elements = Elements::parse(&[80, 4, 1, 1, 1, 0, 200, 1, 9, 141, 2, 1, b'x']).unwrap();
    let frame = DiagnosticRequestFrame {
        dialog_token: 7,
        elements,
    };
    let mut out = [0; 32];
    let n = frame.encode(&mut out).unwrap();
    assert_eq!(
        DiagnosticRequestFrame::parse(&out[..n]).unwrap().elements,
        elements
    );
    let reversed = Elements::parse(&[141, 2, 1, b'x', 80, 4, 1, 1, 1, 0]).unwrap();
    assert!(
        DiagnosticRequestFrame {
            dialog_token: 1,
            elements: reversed
        }
        .validate()
        .is_err()
    );
    assert!(
        DiagnosticRequestFrame::parse(&[10, 2, 1, 80, 4, 1, 1, 1, 0, 80, 4, 1, 2, 1, 0]).is_err()
    );
}

#[test]
fn credentials_keep_every_value_and_known_text_encodings_are_validated() {
    let values = [0, 3, 2, 3, 6];
    validate_diagnostic_information(Elements::parse(&values).unwrap()).unwrap();
    for bytes in [
        &[8, 1, 0xff][..],
        &[3, 3, 1, 2, 0xff][..],
        &[20, 1, 0xff][..],
    ] {
        assert!(validate_diagnostic_information(Elements::parse(bytes).unwrap()).is_err());
    }
    validate_diagnostic_information(Elements::parse(&[20, 2, 0xc3, 0xa9]).unwrap()).unwrap();
}
