//! Caller-owned artifact bindings for one validator invocation.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::Result;

const PATH_ROLES: &[&str] = &[
    "artifact",
    "companion",
    "vendor-artifact",
    "vendor-inventory",
    "vendor-companion",
    "rom-artifact",
    "rom-companion",
    "archive-artifact",
    "archive-inventory",
    "archive-companion",
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
        let input = fs::read_to_string(path)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut schema = None;
        let mut inputs = Vec::new();
        let mut unique_roles = BTreeSet::new();

        for (index, raw_line) in input.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let (directive, value) = split_value(line, line_number)?;
            match directive {
                "schema" => {
                    if schema.replace(value.parse::<u32>()?).is_some() {
                        return Err(format!("duplicate schema at line {line_number}").into());
                    }
                }
                "input" => {
                    let (role, path) = split_value(value, line_number)?;
                    if !PATH_ROLES.contains(&role) && !is_source_path_role(role) {
                        return Err(format!(
                            "unsupported input role {role:?} at line {line_number}"
                        )
                        .into());
                    }
                    if role != "companion" && !unique_roles.insert(role.to_owned()) {
                        return Err(
                            format!("duplicate input role {role:?} at line {line_number}").into(),
                        );
                    }
                    let path = Path::new(path);
                    inputs.push((
                        role.to_owned(),
                        if path.is_absolute() {
                            path.to_owned()
                        } else {
                            base.join(path)
                        },
                    ));
                }
                _ => {
                    return Err(format!(
                        "unknown run-spec directive {directive:?} at line {line_number}"
                    )
                    .into());
                }
            }
        }
        if schema != Some(1) {
            return Err("run spec requires schema 1".into());
        }
        if inputs.is_empty() {
            return Err("run spec has no inputs".into());
        }
        Ok(Self { inputs })
    }

    pub(crate) fn append_defaults(
        &self,
        arguments: &mut Vec<String>,
        accepts_role: impl Fn(&str) -> bool,
        is_overridden: impl Fn(&str, &[String]) -> bool,
    ) {
        let overridden = self
            .inputs
            .iter()
            .filter(|(role, _)| accepts_role(role))
            .map(|(role, _)| role)
            .filter(|role| {
                let option = format!("--{role}");
                arguments.iter().any(|argument| argument == &option)
                    || is_overridden(role, arguments)
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        for (role, path) in &self.inputs {
            if !accepts_role(role) {
                continue;
            }
            if overridden.contains(role) {
                continue;
            }
            arguments.push(format!("--{role}"));
            arguments.push(path.display().to_string());
        }
    }

    pub(crate) fn inputs(&self) -> &[(String, PathBuf)] {
        &self.inputs
    }
}

fn split_value(line: &str, line_number: usize) -> Result<(&str, &str)> {
    line.split_once(char::is_whitespace)
        .map(|(key, value)| (key, value.trim()))
        .filter(|(_, value)| !value.is_empty())
        .ok_or_else(|| format!("directive needs a value at line {line_number}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_inputs_are_resolved_relative_to_the_run_spec() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-validator-run-spec-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.run");
        std::fs::write(
            &path,
            "schema 1\ninput rom-artifact inputs/rom.elf\ninput rust-artifact /tmp/probes.elf\n",
        )
        .unwrap();
        let run = RunSpec::load(&path).unwrap();
        let mut arguments = Vec::new();
        run.append_defaults(&mut arguments, |_| true, |_, _| false);
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(arguments[0], "--rom-artifact");
        assert!(arguments[1].ends_with("inputs/rom.elf"));
        assert_eq!(&arguments[2..], ["--rust-artifact", "/tmp/probes.elf"]);
    }

    #[test]
    fn arbitrary_source_roles_become_source_qualified_options() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-validator-source-run-spec-{}",
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
        let mut arguments = Vec::new();
        run.append_defaults(&mut arguments, |_| true, |_, _| false);
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(arguments[0], "--source-artifact:libpp");
        assert!(arguments[1].ends_with("inputs/libpp.elf"));
        assert_eq!(arguments[2], "--source-inventory:libpp");
        assert!(arguments[3].ends_with("inputs/libpp.a"));
    }

    #[test]
    fn command_role_filter_omits_unrelated_project_bindings() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-validator-filtered-run-spec-{}",
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
        let mut arguments = Vec::new();
        run.append_defaults(
            &mut arguments,
            |role| role.starts_with("source-artifact:"),
            |_, _| false,
        );
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(arguments[0], "--source-artifact:rom");
        assert!(arguments[1].ends_with("rom.elf"));
        assert_eq!(arguments.len(), 2);
    }
}
