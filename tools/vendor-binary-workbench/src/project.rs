//! Stable project entry point composing public target knowledge and local inputs.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;
use toml_edit::{DocumentMut, Item};

use crate::{
    Result,
    platform_pack::PlatformPack,
    project_analysis::{
        NavigationIndexSpec, SymbolInventorySpec, load_navigation_index, load_symbol_inventory,
    },
    project_ir::{ProjectIrProfile, load_ir_profiles},
};

pub(crate) const DEFAULT_PROJECT_MANIFEST: &str = "vendor-project.toml";

#[derive(Debug, Error, Diagnostic)]
pub(crate) enum ProjectError {
    #[error("cannot read project manifest {}", path.display())]
    #[diagnostic(code(workbench::project::read))]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{message}")]
    #[diagnostic(code(workbench::project::parse))]
    Parse {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("invalid TOML")]
        span: SourceSpan,
    },
    #[error("{message}")]
    #[diagnostic(code(workbench::project::invalid))]
    Invalid {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("invalid project configuration")]
        span: SourceSpan,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterWorkspacePaths {
    pub(crate) facts: PathBuf,
    pub(crate) model: PathBuf,
    pub(crate) review_output: Option<PathBuf>,
    pub(crate) review_ir_reports: Vec<PathBuf>,
    pub(crate) svd_output: Option<PathBuf>,
    pub(crate) pac: Option<PacOutputSpec>,
    pub(crate) bindings: Option<PacBindingsOutputSpec>,
    pub(crate) api_pack: Option<PathBuf>,
    pub(crate) lint_pack: Option<PathBuf>,
    pub(crate) evidence_catalogs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PacOutputSpec {
    pub(crate) output: PathBuf,
    pub(crate) target: String,
    pub(crate) edition: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PacBindingsOutputSpec {
    pub(crate) output: PathBuf,
    pub(crate) crate_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceWorkspacePaths {
    pub(crate) facts: PathBuf,
    pub(crate) pack: Option<PathBuf>,
    pub(crate) semantic_catalogs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionWorkspacePaths {
    pub(crate) pack: PathBuf,
    pub(crate) profiles: Vec<String>,
    pub(crate) review_output: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectSpec {
    pub(crate) id: String,
    pub(crate) target_spec: PathBuf,
    pub(crate) platform_pack: Option<PlatformPack>,
    pub(crate) run_spec: Option<PathBuf>,
    pub(crate) memory_map: Option<PathBuf>,
    pub(crate) svd_configured: bool,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) symbol_inventory: Option<SymbolInventorySpec>,
    pub(crate) navigation_index: Option<NavigationIndexSpec>,
    pub(crate) ir_profiles: Vec<ProjectIrProfile>,
    pub(crate) registers: Option<RegisterWorkspacePaths>,
    pub(crate) interfaces: Option<InterfaceWorkspacePaths>,
    pub(crate) functions: Option<FunctionWorkspacePaths>,
}

impl ProjectSpec {
    pub(crate) fn discover_from(start: &Path) -> Result<Option<PathBuf>> {
        for directory in start.ancestors() {
            let current = directory.join(DEFAULT_PROJECT_MANIFEST);
            if current.is_file() {
                return Ok(Some(current));
            }
        }
        Ok(None)
    }

    pub(crate) fn load(path: &Path) -> Result<Self> {
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
        let memory_map =
            optional_string(&document, "memory-map").map(|path| resolve_path(base, &path));
        let svd_configured = document.get("svd").is_some();
        let svd_paths = document
            .get("svd")
            .map(|item| {
                let array = item
                    .as_array()
                    .ok_or("project manifest svd must be an array of paths")?;
                array
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        value
                            .as_str()
                            .map(|path| resolve_path(base, path))
                            .ok_or_else(|| {
                                format!("project manifest svd[{index}] must be a string").into()
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
                    .ok_or("project manifest registers must be a table")?;
                if table.contains_key("overlay") {
                    return Err("unknown project registers key \"overlay\"; use the schema-2 \"model\" workspace".into());
                }
                let model = table
                    .get("model")
                    .and_then(Item::as_str)
                    .ok_or("project registers requires string \"model\"")?;
                let review_output = nested_output_path(table, base, "review")?;
                let review_ir_reports =
                    nested_path_array(table, base, "review", "linked-ir")?;
                let svd_output = nested_output_path(table, base, "svd")?;
                let pac = table
                    .get("pac")
                    .map(|item| -> Result<PacOutputSpec> {
                        let pac = item
                            .as_table()
                            .ok_or("project registers.pac must be a table")?;
                        let target = pac
                            .get("target")
                            .and_then(Item::as_str)
                            .unwrap_or("none")
                            .to_owned();
                        if !matches!(target.as_str(), "none" | "riscv") {
                            return Err(format!(
                                "project registers.pac target must be \"none\" or \"riscv\", got {target:?}"
                            )
                            .into());
                        }
                        let edition = pac
                            .get("edition")
                            .and_then(Item::as_str)
                            .unwrap_or("2024")
                            .to_owned();
                        if !matches!(edition.as_str(), "2021" | "2024") {
                            return Err(format!(
                                "project registers.pac edition must be \"2021\" or \"2024\", got {edition:?}"
                            )
                            .into());
                        }
                        Ok(PacOutputSpec {
                            output: resolve_path(
                                base,
                                pac.get("output").and_then(Item::as_str).ok_or(
                                    "project registers.pac requires string \"output\"",
                                )?,
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
                            .ok_or("project registers.bindings must be a table")?;
                        let crate_name = bindings
                            .get("crate-name")
                            .and_then(Item::as_str)
                            .ok_or("project registers.bindings requires string \"crate-name\"")?;
                        open_esp_radio_register_model::validate_pac_crate_name(crate_name)?;
                        Ok(PacBindingsOutputSpec {
                            output: resolve_path(
                                base,
                                bindings.get("output").and_then(Item::as_str).ok_or(
                                    "project registers.bindings requires string \"output\"",
                                )?,
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
                            .ok_or("project registers.api must be a table")?;
                        Ok(resolve_path(
                            base,
                            api.get("pack")
                                .and_then(Item::as_str)
                                .ok_or("project registers.api requires string \"pack\"")?,
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
                            .ok_or("project registers.lints must be a table")?;
                        Ok(resolve_path(
                            base,
                            lints
                                .get("pack")
                                .and_then(Item::as_str)
                                .ok_or("project registers.lints requires string \"pack\"")?,
                        ))
                    })
                    .transpose()?;
                Ok(RegisterWorkspacePaths {
                    facts: resolve_path(
                        base,
                        table
                            .get("facts")
                            .and_then(Item::as_str)
                            .ok_or("project registers requires string \"facts\"")?,
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
                    .ok_or("project manifest interfaces must be a table")?;
                if table.contains_key("semantic-catalogs") {
                    return Err("unknown project interfaces key \"semantic-catalogs\"; semantic catalogs belong to the platform pack".into());
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
                            .ok_or("project interfaces requires string \"facts\"")?,
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
                    .ok_or("project manifest functions must be a table")?;
                let profiles = table
                    .get("profiles")
                    .map(|item| {
                        let values = item
                            .as_array()
                            .ok_or("project functions.profiles must be an array")?;
                        values
                            .iter()
                            .enumerate()
                            .map(|(index, value)| {
                                value.as_str().map(str::to_owned).ok_or_else(|| {
                                    format!("project functions.profiles[{index}] must be a string")
                                        .into()
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
                    return Err(
                        "project [functions] requires at least one [[analysis.ir]] profile".into(),
                    );
                }
                let mut seen = std::collections::BTreeSet::new();
                for profile in &profiles {
                    if !seen.insert(profile) {
                        return Err(
                            format!("duplicate project functions profile {profile:?}").into()
                        );
                    }
                    if !ir_profiles.iter().any(|candidate| candidate.id == *profile) {
                        return Err(format!(
                            "project functions refers to unknown IR profile {profile:?}"
                        )
                        .into());
                    }
                }
                let review_output = table
                    .get("review")
                    .map(|item| -> Result<PathBuf> {
                        let review = item
                            .as_table()
                            .ok_or("project functions.review must be a table")?;
                        review
                            .get("output")
                            .and_then(Item::as_str)
                            .map(|path| resolve_path(base, path))
                            .ok_or_else(|| {
                                "project functions.review requires string \"output\"".into()
                            })
                    })
                    .transpose()?;
                Ok(FunctionWorkspacePaths {
                    pack: resolve_path(
                        base,
                        table
                            .get("pack")
                            .and_then(Item::as_str)
                            .ok_or("project functions requires string \"pack\"")?,
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
                return Err(format!(
                    "project symbol inventory reuses another analysis facts path {}",
                    path.display()
                )
                .into());
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
                return Err(format!(
                    "project navigation index reuses another analysis facts path {}",
                    path.display()
                )
                .into());
            }
        }
        Ok(Self {
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

    pub(crate) fn function_ir_reports(&self) -> Result<Vec<(String, PathBuf)>> {
        let Some(functions) = &self.functions else {
            return Ok(Vec::new());
        };
        functions
            .profiles
            .iter()
            .map(|id| {
                self.ir_profiles
                    .iter()
                    .find(|profile| profile.id == *id)
                    .map(|profile| (id.clone(), profile.output.clone()))
                    .ok_or_else(|| format!("unknown function workspace IR profile {id:?}").into())
            })
            .collect()
    }
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
                .ok_or_else(|| format!("project registers.{name} must be a table"))?
                .get("output")
                .and_then(Item::as_str)
                .ok_or_else(|| format!("project registers.{name} requires string \"output\""))?;
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
        .ok_or_else(|| format!("project registers.{table_name} must be a table"))?;
    let Some(item) = table.get(key) else {
        return Ok(Vec::new());
    };
    let values = item
        .as_array()
        .ok_or_else(|| format!("project registers.{table_name}.{key} must be an array"))?;
    let mut seen = std::collections::BTreeSet::new();
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let value = value.as_str().ok_or_else(|| {
                format!("project registers.{table_name}.{key}[{index}] must be a string")
            })?;
            let path = resolve_path(base, value);
            if !seen.insert(path.clone()) {
                return Err(format!(
                    "duplicate project registers.{table_name}.{key} path {value:?}"
                )
                .into());
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
        return Err(format!("invalid project id {value:?}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_composed_specs_relative_to_the_project() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-workbench-project-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(DEFAULT_PROJECT_MANIFEST);
        std::fs::write(
            &path,
            r#"
schema = 1
id = "fixture"
target-spec = "target.spec"
run-spec = "local.run"
memory-map = "memory.toml"
svd = ["registers/base.svd"]

[analysis.symbols]
output = "generated/symbols.json"

[analysis.navigation]
output = "generated/navigation.json"

[[analysis.ir]]
id = "vendor"
sources = ["rom", "archive"]
symbol-prefix = "phy_"
include-reachable = true
output = "generated/vendor.ir.json"
pseudo-rust = "generated/vendor.pseudo.rs"

[registers]
facts = "generated/mmio.json"
model = "registers/reviewed.toml"

[registers.review]
output = "generated/register-review.md"
linked-ir = ["generated/vendor.ir.json"]

[registers.svd]
output = "generated/device.svd"

[registers.pac]
output = "generated/pac/src/lib.rs"
target = "none"
edition = "2024"

[registers.bindings]
output = "generated/device.bindings"
crate-name = "fixture_pac"

[registers.api]
pack = "registers/api.toml"

[registers.lints]
pack = "registers/lints.toml"

[registers.evidence]
catalogs = ["registers/evidence.toml"]

[interfaces]
facts = "generated/interfaces.json"
pack = "interfaces/reviewed.toml"

[functions]
pack = "functions/reviewed.toml"
profiles = ["vendor"]

[functions.review]
output = "generated/function-review.md"
"#,
        )
        .unwrap();

        let project = ProjectSpec::load(&path).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(project.id, "fixture");
        assert_eq!(project.target_spec, directory.join("target.spec"));
        assert_eq!(project.run_spec, Some(directory.join("local.run")));
        assert_eq!(project.memory_map, Some(directory.join("memory.toml")));
        assert!(project.svd_configured);
        assert_eq!(project.svd_paths, [directory.join("registers/base.svd")]);
        assert_eq!(
            project.symbol_inventory,
            Some(SymbolInventorySpec {
                output: directory.join("generated/symbols.json"),
            })
        );
        assert_eq!(
            project.navigation_index,
            Some(NavigationIndexSpec {
                output: directory.join("generated/navigation.json"),
            })
        );
        assert_eq!(
            project.ir_profiles,
            [ProjectIrProfile {
                id: "vendor".to_owned(),
                sources: vec!["rom".to_owned(), "archive".to_owned()],
                symbol_prefix: "phy_".to_owned(),
                include_reachable: true,
                entry_contract: "none".to_owned(),
                output: directory.join("generated/vendor.ir.json"),
                pseudo_rust: Some(directory.join("generated/vendor.pseudo.rs")),
            }]
        );
        assert_eq!(
            project.registers,
            Some(RegisterWorkspacePaths {
                facts: directory.join("generated/mmio.json"),
                model: directory.join("registers/reviewed.toml"),
                review_output: Some(directory.join("generated/register-review.md")),
                review_ir_reports: vec![directory.join("generated/vendor.ir.json")],
                svd_output: Some(directory.join("generated/device.svd")),
                pac: Some(PacOutputSpec {
                    output: directory.join("generated/pac/src/lib.rs"),
                    target: "none".to_owned(),
                    edition: "2024".to_owned(),
                }),
                bindings: Some(PacBindingsOutputSpec {
                    output: directory.join("generated/device.bindings"),
                    crate_name: "fixture_pac".to_owned(),
                }),
                api_pack: Some(directory.join("registers/api.toml")),
                lint_pack: Some(directory.join("registers/lints.toml")),
                evidence_catalogs: vec![directory.join("registers/evidence.toml")],
            })
        );
        assert_eq!(
            project.interfaces,
            Some(InterfaceWorkspacePaths {
                facts: directory.join("generated/interfaces.json"),
                pack: Some(directory.join("interfaces/reviewed.toml")),
                semantic_catalogs: vec![],
            })
        );
        assert_eq!(
            project.functions,
            Some(FunctionWorkspacePaths {
                pack: directory.join("functions/reviewed.toml"),
                profiles: vec!["vendor".to_owned()],
                review_output: Some(directory.join("generated/function-review.md")),
            })
        );
        assert_eq!(
            project.function_ir_reports().unwrap(),
            [(
                "vendor".to_owned(),
                directory.join("generated/vendor.ir.json")
            )]
        );
    }

    #[test]
    fn discovers_the_nearest_project_manifest_from_a_child_directory() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-workbench-project-discovery-{}",
            std::process::id()
        ));
        let child = directory.join("generated/findings");
        std::fs::create_dir_all(&child).unwrap();
        let manifest = directory.join(DEFAULT_PROJECT_MANIFEST);
        std::fs::write(&manifest, "schema = 1\n").unwrap();

        assert_eq!(ProjectSpec::discover_from(&child).unwrap(), Some(manifest));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_removed_workspace_configuration_keys() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-workbench-project-removed-keys-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let registers = directory.join("registers.toml");
        std::fs::write(
            &registers,
            "schema = 1\nid = \"fixture\"\ntarget-spec = \"target.spec\"\n[registers]\nfacts = \"facts.json\"\noverlay = \"reviewed.toml\"\n",
        )
        .unwrap();
        assert!(
            ProjectSpec::load(&registers)
                .unwrap_err()
                .to_string()
                .contains("unknown project registers key \"overlay\"")
        );

        let interfaces = directory.join("interfaces.toml");
        std::fs::write(
            &interfaces,
            "schema = 1\nid = \"fixture\"\ntarget-spec = \"target.spec\"\n[interfaces]\nfacts = \"facts.json\"\nsemantic-catalogs = []\n",
        )
        .unwrap();
        assert!(
            ProjectSpec::load(&interfaces)
                .unwrap_err()
                .to_string()
                .contains("semantic catalogs belong to the platform pack")
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_generic_analysis_output_collisions() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-workbench-project-analysis-collision-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join(DEFAULT_PROJECT_MANIFEST);
        std::fs::write(
            &manifest,
            r#"
schema = 1
id = "fixture"
target-spec = "target.spec"

[analysis.symbols]
output = "generated/facts.json"

[registers]
facts = "generated/facts.json"
model = "registers/device.toml"
"#,
        )
        .unwrap();
        assert!(
            ProjectSpec::load(&manifest)
                .unwrap_err()
                .to_string()
                .contains("reuses another analysis facts path")
        );
        std::fs::write(
            &manifest,
            r#"
schema = 1
id = "fixture"
target-spec = "target.spec"

[analysis.symbols]
output = "generated/symbols.json"

[analysis.navigation]
output = "generated/facts.json"

[interfaces]
facts = "generated/facts.json"
"#,
        )
        .unwrap();
        assert!(
            ProjectSpec::load(&manifest)
                .unwrap_err()
                .to_string()
                .contains("navigation index reuses another analysis facts path")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn distinguishes_an_explicit_empty_svd_catalog_from_an_omitted_one() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-workbench-project-empty-svd-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let explicit = directory.join("explicit.toml");
        let omitted = directory.join("omitted.toml");
        std::fs::write(
            &explicit,
            "schema = 1\nid = \"explicit\"\ntarget-spec = \"target.spec\"\nsvd = []\n",
        )
        .unwrap();
        std::fs::write(
            &omitted,
            "schema = 1\nid = \"omitted\"\ntarget-spec = \"target.spec\"\n",
        )
        .unwrap();

        let explicit = ProjectSpec::load(&explicit).unwrap();
        let omitted = ProjectSpec::load(&omitted).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert!(explicit.svd_configured);
        assert!(explicit.svd_paths.is_empty());
        assert!(!omitted.svd_configured);
        assert!(omitted.svd_paths.is_empty());
    }
}
