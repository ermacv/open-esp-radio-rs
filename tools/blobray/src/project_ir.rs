//! Reproducible linked-IR generation profiles owned by a project manifest.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use toml_edit::{Item, Table};

use crate::{
    Result,
    project::{AnalysisSymbolFamilyDisposition, AnalysisSymbolFamilySurface, ProjectSource},
    source_id::validate_source_id,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectIrProfile {
    pub(crate) id: String,
    pub(crate) sources: Vec<String>,
    pub(crate) roots: ProjectIrRoots,
    pub(crate) include_reachable: bool,
    pub(crate) entry_contract: String,
    pub(crate) output: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProjectIrRoots {
    All,
    SymbolPrefix(String),
}

impl ProjectIrRoots {
    pub(crate) const fn mode(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::SymbolPrefix(_) => "symbol-prefix",
        }
    }

    pub(crate) fn symbol_prefix(&self) -> &str {
        match self {
            Self::All => "",
            Self::SymbolPrefix(prefix) => prefix,
        }
    }
}

pub(crate) fn load_ir_profiles(
    document: &Table,
    base: &Path,
    source: ProjectSource<'_>,
) -> Result<Vec<ProjectIrProfile>> {
    let Some(analysis_item) = document.get("analysis") else {
        return Ok(Vec::new());
    };
    let analysis = analysis_item.as_table().ok_or_else(|| {
        source.item(
            Some(analysis_item),
            "project manifest analysis must be a table",
        )
    })?;
    let Some(ir_item) = analysis.get("ir") else {
        return Ok(Vec::new());
    };
    let profiles = ir_item.as_array_of_tables().ok_or_else(|| {
        source.item(
            Some(ir_item),
            "project analysis.ir must be an array of tables",
        )
    })?;
    let mut ids = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    profiles
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("project analysis.ir[{index}]");
            let id = required_string(table, "id", &context, source)?;
            validate_profile_id(&id, &context)
                .map_err(|message| source.table_key(table, "id", message))?;
            if !ids.insert(id.clone()) {
                return Err(source.table_key(
                    table,
                    "id",
                    format!("duplicate project IR profile id {id:?}"),
                ));
            }
            let sources = sources(table, &context, source)?;
            let roots = match required_string(table, "roots", &context, source)?.as_str() {
                "all" => {
                    if let Some(item) = table.get("symbol-prefix") {
                        return Err(source.item(
                            Some(item),
                            format!("{context}.symbol-prefix is invalid when roots = \"all\""),
                        ));
                    }
                    ProjectIrRoots::All
                }
                "symbol-prefix" => ProjectIrRoots::SymbolPrefix(required_string(
                    table,
                    "symbol-prefix",
                    &context,
                    source,
                )?),
                value => {
                    return Err(source.table_key(
                        table,
                        "roots",
                        format!(
                            "{context}.roots must be \"all\" or \"symbol-prefix\", got {value:?}"
                        ),
                    ));
                }
            };
            let include_reachable =
                optional_boolean(table, "include-reachable", &context, source)?.unwrap_or(true);
            let entry_contract = optional_string(table, "entry-contract", &context, source)?
                .unwrap_or_else(|| "none".to_owned());
            let output = resolve_path(base, &required_string(table, "output", &context, source)?);
            reserve_output(&mut outputs, &output, &id)
                .map_err(|message| source.table_key(table, "output", message))?;
            if let Some(item) = table.get("pseudo-rust") {
                return Err(source.item(
                    Some(item),
                    format!(
                        "{context}.pseudo-rust was removed; use `inspect function` or focused `ir export --pseudo-rust`"
                    ),
                ));
            }
            Ok(ProjectIrProfile {
                id,
                sources,
                roots,
                include_reachable,
                entry_contract,
                output,
            })
        })
        .collect()
}

pub(crate) fn load_symbol_family_surfaces(
    document: &Table,
    profiles: &[ProjectIrProfile],
    source: ProjectSource<'_>,
) -> Result<Vec<AnalysisSymbolFamilySurface>> {
    let Some(analysis) = document.get("analysis").and_then(Item::as_table) else {
        return Ok(Vec::new());
    };
    let Some(item) = analysis.get("public-symbol-families") else {
        return Ok(Vec::new());
    };
    let entries = item.as_array_of_tables().ok_or_else(|| {
        source.item(
            Some(item),
            "project analysis.public-symbol-families must be an array of tables",
        )
    })?;
    let mut ids = BTreeSet::new();
    let mut families = BTreeSet::new();
    entries
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("project analysis.public-symbol-families[{index}]");
            if let Some((key, item)) = table.iter().find(|(key, _)| {
                ![
                    "id",
                    "protocols",
                    "source",
                    "symbol-prefix",
                    "disposition",
                    "profile",
                    "reason",
                ]
                .contains(key)
            }) {
                return Err(source.item(
                    Some(item),
                    format!("unknown {context} key {key:?}"),
                ));
            }
            let id = required_string(table, "id", &context, source)?;
            validate_profile_id(&id, &context)
                .map_err(|message| source.table_key(table, "id", message))?;
            if !ids.insert(id.clone()) {
                return Err(source.table_key(
                    table,
                    "id",
                    format!("duplicate public symbol family id {id:?}"),
                ));
            }
            let protocol_item = table.get("protocols").ok_or_else(|| {
                source.table_key(
                    table,
                    "protocols",
                    format!("{context} requires protocols"),
                )
            })?;
            let protocol_values = protocol_item.as_array().ok_or_else(|| {
                source.item(
                    Some(protocol_item),
                    format!("{context}.protocols must be an array"),
                )
            })?;
            if protocol_values.is_empty() {
                return Err(source.item(
                    Some(protocol_item),
                    format!("{context}.protocols must not be empty"),
                ));
            }
            let mut seen_protocols = BTreeSet::new();
            let protocols = protocol_values
                .iter()
                .enumerate()
                .map(|(protocol_index, value)| {
                    let protocol = value.as_str().ok_or_else(|| {
                        source.error(
                            value.span(),
                            format!(
                                "{context}.protocols[{protocol_index}] must be a string"
                            ),
                        )
                    })?;
                    let canonical = crate::project::canonical_review_protocol(protocol)
                        .ok_or_else(|| {
                            source.error(
                                value.span(),
                                format!(
                                    "{context}.protocols contains unsupported protocol {protocol:?}; expected one of {}",
                                    crate::project::REVIEW_PROTOCOLS.join(", ")
                                ),
                            )
                        })?;
                    if !seen_protocols.insert(canonical) {
                        return Err(source.error(
                            value.span(),
                            format!("duplicate {context}.protocols value {protocol:?}"),
                        ));
                    }
                    Ok(canonical.to_owned())
                })
                .collect::<Result<Vec<_>>>()?;
            let family_source = required_string(table, "source", &context, source)?;
            validate_source_id(&family_source).map_err(|_| {
                source.table_key(
                    table,
                    "source",
                    format!("invalid source id {family_source:?} in {context}"),
                )
            })?;
            let symbol_prefix = required_string(table, "symbol-prefix", &context, source)?;
            let disposition = match required_string(table, "disposition", &context, source)?.as_str()
            {
                "required" => AnalysisSymbolFamilyDisposition::Required,
                "excluded" => AnalysisSymbolFamilyDisposition::Excluded,
                value => {
                    return Err(source.table_key(
                        table,
                        "disposition",
                        format!(
                            "{context}.disposition must be \"required\" or \"excluded\", got {value:?}"
                        ),
                    ));
                }
            };
            let profile = optional_string(table, "profile", &context, source)?;
            let reason = optional_string(table, "reason", &context, source)?;
            match disposition {
                AnalysisSymbolFamilyDisposition::Required => {
                    if profile.is_none() {
                        return Err(source.table_key(
                            table,
                            "profile",
                            format!("required {context} requires profile"),
                        ));
                    }
                    if reason.is_some() {
                        return Err(source.table_key(
                            table,
                            "reason",
                            format!("required {context} must not declare an exclusion reason"),
                        ));
                    }
                }
                AnalysisSymbolFamilyDisposition::Excluded => {
                    if profile.is_some() {
                        return Err(source.table_key(
                            table,
                            "profile",
                            format!("excluded {context} must not declare profile"),
                        ));
                    }
                    if reason.is_none() {
                        return Err(source.table_key(
                            table,
                            "reason",
                            format!("excluded {context} requires reason"),
                        ));
                    }
                }
            }
            if !families.insert((family_source.clone(), symbol_prefix.clone())) {
                return Err(source.table_key(
                    table,
                    "symbol-prefix",
                    format!(
                        "duplicate public symbol family {family_source}:{symbol_prefix}"
                    ),
                ));
            }
            let covering_profile = profiles.iter().find(|profile| {
                profile.sources.iter().any(|value| value == &family_source)
                    && match &profile.roots {
                        ProjectIrRoots::All => true,
                        ProjectIrRoots::SymbolPrefix(prefix) => prefix == &symbol_prefix,
                    }
            });
            match disposition {
                AnalysisSymbolFamilyDisposition::Required => {
                    if let Some(expected) = profile.as_deref()
                        && let Some(configured) = profiles.iter().find(|value| value.id == expected)
                        && !(configured
                            .sources
                            .iter()
                            .any(|value| value == &family_source)
                            && match &configured.roots {
                                ProjectIrRoots::All => true,
                                ProjectIrRoots::SymbolPrefix(prefix) => prefix == &symbol_prefix,
                            })
                    {
                        return Err(source.table_key(
                            table,
                            "profile",
                            format!(
                                "required public symbol family profile {expected:?} does not analyze source {family_source:?}"
                            ),
                        ));
                    }
                    if let (Some(expected), Some(actual)) = (profile.as_deref(), covering_profile)
                        && expected != actual.id
                    {
                        return Err(source.table_key(
                            table,
                            "profile",
                            format!(
                                "required public symbol family {family_source}:{symbol_prefix} is covered by profile {:?}, not declared profile {expected:?}",
                                actual.id
                            ),
                        ));
                    }
                }
                AnalysisSymbolFamilyDisposition::Excluded if covering_profile.is_some() => {
                    return Err(source.table_key(
                        table,
                        "symbol-prefix",
                        format!(
                            "excluded public symbol family {family_source}:{symbol_prefix} overlaps a configured analysis profile"
                        ),
                    ));
                }
                AnalysisSymbolFamilyDisposition::Excluded => {}
            }
            Ok(AnalysisSymbolFamilySurface {
                id,
                protocols,
                source: family_source,
                symbol_prefix,
                disposition,
                profile,
                reason,
            })
        })
        .collect()
}

fn sources(table: &Table, context: &str, source: ProjectSource<'_>) -> Result<Vec<String>> {
    let Some(item) = table.get("sources") else {
        return Ok(Vec::new());
    };
    let values = item
        .as_array()
        .ok_or_else(|| source.item(Some(item), format!("{context}.sources must be an array")))?;
    if values.is_empty() {
        return Err(source.item(Some(item), format!("{context}.sources must not be empty")));
    }
    let mut seen = BTreeSet::new();
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let source_id = value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    source.error(
                        value.span(),
                        format!("{context}.sources[{index}] must be a non-empty string"),
                    )
                })?;
            validate_source_id(source_id).map_err(|_| {
                source.error(
                    value.span(),
                    format!("invalid source id {source_id:?} in {context}.sources[{index}]"),
                )
            })?;
            if !seen.insert(source_id.to_owned()) {
                return Err(source.error(
                    value.span(),
                    format!("duplicate source {source_id:?} in {context}"),
                ));
            }
            Ok(source_id.to_owned())
        })
        .collect()
}

fn reserve_output(
    outputs: &mut BTreeSet<PathBuf>,
    path: &Path,
    id: &str,
) -> std::result::Result<(), String> {
    if !outputs.insert(path.to_owned()) {
        return Err(format!(
            "project IR profile {id:?} reuses output path {}",
            path.display()
        ));
    }
    Ok(())
}

fn required_string(
    table: &Table,
    key: &str,
    context: &str,
    source: ProjectSource<'_>,
) -> Result<String> {
    optional_string(table, key, context, source)?
        .ok_or_else(|| source.table_key(table, key, format!("{context} requires string {key:?}")))
}

fn optional_string(
    table: &Table,
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

fn optional_boolean(
    table: &Table,
    key: &str,
    context: &str,
    source: ProjectSource<'_>,
) -> Result<Option<bool>> {
    match table.get(key) {
        None => Ok(None),
        Some(Item::Value(value)) => value.as_bool().map(Some).ok_or_else(|| {
            source.error(value.span(), format!("{context}.{key} must be a boolean"))
        }),
        Some(item) => Err(source.item(Some(item), format!("{context}.{key} must be a boolean"))),
    }
}

fn validate_profile_id(id: &str, context: &str) -> std::result::Result<(), String> {
    if id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Ok(())
    } else {
        Err(format!("invalid IR profile id {id:?} in {context}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::DocumentMut;

    #[test]
    fn parses_profiles_and_resolves_outputs_relative_to_the_project() {
        let input = r#"
[analysis]

[[analysis.ir]]
id = "phy"
sources = ["rom", "archive"]
roots = "symbol-prefix"
symbol-prefix = "phy_"
output = "generated/phy.ir"

[[analysis.ir]]
id = "all-rom"
sources = ["rom"]
roots = "all"
include-reachable = false
entry-contract = "none"
output = "generated/rom.ir"
"#;
        let document = input.parse::<DocumentMut>().unwrap();
        let profiles = load_ir_profiles(
            &document,
            Path::new("project"),
            ProjectSource::new(Path::new("project.toml"), input),
        )
        .unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].sources, ["rom", "archive"]);
        assert_eq!(
            profiles[0].roots,
            ProjectIrRoots::SymbolPrefix("phy_".to_owned())
        );
        assert!(profiles[0].include_reachable);
        assert_eq!(profiles[1].roots, ProjectIrRoots::All);
        assert!(!profiles[1].include_reachable);
    }

    #[test]
    fn rejects_project_wide_pseudo_output() {
        let input = r#"
[[analysis.ir]]
id = "all"
roots = "all"
output = "generated/all.ir"
pseudo-rust = "generated/all.pseudo.rs"
"#;
        let document = input.parse::<DocumentMut>().unwrap();
        let error = load_ir_profiles(
            &document,
            Path::new("project"),
            ProjectSource::new(Path::new("project.toml"), input),
        )
        .unwrap_err();
        assert!(error.to_string().contains("pseudo-rust was removed"));
    }

    #[test]
    fn rejects_duplicate_profile_outputs() {
        let input = r#"
[[analysis.ir]]
id = "one"
roots = "all"
output = "generated/shared.json"

[[analysis.ir]]
id = "two"
roots = "all"
output = "generated/shared.json"
"#;
        let document = input.parse::<DocumentMut>().unwrap();
        let error = load_ir_profiles(
            &document,
            Path::new("project"),
            ProjectSource::new(Path::new("project.toml"), input),
        )
        .unwrap_err();
        assert!(error.to_string().contains("reuses output path"));
    }

    #[test]
    fn public_symbol_family_coverage_is_explicit_and_fail_closed() {
        let required_input = r#"
[analysis]

[[analysis.public-symbol-families]]
id = "ieee802154-controller"
protocols = ["ieee802154"]
source = "ieee802154"
symbol-prefix = "esp_ieee802154_"
disposition = "required"
profile = "ieee802154-controller"
"#;
        let required = required_input.parse::<DocumentMut>().unwrap();
        let surfaces = load_symbol_family_surfaces(
            &required,
            &[],
            ProjectSource::new(Path::new("project.toml"), required_input),
        )
        .unwrap();
        assert_eq!(surfaces.len(), 1);
        assert_eq!(
            surfaces[0].disposition,
            AnalysisSymbolFamilyDisposition::Required
        );
        assert_eq!(
            surfaces[0].profile.as_deref(),
            Some("ieee802154-controller")
        );

        let excluded_input = r#"
[analysis]

[[analysis.ir]]
id = "ble-all"
sources = ["ble-controller"]
roots = "all"
output = "generated/ble.ir"

[[analysis.public-symbol-families]]
id = "ble-public"
protocols = ["ble"]
source = "ble-controller"
symbol-prefix = "r_ble_"
disposition = "excluded"
reason = "fixture exclusion"
"#;
        let excluded = excluded_input.parse::<DocumentMut>().unwrap();
        let profiles = load_ir_profiles(
            &excluded,
            Path::new("project"),
            ProjectSource::new(Path::new("project.toml"), excluded_input),
        )
        .unwrap();
        let error = load_symbol_family_surfaces(
            &excluded,
            &profiles,
            ProjectSource::new(Path::new("project.toml"), excluded_input),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("overlaps a configured analysis profile")
        );
    }

    #[test]
    fn root_selection_is_explicit_and_prefix_mode_is_nonempty() {
        for (profile, expected) in [
            (
                "id = \"missing\"\noutput = \"missing.json\"\n",
                "requires string \"roots\"",
            ),
            (
                "id = \"all\"\nroots = \"all\"\nsymbol-prefix = \"phy_\"\noutput = \"all.json\"\n",
                "invalid when roots = \"all\"",
            ),
            (
                "id = \"prefix\"\nroots = \"symbol-prefix\"\noutput = \"prefix.json\"\n",
                "requires string \"symbol-prefix\"",
            ),
        ] {
            let input = format!("[[analysis.ir]]\n{profile}");
            let document = input.parse::<DocumentMut>().unwrap();
            let error = load_ir_profiles(
                &document,
                Path::new("project"),
                ProjectSource::new(Path::new("project.toml"), &input),
            )
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}
