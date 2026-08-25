//! Stable project entry point composing public target knowledge and local inputs.

use std::{
    io,
    path::{Path, PathBuf},
};

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::{
    Error, Result,
    chip_pack::ChipPack,
    ecosystem_pack::EcosystemPack,
    project_analysis::{NavigationIndexSpec, SymbolInventorySpec},
    project_ir::ProjectIrProfile,
    run_spec::InputRole,
    source_id::SourceId,
};

mod load;

pub(crate) const DEFAULT_PROJECT_MANIFEST: &str = "vendor-project.toml";

#[derive(Debug, Error, Diagnostic)]
pub(crate) enum ProjectError {
    #[error("cannot read project manifest {}", path.display())]
    #[diagnostic(code(blobray::project::read))]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{message}")]
    #[diagnostic(code(blobray::project::parse))]
    Parse {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("invalid TOML")]
        span: SourceSpan,
    },
    #[error("{message}")]
    #[diagnostic(code(blobray::project::invalid))]
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
    pub(crate) non_operational_functions: Vec<String>,
    pub(crate) review_output: Option<PathBuf>,
    pub(crate) review_ir_reports: Vec<PathBuf>,
    pub(crate) svd_output: Option<PathBuf>,
    pub(crate) pac_raw: Option<PacRawOutputSpec>,
    pub(crate) bindings: Option<PacBindingsOutputSpec>,
    pub(crate) api_pack: Option<PathBuf>,
    pub(crate) api_output: Option<PathBuf>,
    pub(crate) lint_pack: Option<PathBuf>,
    pub(crate) evidence_catalogs: Vec<PathBuf>,
    pub(crate) reviewed_knowledge: Vec<PathBuf>,
    pub(crate) review_context: open_radio_vendor_review::ApplicabilityContext,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PacRawOutputSpec {
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
    pub(crate) capability_packs: Vec<PathBuf>,
    pub(crate) interface_template_packs: Vec<PathBuf>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectVerificationGate {
    Informational,
    Completion,
    Regression { match_floor: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum VerificationVendorSelection {
    All,
    Prefix(String),
    Symbols(Vec<String>),
}

impl VerificationVendorSelection {
    pub(crate) fn includes(&self, symbol: &str) -> bool {
        match self {
            Self::All => true,
            Self::Prefix(prefix) => symbol.starts_with(prefix),
            Self::Symbols(symbols) => symbols.iter().any(|candidate| candidate == symbol),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerificationVendorSpec {
    pub(crate) source: SourceId,
    pub(crate) selection: VerificationVendorSelection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerificationSuiteSpec {
    pub(crate) id: String,
    pub(crate) vendor: Vec<VerificationVendorSpec>,
    /// Additional linked images available only to platform verification
    /// adapters; they do not enlarge the suite's source inventory.
    pub(crate) auxiliary_sources: Vec<SourceId>,
    pub(crate) rust_artifact_role: InputRole,
    pub(crate) rust_companion_role: Option<InputRole>,
    pub(crate) rust_prefix: String,
    pub(crate) profiles: Vec<PathBuf>,
    pub(crate) dispositions: Vec<PathBuf>,
    pub(crate) evidence_baselines: Vec<PathBuf>,
    pub(crate) gate: ProjectVerificationGate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerificationWorkspacePaths {
    pub(crate) report: PathBuf,
    pub(crate) evidence_index: PathBuf,
    pub(crate) policy: Option<PathBuf>,
    pub(crate) suites: Vec<VerificationSuiteSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewScopeSpec {
    pub(crate) id: String,
    /// Explicit radio domains that benefit from this scope. A scope may be
    /// shared by several protocols; membership is never inferred from its id.
    pub(crate) protocols: Vec<String>,
    pub(crate) profiles: Vec<String>,
    pub(crate) roots: Vec<String>,
    pub(crate) include_reachable: bool,
}

pub(crate) const REVIEW_PROTOCOLS: [&str; 6] =
    ["wifi", "bluetooth", "ble", "ieee802154", "coex", "shared"];

pub(crate) fn canonical_review_protocol(value: &str) -> Option<&'static str> {
    REVIEW_PROTOCOLS
        .into_iter()
        .find(|candidate| *candidate == value)
}

pub(crate) fn normalize_review_protocol_alias(value: &str) -> Option<&'static str> {
    match value {
        "bt" | "bluetooth" => Some("bluetooth"),
        "802.15.4" | "802154" | "ieee802154" => Some("ieee802154"),
        value => canonical_review_protocol(value),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewWorkspaceSpec {
    pub(crate) output: PathBuf,
    pub(crate) publication_scopes: Vec<String>,
    pub(crate) scopes: Vec<ReviewScopeSpec>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnalysisSymbolFamilyDisposition {
    Required,
    Excluded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AnalysisSymbolFamilySurface {
    pub(crate) id: String,
    pub(crate) protocols: Vec<String>,
    pub(crate) source: String,
    pub(crate) symbol_prefix: String,
    pub(crate) disposition: AnalysisSymbolFamilyDisposition,
    pub(crate) profile: Option<String>,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectSpec {
    pub(crate) id: String,
    pub(crate) target_spec: PathBuf,
    pub(crate) ecosystem_packs: Vec<EcosystemPack>,
    pub(crate) chip_pack: Option<ChipPack>,
    /// Investigation-specific compiled analysis logic. When a chip pack also
    /// selects reusable knowledge, composition requires the installed overlay
    /// descriptor to explicitly extend that exact chip provider.
    pub(crate) analysis_provider: Option<String>,
    pub(crate) run_spec: Option<PathBuf>,
    pub(crate) memory_map: Option<PathBuf>,
    pub(crate) svd_paths: Vec<PathBuf>,
    /// Sparse human-reviewed facts. Generated observations and complete
    /// register models are deliberately not stored in these packs.
    pub(crate) reviewed_knowledge: Vec<PathBuf>,
    /// Exact project-local destination for newly accepted sparse facts.
    /// Selection is explicit even when only one pack is configured.
    pub(crate) reviewed_knowledge_default: Option<PathBuf>,
    pub(crate) review_context: open_radio_vendor_review::ApplicabilityContext,
    pub(crate) symbol_inventory: Option<SymbolInventorySpec>,
    pub(crate) navigation_index: Option<NavigationIndexSpec>,
    pub(crate) code: Option<CodeWorkspacePaths>,
    pub(crate) ir_profiles: Vec<ProjectIrProfile>,
    /// Expected and intentionally excluded public symbol families. Undeclared
    /// families are never inferred to be covered or excluded.
    pub(crate) analysis_symbol_families: Vec<AnalysisSymbolFamilySurface>,
    pub(crate) registers: Option<RegisterWorkspacePaths>,
    pub(crate) interfaces: Option<InterfaceWorkspacePaths>,
    pub(crate) functions: Option<FunctionWorkspacePaths>,
    pub(crate) review: Option<ReviewWorkspaceSpec>,
    pub(crate) verification: Option<VerificationWorkspacePaths>,
}

impl ProjectSpec {
    pub(crate) fn apply_to_target(&self, target: &mut crate::TargetSpec) -> Result<()> {
        if let Some(pack) = &self.chip_pack {
            pack.apply_to_target(target)?;
        }
        if let Some(provider) = &self.analysis_provider {
            if let Some(existing) = &target.knowledge_provider {
                let composed = crate::harnesses::compose_provider(existing, provider).map_err(
                    |error| {
                        crate::Error::invalid(format!(
                            "project {:?} cannot compose analysis-provider {provider:?} over chip knowledge-provider {existing:?}: {error}",
                            self.id
                        ))
                    },
                )?;
                target.knowledge_provider = Some(composed.to_owned());
            } else {
                target.knowledge_provider = Some(provider.clone());
            }
        }
        Ok(())
    }

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
