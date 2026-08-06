//! Stable project entry point composing public target knowledge and local inputs.

use std::{
    fs,
    path::{Path, PathBuf},
};

use toml_edit::{DocumentMut, Item};

use crate::{
    MemoryMap, Result,
    project_ir::{ProjectIrProfile, load_ir_profiles},
};

pub(crate) const DEFAULT_PROJECT_MANIFEST: &str = "vendor-validator.toml";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterWorkspacePaths {
    pub(crate) facts: PathBuf,
    pub(crate) model: PathBuf,
    pub(crate) review_output: Option<PathBuf>,
    pub(crate) review_ir_reports: Vec<PathBuf>,
    pub(crate) svd_output: Option<PathBuf>,
    pub(crate) pac: Option<PacOutputSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PacOutputSpec {
    pub(crate) output: PathBuf,
    pub(crate) target: String,
    pub(crate) edition: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceWorkspacePaths {
    pub(crate) facts: PathBuf,
    pub(crate) pack: Option<PathBuf>,
    pub(crate) semantic_catalogs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectSpec {
    pub(crate) id: String,
    pub(crate) target_spec: PathBuf,
    pub(crate) run_spec: Option<PathBuf>,
    pub(crate) memory_map: Option<PathBuf>,
    pub(crate) svd_configured: bool,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) ir_profiles: Vec<ProjectIrProfile>,
    pub(crate) registers: Option<RegisterWorkspacePaths>,
    pub(crate) interfaces: Option<InterfaceWorkspacePaths>,
}

impl ProjectSpec {
    pub(crate) fn discover_from(start: &Path) -> Option<PathBuf> {
        start
            .ancestors()
            .map(|directory| directory.join(DEFAULT_PROJECT_MANIFEST))
            .find(|candidate| candidate.is_file())
    }

    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let document = input.parse::<DocumentMut>()?;
        if document.get("schema").and_then(Item::as_integer) != Some(1) {
            return Err("project manifest requires schema = 1".into());
        }
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let id = required_string(&document, "id")?;
        validate_id(&id)?;
        let target_spec = resolve_path(base, &required_string(&document, "target-spec")?);
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
        let registers = document
            .get("registers")
            .map(|item| -> Result<RegisterWorkspacePaths> {
                let table = item
                    .as_table()
                    .ok_or("project manifest registers must be a table")?;
                let model = match (
                    table.get("model").and_then(Item::as_str),
                    table.get("overlay").and_then(Item::as_str),
                ) {
                    (Some(model), None) | (None, Some(model)) => model,
                    (Some(_), Some(_)) => {
                        return Err(
                            "project registers cannot define both \"model\" and legacy \"overlay\""
                                .into(),
                        );
                    }
                    (None, None) => {
                        return Err(
                            "project registers requires string \"model\" (or legacy \"overlay\")"
                                .into(),
                        );
                    }
                };
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
                })
            })
            .transpose()?;
        let interfaces = document
            .get("interfaces")
            .map(|item| -> Result<InterfaceWorkspacePaths> {
                let table = item
                    .as_table()
                    .ok_or("project manifest interfaces must be a table")?;
                let semantic_catalogs = table
                    .get("semantic-catalogs")
                    .map(|item| {
                        let array = item.as_array().ok_or(
                            "project interfaces semantic-catalogs must be an array of paths",
                        )?;
                        array
                            .iter()
                            .enumerate()
                            .map(|(index, value)| {
                                value
                                    .as_str()
                                    .map(|path| resolve_path(base, path))
                                    .ok_or_else(|| {
                                        format!(
                                            "project interfaces semantic-catalogs[{index}] must be a string"
                                        )
                                        .into()
                                    })
                            })
                            .collect::<Result<Vec<_>>>()
                    })
                    .transpose()?
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
        Ok(Self {
            id,
            target_spec,
            run_spec,
            memory_map,
            svd_configured,
            svd_paths,
            ir_profiles,
            registers,
            interfaces,
        })
    }

    pub(crate) fn load_memory_map(&self) -> Result<Option<MemoryMap>> {
        self.memory_map.as_deref().map(MemoryMap::load).transpose()
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

fn required_string(document: &DocumentMut, key: &str) -> Result<String> {
    optional_string(document, key)
        .ok_or_else(|| format!("project manifest requires string {key:?}").into())
}

fn optional_string(document: &DocumentMut, key: &str) -> Option<String> {
    document.get(key).and_then(Item::as_str).map(str::to_owned)
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
            "open-radio-validator-project-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("vendor-validator.toml");
        std::fs::write(
            &path,
            r#"
schema = 1
id = "fixture"
target-spec = "target.spec"
run-spec = "local.run"
memory-map = "memory.toml"
svd = ["registers/base.svd"]

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

[interfaces]
facts = "generated/interfaces.json"
pack = "interfaces/reviewed.toml"
semantic-catalogs = ["interfaces/embedded-semantics.toml"]
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
            })
        );
        assert_eq!(
            project.interfaces,
            Some(InterfaceWorkspacePaths {
                facts: directory.join("generated/interfaces.json"),
                pack: Some(directory.join("interfaces/reviewed.toml")),
                semantic_catalogs: vec![directory.join("interfaces/embedded-semantics.toml")],
            })
        );
    }

    #[test]
    fn discovers_the_nearest_project_manifest_from_a_child_directory() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-validator-project-discovery-{}",
            std::process::id()
        ));
        let child = directory.join("generated/findings");
        std::fs::create_dir_all(&child).unwrap();
        let manifest = directory.join(DEFAULT_PROJECT_MANIFEST);
        std::fs::write(&manifest, "schema = 1\n").unwrap();

        assert_eq!(ProjectSpec::discover_from(&child), Some(manifest));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn distinguishes_an_explicit_empty_svd_catalog_from_an_omitted_one() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-validator-project-empty-svd-{}",
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
