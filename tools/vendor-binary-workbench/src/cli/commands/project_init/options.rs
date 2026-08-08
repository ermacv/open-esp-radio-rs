//! Parsing and validation of the intentionally small project-init surface.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

use crate::Result;
use crate::cli::{NamedAddressRange, ProjectInitArgs};

pub(super) const DEFAULT_RUST_TARGET: &str = "riscv32imac-unknown-none-elf";

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Options {
    pub(super) directory: PathBuf,
    pub(super) id: String,
    pub(super) ranges: Vec<NamedAddressRange>,
    pub(super) sources: Vec<String>,
    pub(super) rust_target: String,
    pub(super) pac_crate_name: String,
    pub(super) import_svd: Option<PathBuf>,
}

pub(super) fn resolve_options(arguments: ProjectInitArgs) -> Result<Options> {
    let directory = arguments.directory;
    let id = arguments.id;
    let ranges = arguments.mmio;
    let mut sources = arguments
        .source
        .into_iter()
        .map(|source| source.into_string())
        .collect::<Vec<_>>();
    let rust_target = arguments.rust_target;
    let pac_crate_name = arguments.pac_crate_name;
    let import_svd = arguments.import_svd;

    validate_directory(&directory)?;
    validate_stable_id(&id, "project")?;
    if ranges.is_empty() {
        return Err(crate::Error::invalid(
            "project init requires at least one --mmio NAME=START..END",
        ));
    }
    validate_ranges(&ranges)?;
    if sources.is_empty() {
        sources.push("vendor".to_owned());
    }
    let unique_sources = sources.iter().collect::<BTreeSet<_>>();
    if unique_sources.len() != sources.len() {
        return Err(crate::Error::invalid(
            "project init source IDs must be unique",
        ));
    }
    let rust_target = rust_target.unwrap_or_else(|| DEFAULT_RUST_TARGET.to_owned());
    validate_token(&rust_target, "Rust target")?;
    let pac_crate_name = pac_crate_name.unwrap_or_else(|| default_pac_crate_name(&id));
    open_esp_radio_register_model::validate_pac_crate_name(&pac_crate_name)?;
    if import_svd.as_ref().is_some_and(|path| !path.is_file()) {
        return Err(crate::Error::invalid(format!(
            "project init SVD input does not exist: {}",
            import_svd.as_ref().unwrap().display()
        )));
    }

    Ok(Options {
        directory,
        id,
        ranges,
        sources,
        rust_target,
        pac_crate_name,
        import_svd,
    })
}

fn validate_ranges(ranges: &[NamedAddressRange]) -> Result<()> {
    let mut names = BTreeSet::new();
    for range in ranges {
        if !names.insert(&range.name) {
            return Err(crate::Error::invalid(format!(
                "duplicate MMIO range name {:?}",
                range.name
            )));
        }
    }
    let mut sorted = ranges.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|range| (range.start, range.end));
    for pair in sorted.windows(2) {
        if pair[1].start < pair[0].end {
            return Err(crate::Error::invalid(format!(
                "MMIO ranges {:?} and {:?} overlap",
                pair[0].name, pair[1].name
            )));
        }
    }
    Ok(())
}

fn validate_directory(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(crate::Error::invalid(
            "project directory must be a non-empty path without '..'",
        ));
    }
    Ok(())
}

fn validate_stable_id(value: &str, kind: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(crate::Error::invalid(format!(
            "invalid {kind} id {value:?}"
        )));
    }
    Ok(())
}

fn validate_token(value: &str, kind: &str) -> Result<()> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(crate::Error::invalid(format!(
            "{kind} must be one non-empty token"
        )));
    }
    Ok(())
}

fn default_pac_crate_name(id: &str) -> String {
    let mut name = id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if name.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        name.insert_str(0, "project_");
    }
    name.push_str("_pac");
    name
}
