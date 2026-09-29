use super::*;
use crate::session::Received;
use oer_hil_protocol::Envelope;

/// A device event as the host receives it.
fn event<M: oer_hil_protocol::Message>(
    boot_id: u64,
    message_sequence: u32,
    session_id: u64,
    request_id: u32,
    body: M,
) -> Received {
    crate::session::test_support::received(Envelope::new(
        boot_id,
        message_sequence,
        session_id,
        request_id,
        body,
    ))
}
use oer_hil_evidence::run::{Comparison, MeasurementUnit};
use oer_hil_protocol::{
    network::EvidenceRecord, network::FlowTransportEvidence, network::TransportEvidence,
};

#[test]
fn memory_counter_scopes_preserve_full_values_without_cpu_percentages() {
    use oer_hil_protocol::{
        system::MemoryBenchmarkEvidence, system::MemoryBenchmarkMode,
        system::MemoryBenchmarkRequest, system::MemoryBenchmarkSource, system::MemoryBenchmarkStop,
    };
    let cycles = u64::from(u32::MAX) + 100;
    let events = [event(
        7,
        3,
        0,
        9,
        oer_hil_protocol::system::MemoryBenchmarkCompleted(MemoryBenchmarkEvidence {
            request: MemoryBenchmarkRequest {
                mode: MemoryBenchmarkMode::GdmaAsync,
                source: MemoryBenchmarkSource::Psram,
                bytes: 1514,
                frames: 32,
                iterations: 32,
            },
            completed_iterations: 32,
            elapsed_micros: 500,
            elapsed_cycles: cycles,
            elapsed_instructions: 1000,
            foreground_cycles: 200,
            foreground_instructions: 100,
            polls: 64,
            stop: MemoryBenchmarkStop::Completed,
        }),
    )];
    let observations = protocol::observations("boot-001", &events, 100);
    let elapsed = observations
        .iter()
        .find(|value| value.name.ends_with("memory.elapsed.cycles"))
        .unwrap();
    assert_eq!(elapsed.value, cycles);
    assert_eq!(elapsed.unit, MeasurementUnit::Count);
    assert!(
        observations
            .iter()
            .any(|value| value.name.ends_with("memory.frames-per-iteration") && value.value == 32)
    );
    assert!(
        observations
            .iter()
            .any(|value| value.name.ends_with("memory.bytes-per-frame") && value.value == 1514)
    );
    assert!(observations.iter().any(
        |value| value.name.ends_with("memory.bytes-per-iteration") && value.value == 1514 * 32
    ));
    assert!(
        observations
            .iter()
            .any(|value| value.name.ends_with("memory.foreground.cycles") && value.value == 200)
    );
    assert!(
        observations
            .iter()
            .all(|value| value.unit != MeasurementUnit::BasisPoints)
    );
}

fn transport(bytes: u64, elapsed_micros: u64) -> TransportEvidence {
    TransportEvidence {
        rx_maximum_silence_micros: None,
        rx_bytes: bytes,
        tx_bytes: 0,
        rx_units: 2,
        tx_units: 0,
        elapsed_micros,
        transport_errors: 0,
    }
}

#[test]
fn replay_does_not_duplicate_or_replace_the_first_observation() {
    let recorder = Recorder::default();
    let capture = recorder.capture(Path::new("")).unwrap();
    capture.record(
        &[
            event(
                7,
                10,
                3,
                1,
                oer_hil_protocol::network::Evidence(EvidenceRecord::Transport(transport(
                    125, 1_000,
                ))),
            ),
            event(
                7,
                11,
                3,
                2,
                oer_hil_protocol::network::Evidence(EvidenceRecord::Transport(transport(
                    999, 1_000,
                ))),
            ),
        ],
        200,
    );
    let values = recorder.snapshot();
    let bytes: Vec<_> = values
        .iter()
        .filter(|v| v.name.ends_with("transport.rx.bytes"))
        .collect();
    assert_eq!(bytes.len(), 1);
    assert_eq!(bytes[0].value, 125);
    assert_eq!(
        values
            .iter()
            .find(|v| v.name.ends_with("transport.rx.rate"))
            .unwrap()
            .value,
        1_000_000
    );
    assert!(
        values
            .iter()
            .all(|v| v.threshold.is_none() && v.verdict.is_none())
    );
}

#[test]
fn boot_scopes_and_concurrent_flows_keep_distinct_measurements() {
    let recorder = Recorder::default();
    for (boot, value) in [("boot-001", 11), ("boot-002", 22)] {
        recorder.capture(Path::new(boot)).unwrap().record(
            &[
                event(
                    7,
                    1,
                    1,
                    1,
                    oer_hil_protocol::network::Evidence(EvidenceRecord::FlowTransport(
                        FlowTransportEvidence::from_session_total(0, transport(value, 100)),
                    )),
                ),
                event(
                    7,
                    2,
                    1,
                    1,
                    oer_hil_protocol::network::Evidence(EvidenceRecord::FlowTransport(
                        FlowTransportEvidence::from_session_total(1, transport(value + 1, 100)),
                    )),
                ),
            ],
            10,
        );
    }
    let values: Vec<_> = recorder
        .snapshot()
        .into_iter()
        .filter(|v| v.name.ends_with("transport.rx.bytes"))
        .collect();
    assert_eq!(
        values.iter().map(|v| v.value).collect::<Vec<_>>(),
        [11, 12, 22, 23]
    );
    assert!(recorder.capture(Path::new("../other-run")).is_err());
    assert!(recorder.capture(Path::new("/other-run")).is_err());
}

#[test]
fn zero_elapsed_time_does_not_invent_a_rate_and_host_gates_remain_explicit() {
    let recorder = Recorder::default();
    recorder.capture(Path::new("")).unwrap().record(
        &[event(
            7,
            1,
            1,
            1,
            oer_hil_protocol::network::Evidence(EvidenceRecord::Transport(transport(100, 0))),
        )],
        10,
    );
    recorder.record([
        Measurement::observed("icmp.replies.lost", 3, MeasurementUnit::Count)
            .evaluated(Comparison::AtMost, 0),
    ]);
    assert!(
        !recorder
            .snapshot()
            .iter()
            .any(|v| v.name.ends_with(".rate"))
    );
    assert_eq!(
        recorder
            .snapshot()
            .iter()
            .filter(|v| v.threshold.is_some())
            .count(),
        1
    );
}
#[test]
fn semantic_checks_record_only_explicit_observations() {
    let recorder = super::Recorder::default();
    assert!(recorder.snapshot().is_empty());
    recorder.check("wifi.maintenance.transaction-valid", false);
    recorder.check("wifi.maintenance.transaction-valid", true);
    let values = recorder.snapshot();
    assert_eq!(values[0].value, 0);
    assert_eq!(
        values[0].verdict,
        Some(oer_hil_evidence::run::MeasurementVerdict::Failed)
    );
    assert_eq!(values[0].threshold.unwrap().value, 1);
    recorder.check("wifi.maintenance.same-link", true);
    assert_eq!(
        recorder
            .snapshot()
            .iter()
            .filter(|value| value.verdict == Some(oer_hil_evidence::run::MeasurementVerdict::Passed))
            .count(),
        1
    );
}

#[test]
fn aggregate_fill_projects_one_measurement_set_per_association() {
    use oer_hil_protocol::wifi::WifiApAggregateFill;
    let fill = |association_id, aggregates, subframes| {
        oer_hil_protocol::wifi::AccessPointAggregateFill(WifiApAggregateFill {
            generation: 4,
            association_id,
            aggregates,
            subframes,
            maximum_subframes: 32,
            histogram: [1, 2, 3, 4, aggregates - 10],
        })
    };
    let events = [
        event(7, 3, 0, 9, fill(1, 100, 2_900)),
        event(7, 3, 0, 10, fill(2, 60, 1_700)),
    ];
    let observations = protocol::observations("boot-001", &events, 100);
    let value = |name: &str| {
        observations
            .iter()
            .find(|observation| observation.name.ends_with(name))
            .map(|observation| observation.value)
    };
    assert_eq!(value("ap.aggregate-fill.aid-1.aggregates"), Some(100));
    assert_eq!(value("ap.aggregate-fill.aid-1.subframes"), Some(2_900));
    assert_eq!(value("ap.aggregate-fill.aid-2.subframes"), Some(1_700));
    assert_eq!(value("ap.aggregate-fill.aid-2.maximum-subframes"), Some(32));
    assert_eq!(value("ap.aggregate-fill.aid-2.histogram-32"), Some(50));
    assert_eq!(value("ap.aggregate-fill.aid-1.histogram-2-7"), Some(2));
}
