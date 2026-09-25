//! Driver verification inside the calling process.
//!
//! The caller supplies the request, the ELF bytes of every target source and
//! the effect contracts and layout projections its relations select. Contracts
//! and projections are reviewed outside Blobray and selected by the digest of
//! their canonical encoding. Records stay in memory: no project, content store,
//! journal or knowledge review participates. Goals must not need symbol
//! resolution, and runtime tables and reviewed call pairs need a project.
pub use crate::code_coverage::report_in_process as coverage;
use crate::execution::{Resolved, Sources, run_resolved};
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

pub struct InProcessComparison<'a> {
    pub request: &'a ExecutionRequest,
    /// ELF bytes of the vendor target's source, then of its companions.
    pub vendor: &'a [&'a [u8]],
    /// ELF bytes of the replacement target's sources, when it has one.
    pub replacement: Option<&'a [&'a [u8]]>,
    pub effects: &'a [EffectContract],
    pub projections: &'a [LayoutProjection],
}

pub struct InProcessResult {
    pub records: Vec<ExecutionEvidence>,
    pub verdict: Option<ComparisonVerdict>,
    pub complete: bool,
}

fn unsupported(what: &str) -> Error {
    Error::new(
        ErrorCode::InvalidRequest,
        format!("in-process verification does not support {what}"),
    )
}

/// Execute and compare every case of `input.request`.
pub fn verify(
    input: &InProcessComparison<'_>,
    executor: &dyn Executor,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<InProcessResult> {
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
    let mut targets = vec![(&request.vendor, input.vendor)];
    if let (Some(target), Some(executables)) = (&request.replacement, input.replacement) {
        targets.push((target, executables));
    }
    let sources = Sources::from_executables(&targets, memory, control)?;
    let mut records = Vec::new();
    let (verdict, complete) = run_resolved(
        &Resolved {
            request,
            goals: &goals,
            tables: &[],
            pairs: &[],
            projections: &projections,
            effects: &effects,
            sources: &sources,
        },
        executor,
        memory,
        &mut |record, _| {
            records.push(record);
            Ok(())
        },
        control,
    )?;
    Ok(InProcessResult {
        records,
        verdict,
        complete,
    })
}
