//! Stable project entry point composing public target knowledge and local inputs.

use std::{
    fs,
    path::{Path, PathBuf},
};

use toml_edit::{DocumentMut, Item};

use crate::{MemoryMap, Result};

pub(crate) const DEFAULT_PROJECT_MANIFEST: &str = "vendor-validator.toml";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterWorkspacePaths {
    pub(crate) facts: PathBuf,
    pub(crate) overlay: PathBuf,
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
    pub(crate) svd_paths: Vec<PathBuf>,
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
        let registers = document
            .get("registers")
            .map(|item| -> Result<RegisterWorkspacePaths> {
                let table = item
                    .as_table()
                    .ok_or("project manifest registers must be a table")?;
                Ok(RegisterWorkspacePaths {
                    facts: resolve_path(
                        base,
                        table
                            .get("facts")
                            .and_then(Item::as_str)
                            .ok_or("project registers requires string \"facts\"")?,
                    ),
                    overlay: resolve_path(
                        base,
                        table
                            .get("overlay")
                            .and_then(Item::as_str)
                            .ok_or("project registers requires string \"overlay\"")?,
                    ),
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
            svd_paths,
            registers,
            interfaces,
        })
    }

    pub(crate) fn load_memory_map(&self) -> Result<Option<MemoryMap>> {
        self.memory_map.as_deref().map(MemoryMap::load).transpose()
    }
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

[registers]
facts = "generated/mmio.json"
overlay = "registers/reviewed.toml"

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
        assert_eq!(project.svd_paths, [directory.join("registers/base.svd")]);
        assert_eq!(
            project.registers,
            Some(RegisterWorkspacePaths {
                facts: directory.join("generated/mmio.json"),
                overlay: directory.join("registers/reviewed.toml"),
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
}
