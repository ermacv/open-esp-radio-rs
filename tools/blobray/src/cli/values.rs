//! Reusable value grammar shared by clap arguments and run-spec resolution.

use std::{path::PathBuf, str::FromStr};

use crate::{parse_u32, run_spec::InputRole, source_id::SourceId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectInputBinding {
    pub(crate) role: InputRole,
    pub(crate) path: PathBuf,
}

impl FromStr for ProjectInputBinding {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (role, path) = split_assignment(value, "ROLE=PATH")?;
        let role =
            InputRole::parse(role).ok_or_else(|| format!("unsupported input role {role:?}"))?;
        Ok(Self {
            role,
            path: PathBuf::from(path),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourcePath {
    pub(crate) source: SourceId,
    pub(crate) path: PathBuf,
}

impl SourcePath {
    pub(crate) fn new(source: SourceId, path: PathBuf) -> Result<Self, String> {
        if path.as_os_str().is_empty() {
            return Err("source path must not be empty".to_owned());
        }
        Ok(Self { source, path })
    }
}

impl FromStr for SourcePath {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (source, path) = split_assignment(value, "SOURCE=PATH")?;
        Self::new(source.parse()?, PathBuf::from(path))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RevisionPath {
    pub(crate) label: String,
    pub(crate) path: PathBuf,
}

impl FromStr for RevisionPath {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (label, path) = split_assignment(value, "LABEL=PATH")?;
        if !stable_id(label) {
            return Err(format!("invalid revision label {label:?}"));
        }
        Ok(Self {
            label: label.to_owned(),
            path: PathBuf::from(path),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceValue {
    pub(crate) source: SourceId,
    pub(crate) value: String,
}

impl FromStr for SourceValue {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (source, value) = split_assignment(value, "SOURCE=VALUE")?;
        Ok(Self {
            source: source.parse()?,
            value: value.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NamedAddressRange {
    pub(crate) name: String,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl FromStr for NamedAddressRange {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (name, bounds) = split_assignment(value, "NAME=START..END")?;
        if !stable_id(name) {
            return Err(format!("invalid range name {name:?}"));
        }
        let (start, end) = bounds
            .split_once("..")
            .filter(|(start, end)| !start.is_empty() && !end.is_empty())
            .ok_or_else(|| "expected a half-open START..END interval".to_owned())?;
        let start = parse_u32(start).ok_or_else(|| format!("invalid range start {start:?}"))?;
        let end = parse_u32(end).ok_or_else(|| format!("invalid range end {end:?}"))?;
        if start >= end {
            return Err("range start must be less than its exclusive end".to_owned());
        }
        Ok(Self {
            name: name.to_owned(),
            start,
            end,
        })
    }
}

fn split_assignment<'a>(value: &'a str, expected: &str) -> Result<(&'a str, &'a str), String> {
    value
        .split_once('=')
        .filter(|(name, value)| !name.is_empty() && !value.is_empty())
        .ok_or_else(|| format!("expected {expected}"))
}

fn stable_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_paths_keep_equals_signs_after_the_first_separator() {
        let value = "libpp=/tmp/vendor=linked.elf"
            .parse::<SourcePath>()
            .unwrap();
        assert_eq!(value.source.as_str(), "libpp");
        assert_eq!(value.path, PathBuf::from("/tmp/vendor=linked.elf"));
    }

    #[test]
    fn project_input_bindings_use_typed_run_roles() {
        let binding = "source-inventory:archive=/tmp/libphy.a"
            .parse::<ProjectInputBinding>()
            .unwrap();
        assert_eq!(binding.role.to_string(), "source-inventory:archive");
        assert_eq!(binding.path, PathBuf::from("/tmp/libphy.a"));
        assert!("unknown=/tmp/input".parse::<ProjectInputBinding>().is_err());
    }

    #[test]
    fn source_paths_use_the_project_source_id_grammar() {
        assert!("rom=rom.elf".parse::<SourcePath>().is_ok());
        assert!("ROM=rom.elf".parse::<SourcePath>().is_err());
        assert!("wifi.rom=rom.elf".parse::<SourcePath>().is_err());
        assert!("rom=".parse::<SourcePath>().is_err());
    }

    #[test]
    fn revision_paths_accept_commit_hash_labels() {
        let revision = "5e37d4d=/tmp/libbt.a".parse::<RevisionPath>().unwrap();
        assert_eq!(revision.label, "5e37d4d");
        assert_eq!(revision.path, PathBuf::from("/tmp/libbt.a"));
        assert!("bad/label=/tmp/libbt.a".parse::<RevisionPath>().is_err());
        assert!("current=".parse::<RevisionPath>().is_err());
    }

    #[test]
    fn named_ranges_parse_hexadecimal_half_open_bounds() {
        assert_eq!(
            "radio=0x20100000..0x20110000"
                .parse::<NamedAddressRange>()
                .unwrap(),
            NamedAddressRange {
                name: "radio".to_owned(),
                start: 0x2010_0000,
                end: 0x2011_0000,
            }
        );
        assert!("radio=3..3".parse::<NamedAddressRange>().is_err());
        assert!("radio=4..3".parse::<NamedAddressRange>().is_err());
    }
}
