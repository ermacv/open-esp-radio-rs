use std::path::Path;

use oer_hil_image_class::ImageClass;
use oer_hil_run_bundle::run::{
    Comparison, Measurement, MeasurementUnit, Outcome, RunState, ScenarioResult,
    test_support::{repetition, write_run},
};

use super::*;
use crate::samples::Verdict;

/// A completed run `id` below `runs` of `udp-rx` whose repetitions measured
/// `throughput` (gated, higher is better) and an ungated `counter`.
pub(crate) fn run(runs: &Path, id: &str, throughput: &[u64], counter: u64) -> Run {
    let repetitions = throughput
        .iter()
        .enumerate()
        .map(|(index, value)| {
            repetition(
                "udp-rx",
                index as u8 + 1,
                Outcome::Passed,
                None,
                vec![
                    Measurement::observed("rx.bps", *value, MeasurementUnit::BitsPerSecond)
                        .evaluated(Comparison::AtLeast, 1),
                    Measurement::observed("rx.zero-copy", counter, MeasurementUnit::Count),
                ],
            )
        })
        .collect();
    write_run(
        &runs.join(id),
        0,
        RunState::Completed,
        vec![ScenarioResult::from_repetitions(
            "udp-rx".into(),
            ImageClass::DiagnosticTaskResidence,
            throughput.len() as u8,
            repetitions,
        )],
        |_| {},
    );
    Run::load(&runs.join(id)).unwrap()
}

#[test]
fn each_run_contributes_its_mean_and_the_gate_sets_the_direction() {
    let directory = tempfile::tempdir().unwrap();
    let runs = [
        (Arm::A, run(directory.path(), "a1", &[100, 102], 5)),
        (Arm::B, run(directory.path(), "b1", &[110, 112], 5)),
        (Arm::A, run(directory.path(), "a2", &[101], 5)),
        (Arm::B, run(directory.path(), "b2", &[111], 5)),
    ];
    let comparisons = compare(
        &runs
            .iter()
            .map(|(arm, run)| (*arm, run))
            .collect::<Vec<_>>(),
    );
    let rx = comparisons
        .iter()
        .find(|entry| entry.measurement == "rx.bps")
        .unwrap();
    assert_eq!(rx.unit, MeasurementUnit::BitsPerSecond.to_string());
    assert_eq!(rx.better, Some(Better::Higher));
    // One value per run: the mean of its repetitions.
    assert_eq!((rx.comparison.a.mean, rx.comparison.a.count), (101.0, 2));
    assert_eq!((rx.comparison.b.mean, rx.comparison.b.count), (111.0, 2));
    assert_eq!(rx.comparison.difference, 10.0);
    // Two runs per arm are too few to judge.
    assert_eq!(rx.comparison.verdict, Verdict::InsufficientRepetitions);
    let counter = comparisons
        .iter()
        .find(|entry| entry.measurement == "rx.zero-copy")
        .unwrap();
    assert_eq!(counter.better, None);
}
