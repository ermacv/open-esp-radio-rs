//! Caller-owned artifact bindings for one blobray invocation.

use std::{
    collections::BTreeSet,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use miette::{Diagnostic, NamedSource, SourceSpan};
use serde::Deserialize;
use thiserror::Error;

use crate::source_id::SourceId;

#[derive(Debug, Error, Diagnostic)]
pub(crate) enum RunSpecError {
    #[error("cannot read run spec {}", path.display())]
    #[diagnostic(code(blobray::run_spec::read))]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{message}")]
    #[diagnostic(code(blobray::run_spec::invalid))]
    Invalid {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("invalid run-spec input")]
        span: SourceSpan,
    },
}

type Result<T> = std::result::Result<T, RunSpecError>;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum InputRole {
    Artifact,
    Companion,
    VendorArtifact,
    VendorInventory,
    VendorCompanion,
    RustArtifact,
    RustCompanion,
    NamedRustArtifact(SourceId),
    NamedRustCompanion(SourceId),
    SourceArtifact(SourceId),
    SourceInventory(SourceId),
    SourceCompanion(SourceId),
}

impl InputRole {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "artifact" => Some(Self::Artifact),
            "companion" => Some(Self::Companion),
            "vendor-artifact" => Some(Self::VendorArtifact),
            "vendor-inventory" => Some(Self::VendorInventory),
            "vendor-companion" => Some(Self::VendorCompanion),
            "rust-artifact" => Some(Self::RustArtifact),
            "rust-companion" => Some(Self::RustCompanion),
            _ => {
                let (kind, source) = value.split_once(':')?;
                let source = source.parse().ok()?;
                match kind {
                    "source-artifact" => Some(Self::SourceArtifact(source)),
                    "source-inventory" => Some(Self::SourceInventory(source)),
                    "source-companion" => Some(Self::SourceCompanion(source)),
                    "rust-artifact" => Some(Self::NamedRustArtifact(source)),
                    "rust-companion" => Some(Self::NamedRustCompanion(source)),
                    _ => None,
                }
            }
        }
    }

    pub(crate) const fn is_scannable(&self) -> bool {
        matches!(
            self,
            Self::Artifact
                | Self::VendorArtifact
                | Self::VendorInventory
                | Self::RustArtifact
                | Self::NamedRustArtifact(_)
                | Self::SourceArtifact(_)
                | Self::SourceInventory(_)
        )
    }

    /// Inputs whose bytes define a vendor revision rather than the local
    /// verification implementation used to compare against it.
    pub(crate) const fn is_revision_owned(&self) -> bool {
        matches!(
            self,
            Self::VendorArtifact
                | Self::VendorInventory
                | Self::VendorCompanion
                | Self::SourceArtifact(_)
                | Self::SourceInventory(_)
                | Self::SourceCompanion(_)
        )
    }

    pub(crate) const fn is_rust_lineage(&self) -> bool {
        matches!(
            self,
            Self::RustArtifact
                | Self::RustCompanion
                | Self::NamedRustArtifact(_)
                | Self::NamedRustCompanion(_)
        )
    }

    pub(crate) const fn is_revision_companion(&self) -> bool {
        matches!(self, Self::VendorCompanion | Self::SourceCompanion(_))
    }

    pub(crate) const fn is_revision_primary(&self) -> bool {
        matches!(self, Self::VendorArtifact | Self::SourceArtifact(_))
    }

    pub(crate) const fn is_revision_inventory(&self) -> bool {
        matches!(self, Self::VendorInventory | Self::SourceInventory(_))
    }

    pub(crate) const fn is_ambiguous_lineage(&self) -> bool {
        matches!(self, Self::Artifact | Self::Companion)
    }

    pub(crate) fn source_id(&self) -> &str {
        match self {
            Self::VendorArtifact | Self::VendorInventory | Self::VendorCompanion => "vendor",
            Self::RustArtifact | Self::RustCompanion => "rust",
            Self::NamedRustArtifact(source) | Self::NamedRustCompanion(source) => source.as_str(),
            Self::SourceArtifact(source)
            | Self::SourceInventory(source)
            | Self::SourceCompanion(source) => source.as_str(),
            Self::Artifact => "artifact",
            Self::Companion => "companion",
        }
    }

    pub(crate) fn qualified_source_id(&self) -> Option<&str> {
        match self {
            Self::SourceArtifact(source)
            | Self::SourceInventory(source)
            | Self::SourceCompanion(source) => Some(source.as_str()),
            _ => None,
        }
    }

    pub(crate) const fn expects_archive(&self) -> bool {
        matches!(self, Self::VendorInventory | Self::SourceInventory(_))
    }
}

impl fmt::Display for InputRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Artifact => formatter.write_str("artifact"),
            Self::Companion => formatter.write_str("companion"),
            Self::VendorArtifact => formatter.write_str("vendor-artifact"),
            Self::VendorInventory => formatter.write_str("vendor-inventory"),
            Self::VendorCompanion => formatter.write_str("vendor-companion"),
            Self::RustArtifact => formatter.write_str("rust-artifact"),
            Self::RustCompanion => formatter.write_str("rust-companion"),
            Self::NamedRustArtifact(source) => write!(formatter, "rust-artifact:{source}"),
            Self::NamedRustCompanion(source) => write!(formatter, "rust-companion:{source}"),
            Self::SourceArtifact(source) => write!(formatter, "source-artifact:{source}"),
            Self::SourceInventory(source) => write!(formatter, "source-inventory:{source}"),
            Self::SourceCompanion(source) => write!(formatter, "source-companion:{source}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunInput {
    pub(crate) role: InputRole,
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunSpec {
    inputs: Vec<RunInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunDocument {
    schema: u32,
    inputs: Vec<RunInputDocument>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunInputDocument {
    role: String,
    path: PathBuf,
}

impl RunSpec {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|source| RunSpecError::Read {
            path: path.to_owned(),
            source,
        })?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let document: RunDocument = toml_edit::de::from_str(&input).map_err(|error| {
            let span = error.span().unwrap_or(0..input.len().min(1));
            RunSpecError::Invalid {
                message: format!("invalid run TOML: {error}"),
                src: NamedSource::new(path.display().to_string(), input.clone()),
                span: (span.start, span.len().max(1)).into(),
            }
        })?;
        if document.schema != 1 {
            return Err(invalid(path, &input, "run TOML requires schema = 1"));
        }
        let mut inputs = Vec::new();
        let mut unique_roles = BTreeSet::new();
        for entry in document.inputs {
            let Some(role) = InputRole::parse(&entry.role) else {
                return Err(invalid(
                    path,
                    &input,
                    format!("unsupported input role {:?}", entry.role),
                ));
            };
            let repeatable = role == InputRole::Companion
                || matches!(
                    role,
                    InputRole::SourceInventory(_) | InputRole::SourceCompanion(_)
                );
            if !repeatable && !unique_roles.insert(role.clone()) {
                return Err(invalid(
                    path,
                    &input,
                    format!("duplicate input role {:?}", entry.role),
                ));
            }
            inputs.push(RunInput {
                role,
                path: if entry.path.is_absolute() {
                    entry.path
                } else {
                    base.join(entry.path)
                },
            });
        }
        if inputs.is_empty() {
            return Err(invalid(
                path,
                &input,
                "run TOML requires at least one input",
            ));
        }
        Ok(Self { inputs })
    }

    pub(crate) fn inputs(&self) -> &[RunInput] {
        &self.inputs
    }
}

fn invalid(path: &Path, input: &str, message: impl Into<String>) -> RunSpecError {
    RunSpecError::Invalid {
        message: message.into(),
        src: NamedSource::new(path.display().to_string(), input.to_owned()),
        span: (0, 1).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_inputs_are_resolved_relative_to_the_run_spec() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-blobray-run-spec-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.toml");
        std::fs::write(
            &path,
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:rom\"\npath = \"inputs/rom.elf\"\n\n[[inputs]]\nrole = \"rust-artifact\"\npath = \"/tmp/probes.elf\"\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(run.inputs[0].role.to_string(), "source-artifact:rom");
        assert!(run.inputs[0].path.ends_with("inputs/rom.elf"));
        assert_eq!(run.inputs[1].role, InputRole::RustArtifact);
        assert_eq!(run.inputs[1].path, PathBuf::from("/tmp/probes.elf"));
    }

    #[test]
    fn arbitrary_source_roles_become_source_qualified_options() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-blobray-source-run-spec-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.toml");
        std::fs::write(
            &path,
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:libpp\"\npath = \"inputs/libpp.elf\"\n\n[[inputs]]\nrole = \"source-inventory:libpp\"\npath = \"inputs/libpp.a\"\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(run.inputs[0].role.to_string(), "source-artifact:libpp");
        assert!(run.inputs[0].path.ends_with("inputs/libpp.elf"));
        assert_eq!(run.inputs[1].role.to_string(), "source-inventory:libpp");
        assert!(run.inputs[1].path.ends_with("inputs/libpp.a"));
    }

    #[test]
    fn one_link_unit_accepts_multiple_ordered_origin_archives() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-blobray-multiple-inventories-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.toml");
        std::fs::write(
            &path,
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:wifi\"\npath = \"wifi.elf\"\n\n[[inputs]]\nrole = \"source-inventory:wifi\"\npath = \"libnet80211.a\"\n\n[[inputs]]\nrole = \"source-inventory:wifi\"\npath = \"libpp.a\"\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        let inventories = run
            .inputs()
            .iter()
            .filter(|input| matches!(input.role, InputRole::SourceInventory(_)))
            .map(|input| input.path.file_name().unwrap().to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(inventories, ["libnet80211.a", "libpp.a"]);
    }

    #[test]
    fn one_source_accepts_multiple_code_companions() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-blobray-multiple-source-companions-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.toml");
        std::fs::write(
            &path,
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:wifi\"\npath = \"wifi.elf\"\n\n[[inputs]]\nrole = \"source-companion:wifi\"\npath = \"rom.elf\"\n\n[[inputs]]\nrole = \"source-companion:wifi\"\npath = \"libphy.a\"\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        std::fs::remove_dir_all(directory).unwrap();

        assert_eq!(
            run.inputs
                .iter()
                .filter(|input| matches!(input.role, InputRole::SourceCompanion(_)))
                .count(),
            2
        );
    }

    #[test]
    fn named_rust_artifacts_are_independent_suite_bindings() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-blobray-named-rust-run-spec-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.toml");
        std::fs::write(
            &path,
            "schema = 1\n\n[[inputs]]\nrole = \"rust-artifact:phy\"\npath = \"phy.elf\"\n\n[[inputs]]\nrole = \"rust-companion:phy\"\npath = \"support.elf\"\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(run.inputs[0].role.to_string(), "rust-artifact:phy");
        assert_eq!(run.inputs[1].role.to_string(), "rust-companion:phy");
        assert_ne!(run.inputs[0].role, InputRole::RustArtifact);
    }

    #[test]
    fn input_roles_remain_typed_data_for_the_command_resolver() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-blobray-filtered-run-spec-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.toml");
        std::fs::write(
            &path,
            "schema = 1\n\n[[inputs]]\nrole = \"source-artifact:rom\"\npath = \"rom.elf\"\n\n[[inputs]]\nrole = \"source-inventory:rom\"\npath = \"rom.a\"\n\n[[inputs]]\nrole = \"rust-artifact\"\npath = \"probes.elf\"\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(run.inputs.len(), 3);
        assert_eq!(run.inputs[0].role.to_string(), "source-artifact:rom");
        assert_eq!(run.inputs[1].role.to_string(), "source-inventory:rom");
        assert_eq!(run.inputs[2].role, InputRole::RustArtifact);
        assert!(run.inputs[0].role.is_revision_owned());
        assert!(run.inputs[1].role.is_revision_owned());
        assert!(!run.inputs[2].role.is_revision_owned());
        assert!(run.inputs[2].role.is_rust_lineage());
    }
}
