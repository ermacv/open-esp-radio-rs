//! TOML decoding and path resolution for project manifests.

use std::{
    fs,
    path::{Path, PathBuf},
};

use toml_edit::{DocumentMut, Item};

use crate::{
    Result,
    platform_pack::PlatformPack,
    project_analysis::{load_navigation_index, load_symbol_inventory},
    project_ir::load_ir_profiles,
};

use super::*;

pub(super) fn load(path: &Path) -> Result<ProjectSpec> {
    let input = fs::read_to_string(path).map_err(|source| ProjectError::Read {
        path: path.to_owned(),
        source,
    })?;
    let document = input.parse::<DocumentMut>().map_err(|error| {
        let span = error.span().unwrap_or(0..input.len().min(1));
        ProjectError::Parse {
            message: error.message().to_owned(),
            src: NamedSource::new(path.display().to_string(), input.clone()),
            span: (span.start, span.len().max(1)).into(),
        }
    })?;
    if document.get("schema").and_then(Item::as_integer) != Some(1) {
        return Err(project_invalid(
            path,
            &input,
            document.get("schema"),
            "project manifest requires schema = 1",
        )
        .into());
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let id = required_string(&document, "id").ok_or_else(|| {
        project_invalid(
            path,
            &input,
            document.get("id"),
            "project manifest requires string \"id\"",
        )
    })?;
    validate_id(&id).map_err(|_| {
        project_invalid(
            path,
            &input,
            document.get("id"),
            format!("invalid project id {id:?}"),
        )
    })?;
    let target_spec_value = required_string(&document, "target-spec").ok_or_else(|| {
        project_invalid(
            path,
            &input,
            document.get("target-spec"),
            "project manifest requires string \"target-spec\"",
        )
    })?;
    let target_spec = resolve_path(base, &target_spec_value);
    let platform_pack = optional_string(&document, "platform-pack")
        .map(|path| PlatformPack::load(&resolve_path(base, &path)))
        .transpose()?;
    let run_spec = optional_string(&document, "run-spec").map(|path| resolve_path(base, &path));
    let memory_map = optional_string(&document, "memory-map").map(|path| resolve_path(base, &path));
    let svd_configured = document.get("svd").is_some();
    let svd_paths = document
        .get("svd")
        .map(|item| {
            let array = item
                .as_array()
                .ok_or("project manifest svd must be an array of paths")
                .map_err(crate::Error::invalid)?;
            array
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    value
                        .as_str()
                        .map(|path| resolve_path(base, path))
                        .ok_or_else(|| {
                            crate::Error::invalid(format!(
                                "project manifest svd[{index}] must be a string"
                            ))
                        })
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let ir_profiles = load_ir_profiles(&document, base)?;
    let symbol_inventory = load_symbol_inventory(&document, base, &ir_profiles)?;
    let navigation_index =
        load_navigation_index(&document, base, symbol_inventory.as_ref(), &ir_profiles)?;
    let registers = document
        .get("registers")
        .map(|item| -> Result<RegisterWorkspacePaths> {
            let table = item
                .as_table()
                .ok_or("project manifest registers must be a table").map_err(crate::Error::invalid)?;
            if table.contains_key("overlay") {
                return Err(crate::Error::invalid("unknown project registers key \"overlay\"; use the schema-2 \"model\" workspace"));
            }
            let model = table
                .get("model")
                .and_then(Item::as_str)
                .ok_or("project registers requires string \"model\"").map_err(crate::Error::invalid)?;
            let review_output = nested_output_path(table, base, "review")?;
            let review_ir_reports =
                nested_path_array(table, base, "review", "linked-ir")?;
            let svd_output = nested_output_path(table, base, "svd")?;
            let pac = table
                .get("pac")
                .map(|item| -> Result<PacOutputSpec> {
                    let pac = item
                        .as_table()
                        .ok_or("project registers.pac must be a table").map_err(crate::Error::invalid)?;
                    let target = pac
                        .get("target")
                        .and_then(Item::as_str)
                        .unwrap_or("none")
                        .to_owned();
                    if !matches!(target.as_str(), "none" | "riscv") {
                        return Err(crate::Error::invalid(format!(
                            "project registers.pac target must be \"none\" or \"riscv\", got {target:?}"
                        )
                        ));
                    }
                    let edition = pac
                        .get("edition")
                        .and_then(Item::as_str)
                        .unwrap_or("2024")
                        .to_owned();
                    if !matches!(edition.as_str(), "2021" | "2024") {
                        return Err(crate::Error::invalid(format!(
                            "project registers.pac edition must be \"2021\" or \"2024\", got {edition:?}"
                        )
                        ));
                    }
                    Ok(PacOutputSpec {
                        output: resolve_path(
                            base,
                            pac.get("output").and_then(Item::as_str).ok_or(
                                "project registers.pac requires string \"output\"",
                            ).map_err(crate::Error::invalid)?,
                        ),
                        target,
                        edition,
                    })
                })
                .transpose()?;
            let bindings = table
                .get("bindings")
                .map(|item| -> Result<PacBindingsOutputSpec> {
                    let bindings = item
                        .as_table()
                        .ok_or("project registers.bindings must be a table").map_err(crate::Error::invalid)?;
                    let crate_name = bindings
                        .get("crate-name")
                        .and_then(Item::as_str)
                        .ok_or("project registers.bindings requires string \"crate-name\"").map_err(crate::Error::invalid)?;
                    open_esp_radio_register_model::validate_pac_crate_name(crate_name)?;
                    Ok(PacBindingsOutputSpec {
                        output: resolve_path(
                            base,
                            bindings.get("output").and_then(Item::as_str).ok_or(
                                "project registers.bindings requires string \"output\"",
                            ).map_err(crate::Error::invalid)?,
                        ),
                        crate_name: crate_name.to_owned(),
                    })
                })
                .transpose()?;
            let api_pack = table
                .get("api")
                .map(|item| -> Result<PathBuf> {
                    let api = item
                        .as_table()
                        .ok_or("project registers.api must be a table").map_err(crate::Error::invalid)?;
                    Ok(resolve_path(
                        base,
                        api.get("pack")
                            .and_then(Item::as_str)
                            .ok_or("project registers.api requires string \"pack\"").map_err(crate::Error::invalid)?,
                    ))
                })
                .transpose()?;
            let evidence_catalogs =
                nested_path_array(table, base, "evidence", "catalogs")?;
            let lint_pack = table
                .get("lints")
                .map(|item| -> Result<PathBuf> {
                    let lints = item
                        .as_table()
                        .ok_or("project registers.lints must be a table").map_err(crate::Error::invalid)?;
                    Ok(resolve_path(
                        base,
                        lints
                            .get("pack")
                            .and_then(Item::as_str)
                            .ok_or("project registers.lints requires string \"pack\"").map_err(crate::Error::invalid)?,
                    ))
                })
                .transpose()?;
            Ok(RegisterWorkspacePaths {
                facts: resolve_path(
                    base,
                    table
                        .get("facts")
                        .and_then(Item::as_str)
                        .ok_or("project registers requires string \"facts\"").map_err(crate::Error::invalid)?,
                ),
                model: resolve_path(base, model),
                review_output,
                review_ir_reports,
                svd_output,
                pac,
                bindings,
                api_pack,
                lint_pack,
                evidence_catalogs,
            })
        })
        .transpose()?;
    let interfaces = document
        .get("interfaces")
        .map(|item| -> Result<InterfaceWorkspacePaths> {
            let table = item
                .as_table()
                .ok_or("project manifest interfaces must be a table").map_err(crate::Error::invalid)?;
            if table.contains_key("semantic-catalogs") {
                return Err(crate::Error::invalid("unknown project interfaces key \"semantic-catalogs\"; semantic catalogs belong to the platform pack"));
            }
            let semantic_catalogs = platform_pack
                .as_ref()
                .map(|pack| pack.semantic_catalogs.clone())
                .unwrap_or_default();
            Ok(InterfaceWorkspacePaths {
                facts: resolve_path(
                    base,
                    table
                        .get("facts")
                        .and_then(Item::as_str)
                        .ok_or("project interfaces requires string \"facts\"").map_err(crate::Error::invalid)?,
                ),
                pack: table
                    .get("pack")
                    .and_then(Item::as_str)
                    .map(|path| resolve_path(base, path)),
                semantic_catalogs,
            })
        })
        .transpose()?;
    let functions = document
        .get("functions")
        .map(|item| -> Result<FunctionWorkspacePaths> {
            let table = item
                .as_table()
                .ok_or("project manifest functions must be a table")
                .map_err(crate::Error::invalid)?;
            let profiles = table
                .get("profiles")
                .map(|item| {
                    let values = item
                        .as_array()
                        .ok_or("project functions.profiles must be an array")
                        .map_err(crate::Error::invalid)?;
                    values
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            value.as_str().map(str::to_owned).ok_or_else(|| {
                                crate::Error::invalid(format!(
                                    "project functions.profiles[{index}] must be a string"
                                ))
                            })
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_else(|| {
                    ir_profiles
                        .iter()
                        .map(|profile| profile.id.clone())
                        .collect()
                });
            if profiles.is_empty() {
                return Err(crate::Error::invalid(
                    "project [functions] requires at least one [[analysis.ir]] profile",
                ));
            }
            let mut seen = std::collections::BTreeSet::new();
            for profile in &profiles {
                if !seen.insert(profile) {
                    return Err(crate::Error::invalid(format!(
                        "duplicate project functions profile {profile:?}"
                    )));
                }
                if !ir_profiles.iter().any(|candidate| candidate.id == *profile) {
                    return Err(crate::Error::invalid(format!(
                        "project functions refers to unknown IR profile {profile:?}"
                    )));
                }
            }
            let review_output = table
                .get("review")
                .map(|item| -> Result<PathBuf> {
                    let review = item
                        .as_table()
                        .ok_or("project functions.review must be a table")
                        .map_err(crate::Error::invalid)?;
                    review
                        .get("output")
                        .and_then(Item::as_str)
                        .map(|path| resolve_path(base, path))
                        .ok_or_else(|| {
                            crate::Error::invalid(
                                "project functions.review requires string \"output\"",
                            )
                        })
                })
                .transpose()?;
            Ok(FunctionWorkspacePaths {
                pack: resolve_path(
                    base,
                    table
                        .get("pack")
                        .and_then(Item::as_str)
                        .ok_or("project functions requires string \"pack\"")
                        .map_err(crate::Error::invalid)?,
                ),
                profiles,
                review_output,
            })
        })
        .transpose()?;
    if let Some(symbols) = &symbol_inventory {
        let conflicting_fact = registers
            .as_ref()
            .map(|paths| &paths.facts)
            .into_iter()
            .chain(interfaces.as_ref().map(|paths| &paths.facts))
            .find(|path| **path == symbols.output);
        if let Some(path) = conflicting_fact {
            return Err(crate::Error::invalid(format!(
                "project symbol inventory reuses another analysis facts path {}",
                path.display()
            )));
        }
    }
    if let Some(navigation) = &navigation_index {
        let conflicting_fact = registers
            .as_ref()
            .map(|paths| &paths.facts)
            .into_iter()
            .chain(interfaces.as_ref().map(|paths| &paths.facts))
            .find(|path| **path == navigation.output);
        if let Some(path) = conflicting_fact {
            return Err(crate::Error::invalid(format!(
                "project navigation index reuses another analysis facts path {}",
                path.display()
            )));
        }
    }
    Ok(ProjectSpec {
        id,
        target_spec,
        platform_pack,
        run_spec,
        memory_map,
        svd_configured,
        svd_paths,
        symbol_inventory,
        navigation_index,
        ir_profiles,
        registers,
        interfaces,
        functions,
    })
}

fn nested_output_path(
    table: &toml_edit::Table,
    base: &Path,
    name: &str,
) -> Result<Option<PathBuf>> {
    table
        .get(name)
        .map(|item| {
            let output = item
                .as_table()
                .ok_or_else(|| format!("project registers.{name} must be a table"))
                .map_err(crate::Error::invalid)?
                .get("output")
                .and_then(Item::as_str)
                .ok_or_else(|| format!("project registers.{name} requires string \"output\""))
                .map_err(crate::Error::invalid)?;
            Ok(resolve_path(base, output))
        })
        .transpose()
}

fn nested_path_array(
    table: &toml_edit::Table,
    base: &Path,
    table_name: &str,
    key: &str,
) -> Result<Vec<PathBuf>> {
    let Some(item) = table.get(table_name) else {
        return Ok(Vec::new());
    };
    let table = item
        .as_table()
        .ok_or_else(|| format!("project registers.{table_name} must be a table"))
        .map_err(crate::Error::invalid)?;
    let Some(item) = table.get(key) else {
        return Ok(Vec::new());
    };
    let values = item
        .as_array()
        .ok_or_else(|| format!("project registers.{table_name}.{key} must be an array"))
        .map_err(crate::Error::invalid)?;
    let mut seen = std::collections::BTreeSet::new();
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let value = value
                .as_str()
                .ok_or_else(|| {
                    format!("project registers.{table_name}.{key}[{index}] must be a string")
                })
                .map_err(crate::Error::invalid)?;
            let path = resolve_path(base, value);
            if !seen.insert(path.clone()) {
                return Err(crate::Error::invalid(format!(
                    "duplicate project registers.{table_name}.{key} path {value:?}"
                )));
            }
            Ok(path)
        })
        .collect()
}

fn required_string(document: &DocumentMut, key: &str) -> Option<String> {
    optional_string(document, key)
}

fn optional_string(document: &DocumentMut, key: &str) -> Option<String> {
    document.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn project_invalid(
    path: &Path,
    input: &str,
    item: Option<&Item>,
    message: impl Into<String>,
) -> ProjectError {
    let span = item.and_then(Item::span).unwrap_or(0..input.len().min(1));
    ProjectError::Invalid {
        message: message.into(),
        src: NamedSource::new(path.display().to_string(), input.to_owned()),
        span: (span.start, span.len()).into(),
    }
}

fn resolve_path(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

fn validate_id(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(crate::Error::invalid(format!(
            "invalid project id {value:?}"
        )));
    }
    Ok(())
}
