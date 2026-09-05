//! Explicit shared publication scope, without project-context inheritance.

use std::{
    fs,
    path::{Path, PathBuf},
};

use toml_edit::{Item, Table};

use super::{ProjectError, ProjectSource, Result, helpers::*, required_table_string_array};

#[cfg(test)]
mod tests;

pub(super) fn load(
    registers: &Table,
    base: &Path,
    source: ProjectSource<'_>,
) -> Result<(Option<PathBuf>, Vec<String>)> {
    let Some(policy) =
        optional_table_string(registers, "ownership-policy", "project registers", source)?
    else {
        return required_table_string_array(registers, "owned-ranges", "project registers", source)
            .map(|ranges| (None, ranges));
    };
    if registers.contains_key("owned-ranges") {
        return Err(source.table_key(
            registers,
            "owned-ranges",
            "project registers must select either ownership-policy or owned-ranges, not both",
        ));
    }
    let path = resolve_path(base, &policy);
    let contents = fs::read_to_string(&path).map_err(|error| {
        crate::Error::invalid(format!(
            "cannot read register ownership policy {}: {error}",
            path.display(),
        ))
    })?;
    let document = toml_edit::Document::parse(contents.as_str()).map_err(|error| {
        let span = error.span().unwrap_or(0..contents.len().min(1));
        ProjectError::Parse {
            message: error.message().to_owned(),
            src: miette::NamedSource::new(path.display().to_string(), contents.clone()),
            span: (span.start, span.len().max(1)).into(),
        }
    })?;
    let policy_source = ProjectSource::new(&path, &contents);
    reject_unknown_keys(
        &document,
        &["schema", "owned-ranges"],
        "register ownership policy",
        policy_source,
    )?;
    if document.get("schema").and_then(Item::as_integer) != Some(1) {
        return Err(policy_source.item(
            document.get("schema"),
            "register ownership policy requires schema = 1",
        ));
    }
    let ranges = required_table_string_array(
        &document,
        "owned-ranges",
        "register ownership policy",
        policy_source,
    )?;
    Ok((Some(path), ranges))
}
