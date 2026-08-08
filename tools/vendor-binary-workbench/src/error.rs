//! Typed facade errors.

use std::{io, num::ParseIntError, path::PathBuf, string::FromUtf8Error};

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::{project::ProjectError, run_spec::RunSpecError, target::TargetError};

#[derive(Clone, Copy)]
pub(crate) struct ManifestContext<'a> {
    kind: &'static str,
    path: &'a std::path::Path,
    input: &'a str,
}

impl<'a> ManifestContext<'a> {
    pub(crate) fn new(kind: &'static str, path: &'a std::path::Path, input: &'a str) -> Self {
        Self { kind, path, input }
    }

    pub(crate) fn error(
        self,
        span: Option<std::ops::Range<usize>>,
        message: impl std::fmt::Display,
    ) -> WorkbenchError {
        WorkbenchError::manifest_source(self.kind, self.path, self.input, message, span)
    }

    pub(crate) fn item(
        self,
        item: Option<&toml_edit::Item>,
        message: impl std::fmt::Display,
    ) -> WorkbenchError {
        self.error(item.and_then(toml_edit::Item::span), message)
    }

    pub(crate) fn table_key(
        self,
        table: &toml_edit::Table,
        key: &str,
        message: impl std::fmt::Display,
    ) -> WorkbenchError {
        self.item(table.get(key), message)
    }
}

#[derive(Debug, Error, Diagnostic)]
pub(crate) enum WorkbenchError {
    #[error("{message}")]
    #[diagnostic(
        code(workbench::input::invalid),
        help("use the leaf command's --help output or correct the referenced project input")
    )]
    InvalidInput { message: String },
    #[error("{message}")]
    #[diagnostic(code(workbench::input::line))]
    InputLine { line: usize, message: String },
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
        #[label("invalid document content")]
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
    #[cfg(feature = "esp32s31-harness")]
    #[error(transparent)]
    Esp32s31Harness(#[from] open_radio_vendor_harness_esp32s31_semantic::Error),
    #[error(transparent)]
    RegisterModel(open_esp_radio_register_model::Error),
}

impl From<open_esp_radio_register_model::Error> for WorkbenchError {
    fn from(error: open_esp_radio_register_model::Error) -> Self {
        let diagnostic = error
            .manifest_diagnostic()
            .map(|(kind, path, reason, span)| (kind, path.to_owned(), reason.to_owned(), span));
        if let Some((kind, path, reason, span)) = diagnostic
            && let Ok(input) = std::fs::read_to_string(&path)
        {
            return Self::manifest_source(kind, &path, &input, reason, span);
        }
        Self::RegisterModel(error)
    }
}

impl WorkbenchError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: message.into(),
        }
    }

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
        let span = normalize_source_span(input, span);
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

    pub(crate) fn at_line(self, line: usize) -> Self {
        if matches!(self, Self::InputLine { .. }) {
            self
        } else {
            Self::InputLine {
                line,
                message: self.to_string(),
            }
        }
    }

    pub(crate) fn manifest_document(
        kind: &'static str,
        path: &std::path::Path,
        input: &str,
        error: Self,
    ) -> Self {
        let line = match &error {
            Self::InputLine { line, .. } => Some(*line),
            Self::Json(error) => Some(error.line()),
            _ => None,
        };
        match line.and_then(|line| source_line_span(input, line)) {
            Some(span) => Self::manifest_source(kind, path, input, error, Some(span)),
            None => Self::manifest(kind, path, error),
        }
    }
}

fn normalize_source_span(
    input: &str,
    span: Option<std::ops::Range<usize>>,
) -> std::ops::Range<usize> {
    match span {
        Some(span) if !span.is_empty() => span,
        Some(span) if span.start < input.len() => span.start..span.start + 1,
        Some(_) if !input.is_empty() => input.len() - 1..input.len(),
        _ => 0..input.len().min(1),
    }
}

fn source_line_span(input: &str, line: usize) -> Option<std::ops::Range<usize>> {
    if line == 0 {
        return None;
    }
    let mut offset = 0;
    for (index, physical_line) in input.split_inclusive('\n').enumerate() {
        let length = physical_line.trim_end_matches(['\r', '\n']).len();
        if index + 1 == line {
            return Some(offset..offset + length.max(1).min(physical_line.len()));
        }
        offset += physical_line.len();
    }
    if line == input.lines().count() && !input.ends_with('\n') {
        return Some(input.len().saturating_sub(1)..input.len());
    }
    None
}

pub(crate) type Result<T> = std::result::Result<T, WorkbenchError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_diagnostic_retains_the_physical_source_line() {
        let input = "{\n  \"value\":,\n}\n";
        let error = serde_json::from_str::<serde_json::Value>(input).unwrap_err();
        let error = WorkbenchError::manifest_document(
            "fixture report",
            std::path::Path::new("fixture.json"),
            input,
            error.into(),
        );

        assert!(matches!(
            error,
            WorkbenchError::ManifestSource { span, .. }
                if span.offset() == input.find("  \"value\":,").unwrap()
        ));
    }

    #[test]
    fn register_model_diagnostic_is_promoted_to_a_source_diagnostic() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-register-diagnostic-{}.toml",
            std::process::id()
        ));
        let input = "schema = [\n";
        std::fs::write(&path, input).unwrap();
        let error = open_esp_radio_register_model::RegisterModel::load(&path).unwrap_err();
        let error = WorkbenchError::from(error);
        std::fs::remove_file(&path).unwrap();

        assert!(matches!(
            error,
            WorkbenchError::ManifestSource {
                kind: "register model manifest",
                path: reported,
                span,
                ..
            } if reported == path && !span.is_empty()
        ));
    }
}
