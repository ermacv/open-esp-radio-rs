use super::*;
use open_esp_radio_hil_protocol::{StationPauseEvidence, StationPauseResult};

#[test]
fn detail_requires_matching_request_boot_session_and_preceding_completion() {
    let detail = Envelope::new(
        7,
        1,
        0,
        42,
        Event::StationPhyTxWaits(PhyTxWaitEvidence::default()),
    );
    let completion = Envelope::new(
        7,
        2,
        0,
        42,
        Event::StationPauseCompleted(StationPauseEvidence {
            timings: None,
            tracking: None,
            result: StationPauseResult::Resumed,
            elapsed_micros: 1,
        }),
    );
    assert_eq!(
        tx_waits(&[detail.clone(), completion.clone()], &completion).unwrap(),
        Some(PhyTxWaitEvidence::default())
    );
    assert_eq!(
        tx_waits(&[completion.clone(), detail.clone()], &completion).unwrap(),
        None
    );
    for unrelated in [
        Envelope {
            request_id: 41,
            ..detail.clone()
        },
        Envelope {
            boot_id: 6,
            ..detail.clone()
        },
        Envelope {
            session_id: 3,
            ..detail.clone()
        },
    ] {
        assert_eq!(
            tx_waits(&[unrelated, completion.clone()], &completion).unwrap(),
            None
        );
    }
    assert!(
        tx_waits(
            &[detail.clone(), detail.clone(), completion.clone()],
            &completion
        )
        .is_err()
    );
    assert!(tx_waits(&[detail], &completion).is_err());
}

#[test]
fn timer_detail_is_distinct_from_tx_detail_and_requires_its_own_correlation() {
    use open_esp_radio_hil_protocol::TimerWindowEvidence;
    let timer_event = Envelope::new(
        7,
        1,
        0,
        42,
        Event::StationTimerObserved(TimerWindowEvidence::default()),
    );
    let tx = Envelope::new(
        7,
        2,
        0,
        42,
        Event::StationPhyTxWaits(PhyTxWaitEvidence::default()),
    );
    let completion = Envelope::new(
        7,
        3,
        0,
        42,
        Event::StationPauseCompleted(StationPauseEvidence {
            timings: None,
            tracking: None,
            result: StationPauseResult::Resumed,
            elapsed_micros: 1,
        }),
    );
    let stream = [timer_event.clone(), tx, completion.clone()];
    assert_eq!(
        timer(&stream, &completion).unwrap(),
        Some(TimerWindowEvidence::default())
    );
    assert_eq!(
        tx_waits(&stream, &completion).unwrap(),
        Some(PhyTxWaitEvidence::default())
    );
    assert!(
        timer(
            &[timer_event.clone(), timer_event.clone(), completion.clone()],
            &completion
        )
        .is_err()
    );
    assert_eq!(
        timer(&[completion.clone(), timer_event.clone()], &completion).unwrap(),
        None
    );
    for unrelated in [
        Envelope {
            request_id: 1,
            ..timer_event.clone()
        },
        Envelope {
            boot_id: 6,
            ..timer_event.clone()
        },
        Envelope {
            session_id: 1,
            ..timer_event
        },
    ] {
        assert_eq!(
            timer(&[unrelated, completion.clone()], &completion).unwrap(),
            None
        );
    }
}

#[test]
fn service_detail_is_correlated_and_cannot_arrive_after_completion() {
    let detail = Envelope::new(
        7,
        1,
        0,
        42,
        Event::StationTrackingService(Default::default()),
    );
    let completion = Envelope::new(
        7,
        2,
        0,
        42,
        Event::StationPauseCompleted(StationPauseEvidence {
            timings: None,
            tracking: None,
            result: StationPauseResult::Resumed,
            elapsed_micros: 1,
        }),
    );
    assert!(
        service(&[detail.clone(), completion.clone()], &completion)
            .unwrap()
            .is_some()
    );
    assert!(
        service(&[completion.clone(), detail.clone()], &completion)
            .unwrap()
            .is_none()
    );
    assert!(
        service(
            &[detail.clone(), detail.clone(), completion.clone()],
            &completion
        )
        .is_err()
    );
    let unrelated = Envelope {
        request_id: 41,
        ..detail
    };
    assert!(
        service(&[unrelated, completion.clone()], &completion)
            .unwrap()
            .is_none()
    );
}

#[test]
fn rfpll_terminal_detail_rejects_duplicates_and_unrelated_or_late_records() {
    let value = open_esp_radio_hil_protocol::RfpllEvidence {
        sample_age_micros: None,
        temperature: 10,
        reference_before: 10,
        reference_after: 10,
        threshold: 15,
        channel: 13,
        correction: None,
    };
    let event = Envelope::new(7, 1, 0, 42, Event::StationRfpllObserved(value));
    let completion = Envelope::new(
        7,
        2,
        0,
        42,
        Event::StationPauseCompleted(StationPauseEvidence {
            timings: None,
            tracking: None,
            result: StationPauseResult::Resumed,
            elapsed_micros: 1,
        }),
    );
    assert_eq!(
        rfpll(&[event.clone(), completion.clone()], &completion).unwrap(),
        Some(value)
    );
    assert_eq!(
        rfpll(&[completion.clone(), event.clone()], &completion).unwrap(),
        None
    );
    assert!(
        rfpll(
            &[event.clone(), event.clone(), completion.clone()],
            &completion
        )
        .is_err()
    );
    for other in [
        Envelope {
            boot_id: 8,
            ..event.clone()
        },
        Envelope {
            request_id: 41,
            ..event.clone()
        },
        Envelope {
            session_id: 1,
            ..event
        },
    ] {
        assert_eq!(
            rfpll(&[other, completion.clone()], &completion).unwrap(),
            None
        );
    }
}

#[test]
fn rx_gain_requires_matching_request_boot_session_and_preceding_completion() {
    let detail = Envelope::new(
        7,
        1,
        0,
        42,
        Event::StationPhyRxGain(open_esp_radio_hil_protocol::PhyRxGainEvidence::default()),
    );
    let completion = Envelope::new(
        7,
        2,
        0,
        42,
        Event::StationPauseCompleted(StationPauseEvidence {
            timings: None,
            tracking: None,
            result: StationPauseResult::Resumed,
            elapsed_micros: 1,
        }),
    );
    assert_eq!(
        rx_gain(&[detail.clone(), completion.clone()], &completion).unwrap(),
        Some(open_esp_radio_hil_protocol::PhyRxGainEvidence::default())
    );
    assert_eq!(
        rx_gain(&[completion.clone(), detail.clone()], &completion).unwrap(),
        None
    );
    for unrelated in [
        Envelope {
            request_id: 41,
            ..detail.clone()
        },
        Envelope {
            boot_id: 6,
            ..detail.clone()
        },
        Envelope {
            session_id: 3,
            ..detail.clone()
        },
    ] {
        assert_eq!(
            rx_gain(&[unrelated, completion.clone()], &completion).unwrap(),
            None
        );
    }
    assert!(
        rx_gain(
            &[detail.clone(), detail.clone(), completion.clone()],
            &completion
        )
        .is_err()
    );
    assert!(rx_gain(&[detail], &completion).is_err());
}
