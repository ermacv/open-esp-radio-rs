//! SVD-aware extraction of direct MMIO traces from compiled RISC-V code.
//!
//! This is deliberately an instruction/ELF tool, not a source-text policy
//! checker. Direct trace comparison handles straight-line leaves exactly. The
//! stricter reference resolver additionally composes supported calls and
//! bounded acyclic symbolic branches while failing closed on unresolved MMIO
//! addressing, loops and unsupported effects.

mod analysis;
mod artifact;
mod cli;
mod codegen;
mod execution;
mod external_abi;
mod ir;
mod mmio;
mod parse;
mod qualification;
#[cfg(test)]
mod test_support;
mod verification;

use analysis::*;
use cli::run;
use ir::*;
use mmio::MmioRegisterMap;
#[cfg(test)]
use mmio::{Register, Window, reject_register_collisions};
use parse::u32_literal as parse_u32;
#[cfg(test)]
use test_support::trace_disassembly;
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
