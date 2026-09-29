use oer_hil_schema::run::{Comparison, MeasurementUnit, Outcome, Threshold};

use super::*;

#[test]
fn a_variant_is_a_revision_and_its_dependency_overrides() {
    assert_eq!(
        "rev=main;override:xarxa=/src/xarxa".parse::<VariantSpec>(),
        Ok(VariantSpec {
            revision: String::from("main"),
            overrides: vec![(Dependency::Xarxa, PathBuf::from("/src/xarxa"))],
            features: FeatureDelta::default(),
        })
    );
    assert_eq!(
        "features=+trace,-psram-stack"
            .parse::<VariantSpec>()
            .unwrap()
            .features
            .to_string(),
        "+trace,-psram-stack"
    );
    assert_eq!(
        "override:esp-hal=/h"
            .parse::<VariantSpec>()
            .unwrap()
            .revision,
        "HEAD"
    );
    for invalid in [
        "rev=a;rev=b",
        "override:xarxa=/a;override:xarxa=/b",
        "override:tokio=/t",
        "override:xarxa",
        "features=x",
        "features=+a;features=+b",
        "colour=red",
    ] {
        assert!(invalid.parse::<VariantSpec>().is_err(), "{invalid}");
    }
}

fn run(id: &str, throughput: &[f64], counter: f64) -> hil_runs::Run {
    let measurement = |name: &str, value, threshold| hil_runs::Measurement {
        name: name.into(),
        value: Some(value),
        unit: MeasurementUnit::BitsPerSecond,
        threshold,
        verdict: None,
    };
    hil_runs::Run {
        id: id.into(),
        directory: PathBuf::new(),
        started_millis: 0,
        state: hil_runs::State::Completed,
        outcome: Some(Outcome::Passed),
        commit: None,
        dirty: false,
        checkout: None,
        images: Vec::new(),
        replayed: Vec::new(),
        scenarios: vec![hil_runs::ScenarioRun {
            id: String::from("udp-rx"),
            image: String::from("diagnostic-task-residence"),
            outcome: Outcome::Passed,
            failure: None,
            repetitions: throughput
                .iter()
                .enumerate()
                .map(|(index, value)| hil_runs::Repetition {
                    number: index as u64 + 1,
                    outcome: Outcome::Passed,
                    failure: None,
                    measurements: vec![
                        measurement(
                            "rx.bps",
                            *value,
                            Some(Threshold {
                                comparison: Comparison::AtLeast,
                                value: 1,
                            }),
                        ),
                        measurement("rx.zero-copy", counter, None),
                    ],
                    directory: None,
                })
                .collect(),
        }],
        observer: None,
    }
}

#[test]
fn each_run_contributes_its_mean_and_the_gate_sets_the_direction() {
    let runs = [
        (Arm::A, run("a1", &[100.0, 102.0], 5.0)),
        (Arm::B, run("b1", &[110.0, 112.0], 5.0)),
        (Arm::A, run("a2", &[101.0], 5.0)),
        (Arm::B, run("b2", &[111.0], 5.0)),
    ];
    let collected = samples(&runs);
    let (unit, better, a, b) = &collected[&(String::from("udp-rx"), String::from("rx.bps"))];
    assert_eq!(unit, &MeasurementUnit::BitsPerSecond.to_string());
    assert_eq!(*better, Some(Better::Higher));
    assert_eq!(a, &[101.0, 101.0]);
    assert_eq!(b, &[111.0, 111.0]);
    let comparisons = compare(&runs);
    let rx = comparisons
        .iter()
        .find(|entry| entry.measurement == "rx.bps")
        .unwrap();
    assert_eq!(rx.comparison.difference, 10.0);
    // Two runs per arm are too few to judge.
    assert_eq!(
        rx.comparison.verdict,
        hil_perf::AbVerdict::InsufficientRepetitions
    );
    let counter = comparisons
        .iter()
        .find(|entry| entry.measurement == "rx.zero-copy")
        .unwrap();
    assert_eq!(counter.better, None);
}

#[test]
fn the_summary_names_the_variants_and_each_verdict() {
    let runs = [
        (Arm::A, run("a1", &[100.0], 5.0)),
        (Arm::A, run("a2", &[101.0], 5.0)),
        (Arm::A, run("a3", &[99.0], 5.0)),
        (Arm::B, run("b1", &[120.0], 5.0)),
        (Arm::B, run("b2", &[121.0], 5.0)),
        (Arm::B, run("b3", &[119.0], 5.0)),
    ];
    let variant = |commit: &str| Variant {
        commit: commit.into(),
        overrides: Vec::new(),
        features: FeatureDelta::default(),
    };
    let report = Report {
        schema: REPORT_SCHEMA,
        id: String::from("7"),
        a: variant("aaaaaaaaaaaaaaaa"),
        b: Variant {
            commit: String::from("aaaaaaaaaaaaaaaa"),
            overrides: vec![DependencyOverride {
                dependency: Dependency::Xarxa,
                path: PathBuf::from("/x"),
                commit: String::from("ea9385e"),
                dirty: true,
            }],
            features: FeatureDelta::default(),
        },
        scenarios: vec![String::from("udp-rx")],
        repetitions: 3,
        layout_seeds: 1,
        runs: Vec::new(),
        comparisons: compare(&runs),
    };
    let text = summary(&report);
    assert!(
        text.contains("A aaaaaaaaaaaa vs B aaaaaaaaaaaa +xarxa@ea9385e+"),
        "{text}"
    );
    assert!(
        text.contains("rx.bps") && text.contains("significant, B better"),
        "{text}"
    );
    assert!(
        text.contains("rx.zero-copy") && text.contains("(ungated)"),
        "{text}"
    );
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["b"]["overrides"][0]["dependency"], "xarxa");
}

#[test]
fn every_replay_round_leases_the_same_work_so_its_estimate_is_a_round() {
    let scenarios = [String::from("udp-rx"), String::from("udp-tx")];
    assert_eq!(round_work(&scenarios), round_work(&scenarios.clone()));
    assert_eq!(round_work(&scenarios), "ab round udp-rx udp-tx");
}
