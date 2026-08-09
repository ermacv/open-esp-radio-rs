//! TOML decoding and path resolution for project manifests.

use std::{
    fs,
    path::{Path, PathBuf},
};

use toml_edit::{Item, Table};

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
    let document = toml_edit::Document::parse(input.as_str()).map_err(|error| {
        let span = error.span().unwrap_or(0..input.len().min(1));
        ProjectError::Parse {
            message: error.message().to_owned(),
            src: NamedSource::new(path.display().to_string(), input.clone()),
            span: (span.start, span.len().max(1)).into(),
        }
    })?;
    let source = ProjectSource::new(path, &input);
    if document.get("schema").and_then(Item::as_integer) != Some(1) {
        return Err(source.item(
            document.get("schema"),
            "project manifest requires schema = 1",
        ));
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let id = required_string(&document, "id", source)?;
    validate_id(&id).map_err(|message| source.item(document.get("id"), message))?;
    let target_spec_value = required_string(&document, "target-spec", source)?;
    let target_spec = resolve_path(base, &target_spec_value);
    let platform_pack = optional_string(&document, "platform-pack", source)?
        .map(|path| PlatformPack::load(&resolve_path(base, &path)))
        .transpose()?;
    let run_spec =
        optional_string(&document, "run-spec", source)?.map(|path| resolve_path(base, &path));
    let memory_map =
        optional_string(&document, "memory-map", source)?.map(|path| resolve_path(base, &path));
    let svd_configured = document.get("svd").is_some();
    let svd_paths = document
        .get("svd")
        .map(|item| {
            let array = item.as_array().ok_or_else(|| {
                source.item(Some(item), "project manifest svd must be an array of paths")
            })?;
            array
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    value
                        .as_str()
                        .map(|path| resolve_path(base, path))
                        .ok_or_else(|| {
                            source.error(
                                value.span(),
                                format!("project manifest svd[{index}] must be a string"),
                            )
                        })
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let ir_profiles = load_ir_profiles(&document, base, source)?;
    let symbol_inventory = load_symbol_inventory(&document, base, &ir_profiles, source)?;
    let navigation_index = load_navigation_index(
        &document,
        base,
        symbol_inventory.as_ref(),
        &ir_profiles,
        source,
    )?;
    let code = document
        .get("code")
        .map(|item| -> Result<CodeWorkspacePaths> {
            let table = item
                .as_table()
                .ok_or_else(|| source.item(Some(item), "project manifest code must be a table"))?;
            if symbol_inventory.is_none() {
                return Err(source.item(
                    Some(item),
                    "project [code] requires [analysis.symbols] generated facts",
                ));
            }
            let review_output = table
                .get("review")
                .map(|item| -> Result<PathBuf> {
                    let review = item.as_table().ok_or_else(|| {
                        source.item(Some(item), "project code.review must be a table")
                    })?;
                    Ok(resolve_path(
                        base,
                        &table_string(review, "output", "project code.review", source)?,
                    ))
                })
                .transpose()?;
            Ok(CodeWorkspacePaths {
                pack: resolve_path(base, &table_string(table, "pack", "project code", source)?),
                review_output,
            })
        })
        .transpose()?;
    let registers = document
        .get("registers")
        .map(|item| -> Result<RegisterWorkspacePaths> {
            let table = item
                .as_table()
                .ok_or_else(|| source.item(Some(item), "project manifest registers must be a table"))?;
            if let Some(overlay) = table.get("overlay") {
                return Err(source.item(Some(overlay), "unknown project registers key \"overlay\"; use the schema-2 \"model\" workspace"));
            }
            let model = table_string(table, "model", "project registers", source)?;
            let owned_ranges = required_table_string_array(
                table,
                "owned-ranges",
                "project registers",
                source,
            )?;
            let review_output = nested_output_path(table, base, "review", source)?;
            let review_ir_reports =
                nested_path_array(table, base, "review", "linked-ir", source)?;
            let non_operational_functions =
                nested_string_array(table, "review", "non-operational-functions", source)?;
            let svd_output = nested_output_path(table, base, "svd", source)?;
            let pac = table
                .get("pac")
                .map(|item| -> Result<PacOutputSpec> {
                    let pac = item
                        .as_table()
                        .ok_or_else(|| source.item(Some(item), "project registers.pac must be a table"))?;
                    let target = optional_table_string(pac, "target", "project registers.pac", source)?
                        .unwrap_or_else(|| "none".to_owned());
                    if !matches!(target.as_str(), "none" | "riscv") {
                        return Err(source.table_key(pac, "target", format!(
                            "project registers.pac target must be \"none\" or \"riscv\", got {target:?}"
                        )));
                    }
                    let edition = optional_table_string(pac, "edition", "project registers.pac", source)?
                        .unwrap_or_else(|| "2024".to_owned());
                    if !matches!(edition.as_str(), "2021" | "2024") {
                        return Err(source.table_key(pac, "edition", format!(
                            "project registers.pac edition must be \"2021\" or \"2024\", got {edition:?}"
                        )));
                    }
                    Ok(PacOutputSpec {
                        output: resolve_path(base, &table_string(pac, "output", "project registers.pac", source)?),
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
                        .ok_or_else(|| source.item(Some(item), "project registers.bindings must be a table"))?;
                    let crate_name = table_string(bindings, "crate-name", "project registers.bindings", source)?;
                    open_esp_radio_register_model::validate_pac_crate_name(&crate_name)
                        .map_err(|error| source.table_key(bindings, "crate-name", error.to_string()))?;
                    Ok(PacBindingsOutputSpec {
                        output: resolve_path(base, &table_string(bindings, "output", "project registers.bindings", source)?),
                        crate_name,
                    })
                })
                .transpose()?;
            let api_pack = table
                .get("api")
                .map(|item| -> Result<PathBuf> {
                    let api = item
                        .as_table()
                        .ok_or_else(|| source.item(Some(item), "project registers.api must be a table"))?;
                    Ok(resolve_path(
                        base,
                        &table_string(api, "pack", "project registers.api", source)?,
                    ))
                })
                .transpose()?;
            let evidence_catalogs =
                nested_path_array(table, base, "evidence", "catalogs", source)?;
            let lint_pack = table
                .get("lints")
                .map(|item| -> Result<PathBuf> {
                    let lints = item
                        .as_table()
                        .ok_or_else(|| source.item(Some(item), "project registers.lints must be a table"))?;
                    Ok(resolve_path(
                        base,
                        &table_string(lints, "pack", "project registers.lints", source)?,
                    ))
                })
                .transpose()?;
            Ok(RegisterWorkspacePaths {
                facts: resolve_path(
                    base,
                    &table_string(table, "facts", "project registers", source)?,
                ),
                model: resolve_path(base, &model),
                owned_ranges,
                non_operational_functions,
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
                .ok_or_else(|| source.item(Some(item), "project manifest interfaces must be a table"))?;
            if let Some(catalogs) = table.get("semantic-catalogs") {
                return Err(source.item(Some(catalogs), "unknown project interfaces key \"semantic-catalogs\"; semantic catalogs belong to the platform pack"));
            }
            let semantic_catalogs = platform_pack
                .as_ref()
                .map(|pack| pack.semantic_catalogs.clone())
                .unwrap_or_default();
            Ok(InterfaceWorkspacePaths {
                facts: resolve_path(
                    base,
                    &table_string(table, "facts", "project interfaces", source)?,
                ),
                pack: optional_table_string(table, "pack", "project interfaces", source)?
                    .map(|path| resolve_path(base, &path)),
                semantic_catalogs,
            })
        })
        .transpose()?;
    let functions = document
        .get("functions")
        .map(|item| -> Result<FunctionWorkspacePaths> {
            let table = item.as_table().ok_or_else(|| {
                source.item(Some(item), "project manifest functions must be a table")
            })?;
            let profiles = table
                .get("profiles")
                .map(|item| {
                    let values = item.as_array().ok_or_else(|| {
                        source.item(Some(item), "project functions.profiles must be an array")
                    })?;
                    values
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            value.as_str().map(str::to_owned).ok_or_else(|| {
                                source.error(
                                    value.span(),
                                    format!("project functions.profiles[{index}] must be a string"),
                                )
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
                return Err(source.item(
                    table.get("profiles").or(Some(item)),
                    "project [functions] requires at least one [[analysis.ir]] profile",
                ));
            }
            let mut seen = std::collections::BTreeSet::new();
            for profile in &profiles {
                if !seen.insert(profile) {
                    return Err(source.table_key(
                        table,
                        "profiles",
                        format!("duplicate project functions profile {profile:?}"),
                    ));
                }
                if !ir_profiles.iter().any(|candidate| candidate.id == *profile) {
                    return Err(source.table_key(
                        table,
                        "profiles",
                        format!("project functions refers to unknown IR profile {profile:?}"),
                    ));
                }
            }
            let review_output = table
                .get("review")
                .map(|item| -> Result<PathBuf> {
                    let review = item.as_table().ok_or_else(|| {
                        source.item(Some(item), "project functions.review must be a table")
                    })?;
                    Ok(resolve_path(
                        base,
                        &table_string(review, "output", "project functions.review", source)?,
                    ))
                })
                .transpose()?;
            Ok(FunctionWorkspacePaths {
                pack: resolve_path(
                    base,
                    &table_string(table, "pack", "project functions", source)?,
                ),
                profiles,
                review_output,
            })
        })
        .transpose()?;
    let verification = document
        .get("verification")
        .map(|item| -> Result<VerificationWorkspacePaths> {
            let table = item.as_table().ok_or_else(|| {
                source.item(Some(item), "project manifest verification must be a table")
            })?;
            let profiles = table
                .get("profiles")
                .and_then(Item::as_array)
                .ok_or_else(|| {
                    source.table_key(
                        table,
                        "profiles",
                        "project verification.profiles must be a non-empty array",
                    )
                })?
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    value
                        .as_str()
                        .map(|path| resolve_path(base, path))
                        .ok_or_else(|| {
                            source.error(
                                value.span(),
                                format!("project verification.profiles[{index}] must be a string"),
                            )
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            if profiles.is_empty() {
                return Err(source.table_key(
                    table,
                    "profiles",
                    "project verification.profiles must not be empty",
                ));
            }
            let mut unique = std::collections::BTreeSet::new();
            if let Some(duplicate) = profiles.iter().find(|path| !unique.insert(*path)) {
                return Err(source.table_key(
                    table,
                    "profiles",
                    format!(
                        "project verification repeats profile file {}",
                        duplicate.display()
                    ),
                ));
            }
            let rust_prefix =
                optional_table_string(table, "rust-prefix", "project verification", source)?;
            Ok(VerificationWorkspacePaths {
                profiles,
                rust_prefix,
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
            let item = document
                .get("analysis")
                .and_then(Item::as_table)
                .and_then(|analysis| analysis.get("symbols"))
                .and_then(Item::as_table)
                .and_then(|symbols| symbols.get("output"));
            return Err(source.item(
                item,
                format!(
                    "project symbol inventory reuses another analysis facts path {}",
                    path.display()
                ),
            ));
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
            let item = document
                .get("analysis")
                .and_then(Item::as_table)
                .and_then(|analysis| analysis.get("navigation"))
                .and_then(Item::as_table)
                .and_then(|navigation| navigation.get("output"));
            return Err(source.item(
                item,
                format!(
                    "project navigation index reuses another analysis facts path {}",
                    path.display()
                ),
            ));
        }
    }
    if let Some(code) = &code {
        let mut generated = Vec::<(&str, &Path)>::new();
        if let Some(symbols) = &symbol_inventory {
            generated.push(("symbol inventory", &symbols.output));
        }
        if let Some(navigation) = &navigation_index {
            generated.push(("navigation index", &navigation.output));
        }
        for profile in &ir_profiles {
            generated.push(("linked-IR report", &profile.output));
            if let Some(path) = &profile.pseudo_rust {
                generated.push(("pseudo-Rust report", path));
            }
        }
        if let Some(paths) = &registers {
            generated.push(("register facts", &paths.facts));
            if let Some(path) = &paths.review_output {
                generated.push(("register review", path));
            }
            if let Some(path) = &paths.svd_output {
                generated.push(("generated SVD", path));
            }
            if let Some(pac) = &paths.pac {
                generated.push(("generated PAC", &pac.output));
            }
            if let Some(bindings) = &paths.bindings {
                generated.push(("generated PAC bindings", &bindings.output));
            }
        }
        if let Some(paths) = &interfaces {
            generated.push(("interface facts", &paths.facts));
        }
        if let Some(path) = functions
            .as_ref()
            .and_then(|paths| paths.review_output.as_ref())
        {
            generated.push(("function review", path));
        }
        if let Some((owner, path)) = generated.iter().find(|(_, output)| **output == code.pack) {
            return Err(source.item(
                document.get("code"),
                format!(
                    "reviewed code-boundary pack {} reuses {owner} output path",
                    path.display()
                ),
            ));
        }
        if let Some(review) = &code.review_output {
            if review == &code.pack {
                return Err(source.item(
                    document.get("code"),
                    "generated code-boundary review reuses the reviewed pack path",
                ));
            }
            if let Some((owner, path)) = generated.iter().find(|(_, output)| **output == *review) {
                return Err(source.item(
                    document.get("code"),
                    format!(
                        "generated code-boundary review {} reuses {owner} output path",
                        path.display()
                    ),
                ));
            }
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
        code,
        ir_profiles,
        registers,
        interfaces,
        functions,
        verification,
    })
}

fn nested_output_path(
    table: &toml_edit::Table,
    base: &Path,
    name: &str,
    source: ProjectSource<'_>,
) -> Result<Option<PathBuf>> {
    table
        .get(name)
        .map(|item| {
            let output = item.as_table().ok_or_else(|| {
                source.item(
                    Some(item),
                    format!("project registers.{name} must be a table"),
                )
            })?;
            let output = table_string(
                output,
                "output",
                &format!("project registers.{name}"),
                source,
            )?;
            Ok(resolve_path(base, &output))
        })
        .transpose()
}

fn required_table_string_array(
    table: &toml_edit::Table,
    key: &str,
    context: &str,
    source: ProjectSource<'_>,
) -> Result<Vec<String>> {
    let item = table
        .get(key)
        .ok_or_else(|| source.table_key(table, key, format!("{context} requires {key:?}")))?;
    let values = item
        .as_array()
        .ok_or_else(|| source.item(Some(item), format!("{context}.{key} must be an array")))?;
    if values.is_empty() {
        return Err(source.item(
            Some(item),
            format!("{context}.{key} must contain at least one range name"),
        ));
    }
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
                        format!("{context}.{key}[{index}] must be a non-empty string"),
                    )
                })?;
            if !seen.insert(value) {
                return Err(
                    source.error(span, format!("duplicate {context}.{key} range {value:?}"))
                );
            }
            Ok(value.to_owned())
        })
        .collect()
}

fn nested_path_array(
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

fn nested_string_array(
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

fn required_string(document: &Table, key: &str, source: ProjectSource<'_>) -> Result<String> {
    optional_string(document, key, source)?.ok_or_else(|| {
        source.item(
            document.get(key),
            format!("project manifest requires string {key:?}"),
        )
    })
}

fn optional_string(
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

fn table_string(
    table: &toml_edit::Table,
    key: &str,
    context: &str,
    source: ProjectSource<'_>,
) -> Result<String> {
    optional_table_string(table, key, context, source)?
        .ok_or_else(|| source.table_key(table, key, format!("{context} requires string {key:?}")))
}

fn optional_table_string(
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

fn resolve_path(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

fn validate_id(value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid project id {value:?}"));
    }
    Ok(())
}
