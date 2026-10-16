//! The conformance harness template (#761, owner decision Q3) on a toy
//! port: a suite is `exercise(case) -> Trace` plus a pure
//! `verify(case, trace)`, and one scenario runs it with the host model's
//! harness and with the stand's proxy, which only returns the traces the
//! target reports. The verdict is recomputed from the stored trace.

use core::num::NonZeroU32;

use oer_hil_executor::{Infrastructure, Recorder, Run, execute};
use oer_hil_model::{
    Agreement, BoundName, Estimate, Factor, Finding, Instance, InstanceName, MissingBound, Purpose,
    ReportedBounds, Resources, ScenarioId, Spec, StandProfile, Subject, TargetCapabilities,
    Verdict, Wait,
};
use serde::{Deserialize, Serialize};

// ---- What a port's conformance crate provides (for Wi-Fi,
// `…-lower-mac-conformance` with its `wire` feature). ----

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Case {
    OneFrame,
    CloseGate,
}

/// A trace: identities read from the real returned owners, in order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Trace {
    submitted: Vec<u8>,
    terminals: Vec<u8>,
    /// Terminal latency on the backend's clock, in microseconds.
    latency_micros: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Violation {
    Missing,
    Order,
    Bound,
}

trait Harness {
    fn exercise(&mut self, case: Case) -> impl Future<Output = Trace>;
}

/// The suite's pure verdict, shared by the model tests and the stand.
fn verify(case: Case, trace: &Trace, terminal_bound_micros: u64) -> Result<(), Violation> {
    let expected = match case {
        Case::OneFrame => 1,
        Case::CloseGate => 2,
    };
    if trace.terminals.len() != expected || trace.submitted.len() != expected {
        return Err(Violation::Missing);
    }
    if trace.terminals != trace.submitted {
        return Err(Violation::Order);
    }
    if trace.latency_micros > terminal_bound_micros {
        return Err(Violation::Bound);
    }
    Ok(())
}

// ---- The scenario, in a block's scenario crate. ----

struct TxPort;

impl Subject for TxPort {
    const CONTRACT: &'static str = "toy-port::TxPort";
}

struct TxConformance;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Params {
    cases: Vec<Case>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Criteria {
    /// The backend's terminal bound, as the image reports it.
    terminal_bound_micros: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct Traces(Vec<(Case, Trace)>);

impl Spec for TxConformance {
    const ID: ScenarioId = ScenarioId::new("toy-tx-conformance");
    const PURPOSE: Purpose = Purpose::Conformance;
    type Subject = TxPort;
    type Params = Params;
    type Criteria = Criteria;
    type Trace = Traces;

    fn resources(_: &Params) -> Resources {
        Resources::default()
    }

    fn target(_: &Params) -> TargetCapabilities {
        TargetCapabilities::of(
            Some("oer-esp32s31-ieee80211-system"),
            &["conformance/toy-port"],
        )
    }

    fn estimate(
        params: &Params,
        bounds: &ReportedBounds,
        profile: &StandProfile,
    ) -> Result<Estimate, MissingBound> {
        let terminal = Wait::Contract {
            bound: BoundName::new("toy-port.terminal"),
            factor: Factor::new(2, NonZeroU32::MIN).unwrap(),
        };
        Estimate::from_waits(&[], &[terminal], params.cases.len() as u64, bounds, profile)
    }

    fn verdict(traces: &Traces, criteria: &Criteria) -> Verdict {
        Verdict::from_findings(
            traces
                .0
                .iter()
                .filter_map(|(case, trace)| {
                    verify(*case, trace, criteria.terminal_bound_micros)
                        .err()
                        .map(|violation| {
                            Finding::new(format!("{case:?}"), format!("{violation:?}"))
                        })
                })
                .collect(),
        )
    }
}

impl<H: Harness> Run<H> for TxConformance {
    async fn run(
        params: &Params,
        harness: &mut H,
        _: &mut Recorder,
    ) -> Result<Traces, Infrastructure> {
        let mut traces = Vec::new();
        for &case in &params.cases {
            traces.push((case, harness.exercise(case).await));
        }
        Ok(Traces(traces))
    }
}

// ---- Two executors. ----

/// The host model.
struct Model {
    slow: bool,
}

impl Harness for Model {
    async fn exercise(&mut self, case: Case) -> Trace {
        let frames: Vec<u8> = match case {
            Case::OneFrame => vec![0],
            Case::CloseGate => vec![0, 1],
        };
        Trace {
            submitted: frames.clone(),
            terminals: frames,
            latency_micros: if self.slow { 9_000 } else { 400 },
        }
    }
}

/// The stand's proxy: it asks the target to exercise a case and decodes the
/// trace the target reports; here the reports are prepared wire bytes.
struct StandProxy {
    reports: Vec<Vec<u8>>,
}

impl Harness for StandProxy {
    async fn exercise(&mut self, _: Case) -> Trace {
        serde_json::from_slice(&self.reports.remove(0)).unwrap()
    }
}

fn instance() -> Instance<TxConformance> {
    Instance::new(
        InstanceName::new("one-frame-and-gate"),
        Params {
            cases: vec![Case::OneFrame, Case::CloseGate],
        },
    )
}

const CRITERIA: Criteria = Criteria {
    terminal_bound_micros: 1_000,
};

#[test]
fn the_model_and_the_stand_run_one_scenario_and_one_verdict() {
    let on_model =
        embassy_futures::block_on(execute(&instance(), &CRITERIA, &mut Model { slow: false }))
            .unwrap();
    assert_eq!(on_model.verdict, Verdict::Pass);

    let reports = on_model
        .trace
        .0
        .iter()
        .map(|(_, trace)| serde_json::to_vec(trace).unwrap())
        .collect();
    let on_stand =
        embassy_futures::block_on(execute(&instance(), &CRITERIA, &mut StandProxy { reports }))
            .unwrap();
    assert_eq!(on_stand.verdict, Verdict::Pass);
    assert_eq!(on_stand.identity, on_model.identity);
}

#[test]
fn a_stored_trace_is_judged_again_without_a_run() {
    let outcome =
        embassy_futures::block_on(execute(&instance(), &CRITERIA, &mut Model { slow: true }))
            .unwrap();
    assert!(!outcome.verdict.passed());

    let stored = serde_json::to_vec(&outcome.trace).unwrap();
    let relaxed = Criteria {
        terminal_bound_micros: 10_000,
    };
    let recomputed = TxConformance::verdict(&serde_json::from_slice(&stored).unwrap(), &relaxed);
    assert_eq!(recomputed, Verdict::Pass);
    assert!(matches!(
        Agreement::of(&outcome.verdict, recomputed),
        Agreement::Differs { .. }
    ));
}

#[test]
fn the_estimate_comes_from_the_bound_the_image_reports() {
    let params = instance().params;
    let bounds = ReportedBounds::default().with("toy-port.terminal", 1_000);
    let estimate = TxConformance::estimate(&params, &bounds, &StandProfile::default()).unwrap();
    assert_eq!(estimate.total_micros(), 2 * 2_000);
    assert!(
        TxConformance::estimate(
            &params,
            &ReportedBounds::default(),
            &StandProfile::default()
        )
        .is_err()
    );
}
