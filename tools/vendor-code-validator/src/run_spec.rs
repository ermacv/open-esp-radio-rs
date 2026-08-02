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
                    if !PATH_ROLES.contains(&role) {
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

    pub(crate) fn append_defaults(&self, arguments: &mut Vec<String>) {
        let overridden = self
            .inputs
            .iter()
            .map(|(role, _)| role)
            .filter(|role| {
                let option = format!("--{role}");
                arguments.iter().any(|argument| argument == &option)
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        for (role, path) in &self.inputs {
            if overridden.contains(role) {
                continue;
            }
            arguments.push(format!("--{role}"));
            arguments.push(path.display().to_string());
        }
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
        run.append_defaults(&mut arguments);
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(arguments[0], "--rom-artifact");
        assert!(arguments[1].ends_with("inputs/rom.elf"));
        assert_eq!(&arguments[2..], ["--rust-artifact", "/tmp/probes.elf"]);
    }
}
