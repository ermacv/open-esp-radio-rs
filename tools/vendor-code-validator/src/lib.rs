//! Compiled vendor-code analysis, reference generation and Rust validation.
//!
//! This facade composes the standalone neutral model, RISC-V backend and
//! ESP32-S31 harness with CLI and verification workflows. Platform semantic
//! adapters remain here during the next migration slice.

mod analysis;
mod cli;
mod digest;
mod harnesses;
mod interfaces;
mod memory_map;
mod orchestration;
mod parse;
mod project;
mod registers;
mod run_spec;
mod target;
#[cfg(test)]
mod test_support;
mod verification;

use analysis::*;
use cli::run;
pub(crate) use digest::artifact_sha256;
#[cfg(test)]
pub(crate) use harnesses::esp32s31::entry_contract;
#[cfg(test)]
pub(crate) use harnesses::esp32s31::external_abi;
use memory_map::MemoryMap;
#[cfg(test)]
pub(crate) use open_radio_vendor_backend_riscv::Rv32CallArguments;
pub(crate) use open_radio_vendor_backend_riscv::{
    artifact, codegen, direct_target_audit, execution, interface_discovery,
};
use open_radio_vendor_validator_model::*;
#[cfg(test)]
use open_radio_vendor_validator_model::{Register, Window, reject_register_collisions};
pub(crate) use orchestration::generated_reference;
use parse::u32_literal as parse_u32;
use project::ProjectSpec;
use run_spec::RunSpec;
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

type Error = Box<dyn std::error::Error>;
type Result<T> = std::result::Result<T, Error>;
pub fn main_entry() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => {
            cli::usage();
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
