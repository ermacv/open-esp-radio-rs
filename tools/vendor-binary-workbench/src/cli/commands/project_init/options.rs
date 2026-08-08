//! Parsing and validation of the intentionally small project-init surface.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

use crate::cli::ProjectInitArgs;
use crate::{Result, parse_u32, source_id::validate_source_id};

pub(super) const DEFAULT_RUST_TARGET: &str = "riscv32imac-unknown-none-elf";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MmioRange {
    pub(super) name: String,
    pub(super) start: u32,
    pub(super) end: u32,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Options {
    pub(super) directory: PathBuf,
    pub(super) id: String,
    pub(super) ranges: Vec<MmioRange>,
    pub(super) sources: Vec<String>,
    pub(super) rust_target: String,
    pub(super) pac_crate_name: String,
    pub(super) import_svd: Option<PathBuf>,
}

pub(super) fn resolve_options(arguments: ProjectInitArgs) -> Result<Options> {
    let directory = arguments.directory;
    let id = arguments.id;
    let mut ranges = Vec::new();
    for range in arguments.mmio {
        ranges.push(parse_range(&range)?);
    }
    let mut sources = arguments.source;
    for source in &sources {
        validate_source_id(source)?;
    }
    let rust_target = arguments.rust_target;
    let pac_crate_name = arguments.pac_crate_name;
    let import_svd = arguments.import_svd;

    validate_directory(&directory)?;
    validate_stable_id(&id, "project")?;
    if ranges.is_empty() {
        return Err("project init requires at least one --mmio NAME=START..END".into());
    }
    validate_ranges(&ranges)?;
    if sources.is_empty() {
        sources.push("vendor".to_owned());
    }
    let unique_sources = sources.iter().collect::<BTreeSet<_>>();
    if unique_sources.len() != sources.len() {
        return Err("project init source IDs must be unique".into());
    }
    let rust_target = rust_target.unwrap_or_else(|| DEFAULT_RUST_TARGET.to_owned());
    validate_token(&rust_target, "Rust target")?;
    let pac_crate_name = pac_crate_name.unwrap_or_else(|| default_pac_crate_name(&id));
    open_esp_radio_register_model::validate_pac_crate_name(&pac_crate_name)?;
    if import_svd.as_ref().is_some_and(|path| !path.is_file()) {
        return Err(format!(
            "project init SVD input does not exist: {}",
            import_svd.as_ref().unwrap().display()
        )
        .into());
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

fn parse_range(value: &str) -> Result<MmioRange> {
    let (name, bounds) = value
        .split_once('=')
        .filter(|(name, bounds)| !name.is_empty() && !bounds.is_empty())
        .ok_or("--mmio requires NAME=START..END")?;
    validate_stable_id(name, "MMIO range")?;
    let (start, end) = bounds
        .split_once("..")
        .filter(|(start, end)| !start.is_empty() && !end.is_empty())
        .ok_or("--mmio requires a half-open START..END interval")?;
    let start = parse_u32(start).ok_or("invalid --mmio start")?;
    let end = parse_u32(end).ok_or("invalid --mmio end")?;
    if start >= end {
        return Err("--mmio start must be less than its exclusive end".into());
    }
    Ok(MmioRange {
        name: name.to_owned(),
        start,
        end,
    })
}

fn validate_ranges(ranges: &[MmioRange]) -> Result<()> {
    let mut names = BTreeSet::new();
    for range in ranges {
        if !names.insert(&range.name) {
            return Err(format!("duplicate MMIO range name {:?}", range.name).into());
        }
    }
    let mut sorted = ranges.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|range| (range.start, range.end));
    for pair in sorted.windows(2) {
        if pair[1].start < pair[0].end {
            return Err(format!(
                "MMIO ranges {:?} and {:?} overlap",
                pair[0].name, pair[1].name
            )
            .into());
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
        return Err("project directory must be a non-empty path without '..'".into());
    }
    Ok(())
}

fn validate_stable_id(value: &str, kind: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid {kind} id {value:?}").into());
    }
    Ok(())
}

fn validate_token(value: &str, kind: &str) -> Result<()> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(format!("{kind} must be one non-empty token").into());
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
