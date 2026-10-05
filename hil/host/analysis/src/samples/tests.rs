use oer_hil_image_class::ImageClass;
use oer_hil_run_bundle::run::{
    Measurement, MeasurementUnit, Outcome, RunState, ScenarioResult, Threshold,
    test_support::{repetition, write_run},
};

use super::*;

pub(crate) fn rate(value: u64) -> Measurement {
    Measurement::observed("udp.tx.host-rate", value, MeasurementUnit::BitsPerSecond)
        .evaluated(Comparison::AtLeast, 1_000_000)
}

/// A suite of one `udp-tx` scenario whose repetitions measured `values` and
/// an ungated count.
pub(crate) fn suite(values: &[u64]) -> SuiteResult {
    let directory = tempfile::tempdir().unwrap();
    let run = directory.path().join("1-a");
    let repetitions = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            repetition(
                "udp-tx",
                index as u8 + 1,
                Outcome::Passed,
                None,
                vec![
                    rate(*value),
                    Measurement::observed("ungated", 1, MeasurementUnit::Count),
                ],
            )
        })
        .collect();
    write_run(
        &run,
        1,
        RunState::Completed,
        vec![ScenarioResult::from_repetitions(
            "udp-tx".into(),
            ImageClass::Performance,
            values.len() as u8,
            repetitions,
        )],
        |_| {},
    );
    oer_hil_run_bundle::RunBundle::open(&run)
        .unwrap()
        .unwrap()
        .suite()
        .unwrap()
        .unwrap()
}

#[test]
fn every_measurement_is_one_sample_per_scenario_and_name_with_its_gate() {
    let samples = samples(&suite(&[115_000_000, 116_000_000]));
    assert_eq!(samples.len(), 2);
    let rate = samples
        .iter()
        .find(|sample| sample.measurement == "udp.tx.host-rate")
        .unwrap();
    assert_eq!(rate.values, [115e6, 116e6]);
    assert_eq!(rate.better(), Some(Better::Higher));
    assert_eq!(rate.gate.unwrap().threshold, 1e6);
    let ungated = samples
        .iter()
        .find(|sample| sample.measurement == "ungated")
        .unwrap();
    assert_eq!(ungated.gate, None);
    assert_eq!(ungated.values, [1.0, 1.0]);
    let gate = |comparison| Threshold {
        comparison,
        value: 3,
    };
    assert_eq!(
        Gate::of(&gate(Comparison::AtMost)).unwrap().better,
        Better::Lower
    );
    assert_eq!(Gate::of(&gate(Comparison::Exactly)), None);
}

#[test]
fn a_comparison_needs_repetitions_and_a_difference_beyond_its_interval() {
    let verdict = |better, a: &[f64], b: &[f64]| compare(better, a, b).unwrap().verdict;
    // Two values a side are too few to judge.
    assert_eq!(
        verdict(Some(Better::Higher), &[100.0, 101.0], &[120.0, 121.0]),
        Verdict::InsufficientRepetitions
    );
    // A clear gain in the better direction, and the same data read with
    // the opposite direction.
    let a = [100.0, 101.0, 99.0, 100.5];
    let b = [110.0, 111.0, 109.5, 110.5];
    assert_eq!(
        verdict(Some(Better::Higher), &a, &b),
        Verdict::Significant { better: Arm::B }
    );
    assert_eq!(
        verdict(Some(Better::Lower), &a, &b),
        Verdict::Significant { better: Arm::A }
    );
    // Overlapping noisy arms stay within noise.
    assert_eq!(
        verdict(
            Some(Better::Higher),
            &[100.0, 110.0, 90.0],
            &[103.0, 95.0, 112.0]
        ),
        Verdict::WithinNoise
    );
    // Perfectly repeatable but below the practical tolerance.
    assert_eq!(
        verdict(Some(Better::Higher), &[100.0; 3], &[101.0; 3]),
        Verdict::WithinNoise
    );
    // Without a gate the same gain is a change, not an improvement.
    assert_eq!(verdict(None, &a, &b), Verdict::Changed { higher: Arm::B });
    let comparison = compare(Some(Better::Higher), &a, &b).unwrap();
    assert!((comparison.difference - 10.125).abs() < 1e-9);
    assert!(comparison.interval > 0.0 && comparison.interval < comparison.difference);
    assert!(compare(Some(Better::Higher), &[], &b).is_none());
}

#[test]
fn a_change_is_judged_against_the_baseline_noise_in_the_better_direction() {
    let baseline = Spread::of(&[115e6, 116e6, 117e6]).unwrap();
    let within = Spread::of(&[114e6]).unwrap();
    let lower = Spread::of(&[105e6]).unwrap();
    let higher = Spread::of(&[125e6]).unwrap();
    assert_eq!(change(&baseline, Better::Higher, &within), Change::Within);
    assert_eq!(change(&baseline, Better::Higher, &lower), Change::Regressed);
    assert_eq!(change(&baseline, Better::Higher, &higher), Change::Improved);
    assert_eq!(change(&baseline, Better::Lower, &higher), Change::Regressed);
    assert_eq!(change(&baseline, Better::Lower, &lower), Change::Improved);
}

#[test]
fn the_t_table_is_conservative_and_approaches_the_normal_value() {
    assert_eq!(t_critical_95(0.4), 12.706);
    assert_eq!(t_critical_95(4.9), 2.776);
    assert_eq!(t_critical_95(45.0), 2.000);
    assert_eq!(t_critical_95(500.0), 1.960);
}
