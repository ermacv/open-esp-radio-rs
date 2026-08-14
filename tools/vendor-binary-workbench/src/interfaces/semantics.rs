//! Reusable semantic operations kept independent from chip table layouts.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    fs,
    path::Path,
};

use toml_edit::{Array, DocumentMut, Item, Table};

use crate::Result;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SemanticOperation {
    pub(crate) id: String,
    pub(crate) domain: String,
    pub(crate) summary: String,
    pub(crate) argument_roles: Vec<String>,
    pub(crate) return_role: String,
    pub(crate) effects: Vec<String>,
    pub(crate) replacement: Option<String>,
    pub(crate) variadic: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SemanticCatalogs {
    operations: BTreeMap<String, SemanticOperation>,
}

impl SemanticCatalogs {
    #[tracing::instrument(name = "load_semantic_catalogs", skip_all, fields(catalogs = paths.len()))]
    pub(crate) fn load(paths: &[impl AsRef<Path>]) -> Result<Self> {
        let mut operations = BTreeMap::new();
        for path in paths {
            let path = path.as_ref();
            let input = fs::read_to_string(path)?;
            let document = input.parse::<DocumentMut>().map_err(|error| {
                crate::error::WorkbenchError::manifest_source(
                    "knowledge pack",
                    path,
                    &input,
                    &error,
                    error.span(),
                )
            })?;
            if document.get("schema").and_then(Item::as_integer) != Some(1) {
                return Err(crate::Error::invalid(format!(
                    "{} requires schema = 1",
                    path.display()
                )));
            }
            let catalog_id = required_string(document.as_item(), "id", "knowledge pack")?;
            validate_dotted_id(&catalog_id, "knowledge pack id")?;
            let tables = document
                .get("operations")
                .and_then(Item::as_array_of_tables)
                .ok_or_else(|| format!("{} requires [[operations]]", path.display()))
                .map_err(crate::Error::invalid)?;
            if tables.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "{} has no semantic operations",
                    path.display()
                )));
            }
            for (index, table) in tables.iter().enumerate() {
                let context = format!("{} operations[{index}]", path.display());
                let operation = parse_operation(table, &context)?;
                match operations.entry(operation.id.clone()) {
                    Entry::Vacant(entry) => {
                        entry.insert(operation);
                    }
                    Entry::Occupied(entry) if entry.get() == &operation => {
                        // Identical knowledge is order-independent and may be
                        // shared by family and chip composition layers.
                    }
                    Entry::Occupied(_) => {
                        return Err(crate::Error::invalid(format!(
                            "conflicting semantic operation {:?} across configured knowledge packs",
                            required_table_string(table, "id", &context)?
                        )));
                    }
                }
            }
        }
        Ok(Self { operations })
    }

    pub(crate) fn get(&self, id: &str) -> Option<&SemanticOperation> {
        self.operations.get(id)
    }

    pub(crate) fn len(&self) -> usize {
        self.operations.len()
    }
}

pub(super) fn validate_dotted_id(value: &str, context: &str) -> Result<()> {
    if value.is_empty()
        || value.split('.').any(|segment| {
            segment.is_empty()
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                || !segment
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_lowercase)
        })
    {
        return Err(crate::Error::invalid(format!(
            "invalid {context} {value:?}"
        )));
    }
    Ok(())
}

fn parse_operation(table: &Table, context: &str) -> Result<SemanticOperation> {
    let operation = SemanticOperation {
        id: required_table_string(table, "id", context)?,
        domain: required_table_string(table, "domain", context)?,
        summary: required_table_string(table, "summary", context)?,
        argument_roles: required_string_array(table, "argument-roles", context)?,
        return_role: optional_table_string(table, "return-role")
            .unwrap_or_else(|| "none".to_owned()),
        effects: required_string_array(table, "effects", context)?,
        replacement: optional_table_string(table, "replacement"),
        variadic: table
            .get("variadic")
            .map(|item| -> Result<bool> {
                item.as_bool().ok_or_else(|| {
                    crate::Error::invalid(format!("{context}.variadic must be a boolean"))
                })
            })
            .transpose()?
            .unwrap_or(false),
    };
    validate_dotted_id(&operation.id, "semantic operation id")?;
    validate_dotted_id(&operation.domain, "semantic domain")?;
    if operation.id != operation.domain
        && !operation.id.starts_with(&format!("{}.", operation.domain))
    {
        return Err(crate::Error::invalid(format!(
            "semantic operation {:?} is outside domain {:?}",
            operation.id, operation.domain
        )));
    }
    if operation.summary.trim().is_empty() {
        return Err(crate::Error::invalid(format!(
            "{context}.summary must not be empty"
        )));
    }
    for role in operation
        .argument_roles
        .iter()
        .chain(std::iter::once(&operation.return_role))
    {
        validate_dotted_id(role, "semantic value role")?;
    }
    if operation.effects.is_empty() {
        return Err(crate::Error::invalid(format!(
            "{context}.effects must not be empty"
        )));
    }
    for effect in &operation.effects {
        validate_dotted_id(effect, "semantic effect")?;
    }
    if let Some(replacement) = &operation.replacement {
        validate_dotted_id(replacement, "semantic replacement hint")?;
    }
    Ok(operation)
}

fn required_string(item: &Item, key: &str, context: &str) -> Result<String> {
    item.get(key)
        .and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires string {key:?}")))
}

fn required_table_string(table: &Table, key: &str, context: &str) -> Result<String> {
    optional_table_string(table, key)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires string {key:?}")))
}

fn optional_table_string(table: &Table, key: &str) -> Option<String> {
    table.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn required_string_array(table: &Table, key: &str, context: &str) -> Result<Vec<String>> {
    let array = table
        .get(key)
        .and_then(Item::as_array)
        .ok_or_else(|| format!("{context} requires array {key:?}"))
        .map_err(crate::Error::invalid)?;
    parse_string_array(array, key, context)
}

fn parse_string_array(array: &Array, key: &str, context: &str) -> Result<Vec<String>> {
    array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                crate::Error::invalid(format!("{context}.{key}[{index}] must be a string"))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(id: &str, summary: &str) -> String {
        format!(
            "schema = 1\nid = \"{id}\"\n[[operations]]\nid = \"rtos.yield-now\"\ndomain = \"rtos\"\nsummary = \"{summary}\"\nargument-roles = []\nreturn-role = \"none\"\neffects = [\"scheduler.yield-request\"]\n"
        )
    }

    #[test]
    fn composition_deduplicates_identical_knowledge_and_rejects_conflicts() {
        let root = std::env::temp_dir().join(format!(
            "workbench-knowledge-composition-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.toml");
        let same = root.join("same.toml");
        let conflict = root.join("conflict.toml");
        std::fs::write(&first, catalog("fixture.first", "Yield")).unwrap();
        std::fs::write(&same, catalog("fixture.same", "Yield")).unwrap();
        std::fs::write(&conflict, catalog("fixture.conflict", "Different meaning")).unwrap();

        assert_eq!(SemanticCatalogs::load(&[&first, &same]).unwrap().len(), 1);
        let error = SemanticCatalogs::load(&[&first, &conflict]).unwrap_err();
        assert!(error.to_string().contains("conflicting semantic operation"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
