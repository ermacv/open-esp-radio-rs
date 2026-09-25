//! Driver verification inside the calling process.
//!
//! The caller supplies the request, the ELF bytes of every target source and
//! the effect contracts and layout projections its relations select. Contracts
//! and projections are reviewed outside Blobray and selected by the digest of
//! their canonical encoding. Records stay in memory: no project, content store,
//! journal or knowledge review participates. Goals must not need symbol
//! resolution, and runtime tables and reviewed call pairs need a project.
//!
//! The vendor side of a request can execute once and be reused by requests
//! that differ only in their replacement side, such as the same request over
//! patched production images; only the replacement then executes. Reuse
//! yields the records of a full execution.
pub use crate::code_coverage::report_in_process as coverage;
use crate::execution::{Resolved, RunOutcome, Sources, VendorSide, run_resolved};
use crate::*;

fn digest(value: &impl serde::Serialize) -> Result<ArtifactId> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| Error::new(ErrorCode::InvalidRequest, e.to_string()))?;
    Ok(ArtifactId::of_bytes(&bytes))
}

/// The selection of an effect contract reviewed outside Blobray.
pub fn effect_contract_ref(contract: &EffectContract) -> Result<EffectContractRef> {
    Ok(EffectContractRef::Content {
        contract: digest(contract)?,
    })
}

/// The selection of a layout projection reviewed outside Blobray.
pub fn projection_ref(projection: &LayoutProjection) -> Result<ProjectionRef> {
    Ok(ProjectionRef::Content {
        projection: digest(projection)?,
    })
}

/// Vendor observations of one request's cases and the vendor coverage over
/// them, from `vendor`. They are reused only by requests with the same key.
pub struct VendorResults {
    key: ArtifactId,
    cases: Vec<ExecutionObservation>,
    coverage: Vec<ExecutionCoverage>,
}

/// The key of the vendor side of `input` under `executor`: everything the
/// vendor observations depend on when no replacement case blocks them.
fn vendor_key(input: &InProcessComparison<'_>, executor: &dyn Executor) -> Result<ArtifactId> {
    let request = input.request;
    digest(&serde_json::json!({
        "schema": EXECUTION_SCHEMA,
        "environment": crate::EXECUTION_ENVIRONMENT,
        "executor": executor.identity(),
        "executables": input
            .vendor
            .iter()
            .map(|bytes| ArtifactId::of_bytes(bytes))
            .collect::<Vec<_>>(),
        "target": &request.vendor,
        "max_events": request.max_events,
        "cases": request
            .cases
            .iter()
            .map(|case| (case.reset, case.stack_fill, &case.vendor))
            .collect::<Vec<_>>(),
    }))
}

pub struct InProcessComparison<'a> {
    pub request: &'a ExecutionRequest,
    /// ELF bytes of the vendor target's source, then of its companions.
    pub vendor: &'a [&'a [u8]],
    /// ELF bytes of the replacement target's sources, when it has one.
    pub replacement: Option<&'a [&'a [u8]]>,
    pub effects: &'a [EffectContract],
    pub projections: &'a [LayoutProjection],
    /// Vendor results of this request's vendor side, from `vendor`.
    pub vendor_results: Option<&'a VendorResults>,
}

pub struct InProcessResult {
    pub records: Vec<ExecutionEvidence>,
    pub verdict: Option<ComparisonVerdict>,
    pub complete: bool,
    /// Whether the given vendor results replaced executing the vendor side.
    pub vendor_reused: bool,
}

fn unsupported(what: &str) -> Error {
    Error::new(
        ErrorCode::InvalidRequest,
        format!("in-process verification does not support {what}"),
    )
}

/// Validate `input`, then run its request with the vendor side from `vendor`.
fn run(
    input: &InProcessComparison<'_>,
    vendor: &mut VendorSide<'_>,
    executor: &dyn Executor,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<(Vec<ExecutionEvidence>, RunOutcome)> {
    let request = input.request;
    // Capabilities this path does not have fail before anything else.
    for case in &request.cases {
        for invocation in std::iter::once(&case.vendor).chain(case.replacement.as_ref()) {
            if !matches!(
                invocation.goal,
                ExecutionGoal::Return | ExecutionGoal::ObserveDequeue { .. }
            ) {
                return Err(unsupported("symbol goals"));
            }
            if !invocation.tables.is_empty() {
                return Err(unsupported("runtime tables"));
            }
        }
        if case
            .relation
            .as_ref()
            .is_some_and(|r| r.reviewed_calls.is_some())
        {
            return Err(unsupported("reviewed call pairs"));
        }
    }
    request.validate()?;
    if request.replacement.is_some() != input.replacement.is_some() {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "replacement executables must match the request",
        ));
    }
    let effects = input
        .effects
        .iter()
        .map(|contract| {
            contract.validate()?;
            Ok(ResolvedEffectContract {
                review: effect_contract_ref(contract)?,
                contract: contract.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let projections = input
        .projections
        .iter()
        .map(|projection| {
            projection.validate()?;
            Ok(ResolvedProjection {
                review: projection_ref(projection)?,
                projection: projection.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut goals = Vec::with_capacity(request.cases.len());
    for (phase, case) in request.cases.iter().enumerate() {
        control.checkpoint(1)?;
        let mut resolved = [None; 2];
        for (side, invocation) in std::iter::once(&case.vendor)
            .chain(case.replacement.as_ref())
            .enumerate()
        {
            resolved[side] = Some(match &invocation.goal {
                ExecutionGoal::ObserveDequeue { .. } => ResolvedExecutionGoal::ObserveDequeue,
                _ => ResolvedExecutionGoal::Return,
            });
        }
        if let Some(relation) = &case.relation {
            if let Some(selected) = selected_effect_contract(Some(relation), &effects)? {
                selected.contract.validate_use(request, phase)?;
            }
            if let Some(selected) = selected_projection(Some(relation), &projections)? {
                selected.projection.validate_use(request, phase)?;
            }
        }
        goals.push(resolved);
    }
    let mut targets = Vec::new();
    if matches!(vendor, VendorSide::Execute(_)) {
        targets.push((&request.vendor, input.vendor));
    }
    if let (Some(target), Some(executables)) = (&request.replacement, input.replacement) {
        targets.push((target, executables));
    }
    let sources = Sources::from_executables(&targets, memory, control)?;
    let mut records = Vec::new();
    let outcome = run_resolved(
        &Resolved {
            request,
            goals: &goals,
            tables: &[],
            pairs: &[],
            projections: &projections,
            effects: &effects,
            sources: &sources,
        },
        vendor,
        executor,
        memory,
        &mut |record, _| {
            records.push(record);
            Ok(())
        },
        control,
    )?;
    Ok((records, outcome))
}

/// Execute the vendor side of every case of `input.request` once, for reuse
/// by `verify` of requests with the same vendor side.
pub fn vendor(
    input: &InProcessComparison<'_>,
    executor: &dyn Executor,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<VendorResults> {
    let mut request = input.request.clone();
    request.replacement = None;
    request.binding = None;
    for case in &mut request.cases {
        case.replacement = None;
        case.relation = None;
    }
    let mut cases = Vec::new();
    let (records, _) = run(
        &InProcessComparison {
            request: &request,
            replacement: None,
            vendor_results: None,
            ..*input
        },
        &mut VendorSide::Execute(Some(&mut cases)),
        executor,
        memory,
        control,
    )?;
    Ok(VendorResults {
        key: vendor_key(input, executor)?,
        cases,
        coverage: records
            .into_iter()
            .filter_map(|record| match record {
                ExecutionEvidence::Coverage {
                    replacement: false,
                    coverage,
                } => Some(coverage),
                _ => None,
            })
            .collect(),
    })
}

/// Execute and compare every case of `input.request`, reusing
/// `input.vendor_results` unless a replacement case blocks the vendor side.
pub fn verify(
    input: &InProcessComparison<'_>,
    executor: &dyn Executor,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<InProcessResult> {
    if let Some(reused) = input.vendor_results {
        if reused.key != vendor_key(input, executor)?
            || reused.cases.len() != input.request.cases.len()
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "vendor results belong to another vendor side",
            ));
        }
        let (records, outcome) = run(
            input,
            &mut VendorSide::Reuse {
                cases: &reused.cases,
                coverage: &reused.coverage,
            },
            executor,
            memory,
            control,
        )?;
        // A replacement case that blocked the vendor side makes the vendor
        // observations depend on it; such a request executes fully.
        if outcome.vendor_independent {
            return Ok(InProcessResult {
                records,
                verdict: outcome.verdict,
                complete: outcome.complete,
                vendor_reused: true,
            });
        }
    }
    let (records, outcome) = run(
        input,
        &mut VendorSide::Execute(None),
        executor,
        memory,
        control,
    )?;
    Ok(InProcessResult {
        records,
        verdict: outcome.verdict,
        complete: outcome.complete,
        vendor_reused: false,
    })
}
