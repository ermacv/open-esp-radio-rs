//! Vendor Binary Workbench facade and CLI implementation.
//!
//! This facade composes neutral contracts and analysis/semantics layers with
//! the RISC-V backend, ESP32-S31 harness, CLI and verification workflows.

mod analysis;
mod application;
mod artifacts;
mod cli;
mod code_workspace;
mod digest;
mod error;
mod function_investigation;
mod function_workspace;
mod harnesses;
mod interfaces;
mod linked_ir_export;
mod memory_map;
mod navigation;
mod orchestration;
mod parse;
mod platform_pack;
mod project;
mod project_analysis;
mod project_ir;
mod qualification;
mod register_catalog;
mod registers;
mod review_scopes;
mod run_spec;
mod source_id;
mod target;
#[cfg(test)]
mod test_support;
mod tui;
mod verification;

use analysis::*;
pub use application::*;
use cli::run;
pub(crate) use digest::{artifact_path_sha256, artifact_sha256};
use error::WorkbenchError;
pub use function_investigation::{
    CallGraphEdgeEvidence, CallKnowledgeEvidence, EventDispatchBindingEvidence,
    EventDispatchEvidence, FunctionInvestigationReport, InvestigationLedgerEntry,
    OriginFunctionEvidence, ReviewedPathEvidence, ReviewedPreconditionEvidence,
    SemanticFunctionEvidence,
};
#[cfg(all(test, feature = "esp32s31-harness"))]
pub(crate) use harnesses::esp32s31::entry_contract;
use memory_map::MemoryMap;
#[cfg(all(test, feature = "esp32s31-harness"))]
use open_radio_vendor_analysis_model::reject_register_collisions;
use open_radio_vendor_analysis_model::*;
#[cfg(test)]
use open_radio_vendor_analysis_model::{MmioRegion, Register};
#[cfg(test)]
pub(crate) use open_radio_vendor_backend_riscv::Rv32CallArguments;
pub use open_radio_vendor_backend_riscv::artifact::{
    FunctionBasicBlock, FunctionBody, FunctionControlFlow, FunctionControlFlowKind,
    FunctionInstruction, FunctionInstructionRelocation, FunctionLabel,
};
pub use open_radio_vendor_backend_riscv::execution::Scenario as ExecutionScenario;
pub(crate) use open_radio_vendor_backend_riscv::{
    artifact, codegen, direct_target_audit, execution, interface_discovery,
};
pub(crate) use open_radio_vendor_execution_model as execution_model;
pub use open_radio_vendor_execution_model::{
    DeviceModel as ExecutionDeviceModel, DeviceModelCoverage as ExecutionDeviceModelCoverage,
    DeviceModelDescriptor as ExecutionDeviceModelDescriptor,
    DeviceModelInstance as ExecutionDeviceModelInstance,
    DeviceModelOutcome as ExecutionDeviceModelOutcome,
    DeviceModelRegistry as ExecutionDeviceModelRegistry,
    DeviceModelSpec as ExecutionDeviceModelSpec, TableInstance as ExecutionTableInstance,
    TableInstanceSlot as ExecutionTableInstanceSlot,
    TableLifecycleEvent as ExecutionTableLifecycleEvent,
    TableSlotTarget as ExecutionTableSlotTarget,
};
pub use open_radio_vendor_semantics::{EquivalenceMode, EquivalenceOutcome, EquivalenceVerdict};
pub(crate) use orchestration::generated_reference;
use parse::u32_literal as parse_u32;
use project::ProjectSpec;
use target::TargetSpec;
#[cfg(all(test, feature = "esp32s31-harness"))]
use test_support::private_input;
#[cfg(test)]
use test_support::trace_disassembly;
use verification::*;
pub use verification::{
    AlignedTraceItemReport, AllocationLifecycleReport, ArtifactIdentity, ArtifactReport,
    BranchDecisionReport, BranchOutcomeReport, CaseReport, ComparisonSummary, ControlFlowReport,
    CoverageReport, DeviceModelCoverageReport, DeviceModelReport, DifferenceKind,
    EventProducerReport, ExecutionComparisonReport, ExecutionEventReport, ExecutionPathReport,
    ExecutionPathSideReport, MemoryChangeReport, OrderedCallReport, RuntimeMemoryBindingReport,
    RuntimeMemoryInstanceReport, ScenarioEnvironmentReport, TableInstanceReport,
    TableInstanceSlotReport, TableLifecycleReport, TableSlotTargetReport, TraceDiffReport,
    TraceItemReport,
};

use std::process::ExitCode;
#[cfg(test)]
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
};

type Error = WorkbenchError;
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

fn render_error(error: Error) -> ExitCode {
    match error {
        WorkbenchError::Cli(error) => {
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
