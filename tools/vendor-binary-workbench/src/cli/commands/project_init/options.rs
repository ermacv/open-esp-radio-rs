//! Parsing and validation of the intentionally small project-init surface.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

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

pub(super) fn parse_options(arguments: Vec<String>) -> Result<Options> {
    let mut directory = None;
    let mut id = None;
    let mut ranges = Vec::new();
    let mut sources = Vec::new();
    let mut rust_target = None;
    let mut pac_crate_name = None;
    let mut import_svd = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--directory" => set_once(
                &mut directory,
                PathBuf::from(take_value(&mut arguments, "--directory")?),
                "--directory",
            )?,
            "--id" => set_once(&mut id, take_value(&mut arguments, "--id")?, "--id")?,
            "--mmio" => ranges.push(parse_range(&take_value(&mut arguments, "--mmio")?)?),
            "--source" => {
                let source = take_value(&mut arguments, "--source")?;
                validate_source_id(&source)?;
                sources.push(source);
            }
            "--rust-target" => set_once(
                &mut rust_target,
                take_value(&mut arguments, "--rust-target")?,
                "--rust-target",
            )?,
            "--pac-crate-name" => set_once(
                &mut pac_crate_name,
                take_value(&mut arguments, "--pac-crate-name")?,
                "--pac-crate-name",
            )?,
            "--import-svd" => set_once(
                &mut import_svd,
                PathBuf::from(take_value(&mut arguments, "--import-svd")?),
                "--import-svd",
            )?,
            _ => return Err(format!("unknown project init option: {argument}").into()),
        }
    }

    let directory = directory.ok_or("project init requires --directory PATH")?;
    validate_directory(&directory)?;
    let id = id.ok_or("project init requires --id ID")?;
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

fn take_value(arguments: &mut impl Iterator<Item = String>, option: &str) -> Result<String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value").into())
}

fn set_once<T>(slot: &mut Option<T>, value: T, option: &str) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(format!("duplicate {option}").into());
    }
    Ok(())
}
