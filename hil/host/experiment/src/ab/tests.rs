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

/// A completed run `id` below `runs` of `udp-rx` that measured
/// `throughput` once (gated, higher is better) and an ungated counter.
fn run(runs: &Path, id: &str, throughput: u64) -> Run {
    use oer_hil_image_class::ImageClass;
    use oer_hil_run_bundle::run::{
        Comparison, Measurement, MeasurementUnit, RunState, ScenarioResult,
        test_support::{repetition, write_run},
    };
    write_run(
        &runs.join(id),
        0,
        RunState::Completed,
        vec![ScenarioResult::from_repetitions(
            "udp-rx".into(),
            ImageClass::DiagnosticTaskResidence,
            1,
            vec![repetition(
                "udp-rx",
                1,
                Outcome::Passed,
                None,
                vec![
                    Measurement::observed("rx.bps", throughput, MeasurementUnit::BitsPerSecond)
                        .evaluated(Comparison::AtLeast, 1),
                    Measurement::observed("rx.zero-copy", 5, MeasurementUnit::Count),
                ],
            )],
        )],
        |_| {},
    );
    Run::load(&runs.join(id)).unwrap()
}

#[test]
fn the_summary_names_the_variants_and_each_verdict() {
    let directory = tempfile::tempdir().unwrap();
    let runs = [
        (Arm::A, "a1", 100),
        (Arm::A, "a2", 101),
        (Arm::A, "a3", 99),
        (Arm::B, "b1", 120),
        (Arm::B, "b2", 121),
        (Arm::B, "b3", 119),
    ]
    .map(|(arm, id, throughput)| (arm, run(directory.path(), id, throughput)));
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
        comparisons: arms::compare(
            &runs
                .iter()
                .map(|(arm, run)| (*arm, run))
                .collect::<Vec<_>>(),
        ),
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
