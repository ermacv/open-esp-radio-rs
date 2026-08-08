//! Typed facade errors.

use std::{io, num::ParseIntError, string::FromUtf8Error};

use miette::Diagnostic;
use thiserror::Error;

use crate::{project::ProjectError, run_spec::RunSpecError, target::TargetError};

#[derive(Debug, Error, Diagnostic)]
pub(crate) enum WorkbenchError {
    #[error("{message}")]
    #[diagnostic(
        code(workbench::input::invalid),
        help("use the leaf command's --help output or correct the referenced project input")
    )]
    InvalidInput { message: String },
    #[error(transparent)]
    Cli(#[from] clap::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Integer(#[from] ParseIntError),
    #[error(transparent)]
    Toml(#[from] toml_edit::TomlError),
    #[error(transparent)]
    TomlSerialize(#[from] toml_edit::ser::Error),
    #[error(transparent)]
    Utf8(#[from] FromUtf8Error),
    #[error(transparent)]
    Svd(#[from] svd_rs::SvdError),
    #[error(transparent)]
    DiagnosticInstall(#[from] miette::InstallError),
    #[error(transparent)]
    TracingInstall(#[from] tracing_subscriber::util::TryInitError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    RunSpec(#[from] RunSpecError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Target(#[from] TargetError),
    #[error(transparent)]
    Analysis(#[from] open_radio_vendor_analysis_model::Error),
    #[error(transparent)]
    Semantics(#[from] open_radio_vendor_semantics::Error),
    #[error(transparent)]
    RiscvBackend(#[from] open_radio_vendor_backend_riscv::Error),
    #[error(transparent)]
    Esp32s31Harness(#[from] open_radio_vendor_harness_esp32s31_semantic::Error),
    #[error(transparent)]
    RegisterModel(#[from] open_esp_radio_register_model::Error),
}

impl From<String> for WorkbenchError {
    fn from(value: String) -> Self {
        Self::InvalidInput { message: value }
    }
}

impl From<&str> for WorkbenchError {
    fn from(value: &str) -> Self {
        Self::InvalidInput {
            message: value.to_owned(),
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, WorkbenchError>;
