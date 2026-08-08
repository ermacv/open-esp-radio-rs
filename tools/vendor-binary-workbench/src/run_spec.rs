//! Caller-owned artifact bindings for one workbench invocation.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

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

const PATH_ROLES: &[&str] = &[
    "artifact",
    "companion",
    "vendor-artifact",
    "vendor-inventory",
    "vendor-companion",
    "rust-artifact",
    "rust-companion",
];

fn is_source_path_role(role: &str) -> bool {
    let Some((kind, source)) = role.split_once(':') else {
        return false;
    };
    matches!(
        kind,
        "source-artifact" | "source-inventory" | "source-companion"
    ) && crate::source_id::is_source_id(source)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunSpec {
    inputs: Vec<(String, PathBuf)>,
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
                    let (role, input_path) = split_value(value, path, &input, index)?;
                    if !PATH_ROLES.contains(&role) && !is_source_path_role(role) {
                        return Err(invalid(
                            path,
                            &input,
                            index,
                            format!("unsupported input role {role:?}"),
                        ));
                    }
                    if role != "companion" && !unique_roles.insert(role.to_owned()) {
                        return Err(invalid(
                            path,
                            &input,
                            index,
                            format!("duplicate input role {role:?}"),
                        ));
                    }
                    let input_path = Path::new(input_path);
                    inputs.push((
                        role.to_owned(),
                        if input_path.is_absolute() {
                            input_path.to_owned()
                        } else {
                            base.join(input_path)
                        },
                    ));
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

    pub(crate) fn inputs(&self) -> &[(String, PathBuf)] {
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
        assert_eq!(run.inputs[0].0, "source-artifact:rom");
        assert!(run.inputs[0].1.ends_with("inputs/rom.elf"));
        assert_eq!(run.inputs[1].0, "rust-artifact");
        assert_eq!(run.inputs[1].1, PathBuf::from("/tmp/probes.elf"));
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
        assert_eq!(run.inputs[0].0, "source-artifact:libpp");
        assert!(run.inputs[0].1.ends_with("inputs/libpp.elf"));
        assert_eq!(run.inputs[1].0, "source-inventory:libpp");
        assert!(run.inputs[1].1.ends_with("inputs/libpp.a"));
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
        assert_eq!(run.inputs[0].0, "source-artifact:rom");
        assert_eq!(run.inputs[1].0, "source-inventory:rom");
        assert_eq!(run.inputs[2].0, "rust-artifact");
    }
}
