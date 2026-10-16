//! The conformance harness template (#761 Q3, architecture section 3) on
//! a toy port.
//!
//! A port's conformance crate (for Wi-Fi `…-lower-mac-conformance`, with
//! its `wire` feature) holds:
//! - the trace types, each trace starting with a header that carries the
//!   bounds the image reports and the clock domain of its times;
//! - `Harness::exercise(case) -> Result<Trace, Infrastructure>`;
//! - a public pure `verify(case, &trace)` that reads the bounds from the
//!   header, never from criteria or host constants.
//!
//! One scenario runs it with the host model's harness and with the stand's
//! proxy, which decodes the postcard reports the target sends.

use std::collections::{BTreeSet, VecDeque};

use oer_hil_executor::{
    Cause, Executed, Executor, Infrastructure, Model, Recorder, Run, Stand, execute,
};
use oer_hil_model::{
    BoundName, Bounds, Composition, Conformance, Estimate, Factor, Finding, Instance,
    ReportedBounds, Resources, ScenarioId, Spec, StandProfile, Subject, TargetCapabilities,
    Verdict, Wait, WaitError, name,
};
use serde::{Deserialize, Serialize};

// ---- What a port's conformance crate provides. ----

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Case {
    OneFrame,
    CloseGate,
}

/// Which clock a trace's times are on: the backend's monotonic clock of
/// one boot (#563 decision 6), so the host can map them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Clock {
    source: String,
    generation: u32,
}

/// What every trace starts with.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Header {
    /// The backend's actual terminal bound, as the image reports it.
    terminal_bound_micros: u64,
    clock: Clock,
}

/// Identities read from the real returned owners, in order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Trace {
    header: Header,
    submitted: Vec<u8>,
    terminals: Vec<u8>,
    /// Submission to terminal on the header's clock.
    latency_micros: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Violation {
    Missing,
    Order,
    Bound,
}

trait Harness {
    async fn exercise(&mut self, case: Case) -> Result<Trace, Infrastructure>;
}

/// The suite's pure verdict, shared by the model tests and the stand.
fn verify(case: Case, trace: &Trace) -> Result<(), Violation> {
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
    if trace.latency_micros > trace.header.terminal_bound_micros {
        return Err(Violation::Bound);
    }
    Ok(())
}

// ---- The scenario, in a block's scenario crate. ----

struct TxPort;

const TERMINAL: BoundName = name!(BoundName, "toy-port.terminal");

impl Subject for TxPort {
    const CONTRACT: &'static str = "toy-port::TxPort";

    fn declared_bounds() -> ReportedBounds {
        ReportedBounds::default().with(TERMINAL, 1_000)
    }
}

struct TxConformance;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Params {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Traces(Vec<(Case, Trace)>);

impl Spec for TxConformance {
    const ID: ScenarioId = name!(ScenarioId, "toy-tx-conformance");
    type Purpose = Conformance;
    type Subject = TxPort;
    type Params = Params;
    /// Conformance has no thresholds of its own: the suite and the bounds
    /// in the trace decide.
    type Criteria = ();
    type Trace = Traces;

    fn resources(_: &Params) -> Resources {
        Resources::default()
    }

    fn target(_: &Params) -> TargetCapabilities {
        TargetCapabilities {
            composition: Composition::Product(name!(
                CompositionName,
                "oer-esp32s31-ieee80211-system"
            )),
            capabilities: BTreeSet::from([name!(CapabilityKey, "conformance.toy-port")]),
        }
    }

    fn estimate(
        params: &Params,
        bounds: &Bounds,
        profile: &StandProfile,
    ) -> Result<Estimate, WaitError> {
        let factor = Factor::new(2, core::num::NonZeroU32::MIN).map_err(|_| WaitError::Overflow)?;
        Estimate::from_waits(
            &[],
            &[Wait::contract(TERMINAL, factor)],
            params.cases.len() as u64,
            bounds,
            profile,
        )
    }

    fn verdict(traces: &Traces, (): &()) -> Verdict {
        Verdict::from_findings(
            traces
                .0
                .iter()
                .filter_map(|(case, trace)| {
                    verify(*case, trace).err().map(|violation| {
                        Finding::new(format!("{case:?}"), format!("{violation:?}"))
                    })
                })
                .collect(),
        )
    }
}

impl<X> Run<X> for TxConformance
where
    X: Executor,
    X::Driver: Harness,
{
    async fn run(
        params: &Params,
        executor: &mut X,
        _: &mut Recorder,
    ) -> Result<Traces, Infrastructure> {
        let mut traces = Vec::new();
        for &case in &params.cases {
            traces.push((case, executor.driver().exercise(case).await?));
        }
        Ok(Traces(traces))
    }
}

// ---- Two drivers. ----

fn header() -> Header {
    Header {
        terminal_bound_micros: 1_000,
        clock: Clock {
            source: "model".into(),
            generation: 0,
        },
    }
}

/// The host model.
struct ModelPort;

impl Harness for ModelPort {
    async fn exercise(&mut self, case: Case) -> Result<Trace, Infrastructure> {
        let frames: Vec<u8> = match case {
            Case::OneFrame => vec![0],
            Case::CloseGate => vec![0, 1],
        };
        Ok(Trace {
            header: header(),
            submitted: frames.clone(),
            terminals: frames,
            latency_micros: 400,
        })
    }
}

/// The stand's proxy: it asks the target to exercise a case and decodes
/// the postcard report the target sends; here the reports are prepared.
struct StandProxy {
    reports: VecDeque<Vec<u8>>,
}

impl Harness for StandProxy {
    async fn exercise(&mut self, case: Case) -> Result<Trace, Infrastructure> {
        let operation = name!(Operation, "target.report");
        let report = self.reports.pop_front().ok_or_else(|| Infrastructure {
            operation: operation.clone(),
            cause: format!("no report for {case:?}"),
        })?;
        postcard::from_bytes(&report).map_err(|error| Infrastructure {
            operation,
            cause: error.to_string(),
        })
    }
}

/// A trace as an S31 target would report it, written independently of
/// the model: its own clock, its own bound, its own latencies.
fn target_trace(terminals: Vec<u8>, latency_micros: u64) -> Vec<u8> {
    let submitted: Vec<u8> = (0..terminals.len() as u8).collect();
    let trace = Trace {
        header: Header {
            terminal_bound_micros: 3_000,
            clock: Clock {
                source: "esp32s31-systimer".into(),
                generation: 7,
            },
        },
        submitted,
        terminals,
        latency_micros,
    };
    postcard::to_allocvec(&trace).unwrap()
}

fn instance() -> Instance<TxConformance> {
    Instance::new(
        name!(InstanceName, "one-frame-and-gate"),
        Params {
            cases: vec![Case::OneFrame, Case::CloseGate],
        },
    )
}

#[test]
fn the_model_passes_its_own_suite() {
    let outcome =
        embassy_futures::block_on(execute(&instance(), &(), &mut Model(ModelPort))).unwrap();
    assert_eq!(outcome.executed, Executed::Model);
    assert_eq!(outcome.verdict, Verdict::Pass);
}

#[test]
fn the_stand_judges_the_target_traces_with_the_bounds_they_carry() {
    // 2_500 µs passes the target's own 3_000 µs bound, which the model's
    // 1_000 µs would refuse.
    let reports = VecDeque::from([
        target_trace(vec![0], 2_500),
        target_trace(vec![0, 1], 2_900),
    ]);
    let on_stand = embassy_futures::block_on(execute(
        &instance(),
        &(),
        &mut Stand(StandProxy { reports }),
    ))
    .unwrap();
    assert_eq!(on_stand.executed, Executed::Stand);
    assert_eq!(on_stand.verdict, Verdict::Pass);

    let on_model =
        embassy_futures::block_on(execute(&instance(), &(), &mut Model(ModelPort))).unwrap();
    assert_eq!(on_stand.identity, on_model.identity);
}

#[test]
fn a_target_trace_out_of_order_fails_with_its_finding() {
    let reports = VecDeque::from([target_trace(vec![0], 100), target_trace(vec![1, 0], 100)]);
    let outcome = embassy_futures::block_on(execute(
        &instance(),
        &(),
        &mut Stand(StandProxy { reports }),
    ))
    .unwrap();
    assert_eq!(
        outcome.verdict,
        Verdict::from_findings(vec![Finding::new("CloseGate", "Order")])
    );
}

#[test]
fn a_missing_report_is_infrastructure_not_a_verdict() {
    let reports = VecDeque::from([target_trace(vec![0], 100)]);
    let failed = embassy_futures::block_on(execute(
        &instance(),
        &(),
        &mut Stand(StandProxy { reports }),
    ))
    .unwrap_err();
    assert!(matches!(
        failed.cause,
        Cause::Infrastructure(Infrastructure { .. })
    ));
}

#[test]
fn a_stored_trace_is_judged_again_without_a_run() {
    let reports = VecDeque::from([target_trace(vec![0], 3_500), target_trace(vec![0, 1], 100)]);
    let outcome = embassy_futures::block_on(execute(
        &instance(),
        &(),
        &mut Stand(StandProxy { reports }),
    ))
    .unwrap();
    let stored = postcard::to_allocvec(&outcome.trace).unwrap();
    let recomputed = TxConformance::verdict(&postcard::from_bytes(&stored).unwrap(), &());
    assert_eq!(recomputed, outcome.verdict);
    assert_eq!(
        recomputed,
        Verdict::from_findings(vec![Finding::new("OneFrame", "Bound")])
    );
}
