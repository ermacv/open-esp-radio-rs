use super::*;

#[test]
fn every_legal_tx_length_has_exact_layout_and_zero_fcs() {
    let mut bytes = [0xa5; FRAME_BUFFER_SIZE];
    let source = [0x5a; MAX_MAC_FRAME_SIZE];

    for length in MIN_MAC_FRAME_SIZE..=MAX_MAC_FRAME_SIZE {
        let phr = prepare_tx(&mut bytes, &source[..length]).unwrap();
        let view = TxFrameView::new(&bytes, phr);
        assert_eq!(usize::from(phr), length + 2);
        assert_eq!(view.mac_bytes(), &source[..length]);
        assert_eq!(view.reserved_fcs(), &[0, 0]);
        assert_eq!(view.dma_length(), length + 3);
        assert!(
            view.buffer()[view.dma_length()..]
                .iter()
                .all(|byte| *byte == 0)
        );
    }
}

#[test]
fn invalid_tx_lengths_do_not_modify_destination() {
    let mut bytes = [0xa5; FRAME_BUFFER_SIZE];
    let original = bytes;
    assert_eq!(
        prepare_tx(&mut bytes, &[]),
        Err(TxFrameError::MacLengthOutOfRange { length: 0 })
    );
    assert_eq!(bytes, original);
    assert_eq!(
        prepare_tx(&mut bytes, &[0; MAX_MAC_FRAME_SIZE + 1]),
        Err(TxFrameError::MacLengthOutOfRange {
            length: MAX_MAC_FRAME_SIZE + 1
        })
    );
    assert_eq!(bytes, original);
}

#[test]
fn every_fcf_octet_is_classified_before_destination_mutation() {
    for fcf in u8::MIN..=u8::MAX {
        let mut bytes = [0xa5; FRAME_BUFFER_SIZE];
        let original = bytes;
        let frame_type = fcf & FRAME_TYPE_MASK;
        let result = prepare_tx(&mut bytes, &[fcf]);

        if frame_type <= MAX_SUPPORTED_FRAME_TYPE {
            let phr = result.unwrap();
            let view = TxFrameView::new(&bytes, phr);
            assert_eq!(
                view.acknowledgement_requested(),
                fcf & ACKNOWLEDGEMENT_REQUEST_BIT != 0,
                "FCF 0x{fcf:02x}",
            );
            assert_eq!(view.mac_bytes(), &[fcf]);
        } else {
            assert_eq!(
                result,
                Err(TxFrameError::UnsupportedFrameType { frame_type }),
                "FCF 0x{fcf:02x}",
            );
            assert_eq!(bytes, original, "FCF 0x{fcf:02x}");
        }
    }
}

#[test]
fn rx_minimum_and_maximum_layouts_are_exact() {
    for phr in [MIN_PHR_LENGTH, MAX_PHR_LENGTH] {
        let mut bytes = [0; FRAME_BUFFER_SIZE];
        bytes[0] = phr;
        bytes[phr as usize - 1] = (-42_i8) as u8;
        bytes[phr as usize] = 211;
        let view = RxFrameView::parse(&bytes).unwrap();
        assert_eq!(view.phr_length(), phr);
        assert_eq!(view.mac_bytes().len(), phr as usize - 2);
        assert_eq!(view.rssi(), -42);
        assert_eq!(view.lqi(), 211);
    }
}

#[test]
fn rx_rejects_every_out_of_range_phr() {
    for phr in 0_u8..=u8::MAX {
        let mut bytes = [0; FRAME_BUFFER_SIZE];
        bytes[0] = phr;
        let result = RxFrameView::parse(&bytes);
        assert_eq!(
            result.is_ok(),
            (MIN_PHR_LENGTH..=MAX_PHR_LENGTH).contains(&phr)
        );
    }
}
