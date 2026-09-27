use super::{
    Ieee802154RxStateCode, Ieee802154TxSecurityError, Ieee802154TxSecurityErrorObservation,
};

#[test]
fn transmit_security_error_uses_the_public_ll_reasons() {
    assert_eq!(
        Ieee802154TxSecurityErrorObservation::from_field(0),
        Ieee802154TxSecurityErrorObservation::None
    );
    for (value, reason) in [
        (1, Ieee802154TxSecurityError::FrameControlNotSet),
        (2, Ieee802154TxSecurityError::ReservedSecurityLevel),
        (3, Ieee802154TxSecurityError::HeaderParse),
        (4, Ieee802154TxSecurityError::PayloadError),
        (5, Ieee802154TxSecurityError::FrameCounterSuppression),
    ] {
        assert_eq!(
            Ieee802154TxSecurityErrorObservation::from_field(value),
            Ieee802154TxSecurityErrorObservation::Named(reason)
        );
    }
    for value in 6..=0x0f {
        assert_eq!(
            Ieee802154TxSecurityErrorObservation::from_field(value),
            Ieee802154TxSecurityErrorObservation::Unclassified
        );
    }
}

/// `ieee802154_ll_is_current_rx_frame` is `rx_state > RECEIVE_SFD`.
#[test]
fn a_frame_is_in_progress_only_after_the_sfd_state() {
    for state in 0..=Ieee802154RxStateCode::MAX {
        assert_eq!(
            Ieee802154RxStateCode::from_field(state).is_after_receive_sfd(),
            state > Ieee802154RxStateCode::RECEIVE_SFD
        );
    }
}

/// The masked event clear maps every raw field back to the event it came from.
#[test]
fn every_raw_event_maps_back_to_its_event() {
    use super::super::event_of;
    use super::{Ieee802154Event, raw_event};
    for event in [
        Ieee802154Event::TxDone,
        Ieee802154Event::RxDone,
        Ieee802154Event::AckTxDone,
        Ieee802154Event::AckRxDone,
        Ieee802154Event::RxAbort,
        Ieee802154Event::TxAbort,
        Ieee802154Event::EdDone,
        Ieee802154Event::Timer0Overflow,
        Ieee802154Event::Timer1Overflow,
        Ieee802154Event::ClockCountMatch,
        Ieee802154Event::TxSfdDone,
        Ieee802154Event::RxSfdDone,
    ] {
        assert_eq!(event_of(raw_event(event)), event);
    }
}
