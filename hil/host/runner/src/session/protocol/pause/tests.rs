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
