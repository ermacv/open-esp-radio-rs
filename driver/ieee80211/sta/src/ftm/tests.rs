use super::*;
use open_esp_radio_ieee80211::ftm::{
    FTM_MEASUREMENT_PREFIX_LEN, FTM_PARAMETERS_ELEMENT_LEN, FtmMeasurementFields,
    encode_measurement,
};

fn ps(value: u64) -> FtmTimestampPs {
    FtmTimestampPs::new(value).unwrap()
}

fn request_parameters(count: u8) -> FtmRequestParameters {
    FtmRequestParameters::new(
        0,
        FtmBurstDuration::Millis8,
        2,
        None,
        true,
        count,
        FtmFormatAndBandwidth::HtMixed20Mhz,
        0,
    )
    .unwrap()
}

fn config(count: u8, attempts: u8) -> FtmRequesterConfig {
    FtmRequesterConfig::new(request_parameters(count), 1_000, 100, 10_000, attempts).unwrap()
}

fn response(count: u8) -> FtmResponseParameters {
    FtmResponseParameters {
        status: FtmResponseStatus::Success,
        number_of_bursts_exponent: 0,
        burst_duration: FtmBurstDuration::Millis8,
        min_delta_ftm_100us: 2,
        partial_tsf_timer: 10,
        asap_capable: true,
        asap: true,
        ftms_per_burst: count,
        format_and_bandwidth: FtmFormatAndBandwidth::HtMixed20Mhz,
        burst_period_100ms: 0,
    }
}

fn measurement_body<const N: usize>(
    fields: FtmMeasurementFields,
    parameters: Option<FtmResponseParameters>,
) -> [u8; N] {
    let mut body = [0_u8; N];
    let element = parameters.map(|parameters| parameters.encode_element().unwrap());
    let information_elements = element.as_ref().map_or(&[][..], |element| &element[..]);
    assert_eq!(
        encode_measurement(fields, information_elements, &mut body),
        Ok(N)
    );
    body
}

fn initial_body(
    token: u8,
    count: u8,
) -> [u8; FTM_MEASUREMENT_PREFIX_LEN + FTM_PARAMETERS_ELEMENT_LEN] {
    measurement_body(
        FtmMeasurementFields {
            dialog_token: token,
            follow_up_dialog_token: 0,
            tod: FtmTimestampPs::ZERO,
            toa: FtmTimestampPs::ZERO,
            tod_error: FtmTodError {
                max_error_exponent: 0,
                not_continuous: false,
            },
            toa_error: FtmToaError {
                max_error_exponent: 0,
            },
        },
        Some(response(count)),
    )
}

fn follow_up_body(
    dialog_token: u8,
    follow_up_dialog_token: u8,
    t1: u64,
    t4: u64,
) -> [u8; FTM_MEASUREMENT_PREFIX_LEN] {
    measurement_body(
        FtmMeasurementFields {
            dialog_token,
            follow_up_dialog_token,
            tod: ps(t1),
            toa: ps(t4),
            tod_error: FtmTodError {
                max_error_exponent: 2,
                not_continuous: false,
            },
            toa_error: FtmToaError {
                max_error_exponent: 3,
            },
        },
        None,
    )
}

fn start_published<const N: usize>(requester: &mut FtmRequester<N>, peer: [u8; 6]) {
    requester.start(peer, 10).unwrap();
    let FtmRequesterService::Transmit(transmission) = requester.service(10).unwrap() else {
        panic!("request must be ready")
    };
    assert_eq!(transmission.body()[..3], [4, 32, 1]);
    requester
        .complete_transmission(transmission, true, 20)
        .unwrap();
}

#[test]
fn config_rejects_unbounded_or_scheduled_profiles() {
    let multiple = FtmRequestParameters::new(
        1,
        FtmBurstDuration::Millis8,
        2,
        None,
        true,
        4,
        FtmFormatAndBandwidth::HtMixed20Mhz,
        1,
    )
    .unwrap();
    assert_eq!(
        FtmRequesterConfig::new(multiple, 1, 1, 1, 1),
        Err(FtmRequesterConfigError::MultipleBurstsUnsupported)
    );
    let unbounded = FtmRequestParameters::new(
        0,
        FtmBurstDuration::Millis8,
        2,
        None,
        true,
        0,
        FtmFormatAndBandwidth::HtMixed20Mhz,
        0,
    )
    .unwrap();
    assert_eq!(
        FtmRequesterConfig::new(unbounded, 1, 1, 1, 1),
        Err(FtmRequesterConfigError::NoMeasurementCountPreferenceUnsupported)
    );
}

#[test]
fn request_retries_have_affine_transmission_identity() {
    let mut requester = FtmRequester::<2>::new(config(2, 2));
    requester.start([1; 6], 0).unwrap();
    let FtmRequesterService::Transmit(first) = requester.service(0).unwrap() else {
        panic!()
    };
    assert_eq!(
        requester.complete_transmission(first, false, 10),
        Ok(FtmRequesterEvent::RequestRetryScheduled {
            retry_at_micros: 110
        })
    );
    assert_eq!(requester.service(109), Ok(FtmRequesterService::Idle));
    let FtmRequesterService::Transmit(second) = requester.service(110).unwrap() else {
        panic!()
    };
    assert_ne!(
        first.transmission_generation(),
        second.transmission_generation()
    );
    assert_eq!(
        requester.complete_transmission(first, true, 111),
        Err(FtmRequesterError::StaleTransmission)
    );
    requester.complete_transmission(second, true, 111).unwrap();
    assert_eq!(requester.next_deadline_micros(), Some(1_111));
}

#[test]
fn mutated_body_cannot_complete_or_reject_the_exact_pending_transmission() {
    let mut completion = FtmRequester::<1>::new(config(2, 1));
    completion.start([1; 6], 0).unwrap();
    let FtmRequesterService::Transmit(valid_completion) = completion.service(0).unwrap() else {
        panic!()
    };
    let mut mutated_completion = valid_completion;
    mutated_completion.body[2] = 0;
    assert_eq!(
        completion.complete_transmission(mutated_completion, true, 10),
        Err(FtmRequesterError::StaleTransmission)
    );
    assert_eq!(
        completion.complete_transmission(valid_completion, true, 10),
        Ok(FtmRequesterEvent::RequestPublished {
            response_deadline_micros: 1_010,
            session_deadline_micros: 10_010,
        })
    );

    let mut admission = FtmRequester::<1>::new(config(2, 1));
    admission.start([2; 6], 20).unwrap();
    let FtmRequesterService::Transmit(valid_admission) = admission.service(20).unwrap() else {
        panic!()
    };
    let mut mutated_admission = valid_admission;
    mutated_admission.body[0] = 0;
    assert_eq!(
        admission.reject_hardware_admission(mutated_admission),
        Err(FtmRequesterError::StaleTransmission)
    );
    assert_eq!(
        admission.reject_hardware_admission(valid_admission),
        Ok(FtmRequesterEvent::Failed(
            FtmSessionFailure::HardwareAdmissionRejected
        ))
    );
}

#[test]
fn three_ftm_frames_deliver_two_owned_samples_without_claiming_distance() {
    let peer = [0x22; 6];
    let mut requester = FtmRequester::<2>::new(config(3, 1));
    start_published(&mut requester, peer);

    let initial = initial_body(1, 3);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(1_100), ps(1_200)).unwrap()),
            30,
        ),
        Ok(FtmMeasurementDisposition::InitialAccepted {
            dialog_token: 1,
            allocated_ftms_per_burst: 3,
            maximum_samples: 2,
        })
    );

    let follow_up = follow_up_body(2, 1, 1_000, 1_400);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&follow_up).unwrap(),
            Some(FtmLocalExchangeTiming::new(2, ps(1_500), ps(1_600)).unwrap()),
            40,
        ),
        Ok(FtmMeasurementDisposition::SampleAccepted {
            dialog_token: 1,
            sample_index: 0,
        })
    );
    let terminal = follow_up_body(0, 2, 1_400, 1_800);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&terminal).unwrap(),
            None,
            50,
        ),
        Ok(FtmMeasurementDisposition::Complete { samples: 2 })
    );

    let result = requester.take_result().unwrap();
    assert_eq!(result.peer, peer);
    assert_eq!(result.sample_count(), 2);
    assert_eq!(
        result.sample(0).unwrap().raw_interval_difference(),
        Ok(FtmRawIntervalDifferencePs {
            responder_round_trip_ps: 400,
            initiator_turnaround_ps: 100,
            difference_ps: 300,
        })
    );
    assert_eq!(
        result.sample(1).unwrap().raw_interval_difference(),
        Ok(FtmRawIntervalDifferencePs {
            responder_round_trip_ps: 400,
            initiator_turnaround_ps: 100,
            difference_ps: 300,
        })
    );
    assert!(requester.is_idle());
}

#[test]
fn ftm_retransmission_deduplicates_follow_up_but_owns_new_token() {
    let peer = [3; 6];
    let mut requester = FtmRequester::<2>::new(config(3, 1));
    start_published(&mut requester, peer);
    let initial = initial_body(1, 3);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            30,
        )
        .unwrap();
    let measured = follow_up_body(2, 1, 80, 160);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&measured).unwrap(),
            Some(FtmLocalExchangeTiming::new(2, ps(200), ps(220)).unwrap()),
            40,
        )
        .unwrap();
    let retry = follow_up_body(3, 1, 80, 160);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&retry).unwrap(),
            Some(FtmLocalExchangeTiming::new(3, ps(300), ps(320)).unwrap()),
            50,
        ),
        Ok(FtmMeasurementDisposition::DuplicateSample { dialog_token: 1 })
    );
    let terminal = follow_up_body(0, 3, 280, 360);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&terminal).unwrap(),
            None,
            60,
        )
        .unwrap();
    let result = requester.take_result().unwrap();
    assert_eq!(result.sample_count(), 2);
    assert!(result.sample(0).is_some());
    assert_eq!(result.sample(1).unwrap().dialog_token, 3);
}

#[test]
fn abandoned_retransmission_token_cannot_be_reused() {
    let peer = [4; 6];
    let mut requester = FtmRequester::<3>::new(config(4, 1));
    start_published(&mut requester, peer);
    let initial = initial_body(1, 4);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            30,
        )
        .unwrap();
    let measured = follow_up_body(2, 1, 80, 160);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&measured).unwrap(),
            Some(FtmLocalExchangeTiming::new(2, ps(200), ps(220)).unwrap()),
            40,
        )
        .unwrap();
    let retry = follow_up_body(3, 1, 80, 160);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&retry).unwrap(),
            Some(FtmLocalExchangeTiming::new(3, ps(300), ps(320)).unwrap()),
            50,
        )
        .unwrap();
    let reused = follow_up_body(2, 3, 280, 360);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&reused).unwrap(),
            Some(FtmLocalExchangeTiming::new(2, ps(400), ps(420)).unwrap()),
            60,
        ),
        Err(FtmRequesterError::DialogTokenReused)
    );
    assert_eq!(
        requester.failure(),
        Some(FtmSessionFailure::ProtocolViolation)
    );
}

#[test]
fn capacity_and_hardware_admission_fail_before_publication() {
    let mut too_small = FtmRequester::<1>::new(config(2, 1));
    assert_eq!(too_small.start([1; 6], 0), Ok(1));
    assert!(matches!(
        too_small.service(0),
        Ok(FtmRequesterService::Transmit(_))
    ));

    let mut too_small = FtmRequester::<1>::new(config(3, 1));
    assert_eq!(
        too_small.start([1; 6], 0),
        Err(FtmRequesterError::CapacityTooSmall {
            required_samples: 2,
            capacity: 1,
        })
    );

    let mut requester = FtmRequester::<2>::new(config(3, 1));
    requester.start([1; 6], 0).unwrap();
    let FtmRequesterService::Transmit(transmission) = requester.service(0).unwrap() else {
        panic!()
    };
    assert_eq!(
        requester.reject_hardware_admission(transmission),
        Ok(FtmRequesterEvent::Failed(
            FtmSessionFailure::HardwareAdmissionRejected
        ))
    );
    assert_eq!(
        requester.take_failure(),
        Ok(FtmSessionFailure::HardwareAdmissionRejected)
    );
}

#[test]
fn one_ftm_frame_is_a_terminal_initial_with_zero_samples() {
    let peer = [5; 6];
    let mut requester = FtmRequester::<0>::new(config(1, 1));
    start_published(&mut requester, peer);
    let terminal_initial = initial_body(0, 1);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&terminal_initial).unwrap(),
            None,
            30,
        ),
        Ok(FtmMeasurementDisposition::Complete { samples: 0 })
    );
    let result = requester.take_result().unwrap();
    assert_eq!(result.negotiated.ftms_per_burst, 1);
    assert_eq!(result.sample_count(), 0);

    let mut nonterminal = FtmRequester::<0>::new(config(1, 1));
    start_published(&mut nonterminal, peer);
    let invalid = initial_body(1, 1);
    assert_eq!(
        nonterminal.on_measurement(
            peer,
            FtmMeasurement::decode_body(&invalid).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            30,
        ),
        Err(FtmRequesterError::TooManyMeasurements)
    );
    assert_eq!(nonterminal.sample_count, 0);
}

#[test]
fn initial_retransmission_replaces_timing_and_tokens_wrap_consecutively() {
    let peer = [6; 6];
    let mut requester = FtmRequester::<2>::new(config(3, 1));
    start_published(&mut requester, peer);
    let initial = initial_body(254, 3);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(254, ps(100), ps(120)).unwrap()),
            30,
        )
        .unwrap();

    let retransmission = initial_body(255, 3);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&retransmission).unwrap(),
            Some(FtmLocalExchangeTiming::new(255, ps(200), ps(220)).unwrap()),
            40,
        ),
        Ok(FtmMeasurementDisposition::InitialRetransmissionAccepted {
            abandoned_dialog_token: 254,
            dialog_token: 255,
        })
    );

    let follow_up = follow_up_body(1, 255, 180, 260);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&follow_up).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(300), ps(320)).unwrap()),
            50,
        ),
        Ok(FtmMeasurementDisposition::SampleAccepted {
            dialog_token: 255,
            sample_index: 0,
        })
    );
    let terminal = follow_up_body(0, 1, 280, 360);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&terminal).unwrap(),
            None,
            60,
        )
        .unwrap();
    let result = requester.take_result().unwrap();
    assert_eq!(result.sample_count(), 2);
    assert_eq!(result.sample(0).unwrap().dialog_token, 255);
    assert_eq!(result.sample(0).unwrap().initiator_arrival_t2_ps, ps(200));
}

#[test]
fn nonconsecutive_dialog_token_fails_before_sample_mutation() {
    let peer = [7; 6];
    let mut requester = FtmRequester::<2>::new(config(3, 1));
    start_published(&mut requester, peer);
    let initial = initial_body(1, 3);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            30,
        )
        .unwrap();
    let skipped = follow_up_body(3, 1, 80, 160);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&skipped).unwrap(),
            Some(FtmLocalExchangeTiming::new(3, ps(200), ps(220)).unwrap()),
            40,
        ),
        Err(FtmRequesterError::DialogTokenOutOfSequence {
            expected: 2,
            actual: 3,
        })
    );
    assert_eq!(requester.sample_count, 0);
    assert_eq!(
        requester.failure(),
        Some(FtmSessionFailure::ProtocolViolation)
    );
}

#[test]
fn initial_retransmission_must_preserve_the_negotiated_body() {
    let peer = [8; 6];
    let mut requester = FtmRequester::<2>::new(config(3, 1));
    start_published(&mut requester, peer);
    let initial = initial_body(1, 3);
    requester
        .on_measurement(
            peer,
            FtmMeasurement::decode_body(&initial).unwrap(),
            Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
            30,
        )
        .unwrap();

    let changed = initial_body(2, 2);
    assert_eq!(
        requester.on_measurement(
            peer,
            FtmMeasurement::decode_body(&changed).unwrap(),
            Some(FtmLocalExchangeTiming::new(2, ps(200), ps(220)).unwrap()),
            40,
        ),
        Err(FtmRequesterError::InitialRetransmissionMismatch)
    );
    assert_eq!(requester.sample_count, 0);
    assert_eq!(
        requester.failure(),
        Some(FtmSessionFailure::ProtocolViolation)
    );
}

#[test]
fn timestamp_wrap_is_bounded_to_half_range() {
    let exchange = FtmRawExchange {
        dialog_token: 1,
        responder_departure_t1_ps: ps(FTM_TIMESTAMP_MASK - 9),
        responder_ack_arrival_t4_ps: ps(20),
        initiator_arrival_t2_ps: ps(100),
        initiator_ack_departure_t3_ps: ps(110),
        tod_error: FtmTodError {
            max_error_exponent: 1,
            not_continuous: false,
        },
        toa_error: FtmToaError {
            max_error_exponent: 1,
        },
    };
    assert_eq!(
        exchange.raw_interval_difference(),
        Ok(FtmRawIntervalDifferencePs {
            responder_round_trip_ps: 30,
            initiator_turnaround_ps: 10,
            difference_ps: 20,
        })
    );
    let ambiguous = FtmRawExchange {
        responder_ack_arrival_t4_ps: ps(FTM_TIMESTAMP_HALF_RANGE),
        responder_departure_t1_ps: ps(0),
        ..exchange
    };
    assert_eq!(
        ambiguous.raw_interval_difference(),
        Err(FtmTimestampArithmeticError::AmbiguousResponderWrap)
    );
}
