//! Stable project entry point composing public target knowledge and local inputs.

use std::{
    io,
    path::{Path, PathBuf},
};

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::{
    Error, Result,
    platform_pack::PlatformPack,
    project_analysis::{NavigationIndexSpec, SymbolInventorySpec},
    project_ir::ProjectIrProfile,
};

mod load;

pub(crate) const DEFAULT_PROJECT_MANIFEST: &str = "vendor-project.toml";

#[derive(Debug, Error, Diagnostic)]
pub(crate) enum ProjectError {
    #[error("cannot read project manifest {}", path.display())]
    #[diagnostic(code(workbench::project::read))]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{message}")]
    #[diagnostic(code(workbench::project::parse))]
    Parse {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("invalid TOML")]
        span: SourceSpan,
    },
    #[error("{message}")]
    #[diagnostic(code(workbench::project::invalid))]
    Invalid {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("invalid project configuration")]
        span: SourceSpan,
    },
}

/// Source context used by every project-manifest section decoder.
///
/// Keeping this next to `ProjectError` prevents nested configuration modules
/// from falling back to location-less string errors.
#[derive(Clone, Copy)]
pub(crate) struct ProjectSource<'a> {
    path: &'a Path,
    input: &'a str,
}

impl<'a> ProjectSource<'a> {
    pub(crate) const fn new(path: &'a Path, input: &'a str) -> Self {
        Self { path, input }
    }

    pub(crate) fn error(
        self,
        span: Option<std::ops::Range<usize>>,
        message: impl Into<String>,
    ) -> Error {
        let span = span
            .filter(|span| span.start < self.input.len())
            .unwrap_or(0..self.input.len().min(1));
        let length = span
            .len()
            .max(1)
            .min(self.input.len().saturating_sub(span.start).max(1));
        ProjectError::Invalid {
            message: message.into(),
            src: NamedSource::new(self.path.display().to_string(), self.input.to_owned()),
            span: (span.start, length).into(),
        }
        .into()
    }

    pub(crate) fn item(self, item: Option<&toml_edit::Item>, message: impl Into<String>) -> Error {
        self.error(item.and_then(toml_edit::Item::span), message)
    }

    pub(crate) fn table_key(
        self,
        table: &toml_edit::Table,
        key: &str,
        message: impl Into<String>,
    ) -> Error {
        self.error(
            table
                .get(key)
                .and_then(toml_edit::Item::span)
                .or_else(|| table.span()),
            message,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterWorkspacePaths {
    pub(crate) facts: PathBuf,
    pub(crate) model: PathBuf,
    pub(crate) owned_ranges: Vec<String>,
    pub(crate) review_output: Option<PathBuf>,
    pub(crate) review_ir_reports: Vec<PathBuf>,
    pub(crate) svd_output: Option<PathBuf>,
    pub(crate) pac: Option<PacOutputSpec>,
    pub(crate) bindings: Option<PacBindingsOutputSpec>,
    pub(crate) api_pack: Option<PathBuf>,
    pub(crate) lint_pack: Option<PathBuf>,
    pub(crate) evidence_catalogs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PacOutputSpec {
    pub(crate) output: PathBuf,
    pub(crate) target: String,
    pub(crate) edition: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PacBindingsOutputSpec {
    pub(crate) output: PathBuf,
    pub(crate) crate_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceWorkspacePaths {
    pub(crate) facts: PathBuf,
    pub(crate) pack: Option<PathBuf>,
    pub(crate) semantic_catalogs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionWorkspacePaths {
    pub(crate) pack: PathBuf,
    pub(crate) profiles: Vec<String>,
    pub(crate) review_output: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodeWorkspacePaths {
    pub(crate) pack: PathBuf,
    pub(crate) review_output: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerificationWorkspacePaths {
    pub(crate) profiles: Vec<PathBuf>,
    pub(crate) rust_prefix: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectSpec {
    pub(crate) id: String,
    pub(crate) target_spec: PathBuf,
    pub(crate) platform_pack: Option<PlatformPack>,
    pub(crate) run_spec: Option<PathBuf>,
    pub(crate) memory_map: Option<PathBuf>,
    pub(crate) svd_configured: bool,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) symbol_inventory: Option<SymbolInventorySpec>,
    pub(crate) navigation_index: Option<NavigationIndexSpec>,
    pub(crate) code: Option<CodeWorkspacePaths>,
    pub(crate) ir_profiles: Vec<ProjectIrProfile>,
    pub(crate) registers: Option<RegisterWorkspacePaths>,
    pub(crate) interfaces: Option<InterfaceWorkspacePaths>,
    pub(crate) functions: Option<FunctionWorkspacePaths>,
    pub(crate) verification: Option<VerificationWorkspacePaths>,
}

impl ProjectSpec {
    pub(crate) fn discover_from(start: &Path) -> Result<Option<PathBuf>> {
        for directory in start.ancestors() {
            let current = directory.join(DEFAULT_PROJECT_MANIFEST);
            if current.is_file() {
                return Ok(Some(current));
            }
        }
        Ok(None)
    }

    pub(crate) fn load(path: &Path) -> Result<Self> {
        load::load(path)
    }

    pub(crate) fn function_ir_reports(&self) -> Result<Vec<(String, PathBuf)>> {
        let Some(functions) = &self.functions else {
            return Ok(Vec::new());
        };
        functions
            .profiles
            .iter()
            .map(|id| {
                self.ir_profiles
                    .iter()
                    .find(|profile| profile.id == *id)
                    .map(|profile| (id.clone(), profile.output.clone()))
                    .ok_or_else(|| {
                        crate::Error::invalid(format!(
                            "unknown function workspace IR profile {id:?}"
                        ))
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
