use super::*;
use crate::evidence::run::{Comparison, MeasurementUnit};
use open_esp_radio_hil_protocol::{EvidenceRecord, FlowTransportEvidence, TransportEvidence};

#[test]
fn memory_counter_scopes_preserve_full_values_without_cpu_percentages() {
    use open_esp_radio_hil_protocol::{
        MemoryBenchmarkEvidence, MemoryBenchmarkMode, MemoryBenchmarkRequest,
        MemoryBenchmarkSource, MemoryBenchmarkStop,
    };
    let cycles = u64::from(u32::MAX) + 100;
    let events = [Envelope::new(
        7,
        3,
        0,
        9,
        Event::MemoryBenchmarkCompleted(MemoryBenchmarkEvidence {
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
            Envelope::new(
                7,
                10,
                3,
                1,
                Event::Evidence(EvidenceRecord::Transport(transport(125, 1_000))),
            ),
            Envelope::new(
                7,
                11,
                3,
                2,
                Event::Evidence(EvidenceRecord::Transport(transport(999, 1_000))),
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
                Envelope::new(
                    7,
                    1,
                    1,
                    1,
                    Event::Evidence(EvidenceRecord::FlowTransport(
                        FlowTransportEvidence::from_session_total(0, transport(value, 100)),
                    )),
                ),
                Envelope::new(
                    7,
                    2,
                    1,
                    1,
                    Event::Evidence(EvidenceRecord::FlowTransport(
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
        &[Envelope::new(
            7,
            1,
            1,
            1,
            Event::Evidence(EvidenceRecord::Transport(transport(100, 0))),
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
