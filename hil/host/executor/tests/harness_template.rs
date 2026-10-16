//! The conformance harness template (#761 Q3, architecture section 3, and
//! owner decision 6223045367) on a toy port.
//!
//! A port's conformance crate (for Wi-Fi `…-lower-mac-conformance`) is
//! `no_std`, needs no HIL dependency, and holds:
//! - its bounds as a type, `struct Bounds` with named fields, and the
//!   contract's `const CEILINGS: Bounds`;
//! - its case and trace types, with a `wire` feature;
//! - `Harness::exercise(case)`, which returns the trace and the header of
//!   the backend's actual bounds;
//! - a public pure `verify(case, &trace, &bounds)`: the bounds come as a
//!   value, beside the trace.
//!
//! The HIL agent sends `(case, header, trace)` with the
//! `oer-hil-trace-header` header. The scenario's verdict checks the run's
//! headers (one bound set and clock, a new generation only at a restart),
//! the backend's bounds against the ceilings, the cases against the
//! expected ones as a multiset, and each case with `verify`.

use core::num::NonZeroU32;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use oer_hil_executor::{
    Cause, Executed, Executor, Infrastructure, Model, ModelDriver, Recorder, Run, Stand,
    StandDriver, StandSession, execute,
};
use oer_hil_model::{
    Composition, Conformance, ContractName, Estimate, Factor, Instance, Resources, ScenarioId,
    Spec, StandProfile, Subject, TargetCapabilities, Verdict, Wait, WaitError, name,
};
use oer_hil_trace_header::{Clock, ClockDomain, Restart, RunHeaderError, TraceHeader, check_run};
use oer_time::Duration;
use serde::{Deserialize, Serialize};

// ---- What a port's conformance crate provides. ----

/// A duration a port's crate or a target reports; a scenario crate never
/// writes one.
#[allow(
    clippy::disallowed_methods,
    reason = "stands for a port crate's constant or a target's report"
)]
const fn micros(value: u64) -> Duration {
    Duration::from_micros(value)
}

/// The port's bounds, by name, as the `oer_time::Duration` constants its
/// protocols already use (#761, owner decision 6223240823).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Bounds {
    /// Submission to terminal.
    terminal: Duration,
}

/// The contract's ceilings: no backend may exceed them.
const CEILINGS: Bounds = Bounds {
    terminal: micros(1_000),
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
enum Case {
    OneFrame,
    CloseGate,
    /// The port restarts before its frame, so its clock starts a new
    /// generation.
    AfterRestart,
}

impl Case {
    const fn restart(self) -> Restart {
        match self {
            Self::AfterRestart => Restart::Yes,
            Self::OneFrame | Self::CloseGate => Restart::No,
        }
    }
}

/// Identities read from the real returned owners, in order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Trace {
    submitted: Vec<u8>,
    terminals: Vec<u8>,
    /// Submission to terminal on the header's clock.
    latency: Duration,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Violation {
    Missing,
    Order,
    Bound,
}

trait Harness {
    async fn exercise(
        &mut self,
        case: Case,
    ) -> Result<(TraceHeader<Bounds>, Trace), Infrastructure>;
}

/// The suite's pure verdict, shared by the model tests and the stand; the
/// backend's bounds come as a value.
fn verify(case: Case, trace: &Trace, bounds: &Bounds) -> Result<(), Violation> {
    let expected = match case {
        Case::OneFrame | Case::AfterRestart => 1,
        Case::CloseGate => 2,
    };
    if trace.terminals.len() != expected || trace.submitted.len() != expected {
        return Err(Violation::Missing);
    }
    if trace.terminals != trace.submitted {
        return Err(Violation::Order);
    }
    if trace.latency > bounds.terminal {
        return Err(Violation::Bound);
    }
    Ok(())
}

// ---- A backend's own bounds, checked against the ceilings at compile time. ----

const S31_BOUNDS: Bounds = Bounds {
    terminal: micros(800),
};
const _: () = assert!(
    S31_BOUNDS.terminal.as_micros() <= CEILINGS.terminal.as_micros(),
    "the backend exceeds the contract's ceiling"
);

// ---- The scenario, in a block's scenario crate. ----

struct TxPort;

impl Subject for TxPort {
    const CONTRACT: ContractName = name!(ContractName, "toy-port::TxPort");
}

struct TxConformance;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Params {
    cases: Vec<Case>,
}

/// One report of the target: `(case, header, trace)`.
type Report = (Case, TraceHeader<Bounds>, Trace);

/// The run's reports, with the cases it was asked for.
#[derive(Debug, Deserialize, Serialize)]
struct Traces {
    expected: Vec<Case>,
    runs: Vec<Report>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Finding {
    Header(RunHeaderError),
    AboveCeiling {
        reported: Duration,
        ceiling: Duration,
    },
    Case {
        case: Case,
        violation: Violation,
    },
    /// An expected case has fewer traces than it was asked for.
    Unexercised(Case),
    /// A case has more traces than it was asked for.
    Unexpected(Case),
}

const TWICE: Factor = Factor::times(NonZeroU32::new(2).unwrap());

fn counts<'a>(cases: impl IntoIterator<Item = &'a Case>) -> BTreeMap<Case, usize> {
    let mut counts = BTreeMap::new();
    for case in cases {
        *counts.entry(*case).or_default() += 1;
    }
    counts
}

impl Spec for TxConformance {
    const ID: ScenarioId = name!(ScenarioId, "toy-tx-conformance");
    type Purpose = Conformance;
    type Subject = TxPort;
    type Params = Params;
    /// Conformance has no thresholds of its own: the suite, the reported
    /// bounds and the contract's ceilings decide.
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

    fn estimate(params: &Params, profile: &StandProfile) -> Result<Estimate, WaitError> {
        let cases = u64::try_from(params.cases.len()).map_err(|_| WaitError::Overflow)?;
        Estimate::from_waits(
            &[],
            &[Wait::contract(CEILINGS.terminal, TWICE)],
            cases,
            profile,
        )
    }

    fn verdict(traces: &Traces, (): &()) -> Verdict<Finding> {
        let mut findings = Vec::new();
        if let Err(error) = check_run(
            traces
                .runs
                .iter()
                .map(|(case, header, _)| (header, case.restart())),
        ) {
            findings.push(Finding::Header(error));
        }
        if let Some((_, header, _)) = traces.runs.first()
            && header.bounds.terminal > CEILINGS.terminal
        {
            findings.push(Finding::AboveCeiling {
                reported: header.bounds.terminal,
                ceiling: CEILINGS.terminal,
            });
        }
        let expected = counts(&traces.expected);
        let exercised = counts(traces.runs.iter().map(|(case, _, _)| case));
        for (case, &wanted) in &expected {
            if exercised.get(case).copied().unwrap_or(0) < wanted {
                findings.push(Finding::Unexercised(*case));
            }
        }
        for (case, &got) in &exercised {
            if got > expected.get(case).copied().unwrap_or(0) {
                findings.push(Finding::Unexpected(*case));
            }
        }
        findings.extend(traces.runs.iter().filter_map(|(case, header, trace)| {
            verify(*case, trace, &header.bounds)
                .err()
                .map(|violation| Finding::Case {
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
            let (header, trace) = executor.driver().exercise(case).await?;
            runs.push((case, header, trace));
        }
        Ok(Traces {
            expected: params.cases.clone(),
            runs,
        })
    }
}

// ---- Two drivers. ----

/// The host model.
struct ModelPort {
    generation: u32,
}

impl ModelDriver for ModelPort {}

impl Harness for ModelPort {
    async fn exercise(
        &mut self,
        case: Case,
    ) -> Result<(TraceHeader<Bounds>, Trace), Infrastructure> {
        if case.restart() == Restart::Yes {
            self.generation += 1;
        }
        let frames: Vec<u8> = match case {
            Case::OneFrame | Case::AfterRestart => vec![0],
            Case::CloseGate => vec![0, 1],
        };
        let header = TraceHeader {
            bounds: CEILINGS,
            clock: ClockDomain {
                clock: Clock::Model,
                generation: self.generation,
            },
        };
        Ok((
            header,
            Trace {
                submitted: frames.clone(),
                terminals: frames,
                latency: micros(400),
            },
        ))
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

/// A report for another case than the one asked for.
#[derive(Debug)]
struct OtherCase {
    asked: Case,
    reported: Case,
}

impl core::fmt::Display for OtherCase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "asked for {:?}, the target reported {:?}",
            self.asked, self.reported
        )
    }
}

impl std::error::Error for OtherCase {}

/// The stand's proxy: it asks the target to exercise a case and decodes
/// the postcard report `(case, header, trace)` the target sends; here the
/// reports are prepared.
struct StandProxy {
    reports: VecDeque<Vec<u8>>,
}

impl StandDriver for StandProxy {}

impl Harness for StandProxy {
    async fn exercise(
        &mut self,
        case: Case,
    ) -> Result<(TraceHeader<Bounds>, Trace), Infrastructure> {
        let operation = name!(Operation, "target.report");
        let report = self.reports.pop_front().ok_or_else(|| Infrastructure {
            operation: operation.clone(),
            cause: Box::new(NoReport(case)),
        })?;
        let (reported, header, trace): Report =
            postcard::from_bytes(&report).map_err(|error| Infrastructure {
                operation: operation.clone(),
                cause: Box::new(error),
            })?;
        if reported != case {
            return Err(Infrastructure {
                operation,
                cause: Box::new(OtherCase {
                    asked: case,
                    reported,
                }),
            });
        }
        Ok((header, trace))
    }
}

fn stand(reports: Vec<Vec<u8>>) -> Stand<'static, StandProxy> {
    Stand::open(
        StandSession::for_tests(),
        StandProxy {
            reports: reports.into(),
        },
    )
}

fn s31(bounds: Bounds, generation: u32) -> TraceHeader<Bounds> {
    TraceHeader {
        bounds,
        clock: ClockDomain {
            clock: Clock::Systimer,
            generation,
        },
    }
}

/// A report as an S31 target sends it, written independently of the
/// model: its own clock, its own bounds, its own latencies.
fn report(case: Case, header: TraceHeader<Bounds>, terminals: Vec<u8>, latency: u64) -> Vec<u8> {
    let submitted = if terminals.len() == 1 {
        vec![0]
    } else {
        vec![0, 1]
    };
    let trace = Trace {
        submitted,
        terminals,
        latency: micros(latency),
    };
    postcard::to_allocvec(&(case, header, trace)).unwrap()
}

fn instance(cases: Vec<Case>) -> Instance<TxConformance> {
    Instance::new(name!(InstanceName, "one-frame-and-gate"), Params { cases })
}

fn both() -> Instance<TxConformance> {
    instance(vec![Case::OneFrame, Case::CloseGate])
}

fn verdict_on_stand(instance: &Instance<TxConformance>, reports: Vec<Vec<u8>>) -> Verdict<Finding> {
    embassy_futures::block_on(execute(instance, &(), &mut stand(reports)))
        .unwrap()
        .verdict
}

#[test]
fn the_model_passes_its_own_suite_restart_included() {
    let all = instance(vec![Case::OneFrame, Case::AfterRestart, Case::CloseGate]);
    let outcome = embassy_futures::block_on(execute(
        &all,
        &(),
        &mut Model::new(ModelPort { generation: 0 }),
    ))
    .unwrap();
    assert_eq!(outcome.executed, Executed::Model);
    assert_eq!(outcome.verdict, Verdict::Pass);
}

#[test]
fn the_stand_judges_the_target_traces_with_the_bounds_they_carry() {
    // 700 µs passes the S31's own 800 µs bound.
    let reports = vec![
        report(Case::OneFrame, s31(S31_BOUNDS, 7), vec![0], 700),
        report(Case::CloseGate, s31(S31_BOUNDS, 7), vec![0, 1], 790),
    ];
    let outcome = embassy_futures::block_on(execute(&both(), &(), &mut stand(reports))).unwrap();
    assert_eq!(outcome.executed, Executed::Stand);
    assert_eq!(outcome.verdict, Verdict::Pass);

    // 900 µs keeps the 1 000 µs ceiling but not the S31's own bound.
    let late = vec![
        report(Case::OneFrame, s31(S31_BOUNDS, 7), vec![0], 900),
        report(Case::CloseGate, s31(S31_BOUNDS, 7), vec![0, 1], 100),
    ];
    assert_eq!(
        verdict_on_stand(&both(), late),
        Verdict::from_findings(vec![Finding::Case {
            case: Case::OneFrame,
            violation: Violation::Bound
        }])
    );
}

#[test]
fn a_restart_starts_a_new_generation_and_passes() {
    let all = instance(vec![Case::OneFrame, Case::AfterRestart, Case::CloseGate]);
    let reports = vec![
        report(Case::OneFrame, s31(S31_BOUNDS, 7), vec![0], 100),
        report(Case::AfterRestart, s31(S31_BOUNDS, 8), vec![0], 100),
        report(Case::CloseGate, s31(S31_BOUNDS, 8), vec![0, 1], 100),
    ];
    assert_eq!(verdict_on_stand(&all, reports), Verdict::Pass);
}

#[test]
fn a_new_generation_without_a_restart_is_a_finding() {
    let reports = vec![
        report(Case::OneFrame, s31(S31_BOUNDS, 7), vec![0], 100),
        report(Case::CloseGate, s31(S31_BOUNDS, 8), vec![0, 1], 100),
    ];
    assert_eq!(
        verdict_on_stand(&both(), reports),
        Verdict::from_findings(vec![Finding::Header(RunHeaderError::Generation {
            case: 1
        })])
    );
}

#[test]
fn a_bound_above_the_ceiling_is_a_finding() {
    let above = Bounds {
        terminal: micros(1_500),
    };
    let reports = vec![
        report(Case::OneFrame, s31(above, 7), vec![0], 100),
        report(Case::CloseGate, s31(above, 7), vec![0, 1], 100),
    ];
    assert_eq!(
        verdict_on_stand(&both(), reports),
        Verdict::from_findings(vec![Finding::AboveCeiling {
            reported: micros(1_500),
            ceiling: CEILINGS.terminal
        }])
    );
}

#[test]
fn a_target_trace_out_of_order_fails_with_its_finding() {
    let reports = vec![
        report(Case::OneFrame, s31(S31_BOUNDS, 7), vec![0], 100),
        report(Case::CloseGate, s31(S31_BOUNDS, 7), vec![1, 0], 100),
    ];
    assert_eq!(
        verdict_on_stand(&both(), reports),
        Verdict::from_findings(vec![Finding::Case {
            case: Case::CloseGate,
            violation: Violation::Order
        }])
    );
}

#[test]
fn the_cases_are_compared_as_a_multiset() {
    let traces = Traces {
        expected: vec![Case::OneFrame, Case::CloseGate],
        runs: vec![
            (
                Case::OneFrame,
                s31(S31_BOUNDS, 7),
                Trace {
                    submitted: vec![0],
                    terminals: vec![0],
                    latency: micros(1),
                },
            ),
            (
                Case::OneFrame,
                s31(S31_BOUNDS, 7),
                Trace {
                    submitted: vec![0],
                    terminals: vec![0],
                    latency: micros(1),
                },
            ),
        ],
    };
    assert_eq!(
        TxConformance::verdict(&traces, &()),
        Verdict::from_findings(vec![
            Finding::Unexercised(Case::CloseGate),
            Finding::Unexpected(Case::OneFrame)
        ])
    );
}

#[test]
fn a_run_without_traces_does_not_pass() {
    let outcome = embassy_futures::block_on(execute(
        &instance(Vec::new()),
        &(),
        &mut Model::new(ModelPort { generation: 0 }),
    ))
    .unwrap();
    assert_eq!(
        outcome.verdict,
        Verdict::from_findings(vec![Finding::Header(RunHeaderError::NoTrace)])
    );
}

#[test]
fn a_missing_report_is_infrastructure_not_a_verdict() {
    let failed = embassy_futures::block_on(execute(
        &both(),
        &(),
        &mut stand(vec![report(
            Case::OneFrame,
            s31(S31_BOUNDS, 7),
            vec![0],
            100,
        )]),
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
        report(Case::OneFrame, s31(S31_BOUNDS, 7), vec![0], 900),
        report(Case::CloseGate, s31(S31_BOUNDS, 7), vec![0, 1], 100),
    ];
    let outcome = embassy_futures::block_on(execute(&both(), &(), &mut stand(reports))).unwrap();
    let stored = postcard::to_allocvec(&outcome.trace).unwrap();
    assert_eq!(
        TxConformance::verdict(&postcard::from_bytes(&stored).unwrap(), &()),
        outcome.verdict
    );
}
