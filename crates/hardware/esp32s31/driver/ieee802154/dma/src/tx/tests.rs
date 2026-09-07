use super::*;
use crate::{DMA_LOW, MAX_MAC_FRAME_SIZE};

fn owner(address: u32) -> PinnedTxBuffer {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(TxStorage::new()));
    TxStorage::pin_static_model(storage, DmaFrameAddress::try_new(address).unwrap())
}

#[test]
fn storage_frame_has_exact_geometry() {
    assert_eq!(core::mem::size_of::<TxFrameBuffer>(), FRAME_BUFFER_SIZE);
    assert_eq!(core::mem::align_of::<TxFrameBuffer>(), 4);
    let storage = TxStorage::new();
    assert_eq!((&storage.frame as *const TxFrameBuffer).addr() & 3, 0);
}

#[test]
fn prepared_armed_completed_release_is_linear() {
    let mut owner = owner(DMA_LOW);
    let PreparedTx::AckRequested(prepared) = owner.prepare(&[0x61, 0x88, 0x01]).unwrap() else {
        panic!("FCF requests an ACK");
    };
    assert_eq!(prepared.frame().phr_length(), 5);
    assert_eq!(prepared.frame().reserved_fcs(), &[0, 0]);
    let armed = prepared.arm();
    assert_eq!(armed.dma_address().as_u32(), DMA_LOW);
    let terminal = DmaTerminalEvidence::for_native_model();
    let completed = armed.complete(&terminal);
    assert_eq!(completed.frame().mac_bytes(), &[0x61, 0x88, 0x01]);
    completed.release();
    assert_eq!(owner.state(), TxState::Free);
}

#[test]
fn cancel_returns_prepared_buffer_to_free() {
    let mut owner = owner(DMA_LOW);
    let PreparedTx::AckNotRequested(prepared) = owner.prepare(&[1]).unwrap() else {
        panic!("FCF does not request an ACK");
    };
    prepared.cancel();
    assert_eq!(owner.state(), TxState::Free);
    assert!(owner.prepare(&[2]).is_ok());
}

#[test]
fn every_fcf_octet_mints_only_its_immutable_image_mode() {
    let mut owner = owner(DMA_LOW);
    for fcf in u8::MIN..=u8::MAX {
        let original = owner.storage.as_ref().get_ref().frame.0;
        let frame_type = fcf & 0x07;
        match owner.prepare(&[fcf]) {
            Ok(PreparedTx::AckRequested(prepared)) => {
                assert!(frame_type <= 3, "FCF 0x{fcf:02x}");
                assert_ne!(fcf & 0x20, 0, "FCF 0x{fcf:02x}");
                assert_eq!(prepared.frame().buffer()[1], fcf);
                assert!(prepared.frame().acknowledgement_requested());
                prepared.cancel();
            }
            Ok(PreparedTx::AckNotRequested(prepared)) => {
                assert!(frame_type <= 3, "FCF 0x{fcf:02x}");
                assert_eq!(fcf & 0x20, 0, "FCF 0x{fcf:02x}");
                assert_eq!(prepared.frame().buffer()[1], fcf);
                assert!(!prepared.frame().acknowledgement_requested());
                prepared.cancel();
            }
            Err(TxStorageError::Frame(TxFrameError::UnsupportedFrameType {
                frame_type: rejected,
            })) => {
                assert!(frame_type > 3, "FCF 0x{fcf:02x}");
                assert_eq!(rejected, frame_type);
                assert_eq!(owner.state(), TxState::Free);
                assert_eq!(owner.storage.as_ref().get_ref().frame.0, original);
            }
            Err(error) => panic!("unexpected FCF 0x{fcf:02x} error: {error:?}"),
        }
        assert_eq!(owner.state(), TxState::Free);
    }
}

#[test]
fn mode_is_bound_to_the_copy_not_the_caller_slice() {
    let mut owner = owner(DMA_LOW);
    let mut source = [0x21, 0x88, 0x01];
    let PreparedTx::AckRequested(prepared) = owner.prepare(&source).unwrap() else {
        panic!("fixture requests an ACK");
    };

    source[0] = 0x01;
    assert_eq!(source[0], 0x01);
    assert_eq!(prepared.frame().mac_bytes(), &[0x21, 0x88, 0x01]);
    assert!(prepared.frame().acknowledgement_requested());

    let armed = prepared.arm();
    let completed = armed.complete(&DmaTerminalEvidence::for_native_model());
    completed.release();
    assert_eq!(owner.state(), TxState::Free);
}

#[test]
fn invalid_input_leaves_storage_free() {
    let mut owner = owner(DMA_LOW);
    assert!(matches!(
        owner.prepare(&[]),
        Err(TxStorageError::Frame(TxFrameError::MacLengthOutOfRange {
            length: 0
        }))
    ));
    assert!(matches!(
        owner.prepare(&[0; MAX_MAC_FRAME_SIZE + 1]),
        Err(TxStorageError::Frame(
            TxFrameError::MacLengthOutOfRange { .. }
        ))
    ));
    assert_eq!(owner.state(), TxState::Free);
}
