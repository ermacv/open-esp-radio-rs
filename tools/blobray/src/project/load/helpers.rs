//! Shared strict TOML scalar, array and path decoders.

use std::path::{Path, PathBuf};

use toml_edit::Table;

use crate::{Result, project::ProjectSource};

pub(super) fn reject_unknown_keys(
    table: &Table,
    allowed: &[&str],
    context: &str,
    source: ProjectSource<'_>,
) -> Result<()> {
    if let Some((key, item)) = table.iter().find(|(key, _)| !allowed.contains(key)) {
        return Err(source.item(Some(item), format!("unknown {context} key {key:?}")));
    }
    Ok(())
}

pub(super) fn parse_rust_input_role(
    table: &Table,
    key: &str,
    context: &str,
    source: ProjectSource<'_>,
    optional: bool,
) -> Result<Option<crate::run_spec::InputRole>> {
    let value = if optional {
        optional_table_string(table, key, context, source)?
    } else {
        Some(table_string(table, key, context, source)?)
    };
    value
        .map(|value| {
            let role = crate::run_spec::InputRole::parse(&value).ok_or_else(|| {
                source.table_key(
                    table,
                    key,
                    format!("{context}.{key} has invalid role {value:?}"),
                )
            })?;
            let valid = if key == "rust-artifact-role" {
                matches!(
                    role,
                    crate::run_spec::InputRole::RustArtifact
                        | crate::run_spec::InputRole::NamedRustArtifact(_)
                )
            } else {
                matches!(
                    role,
                    crate::run_spec::InputRole::RustCompanion
                        | crate::run_spec::InputRole::NamedRustCompanion(_)
                )
            };
            if !valid {
                return Err(source.table_key(
                    table,
                    key,
                    format!(
                        "{context}.{key} must name a Rust {kind} role",
                        kind = if key == "rust-artifact-role" {
                            "artifact"
                        } else {
                            "companion"
                        }
                    ),
                ));
            }
            Ok(role)
        })
        .transpose()
}

pub(super) fn table_string_array(
    table: &Table,
    key: &str,
    context: &str,
    source: ProjectSource<'_>,
    allow_empty: bool,
) -> Result<Vec<String>> {
    let item = table
        .get(key)
        .ok_or_else(|| source.table_key(table, key, format!("{context} requires {key:?}")))?;
    let array = item
        .as_array()
        .ok_or_else(|| source.item(Some(item), format!("{context}.{key} must be an array")))?;
    if !allow_empty && array.is_empty() {
        return Err(source.item(Some(item), format!("{context}.{key} must not be empty")));
    }
    let mut unique = std::collections::BTreeSet::new();
    array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let span = value.span();
            let value = value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    source.error(
                        span.clone(),
                        format!("{context}.{key}[{index}] must be a non-empty string"),
                    )
                })?;
            if !unique.insert(value) {
                return Err(
                    source.error(span, format!("duplicate {context}.{key} value {value:?}"))
                );
            }
            Ok(value.to_owned())
        })
        .collect()
}

pub(super) fn table_path_array(
    table: &Table,
    key: &str,
    context: &str,
    base: &Path,
    source: ProjectSource<'_>,
    allow_empty: bool,
) -> Result<Vec<PathBuf>> {
    table_string_array(table, key, context, source, allow_empty).map(|values| {
        values
            .into_iter()
            .map(|value| resolve_path(base, &value))
            .collect()
    })
}

pub(super) fn nested_path_array(
    table: &toml_edit::Table,
    base: &Path,
    table_name: &str,
    key: &str,
    source: ProjectSource<'_>,
) -> Result<Vec<PathBuf>> {
    let Some(item) = table.get(table_name) else {
        return Ok(Vec::new());
    };
    let table = item.as_table().ok_or_else(|| {
        source.item(
            Some(item),
            format!("project registers.{table_name} must be a table"),
        )
    })?;
    let Some(item) = table.get(key) else {
        return Ok(Vec::new());
    };
    let values = item.as_array().ok_or_else(|| {
        source.item(
            Some(item),
            format!("project registers.{table_name}.{key} must be an array"),
        )
    })?;
    let mut seen = std::collections::BTreeSet::new();
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let value = value.as_str().ok_or_else(|| {
                source.error(
                    value.span(),
                    format!("project registers.{table_name}.{key}[{index}] must be a string"),
                )
            })?;
            let path = resolve_path(base, value);
            if !seen.insert(path.clone()) {
                return Err(source.error(
                    values.get(index).and_then(toml_edit::Value::span),
                    format!("duplicate project registers.{table_name}.{key} path {value:?}"),
                ));
            }
            Ok(path)
        })
        .collect()
}

pub(super) fn nested_string_array(
    table: &toml_edit::Table,
    table_name: &str,
    key: &str,
    source: ProjectSource<'_>,
) -> Result<Vec<String>> {
    let Some(item) = table.get(table_name) else {
        return Ok(Vec::new());
    };
    let table = item.as_table().ok_or_else(|| {
        source.item(
            Some(item),
            format!("project registers.{table_name} must be a table"),
        )
    })?;
    let Some(item) = table.get(key) else {
        return Ok(Vec::new());
    };
    let values = item.as_array().ok_or_else(|| {
        source.item(
            Some(item),
            format!("project registers.{table_name}.{key} must be an array"),
        )
    })?;
    let mut seen = std::collections::BTreeSet::new();
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let span = value.span();
            let value = value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    source.error(
                        span.clone(),
                        format!(
                            "project registers.{table_name}.{key}[{index}] must be a non-empty string"
                        ),
                    )
                })?;
            if !seen.insert(value) {
                return Err(source.error(
                    span,
                    format!(
                        "duplicate project registers.{table_name}.{key} function {value:?}"
                    ),
                ));
            }
            Ok(value.to_owned())
        })
        .collect()
}

pub(super) fn required_string(
    document: &Table,
    key: &str,
    source: ProjectSource<'_>,
) -> Result<String> {
    optional_string(document, key, source)?.ok_or_else(|| {
        source.item(
            document.get(key),
            format!("project manifest requires string {key:?}"),
        )
    })
}

pub(super) fn optional_string(
    document: &Table,
    key: &str,
    source: ProjectSource<'_>,
) -> Result<Option<String>> {
    match document.get(key) {
        None => Ok(None),
        Some(item) => item
            .as_str()
            .filter(|value| !value.is_empty())
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| {
                source.item(
                    Some(item),
                    format!("project manifest {key:?} must be a non-empty string"),
                )
            }),
    }
}

pub(super) fn table_string(
    table: &toml_edit::Table,
    key: &str,
    context: &str,
    source: ProjectSource<'_>,
) -> Result<String> {
    optional_table_string(table, key, context, source)?
        .ok_or_else(|| source.table_key(table, key, format!("{context} requires string {key:?}")))
}

pub(super) fn optional_table_string(
    table: &toml_edit::Table,
    key: &str,
    context: &str,
    source: ProjectSource<'_>,
) -> Result<Option<String>> {
    match table.get(key) {
        None => Ok(None),
        Some(item) => item
            .as_str()
            .filter(|value| !value.is_empty())
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| {
                source.item(
                    Some(item),
                    format!("{context}.{key} must be a non-empty string"),
                )
            }),
    }
}

pub(super) fn resolve_path(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

pub(super) fn validate_id(value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid project id {value:?}"));
    }
    Ok(())
}
