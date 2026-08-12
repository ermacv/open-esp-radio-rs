//! RISC-V 32-bit artifact, analysis, execution and reference-code backend.
//!
//! The backend owns RV32 instruction semantics and the `riscv-ilp32` calling
//! convention. Platform knowledge is supplied through [`RiscvHarnessSpec`]
//! and never selected by chip identity inside this crate.

use open_radio_vendor_analysis_model::*;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("cannot read binary artifact {}", path.display())]
    ArtifactRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Format(#[from] std::fmt::Error),

    #[error(transparent)]
    Object(#[from] object::Error),

    #[error(transparent)]
    Analysis(#[from] open_radio_vendor_analysis_model::Error),

    #[error(transparent)]
    ExecutionModel(#[from] open_radio_vendor_execution_model::Error),
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Message(value.to_owned())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn read_artifact(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| Error::ArtifactRead {
        path: path.to_owned(),
        source,
    })
}

pub const RV32_REGISTER_ARGUMENT_COUNT: usize = 8;
pub const RV32_STACK_ARGUMENT_COUNT: usize = 8;
pub const RV32_MODELED_ARGUMENT_COUNT: usize =
    RV32_REGISTER_ARGUMENT_COUNT + RV32_STACK_ARGUMENT_COUNT;
pub type Rv32CallArguments = [SymbolicValue; RV32_MODELED_ARGUMENT_COUNT];

pub fn encode_fence_set(set: rv_asm::FenceSet) -> u8 {
    u8::from(set.device_input) << 3
        | u8::from(set.device_output) << 2
        | u8::from(set.memory_read) << 1
        | u8::from(set.memory_write)
}

pub mod artifact;
pub mod codegen;
pub mod direct_target_audit;
pub mod execution;
pub mod interface_discovery;
pub mod reference_analysis;
pub mod static_analysis;

pub use interface_discovery::{
    InterfaceArgumentValue, InterfaceCallCandidate, InterfaceCallKind, InterfaceDiscovery,
    InterfaceLoad, InterfacePointer, InterfaceRoot, InterfaceSymbolAddressing,
    discover_interface_calls,
};
pub use reference_analysis::{ReferenceResolver, ReferenceSymbolKey};
pub use static_analysis::{
    RiscvHarnessSpec, RiscvSummaryHooks, StructuralCallSite, StructuralPointerContext,
    StructuralProjectedRelocation, SymbolicStack, trace_binary_symbol,
};

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn missing_artifact_error_names_the_binary_path() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-missing-artifact-{}.elf",
            std::process::id()
        ));
        let error = read_artifact(&path).unwrap_err();
        assert!(error.to_string().contains(&path.display().to_string()));
        assert!(error.to_string().contains("binary artifact"));
    }
}
