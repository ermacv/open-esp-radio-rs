//! Blobray facade and CLI implementation.
//!
//! This facade composes neutral contracts and analysis/semantics layers with
//! the RISC-V backend, optional target providers, CLI and verification workflows.

mod analysis;
mod application;
mod artifacts;
mod blocker_resolution;
mod chip_pack;
mod cli;
mod code_workspace;
mod digest;
mod ecosystem_pack;
mod error;
mod flow_investigation;
mod function_investigation;
mod function_workspace;
mod harnesses;
mod interfaces;
mod linked_ir_export;
mod memory_map;
mod navigation;
mod orchestration;
mod parse;
mod progress;
mod project;
mod project_analysis;
mod project_ir;
mod register_catalog;
mod registers;
mod resource_usage;
mod review_scopes;
mod run_spec;
mod shell;
mod source_id;
mod target;
#[cfg(test)]
mod test_support;
mod tui;
mod verification;

use analysis::*;
pub use application::*;
pub use blocker_resolution::{
    BlockerCompletionKind, BlockerCompletionPredicate, BlockerProducerEffect,
    BlockerResolutionOwner, BlockerResolutionRecordKind, BlockerResolutionRoute,
};
use cli::run;
pub(crate) use digest::{artifact_path_sha256, artifact_sha256};
use error::BlobrayError;
pub use function_investigation::{
    CallArgumentEvidence, CallGraphEdgeEvidence, CallKnowledgeEvidence,
    EventDispatchBindingEvidence, EventDispatchEvidence, FunctionInvestigationReport,
    InvestigationLedgerEntry, OriginFunctionEvidence, ReplacementEvidence,
    ReplacementProofEvidence, ReviewedPathEvidence, ReviewedPreconditionEvidence,
    SemanticFunctionEvidence, StoredLinkedIrRecord,
};
pub use harnesses::{KnowledgeProviderDescriptor, ProviderRegistry};
use memory_map::MemoryMap;
use open_radio_vendor_analysis_model::*;
#[cfg(test)]
use open_radio_vendor_analysis_model::{MmioRegion, Register};
pub use open_radio_vendor_backend_riscv::RiscvHarnessSpec;
pub use open_radio_vendor_backend_riscv::artifact::{
    FunctionBasicBlock, FunctionBody, FunctionControlFlow, FunctionControlFlowKind,
    FunctionInstruction, FunctionInstructionRelocation, FunctionLabel,
};
pub use open_radio_vendor_backend_riscv::execution::Scenario as ExecutionScenario;
pub(crate) use open_radio_vendor_backend_riscv::{
    Rv32CallArguments, Rv32IntrinsicResult, artifact, codegen, direct_target_audit, execution,
    interface_discovery,
};
pub(crate) use open_radio_vendor_execution_model as execution_model;
pub use open_radio_vendor_execution_model::{
    DeviceModel as ExecutionDeviceModel, DeviceModelCoverage as ExecutionDeviceModelCoverage,
    DeviceModelDescriptor as ExecutionDeviceModelDescriptor,
    DeviceModelInstance as ExecutionDeviceModelInstance,
    DeviceModelOutcome as ExecutionDeviceModelOutcome,
    DeviceModelRegistry as ExecutionDeviceModelRegistry,
    DeviceModelSpec as ExecutionDeviceModelSpec, ExecutionGoal,
    FifoLifecycleEvent as ExecutionFifoLifecycleEvent,
    FifoServiceBinding as ExecutionFifoServiceBinding,
    FifoServiceInstance as ExecutionFifoServiceInstance,
    FifoServiceOperation as ExecutionFifoServiceOperation, ServiceOutput as ExecutionServiceOutput,
    ServiceValueSource as ExecutionServiceValueSource, TableInstance as ExecutionTableInstance,
    TableInstanceSlot as ExecutionTableInstanceSlot,
    TableLifecycleEvent as ExecutionTableLifecycleEvent,
    TableSlotTarget as ExecutionTableSlotTarget,
};
pub use open_radio_vendor_semantics::{
    EquivalenceMode, EquivalenceOutcome, EquivalenceVerdict, KnowledgeContractSpec, MmioMap,
};
pub(crate) use orchestration::generated_reference;
use parse::u32_literal as parse_u32;
use project::ProjectSpec;
use target::TargetSpec;
#[cfg(test)]
use test_support::trace_disassembly;
use verification::*;
pub use verification::{
    AlignedTraceItemReport, AllocationLifecycleReport, ArtifactIdentity, ArtifactReport,
    BranchDecisionReport, BranchOutcomeReport, CaseReport, ComparisonSummary, ControlFlowReport,
    CoverageReport, DeviceModelCoverageReport, DeviceModelReport, DifferenceKind,
    EventProducerReport, ExecutionComparisonReport, ExecutionEventReport, ExecutionPathReport,
    ExecutionPathSideReport, MemoryChangeReport, MemoryInputReport, OrderedCallReport,
    RuntimeMemoryBindingReport, RuntimeMemoryInstanceReport, ScenarioEnvironmentReport,
    TableInstanceReport, TableInstanceSlotReport, TableLifecycleReport, TableSlotTargetReport,
    TraceDiffReport, TraceItemReport,
};

use std::process::ExitCode;
#[cfg(test)]
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
};

type Error = BlobrayError;
type Result<T> = error::Result<T>;
pub fn main_entry() -> ExitCode {
    let result = match run() {
        Ok(value) => cli::finish_output().map(|()| value),
        Err(error) => Err(error),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => render_error(error),
    }
}

/// Run the CLI with a caller-owned, statically linked platform-provider set.
///
/// A standalone generic build calls [`main_entry`] and has no platform
/// vocabulary. Product repositories use this entry point from a thin host
/// binary so target knowledge never becomes a dependency of the generic tool.
pub fn main_entry_with_providers(registry: &'static ProviderRegistry) -> ExitCode {
    if let Err(message) = harnesses::install_registry(registry) {
        eprintln!("add-on provider initialization failed: {message}");
        return ExitCode::FAILURE;
    }
    main_entry()
}

fn render_error(error: Error) -> ExitCode {
    match error {
        BlobrayError::Cli(error) => {
            let exit_code = error.exit_code();
            let _ = error.print();
            u8::try_from(exit_code)
                .map(ExitCode::from)
                .unwrap_or(ExitCode::FAILURE)
        }
        error => {
            eprintln!("{:?}", miette::Report::new(error));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
