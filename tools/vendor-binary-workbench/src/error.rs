//! Typed facade errors.

use std::{io, num::ParseIntError, path::PathBuf, string::FromUtf8Error};

use miette::{Diagnostic, NamedSource, SourceSpan};
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
    #[error("invalid {kind} {path}: {reason}")]
    #[diagnostic(
        code(workbench::manifest::invalid),
        help("correct the named manifest; line numbers refer to its physical text lines")
    )]
    Manifest {
        kind: &'static str,
        path: PathBuf,
        reason: String,
    },
    #[error("invalid {kind} {path}: {reason}")]
    #[diagnostic(
        code(workbench::manifest::parse),
        help("correct the highlighted syntax in the named manifest")
    )]
    ManifestSource {
        kind: &'static str,
        path: PathBuf,
        reason: String,
        #[source_code]
        src: std::sync::Arc<NamedSource<String>>,
        #[label("invalid document syntax")]
        span: SourceSpan,
    },
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

impl WorkbenchError {
    pub(crate) fn manifest(
        kind: &'static str,
        path: impl Into<PathBuf>,
        error: impl std::fmt::Display,
    ) -> Self {
        Self::Manifest {
            kind,
            path: path.into(),
            reason: error.to_string(),
        }
    }

    pub(crate) fn manifest_source(
        kind: &'static str,
        path: &std::path::Path,
        input: &str,
        error: impl std::fmt::Display,
        span: Option<std::ops::Range<usize>>,
    ) -> Self {
        let span = span.unwrap_or(0..input.len().min(1));
        Self::ManifestSource {
            kind,
            path: path.to_owned(),
            reason: error.to_string(),
            src: std::sync::Arc::new(NamedSource::new(
                path.display().to_string(),
                input.to_owned(),
            )),
            span: span.into(),
        }
    }
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
