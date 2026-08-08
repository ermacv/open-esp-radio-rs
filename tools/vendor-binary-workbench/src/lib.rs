//! Vendor Binary Workbench facade and CLI implementation.
//!
//! This facade composes neutral contracts and analysis/semantics layers with
//! the RISC-V backend, ESP32-S31 harness, CLI and verification workflows.

macro_rules! outputln {
    ($($argument:tt)*) => {{
        crate::cli::output_line(format_args!($($argument)*));
    }};
}

mod analysis;
mod cli;
mod digest;
mod error;
mod function_workspace;
mod harnesses;
mod interfaces;
mod memory_map;
mod orchestration;
mod parse;
mod platform_pack;
mod project;
mod project_ir;
mod project_ir_report;
mod registers;
mod run_spec;
mod source_id;
mod target;
#[cfg(test)]
mod test_support;
mod verification;

use analysis::*;
use cli::run;
pub(crate) use digest::artifact_sha256;
use error::WorkbenchError;
#[cfg(test)]
pub(crate) use harnesses::esp32s31::entry_contract;
#[cfg(test)]
pub(crate) use harnesses::esp32s31::external_abi;
use memory_map::MemoryMap;
use open_radio_vendor_analysis_model::*;
#[cfg(test)]
use open_radio_vendor_analysis_model::{Register, Window, reject_register_collisions};
#[cfg(test)]
pub(crate) use open_radio_vendor_backend_riscv::Rv32CallArguments;
pub(crate) use open_radio_vendor_backend_riscv::{
    artifact, codegen, direct_target_audit, execution, interface_discovery,
};
pub(crate) use orchestration::generated_reference;
use parse::u32_literal as parse_u32;
use project::ProjectSpec;
use target::TargetSpec;
#[cfg(test)]
use test_support::{private_input, trace_disassembly};
use verification::*;

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
