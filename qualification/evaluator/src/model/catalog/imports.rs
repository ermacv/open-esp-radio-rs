//! Resolve explicit, repository-relative catalog imports before validation.

use super::{
    BTreeMap, BTreeSet, CAPABILITY_CATALOG_SCHEMA, CatalogDocument, Path, PathBuf, Result,
    read_contained_file, validate_relative_path,
};

type Documents = BTreeMap<PathBuf, (String, CatalogDocument)>;

pub(super) fn load(root: &Path, paths: &[PathBuf]) -> Result<Documents> {
    let mut roots = BTreeSet::new();
    for path in paths {
        if !roots.insert(path.clone()) {
            return Err(format!("qualification program repeats catalog {}", path.display()).into());
        }
    }
    let mut documents = BTreeMap::new();
    let mut active = BTreeSet::new();
    for path in roots {
        visit(root, &path, &mut active, &mut documents)?;
    }
    Ok(documents)
}

fn visit(
    root: &Path,
    path: &Path,
    active: &mut BTreeSet<PathBuf>,
    documents: &mut Documents,
) -> Result<()> {
    validate_relative_path(path)?;
    if documents.contains_key(path) {
        return Ok(());
    }
    if !active.insert(path.to_owned()) {
        return Err(format!("capability catalog import cycle at {}", path.display()).into());
    }
    let input = read_contained_file(root, path, "capability catalog")?;
    let document: CatalogDocument = toml_edit::de::from_str(&input).map_err(|error| {
        format!(
            "cannot parse capability catalog {}: {error}",
            path.display()
        )
    })?;
    if document.schema != CAPABILITY_CATALOG_SCHEMA {
        return Err(format!(
            "unsupported capability catalog schema {} in {} (expected {CAPABILITY_CATALOG_SCHEMA})",
            document.schema,
            path.display()
        )
        .into());
    }
    let mut imports = BTreeSet::new();
    for import in &document.imports {
        if !imports.insert(import) {
            return Err(format!(
                "catalog {} repeats import {}",
                path.display(),
                import.display()
            )
            .into());
        }
        visit(root, import, active, documents)?;
    }
    active.remove(path);
    documents.insert(path.to_owned(), (input, document));
    Ok(())
}
