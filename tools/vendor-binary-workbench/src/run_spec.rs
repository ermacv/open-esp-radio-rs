//! Caller-owned artifact bindings for one workbench invocation.

use std::{
    collections::BTreeSet,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::source_id::SourceId;

#[derive(Debug, Error, Diagnostic)]
pub(crate) enum RunSpecError {
    #[error("cannot read run spec {}", path.display())]
    #[diagnostic(code(workbench::run_spec::read))]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{message}")]
    #[diagnostic(code(workbench::run_spec::invalid))]
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
                | Self::SourceArtifact(_)
                | Self::SourceInventory(_)
        )
    }

    pub(crate) fn source_id(&self) -> &str {
        match self {
            Self::VendorArtifact | Self::VendorInventory | Self::VendorCompanion => "vendor",
            Self::RustArtifact | Self::RustCompanion => "rust",
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

impl RunSpec {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|source| RunSpecError::Read {
            path: path.to_owned(),
            source,
        })?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut schema = None;
        let mut inputs = Vec::new();
        let mut unique_roles = BTreeSet::new();

        for (index, raw_line) in input.lines().enumerate() {
            let line = raw_line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let (directive, value) = split_value(line, path, &input, index)?;
            match directive {
                "schema" => {
                    let parsed = value.parse::<u32>().map_err(|_| {
                        invalid(path, &input, index, "schema must be an unsigned integer")
                    })?;
                    if schema.replace(parsed).is_some() {
                        return Err(invalid(path, &input, index, "duplicate schema directive"));
                    }
                }
                "input" => {
                    let (role_text, input_path) = split_value(value, path, &input, index)?;
                    let Some(role) = InputRole::parse(role_text) else {
                        return Err(invalid(
                            path,
                            &input,
                            index,
                            format!("unsupported input role {role_text:?}"),
                        ));
                    };
                    if role != InputRole::Companion && !unique_roles.insert(role.clone()) {
                        return Err(invalid(
                            path,
                            &input,
                            index,
                            format!("duplicate input role {role_text:?}"),
                        ));
                    }
                    let input_path = Path::new(input_path);
                    inputs.push(RunInput {
                        role,
                        path: if input_path.is_absolute() {
                            input_path.to_owned()
                        } else {
                            base.join(input_path)
                        },
                    });
                }
                _ => {
                    return Err(invalid(
                        path,
                        &input,
                        index,
                        format!("unknown run-spec directive {directive:?}"),
                    ));
                }
            }
        }
        if schema != Some(1) {
            return Err(invalid(path, &input, 0, "run spec requires schema 1"));
        }
        if inputs.is_empty() {
            return Err(invalid(
                path,
                &input,
                0,
                "run spec requires at least one input",
            ));
        }
        Ok(Self { inputs })
    }

    pub(crate) fn inputs(&self) -> &[RunInput] {
        &self.inputs
    }
}

fn split_value<'a>(
    line: &'a str,
    path: &Path,
    input: &str,
    line_index: usize,
) -> Result<(&'a str, &'a str)> {
    line.split_once(char::is_whitespace)
        .map(|(key, value)| (key, value.trim()))
        .filter(|(_, value)| !value.is_empty())
        .ok_or_else(|| invalid(path, input, line_index, "directive requires a value"))
}

fn invalid(
    path: &Path,
    input: &str,
    line_index: usize,
    message: impl Into<String>,
) -> RunSpecError {
    let offset = input
        .split_inclusive('\n')
        .take(line_index)
        .map(str::len)
        .sum::<usize>();
    let length = input
        .split_inclusive('\n')
        .nth(line_index)
        .map(|line| line.trim_end_matches(['\r', '\n']).len())
        .unwrap_or(0)
        .max(1);
    RunSpecError::Invalid {
        message: message.into(),
        src: NamedSource::new(path.display().to_string(), input.to_owned()),
        span: (offset, length).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_inputs_are_resolved_relative_to_the_run_spec() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-workbench-run-spec-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.run");
        std::fs::write(
            &path,
            "schema 1\ninput source-artifact:rom inputs/rom.elf\ninput rust-artifact /tmp/probes.elf\n",
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
            "open-radio-workbench-source-run-spec-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.run");
        std::fs::write(
            &path,
            "schema 1\ninput source-artifact:libpp inputs/libpp.elf\ninput source-inventory:libpp inputs/libpp.a\n",
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
    fn input_roles_remain_typed_data_for_the_command_resolver() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-workbench-filtered-run-spec-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.run");
        std::fs::write(
            &path,
            "schema 1\ninput source-artifact:rom rom.elf\ninput source-inventory:rom rom.a\ninput rust-artifact probes.elf\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(run.inputs.len(), 3);
        assert_eq!(run.inputs[0].role.to_string(), "source-artifact:rom");
        assert_eq!(run.inputs[1].role.to_string(), "source-inventory:rom");
        assert_eq!(run.inputs[2].role, InputRole::RustArtifact);
    }
}
