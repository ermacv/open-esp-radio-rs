//! The conformance harness template (#761 Q3, architecture section 3) on
//! a toy port.
//!
//! A port's conformance crate (for Wi-Fi `…-lower-mac-conformance`, with
//! its `wire` feature) holds:
//! - the trace types, each trace starting with the model's
//!   [`TraceHeader`]: the bounds the image reports and the clock domain of
//!   its times;
//! - `Harness::exercise(case) -> Result<Trace, Infrastructure>`;
//! - a public pure `verify(case, &trace)` that reads the bounds from the
//!   header, never from criteria or host constants.
//!
//! The scenario's verdict also checks that every expected case has a
//! trace, that all of them carry one header, and that the header's bounds
//! keep the contract's ceilings. One scenario runs with the host model's
//! harness and with the stand's proxy, which decodes the postcard reports
//! the target sends.

use core::num::NonZeroU32;
use std::collections::{BTreeSet, VecDeque};

use oer_hil_executor::{
    Cause, Executed, Executor, Infrastructure, Model, ModelDriver, Recorder, Run, Stand,
    StandDriver, StandSession, execute,
};
use oer_hil_model::{
    BoundName, ClockDomain, Composition, Conformance, ContractName, Estimate, Factor,
    HeaderFinding, Instance, ReportedBounds, Resources, ScenarioId, Spec, StandProfile, Subject,
    TargetCapabilities, TraceHeader, Verdict, Wait, WaitError, name,
};
use serde::{Deserialize, Serialize};

// ---- The contract, with its ceiling. ----

const TERMINAL: BoundName = name!(BoundName, "toy-port.terminal");
/// No backend may take longer than this from submission to terminal.
const TERMINAL_CEILING_MICROS: u64 = 1_000;

struct TxPort;

impl Subject for TxPort {
    const CONTRACT: ContractName = name!(ContractName, "toy-port::TxPort");

    fn ceilings() -> ReportedBounds {
        ReportedBounds::default().with(TERMINAL, TERMINAL_CEILING_MICROS)
    }
}

// ---- A backend's own bound, checked against the ceiling at compile time. ----

const S31_TERMINAL_MICROS: u64 = 800;
const _: () = assert!(
    S31_TERMINAL_MICROS <= TERMINAL_CEILING_MICROS,
    "the backend exceeds the contract's ceiling"
);

// ---- What a port's conformance crate provides. ----

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
enum Case {
    OneFrame,
    CloseGate,
}

/// Identities read from the real returned owners, in order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Trace {
    header: TraceHeader,
    submitted: Vec<u8>,
    terminals: Vec<u8>,
    /// Submission to terminal on the header's clock.
    latency_micros: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Violation {
    Missing,
    Order,
    Bound,
    /// The header reports no terminal bound.
    NoBound,
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
    let bound = trace
        .header
        .bounds
        .get(&TERMINAL)
        .ok_or(Violation::NoBound)?;
    if trace.latency_micros > bound {
        return Err(Violation::Bound);
    }
    Ok(())
}

// ---- The scenario, in a block's scenario crate. ----

struct TxConformance;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Params {
    cases: Vec<Case>,
}

/// The run's traces, with the cases it was asked for.
#[derive(Debug, Deserialize, Serialize)]
struct Traces {
    expected: Vec<Case>,
    runs: Vec<(Case, Trace)>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Finding {
    Header(HeaderFinding),
    Case {
        case: Case,
        violation: Violation,
    },
    /// An expected case has no trace; a run with none fails here too.
    Unexercised(Case),
}

const TWICE: Factor = Factor::times(NonZeroU32::new(2).unwrap());

impl Spec for TxConformance {
    const ID: ScenarioId = name!(ScenarioId, "toy-tx-conformance");
    type Purpose = Conformance;
    type Subject = TxPort;
    type Params = Params;
    /// Conformance has no thresholds of its own: the suite, the bounds in
    /// the trace and the contract's ceilings decide.
    type Criteria = ();
    type Trace = Traces;
    type Finding = Finding;

    fn resources(_: &Params) -> Resources {
        Resources::default()
    }

    fn target(_: &Params) -> TargetCapabilities {
        TargetCapabilities {
            chip: name!(ChipId, "esp32s31"),
            composition: Composition::Product(name!(
                CompositionName,
                "oer-esp32s31-ieee80211-system"
            )),
            capabilities: BTreeSet::from([name!(CapabilityKey, "conformance.toy-port")]),
        }
    }

    fn estimate(
        params: &Params,
        ceilings: &ReportedBounds,
        profile: &StandProfile,
    ) -> Result<Estimate, WaitError> {
        let cases = u64::try_from(params.cases.len()).map_err(|_| WaitError::Overflow)?;
        Estimate::from_waits(
            &[],
            &[Wait::contract(TERMINAL, TWICE)],
            cases,
            ceilings,
            profile,
        )
    }

    fn verdict(traces: &Traces, (): &()) -> Verdict<Finding> {
        let ceilings = TxPort::ceilings();
        let mut findings: Vec<Finding> = TraceHeader::of_run(
            traces.runs.iter().map(|(_, trace)| &trace.header),
            &ceilings,
        )
        .into_iter()
        .filter(|finding| *finding != HeaderFinding::NoTrace)
        .map(Finding::Header)
        .collect();
        let exercised: BTreeSet<Case> = traces.runs.iter().map(|(case, _)| *case).collect();
        findings.extend(
            traces
                .expected
                .iter()
                .filter(|case| !exercised.contains(case))
                .map(|case| Finding::Unexercised(*case)),
        );
        findings.extend(traces.runs.iter().filter_map(|(case, trace)| {
            verify(*case, trace).err().map(|violation| Finding::Case {
                case: *case,
                violation,
            })
        }));
        Verdict::from_findings(findings)
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
        let mut runs = Vec::new();
        for &case in &params.cases {
            runs.push((case, executor.driver().exercise(case).await?));
        }
        Ok(Traces {
            expected: params.cases.clone(),
            runs,
        })
    }
}

// ---- Two drivers. ----

fn header(terminal_micros: u64, clock: &'static str, generation: u32) -> TraceHeader {
    TraceHeader {
        bounds: ReportedBounds::default().with(TERMINAL, terminal_micros),
        clock: ClockDomain {
            clock: oer_hil_model::ClockName::parse(clock).unwrap(),
            generation,
        },
    }
}

/// The host model.
struct ModelPort;

impl ModelDriver for ModelPort {}

impl Harness for ModelPort {
    async fn exercise(&mut self, case: Case) -> Result<Trace, Infrastructure> {
        let frames: Vec<u8> = match case {
            Case::OneFrame => vec![0],
            Case::CloseGate => vec![0, 1],
        };
        Ok(Trace {
            header: header(TERMINAL_CEILING_MICROS, "model", 0),
            submitted: frames.clone(),
            terminals: frames,
            latency_micros: 400,
        })
    }
}

/// The target sent no report for a case.
#[derive(Debug)]
struct NoReport(Case);

impl core::fmt::Display for NoReport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "no report for {:?}", self.0)
    }
}

impl std::error::Error for NoReport {}

/// The stand's proxy: under its lease's session it asks the target to
/// exercise a case and decodes the postcard report the target sends; here
/// the reports are prepared.
struct StandProxy {
    session: StandSession<'static>,
    reports: VecDeque<Vec<u8>>,
}

impl StandDriver<'static> for StandProxy {
    fn session(&self) -> &StandSession<'static> {
        &self.session
    }
}

impl Harness for StandProxy {
    async fn exercise(&mut self, case: Case) -> Result<Trace, Infrastructure> {
        let operation = name!(Operation, "target.report");
        let report = self.reports.pop_front().ok_or_else(|| Infrastructure {
            operation: operation.clone(),
            cause: Box::new(NoReport(case)),
        })?;
        postcard::from_bytes(&report).map_err(|error| Infrastructure {
            operation,
            cause: Box::new(error),
        })
    }
}

fn stand(reports: Vec<Vec<u8>>) -> Stand<'static, StandProxy> {
    Stand::new(StandProxy {
        session: StandSession::for_tests(),
        reports: reports.into(),
    })
}

/// A trace as an S31 target reports it, written independently of the
/// model: its own clock, its own bound, its own latencies.
fn target_trace(header: TraceHeader, terminals: Vec<u8>, latency_micros: u64) -> Vec<u8> {
    let submitted = if terminals.len() == 1 {
        vec![0]
    } else {
        vec![0, 1]
    };
    postcard::to_allocvec(&Trace {
        header,
        submitted,
        terminals,
        latency_micros,
    })
    .unwrap()
}

fn s31() -> TraceHeader {
    header(S31_TERMINAL_MICROS, "esp32s31.systimer", 7)
}

fn instance(cases: Vec<Case>) -> Instance<TxConformance> {
    Instance::new(name!(InstanceName, "one-frame-and-gate"), Params { cases })
}

fn both() -> Instance<TxConformance> {
    instance(vec![Case::OneFrame, Case::CloseGate])
}

#[test]
fn the_model_passes_its_own_suite() {
    let outcome =
        embassy_futures::block_on(execute(&both(), &(), &mut Model::new(ModelPort))).unwrap();
    assert_eq!(outcome.executed, Executed::Model);
    assert_eq!(outcome.verdict, Verdict::Pass);
}

#[test]
fn the_stand_judges_the_target_traces_with_the_bounds_they_carry() {
    // 700 µs passes the S31's own 800 µs bound.
    let reports = vec![
        target_trace(s31(), vec![0], 700),
        target_trace(s31(), vec![0, 1], 790),
    ];
    let on_stand = embassy_futures::block_on(execute(&both(), &(), &mut stand(reports))).unwrap();
    assert_eq!(on_stand.executed, Executed::Stand);
    assert_eq!(on_stand.verdict, Verdict::Pass);

    // 900 µs keeps the 1 000 µs ceiling but not the S31's own bound.
    let late = vec![
        target_trace(s31(), vec![0], 900),
        target_trace(s31(), vec![0, 1], 100),
    ];
    let outcome = embassy_futures::block_on(execute(&both(), &(), &mut stand(late))).unwrap();
    assert_eq!(
        outcome.verdict,
        Verdict::from_findings(vec![Finding::Case {
            case: Case::OneFrame,
            violation: Violation::Bound
        }])
    );
}

#[test]
fn a_bound_above_the_ceiling_is_a_finding() {
    let above = || header(1_500, "esp32s31.systimer", 7);
    let reports = vec![
        target_trace(above(), vec![0], 100),
        target_trace(above(), vec![0, 1], 100),
    ];
    let outcome = embassy_futures::block_on(execute(&both(), &(), &mut stand(reports))).unwrap();
    assert_eq!(
        outcome.verdict,
        Verdict::from_findings(vec![Finding::Header(HeaderFinding::AboveCeiling {
            bound: TERMINAL,
            reported: 1_500,
            ceiling: TERMINAL_CEILING_MICROS,
        })])
    );
}

#[test]
fn cases_of_one_run_carry_one_header() {
    let rebooted = header(S31_TERMINAL_MICROS, "esp32s31.systimer", 8);
    let reports = vec![
        target_trace(s31(), vec![0], 100),
        target_trace(rebooted, vec![0, 1], 100),
    ];
    let outcome = embassy_futures::block_on(execute(&both(), &(), &mut stand(reports))).unwrap();
    assert_eq!(
        outcome.verdict,
        Verdict::from_findings(vec![Finding::Header(HeaderFinding::Changed)])
    );
}

#[test]
fn a_target_trace_out_of_order_fails_with_its_finding() {
    let reports = vec![
        target_trace(s31(), vec![0], 100),
        target_trace(s31(), vec![1, 0], 100),
    ];
    let outcome = embassy_futures::block_on(execute(&both(), &(), &mut stand(reports))).unwrap();
    assert_eq!(
        outcome.verdict,
        Verdict::from_findings(vec![Finding::Case {
            case: Case::CloseGate,
            violation: Violation::Order
        }])
    );
}

#[test]
fn a_run_without_its_cases_does_not_pass() {
    let traces = Traces {
        expected: vec![Case::OneFrame, Case::CloseGate],
        runs: Vec::new(),
    };
    assert_eq!(
        TxConformance::verdict(&traces, &()),
        Verdict::from_findings(vec![
            Finding::Unexercised(Case::OneFrame),
            Finding::Unexercised(Case::CloseGate)
        ])
    );
}

#[test]
fn a_missing_report_is_infrastructure_not_a_verdict() {
    let failed = embassy_futures::block_on(execute(
        &both(),
        &(),
        &mut stand(vec![target_trace(s31(), vec![0], 100)]),
    ))
    .unwrap_err();
    let Cause::Infrastructure(infrastructure) = failed.cause else {
        panic!("infrastructure")
    };
    assert!(infrastructure.cause.is::<NoReport>());
}

#[test]
fn a_stored_trace_is_judged_again_without_a_run() {
    let reports = vec![
        target_trace(s31(), vec![0], 900),
        target_trace(s31(), vec![0, 1], 100),
    ];
    let outcome = embassy_futures::block_on(execute(&both(), &(), &mut stand(reports))).unwrap();
    let stored = postcard::to_allocvec(&outcome.trace).unwrap();
    let recomputed = TxConformance::verdict(&postcard::from_bytes(&stored).unwrap(), &());
    assert_eq!(recomputed, outcome.verdict);
}
