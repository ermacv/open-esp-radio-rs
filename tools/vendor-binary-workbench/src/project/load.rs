//! TOML decoding and path resolution for project manifests.

mod helpers;

use helpers::*;

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
    reject_unknown_keys(
        &document,
        &[
            "schema",
            "id",
            "target-spec",
            "platform-pack",
            "run-spec",
            "memory-map",
            "svd",
            "analysis",
            "code",
            "registers",
            "interfaces",
            "functions",
            "review",
            "qualification",
            "verification",
        ],
        "project manifest",
        source,
    )?;
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
            reject_unknown_keys(table, &["pack", "review"], "project code", source)?;
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
            if let Some(pac) = table.get("pac") {
                return Err(source.item(
                    Some(pac),
                    "unknown project registers key \"pac\"; generated svd2rust output belongs in [registers.pac-raw]",
                ));
            }
            reject_unknown_keys(
                table,
                &[
                    "facts",
                    "model",
                    "owned-ranges",
                    "review",
                    "svd",
                    "pac-raw",
                    "bindings",
                    "api",
                    "evidence",
                    "lints",
                ],
                "project registers",
                source,
            )?;
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
            let pac_raw = table
                .get("pac-raw")
                .map(|item| -> Result<PacRawOutputSpec> {
                    let pac_raw = item
                        .as_table()
                        .ok_or_else(|| source.item(Some(item), "project registers.pac-raw must be a table"))?;
                    let target = optional_table_string(pac_raw, "target", "project registers.pac-raw", source)?
                        .unwrap_or_else(|| "none".to_owned());
                    if !matches!(target.as_str(), "none" | "riscv") {
                        return Err(source.table_key(pac_raw, "target", format!(
                            "project registers.pac-raw target must be \"none\" or \"riscv\", got {target:?}"
                        )));
                    }
                    let edition = optional_table_string(pac_raw, "edition", "project registers.pac-raw", source)?
                        .unwrap_or_else(|| "2024".to_owned());
                    if !matches!(edition.as_str(), "2021" | "2024") {
                        return Err(source.table_key(pac_raw, "edition", format!(
                            "project registers.pac-raw edition must be \"2021\" or \"2024\", got {edition:?}"
                        )));
                    }
                    Ok(PacRawOutputSpec {
                        output: resolve_path(base, &table_string(pac_raw, "output", "project registers.pac-raw", source)?),
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
            let api = table
                .get("api")
                .map(|item| -> Result<(PathBuf, Option<PathBuf>)> {
                    let api = item
                        .as_table()
                        .ok_or_else(|| source.item(Some(item), "project registers.api must be a table"))?;
                    Ok((
                        resolve_path(
                            base,
                            &table_string(api, "pack", "project registers.api", source)?,
                        ),
                        optional_table_string(api, "output", "project registers.api", source)?
                            .map(|path| resolve_path(base, &path)),
                    ))
                })
                .transpose()?;
            let (api_pack, api_output) = api
                .map(|(pack, output)| (Some(pack), output))
                .unwrap_or((None, None));
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
                pac_raw,
                bindings,
                api_pack,
                api_output,
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
            reject_unknown_keys(
                table,
                &["facts", "pack"],
                "project interfaces",
                source,
            )?;
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
            reject_unknown_keys(
                table,
                &["pack", "profiles", "review"],
                "project functions",
                source,
            )?;
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
    let review = load_review_workspace(&document, base, &ir_profiles, source)?;
    let qualification = load_qualification_workspace(&document, base, source)?;
    let verification = load_verification_workspace(&document, base, source)?;
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
        }
        if let Some(paths) = &registers {
            generated.push(("register facts", &paths.facts));
            if let Some(path) = &paths.review_output {
                generated.push(("register review", path));
            }
            if let Some(path) = &paths.svd_output {
                generated.push(("generated SVD", path));
            }
            if let Some(pac_raw) = &paths.pac_raw {
                generated.push(("generated raw PAC", &pac_raw.output));
            }
            if let Some(bindings) = &paths.bindings {
                generated.push(("generated PAC bindings", &bindings.output));
            }
            if let Some(path) = &paths.api_output {
                generated.push(("generated closed PAC domains", path));
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
    let project = ProjectSpec {
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
        review,
        qualification,
        verification,
    };
    crate::qualification::validate_project(&project)?;
    Ok(project)
}

fn load_qualification_workspace(
    document: &Table,
    base: &Path,
    source: ProjectSource<'_>,
) -> Result<Option<QualificationWorkspaceSpec>> {
    let Some(item) = document.get("qualification") else {
        return Ok(None);
    };
    let table = item
        .as_table()
        .ok_or_else(|| source.item(Some(item), "project manifest qualification must be a table"))?;
    reject_unknown_keys(
        table,
        &["pack", "required-features", "hardware-evidence"],
        "project qualification",
        source,
    )?;
    let pack = resolve_path(
        base,
        &table_string(table, "pack", "project qualification", source)?,
    );
    let required_features = table_string_array(
        table,
        "required-features",
        "project qualification",
        source,
        false,
    )?;
    if required_features.is_empty() {
        return Err(source.table_key(
            table,
            "required-features",
            "project qualification requires at least one feature",
        ));
    }
    Ok(Some(QualificationWorkspaceSpec {
        pack,
        required_features,
        hardware_evidence: table
            .get("hardware-evidence")
            .map(|_| {
                table_string(table, "hardware-evidence", "project qualification", source)
                    .map(|path| resolve_path(base, &path))
            })
            .transpose()?,
    }))
}

fn load_review_workspace(
    document: &Table,
    base: &Path,
    ir_profiles: &[crate::project_ir::ProjectIrProfile],
    source: ProjectSource<'_>,
) -> Result<Option<ReviewWorkspaceSpec>> {
    let Some(item) = document.get("review") else {
        return Ok(None);
    };
    let table = item
        .as_table()
        .ok_or_else(|| source.item(Some(item), "project manifest review must be a table"))?;
    reject_unknown_keys(
        table,
        &["output", "publication-scopes", "scopes"],
        "project review",
        source,
    )?;
    let output = resolve_path(
        base,
        &table_string(table, "output", "project review", source)?,
    );
    let publication_scopes =
        table_string_array(table, "publication-scopes", "project review", source, false)?;
    let scopes_item = table.get("scopes").ok_or_else(|| {
        source.table_key(table, "scopes", "project review requires [[review.scopes]]")
    })?;
    let scopes = scopes_item.as_array_of_tables().ok_or_else(|| {
        source.item(
            Some(scopes_item),
            "project review.scopes must be an array of tables",
        )
    })?;
    if scopes.is_empty() {
        return Err(source.item(Some(scopes_item), "project review.scopes must not be empty"));
    }
    let all_profiles = ir_profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect::<Vec<_>>();
    let mut scope_ids = std::collections::BTreeSet::new();
    let scopes = scopes
        .iter()
        .enumerate()
        .map(|(index, scope)| {
            let context = format!("project review.scopes[{index}]");
            reject_unknown_keys(
                scope,
                &["id", "profiles", "roots", "include-reachable"],
                &context,
                source,
            )?;
            let id = table_string(scope, "id", &context, source)?;
            validate_id(&id).map_err(|message| source.table_key(scope, "id", message))?;
            if !scope_ids.insert(id.clone()) {
                return Err(source.table_key(
                    scope,
                    "id",
                    format!("duplicate project review scope {id:?}"),
                ));
            }
            let profiles = if scope.get("profiles").is_some() {
                table_string_array(scope, "profiles", &context, source, false)?
            } else {
                all_profiles.clone()
            };
            for profile in &profiles {
                if !ir_profiles.iter().any(|candidate| candidate.id == *profile) {
                    return Err(source.table_key(
                        scope,
                        "profiles",
                        format!("{context} refers to unknown IR profile {profile:?}"),
                    ));
                }
            }
            let roots = table_string_array(scope, "roots", &context, source, false)?;
            let include_reachable = scope
                .get("include-reachable")
                .map(|item| {
                    item.as_bool().ok_or_else(|| {
                        source.item(
                            Some(item),
                            format!("{context}.include-reachable must be a boolean"),
                        )
                    })
                })
                .transpose()?
                .unwrap_or(true);
            Ok(ReviewScopeSpec {
                id,
                profiles,
                roots,
                include_reachable,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    for publication_scope in &publication_scopes {
        if !scopes.iter().any(|scope| scope.id == *publication_scope) {
            return Err(source.table_key(
                table,
                "publication-scopes",
                format!("project review refers to unknown publication scope {publication_scope:?}"),
            ));
        }
    }
    Ok(Some(ReviewWorkspaceSpec {
        output,
        publication_scopes,
        scopes,
    }))
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

fn load_verification_workspace(
    document: &Table,
    base: &Path,
    source: ProjectSource<'_>,
) -> Result<Option<VerificationWorkspacePaths>> {
    let Some(item) = document.get("verification") else {
        return Ok(None);
    };
    let table = item
        .as_table()
        .ok_or_else(|| source.item(Some(item), "project manifest verification must be a table"))?;
    reject_unknown_keys(table, &["report", "suites"], "project verification", source)?;
    let report = resolve_path(
        base,
        &table_string(table, "report", "project verification", source)?,
    );
    let suites_item = table.get("suites").ok_or_else(|| {
        source.table_key(
            table,
            "suites",
            "project verification requires [[verification.suites]]",
        )
    })?;
    let suites = suites_item.as_array_of_tables().ok_or_else(|| {
        source.item(
            Some(suites_item),
            "project verification.suites must be an array of tables",
        )
    })?;
    if suites.is_empty() {
        return Err(source.item(
            Some(suites_item),
            "project verification.suites must not be empty",
        ));
    }

    let mut suite_ids = std::collections::BTreeSet::new();
    let suites = suites
        .iter()
        .enumerate()
        .map(|(index, suite)| {
            let context = format!("project verification.suites[{index}]");
            reject_unknown_keys(
                suite,
                &[
                    "id",
                    "vendor",
                    "rust-artifact-role",
                    "rust-companion-role",
                    "rust-prefix",
                    "profiles",
                    "dispositions",
                    "baselines",
                    "gate",
                    "match-floor",
                ],
                &context,
                source,
            )?;
            let id = table_string(suite, "id", &context, source)?;
            validate_id(&id).map_err(|message| source.table_key(suite, "id", message))?;
            if !suite_ids.insert(id.clone()) {
                return Err(source.table_key(
                    suite,
                    "id",
                    format!("duplicate project verification suite {id:?}"),
                ));
            }
            let vendor = parse_verification_vendor(suite, &context, source)?;

            let rust_artifact_role =
                parse_rust_input_role(suite, "rust-artifact-role", &context, source, false)?
                    .expect("required Rust artifact role");
            let rust_companion_role =
                parse_rust_input_role(suite, "rust-companion-role", &context, source, true)?;
            let rust_prefix = table_string(suite, "rust-prefix", &context, source)?;
            let profiles = table_path_array(suite, "profiles", &context, base, source, true)?;
            let dispositions =
                table_path_array(suite, "dispositions", &context, base, source, false)?;
            let evidence_baselines =
                table_path_array(suite, "baselines", &context, base, source, true)?;
            let gate_name = table_string(suite, "gate", &context, source)?;
            let match_floor = suite
                .get("match-floor")
                .map(|item| {
                    item.as_integer()
                        .and_then(|value| usize::try_from(value).ok())
                        .ok_or_else(|| {
                            source.item(
                                Some(item),
                                format!("{context}.match-floor must be a non-negative integer"),
                            )
                        })
                })
                .transpose()?;
            let gate = match (gate_name.as_str(), match_floor) {
                ("completion", None) => ProjectVerificationGate::Completion,
                ("regression", Some(match_floor)) => {
                    ProjectVerificationGate::Regression { match_floor }
                }
                ("completion", Some(_)) => {
                    return Err(source.table_key(
                        suite,
                        "match-floor",
                        format!("{context}.match-floor is only valid for gate = \"regression\""),
                    ));
                }
                ("regression", None) => {
                    return Err(source.table_key(
                        suite,
                        "match-floor",
                        format!("{context} regression gate requires match-floor"),
                    ));
                }
                _ => {
                    return Err(source.table_key(
                        suite,
                        "gate",
                        format!("{context}.gate must be \"completion\" or \"regression\""),
                    ));
                }
            };
            Ok(VerificationSuiteSpec {
                id,
                vendor,
                rust_artifact_role,
                rust_companion_role,
                rust_prefix,
                profiles,
                dispositions,
                evidence_baselines,
                gate,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(VerificationWorkspacePaths { report, suites }))
}

fn parse_verification_vendor(
    suite: &Table,
    context: &str,
    source: ProjectSource<'_>,
) -> Result<Vec<VerificationVendorSpec>> {
    let item = suite.get("vendor").ok_or_else(|| {
        source.table_key(
            suite,
            "vendor",
            format!("{context} requires [[verification.suites.vendor]]"),
        )
    })?;
    let tables = item.as_array_of_tables().ok_or_else(|| {
        source.item(
            Some(item),
            format!("{context}.vendor must be an array of tables"),
        )
    })?;
    if tables.is_empty() {
        return Err(source.item(Some(item), format!("{context}.vendor must not be empty")));
    }
    let mut seen = std::collections::BTreeSet::new();
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let entry_context = format!("{context}.vendor[{index}]");
            reject_unknown_keys(
                table,
                &["source", "all", "prefix", "symbols"],
                &entry_context,
                source,
            )?;
            let source_name = table_string(table, "source", &entry_context, source)?;
            let source_id: crate::source_id::SourceId = source_name.parse().map_err(|message| {
                source.table_key(table, "source", format!("{entry_context}: {message}"))
            })?;
            if !seen.insert(source_id.clone()) {
                return Err(source.table_key(
                    table,
                    "source",
                    format!("{context} repeats vendor source {source_id}"),
                ));
            }
            let all = table.get("all").map(|item| {
                item.as_bool().ok_or_else(|| {
                    source.item(Some(item), format!("{entry_context}.all must be a boolean"))
                })
            }).transpose()?;
            let prefix = table.get("prefix").map(|item| {
                item.as_str().map(str::to_owned).ok_or_else(|| {
                    source.item(Some(item), format!("{entry_context}.prefix must be a string"))
                })
            }).transpose()?;
            let symbols = if table.contains_key("symbols") {
                table_string_array(table, "symbols", &entry_context, source, true)?
            } else {
                Vec::new()
            };
            if all == Some(false) {
                return Err(source.table_key(table, "all", format!("{entry_context}.all must be true when present")));
            }
            let configured = usize::from(all == Some(true)) + usize::from(prefix.is_some()) + usize::from(!symbols.is_empty());
            if configured != 1 {
                return Err(source.table_key(
                    table,
                    "source",
                    format!("{entry_context} must configure exactly one of all = true, prefix, or symbols"),
                ));
            }
            let selection = if all == Some(true) {
                VerificationVendorSelection::All
            } else if let Some(prefix) = prefix {
                if prefix.is_empty() {
                    return Err(source.table_key(table, "prefix", format!("{entry_context}.prefix must not be empty")));
                }
                VerificationVendorSelection::Prefix(prefix)
            } else {
                let unique = symbols.iter().collect::<std::collections::BTreeSet<_>>();
                if unique.len() != symbols.len() || symbols.iter().any(String::is_empty) {
                    return Err(source.table_key(table, "symbols", format!("{entry_context}.symbols must contain unique non-empty names")));
                }
                VerificationVendorSelection::Symbols(symbols)
            };
            Ok(VerificationVendorSpec { source: source_id, selection })
        })
        .collect()
}
