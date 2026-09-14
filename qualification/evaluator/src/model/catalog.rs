//! Canonical capability catalogs and source-owned domain inventory.

use super::{
    BTreeMap, BTreeSet, CapabilityDocument, DispositionIndex, HilConfig, ManifestDocument, Path,
    PathBuf, QUALIFICATION_SCHEMA, Result, ScenarioCatalog, Sha256, StaticContext,
    VerificationConfig, fs, slug, validate_capability_declaration, validate_relative_path,
};
use serde::Deserialize;
use sha2::Digest as _;

pub(crate) const CAPABILITY_CATALOG_SCHEMA: u16 = 2;

#[derive(Clone, Debug, Default)]
pub(crate) struct CatalogView {
    pub(crate) sources: Vec<SourceIdentity>,
    pub(crate) capabilities: BTreeMap<String, CapabilityDocument>,
    pub(crate) scopes: BTreeMap<String, CapabilityScope>,
    pub(crate) capability_owners: BTreeMap<String, (String, PathBuf)>,
    pub(crate) source_facts: BTreeMap<String, SourceFact>,
    pub(crate) sections: Vec<InventorySection>,
    pub(crate) items: Vec<InventoryItem>,
    pub(crate) references: Vec<InventoryReference>,
}

#[derive(Clone, Debug)]
pub(crate) struct SourceIdentity {
    pub(crate) id: String,
    pub(crate) schema: u16,
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug)]
pub(crate) enum CapabilityOrigin {
    Program,
    Catalog { id: String, path: PathBuf },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CapabilityLevel {
    NativeSilicon,
    LowerPrimitive,
    ComposedProduct,
}

impl CapabilityLevel {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::NativeSilicon => "native-silicon",
            Self::LowerPrimitive => "lower-primitive",
            Self::ComposedProduct => "composed-product",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SourceStatus {
    Implemented,
    Partial,
    FailClosed,
    Absent,
    HostOnly,
    Diagnostic,
}

impl SourceStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Implemented => "IMPLEMENTED",
            Self::Partial => "PARTIAL",
            Self::FailClosed => "FAIL-CLOSED",
            Self::Absent => "ABSENT",
            Self::HostOnly => "HOST-ONLY",
            Self::Diagnostic => "DIAGNOSTIC",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct CapabilityScope {
    pub(crate) chip: String,
    pub(crate) role: String,
    pub(crate) phy: String,
    pub(crate) security: Vec<String>,
    pub(crate) composition: String,
    pub(crate) level: CapabilityLevel,
    pub(crate) activation_boundary: String,
    pub(crate) limitations: String,
}

impl CapabilityScope {
    fn validate(&self, capability: &str) -> Result<()> {
        slug(&self.chip, "catalog scope chip")?;
        slug(&self.role, "catalog scope role")?;
        slug(&self.phy, "catalog scope PHY")?;
        slug(&self.composition, "catalog scope composition")?;
        if self.security.is_empty() {
            return Err(format!("catalog capability {capability} has no security scope").into());
        }
        let mut security = BTreeSet::new();
        for value in &self.security {
            let value = slug(value, "catalog security scope")?;
            if !security.insert(value.clone()) {
                return Err(format!(
                    "catalog capability {capability} repeats security scope {value}"
                )
                .into());
            }
        }
        if self.activation_boundary.trim().is_empty() || self.limitations.trim().is_empty() {
            return Err(format!(
                "catalog capability {capability} requires activation-boundary and limitations"
            )
            .into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct InventorySection {
    pub(crate) id: String,
    pub(crate) domain: String,
    pub(crate) title: String,
    pub(crate) source_document: PathBuf,
    #[serde(default)]
    pub(crate) overview: String,
    #[serde(default)]
    pub(crate) source_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct InventoryItem {
    pub(crate) id: String,
    pub(crate) section: String,
    pub(crate) title: String,
    pub(crate) status: SourceStatus,
    pub(crate) level: CapabilityLevel,
    pub(crate) scope_and_limitations: String,
    pub(crate) source_paths: Vec<PathBuf>,
    pub(crate) source_fact: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum InventoryReferenceKind {
    QualificationMapping,
    SourceReference,
}

impl InventoryReferenceKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::QualificationMapping => "qualification-mapping",
            Self::SourceReference => "source-reference",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct InventoryReference {
    pub(crate) id: String,
    pub(crate) section: String,
    pub(crate) title: String,
    pub(crate) kind: InventoryReferenceKind,
    pub(crate) details: String,
    #[serde(default)]
    pub(crate) source_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct SourceFact {
    pub(crate) id: String,
    pub(crate) status: SourceStatus,
    pub(crate) level: CapabilityLevel,
    pub(crate) source_contract: super::SourceContract,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct InventoryItemDocument {
    id: String,
    section: String,
    title: String,
    #[serde(default)]
    status: Option<SourceStatus>,
    #[serde(default)]
    level: Option<CapabilityLevel>,
    #[serde(default)]
    scope_and_limitations: Option<String>,
    #[serde(default)]
    source_paths: Vec<PathBuf>,
    #[serde(default)]
    source_fact: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct CatalogValidation {
    verification_project: PathBuf,
    hil_catalog: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct CatalogDocument {
    schema: u16,
    id: String,
    validation: Option<CatalogValidation>,
    #[serde(default)]
    capabilities: Vec<CapabilityDocument>,
    #[serde(default)]
    sections: Vec<InventorySection>,
    #[serde(default)]
    items: Vec<InventoryItemDocument>,
    #[serde(default)]
    source_facts: Vec<SourceFact>,
    #[serde(default)]
    references: Vec<InventoryReference>,
}

struct LoadedCatalog {
    id: String,
    path: PathBuf,
    verification_project: PathBuf,
    hil_catalog: PathBuf,
    document: CatalogDocument,
}

impl CatalogView {
    pub(crate) fn load(root: &Path, paths: &[PathBuf]) -> Result<Self> {
        Self::load_with_program(root, paths, None, &BTreeSet::new())
    }

    pub(crate) fn load_for_program(root: &Path, path: &Path) -> Result<Self> {
        let program = ManifestDocument::load_and_validate(path, root)?;
        if program.catalog().sources.is_empty() {
            return Err("qualification program does not select a capability catalog".into());
        }
        Ok(program.catalog().clone())
    }

    fn load_with_program(
        root: &Path,
        paths: &[PathBuf],
        fallback: Option<(&VerificationConfig, &HilConfig)>,
        allowed_external: &BTreeSet<String>,
    ) -> Result<Self> {
        if paths.is_empty() {
            return Err("no capability catalogs were selected".into());
        }
        let mut view = Self::default();
        let mut catalog_paths = BTreeSet::new();
        let mut catalog_ids = BTreeSet::new();
        let mut capability_owners = BTreeMap::new();
        let mut section_ids = BTreeSet::new();
        let mut item_ids = BTreeSet::new();
        let mut reference_ids = BTreeSet::new();
        let mut loaded_catalogs = Vec::new();

        let mut ordered_paths = paths.to_vec();
        ordered_paths.sort();
        for relative in &ordered_paths {
            validate_relative_path(relative)?;
            if !catalog_paths.insert(relative.clone()) {
                return Err(format!(
                    "qualification program repeats catalog {}",
                    relative.display()
                )
                .into());
            }
            let input = read_contained_file(root, relative, "capability catalog")?;
            let mut document: CatalogDocument =
                toml_edit::de::from_str(&input).map_err(|error| {
                    format!(
                        "cannot parse capability catalog {}: {error}",
                        relative.display()
                    )
                })?;
            if document.schema != CAPABILITY_CATALOG_SCHEMA {
                return Err(format!(
                    "unsupported capability catalog schema {} in {} (expected {CAPABILITY_CATALOG_SCHEMA})",
                    document.schema,
                    relative.display()
                ).into());
            }
            let catalog_id = slug(&document.id, "capability catalog id")?;
            if !catalog_ids.insert(catalog_id.clone()) {
                return Err(format!("duplicate capability catalog id {catalog_id}").into());
            }
            if document.capabilities.is_empty()
                && document.items.is_empty()
                && document.source_facts.is_empty()
                && document.references.is_empty()
            {
                return Err(format!("capability catalog {catalog_id} is empty").into());
            }
            let (verification_project, hil_catalog) = match (document.validation.take(), fallback) {
                (Some(validation), _) => (validation.verification_project, validation.hil_catalog),
                (None, Some((verification, hil))) => (verification.project.clone(), hil.catalog.clone()),
                (None, None) => return Err(format!(
                    "capability catalog {catalog_id} needs [validation] for standalone static checking"
                ).into()),
            };
            validate_relative_path(&verification_project)?;
            validate_relative_path(&hil_catalog)?;
            view.sources.push(SourceIdentity {
                id: catalog_id.clone(),
                schema: document.schema,
                path: relative.clone(),
                sha256: sha256(input.as_bytes()),
            });
            loaded_catalogs.push(LoadedCatalog {
                id: catalog_id,
                path: relative.clone(),
                verification_project,
                hil_catalog,
                document,
            });
        }

        for loaded in &mut loaded_catalogs {
            for fact in std::mem::take(&mut loaded.document.source_facts) {
                let id = slug(&fact.id, "source fact id")?;
                let contract_id = slug(&fact.source_contract.id, "source contract")?;
                if id != contract_id {
                    return Err(format!(
                        "source fact {id} must own a source contract with the same id, not {contract_id}"
                    )
                    .into());
                }
                super::source_contract::validate(
                    std::slice::from_ref(&fact.source_contract),
                    root,
                )?;
                if view.source_facts.insert(id.clone(), fact).is_some() {
                    return Err(format!("duplicate source fact {id}").into());
                }
            }
        }

        for loaded in loaded_catalogs {
            let dispositions =
                DispositionIndex::load_project(&root.join(&loaded.verification_project))?;
            let scenarios = ScenarioCatalog::load(root, &loaded.hil_catalog)?;
            let context = StaticContext {
                root,
                dispositions: &dispositions,
                scenario_catalog: &scenarios,
            };
            for mut capability in loaded.document.capabilities {
                let id = slug(&capability.id, "capability id")?;
                let mut fact_refs = BTreeSet::new();
                let explicit_contracts = std::mem::take(&mut capability.source_contracts);
                for reference in &capability.source_fact_refs {
                    let reference = slug(reference, "source fact reference")?;
                    if !fact_refs.insert(reference.clone()) {
                        return Err(format!(
                            "catalog capability {id} repeats source fact reference {reference}"
                        )
                        .into());
                    }
                    let fact = view.source_facts.get(&reference).ok_or_else(|| {
                        format!(
                            "catalog capability {id} references missing source fact {reference}"
                        )
                    })?;
                    if explicit_contracts
                        .iter()
                        .any(|contract| contract.id == reference)
                    {
                        return Err(format!(
                            "catalog capability {id} cannot override referenced source fact {reference}"
                        )
                        .into());
                    }
                    capability
                        .source_contracts
                        .push(fact.source_contract.clone());
                }
                capability.source_contracts.extend(explicit_contracts);
                let scope = capability.catalog_scope.as_ref().ok_or_else(|| {
                    format!("catalog capability {id} requires catalog-scope metadata")
                })?;
                scope.validate(&id)?;
                validate_capability_declaration(&capability, &context)?;
                if let Some(previous) =
                    capability_owners.insert(id.clone(), (loaded.id.clone(), loaded.path.clone()))
                {
                    return Err(format!(
                        "capability {id} is declared by both catalog {} and {}",
                        previous.0, loaded.id
                    )
                    .into());
                }
                view.scopes.insert(id.clone(), scope.clone());
                view.capability_owners
                    .insert(id.clone(), (loaded.id.clone(), loaded.path.clone()));
                view.capabilities.insert(id, capability);
            }
            for section in loaded.document.sections {
                let id = slug(&section.id, "inventory section id")?;
                slug(&section.domain, "inventory domain")?;
                if section.title.trim().is_empty() {
                    return Err(format!("inventory section {id} has no title").into());
                }
                validate_regular_reference(
                    root,
                    &section.source_document,
                    "inventory source document",
                )?;
                let mut paths = BTreeSet::new();
                for path in &section.source_paths {
                    if !paths.insert(path) {
                        return Err(format!(
                            "inventory section {id} repeats source path {}",
                            path.display()
                        )
                        .into());
                    }
                    validate_regular_reference(root, path, "inventory section source path")?;
                }
                if !section_ids.insert(id.clone()) {
                    return Err(format!("duplicate inventory section {id}").into());
                }
                view.sections.push(section);
            }
            for item in loaded.document.items {
                let id = slug(&item.id, "inventory item id")?;
                slug(&item.section, "inventory section reference")?;
                if item.title.trim().is_empty() {
                    return Err(format!("inventory item {id} requires a title").into());
                }
                let resolved = if let Some(reference) = item.source_fact {
                    let reference = slug(&reference, "source fact reference")?;
                    if item.status.is_some()
                        || item.level.is_some()
                        || item.scope_and_limitations.is_some()
                        || !item.source_paths.is_empty()
                    {
                        return Err(format!(
                            "inventory projection {id} cannot override source fact {reference}"
                        )
                        .into());
                    }
                    let fact = view.source_facts.get(&reference).ok_or_else(|| {
                        format!("inventory item {id} references missing source fact {reference}")
                    })?;
                    InventoryItem {
                        id: id.clone(),
                        section: item.section,
                        title: item.title,
                        status: fact.status,
                        level: fact.level,
                        scope_and_limitations: format!(
                            "{}\n\nLimits: {}",
                            fact.source_contract.scope, fact.source_contract.limits
                        ),
                        source_paths: fact.source_contract.source_paths.clone(),
                        source_fact: Some(reference),
                    }
                } else {
                    let status = item.status.ok_or_else(|| {
                        format!("inventory item {id} requires status or source-fact")
                    })?;
                    let level = item.level.ok_or_else(|| {
                        format!("inventory item {id} requires level or source-fact")
                    })?;
                    let scope_and_limitations = item.scope_and_limitations.ok_or_else(|| {
                        format!("inventory item {id} requires scope-and-limitations or source-fact")
                    })?;
                    if scope_and_limitations.trim().is_empty() {
                        return Err(format!(
                            "inventory item {id} requires non-empty scope-and-limitations"
                        )
                        .into());
                    }
                    let mut paths = BTreeSet::new();
                    for path in &item.source_paths {
                        if !paths.insert(path) {
                            return Err(format!(
                                "inventory item {id} repeats source path {}",
                                path.display()
                            )
                            .into());
                        }
                        validate_regular_reference(root, path, "inventory source path")?;
                    }
                    InventoryItem {
                        id: id.clone(),
                        section: item.section,
                        title: item.title,
                        status,
                        level,
                        scope_and_limitations,
                        source_paths: item.source_paths,
                        source_fact: None,
                    }
                };
                if !item_ids.insert(id.clone()) {
                    return Err(format!("duplicate inventory item {id}").into());
                }
                view.items.push(resolved);
            }
            for reference in loaded.document.references {
                let id = slug(&reference.id, "inventory reference id")?;
                slug(&reference.section, "inventory section reference")?;
                if reference.title.trim().is_empty() || reference.details.trim().is_empty() {
                    return Err(
                        format!("inventory reference {id} requires title and details").into(),
                    );
                }
                let mut paths = BTreeSet::new();
                for path in &reference.source_paths {
                    if !paths.insert(path) {
                        return Err(format!(
                            "inventory reference {id} repeats source path {}",
                            path.display()
                        )
                        .into());
                    }
                    validate_regular_reference(root, path, "inventory reference source path")?;
                }
                if !reference_ids.insert(id.clone()) {
                    return Err(format!("duplicate inventory reference {id}").into());
                }
                view.references.push(reference);
            }
        }
        for item in &view.items {
            if !section_ids.contains(&item.section) {
                return Err(format!(
                    "inventory item {} names missing section {}",
                    item.id, item.section
                )
                .into());
            }
        }
        for reference in &view.references {
            if !section_ids.contains(&reference.section) {
                return Err(format!(
                    "inventory reference {} names missing section {}",
                    reference.id, reference.section
                )
                .into());
            }
        }
        validate_catalog_dependencies(&view.capabilities, allowed_external)?;
        view.sources.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(view)
    }
}

impl ManifestDocument {
    pub(super) fn resolve_catalogs(
        mut self,
        root: &Path,
        program_path: &Path,
        program_input: &str,
    ) -> Result<Self> {
        if self.schema != QUALIFICATION_SCHEMA {
            return Err(format!(
                "unsupported qualification schema {} (expected {QUALIFICATION_SCHEMA})",
                self.schema
            )
            .into());
        }
        self.program_source = Some(SourceIdentity {
            id: self.target.clone(),
            schema: self.schema,
            path: display_path(root, program_path),
            sha256: sha256(program_input.as_bytes()),
        });
        let mut origins = BTreeMap::new();
        let mut inline_ids = BTreeSet::new();
        for capability in &self.capabilities {
            let id = slug(&capability.id, "capability id")?;
            if !inline_ids.insert(id.clone()) {
                return Err(format!("qualification manifest repeats capability {id}").into());
            }
            if capability.catalog_scope.is_some() {
                return Err(format!(
                    "inline capability {id} cannot declare catalog-scope metadata"
                )
                .into());
            }
            if !capability.source_fact_refs.is_empty() {
                return Err(format!(
                    "inline capability {id} cannot reference catalog source facts"
                )
                .into());
            }
            origins.insert(id, CapabilityOrigin::Program);
        }
        if self.catalogs.is_empty() && self.catalog_capabilities.is_empty() {
            self.capability_origins = origins;
            return Ok(self);
        }
        if self.catalogs.is_empty() || self.catalog_capabilities.is_empty() {
            return Err(
                "catalogs and catalog-capabilities must either both be present or both be absent"
                    .into(),
            );
        }
        let view = CatalogView::load_with_program(
            root,
            &self.catalogs,
            Some((&self.verification, &self.hil)),
            &inline_ids,
        )?;
        for id in view.capabilities.keys() {
            if inline_ids.contains(id) {
                return Err(
                    format!("capability {id} is declared both inline and in a catalog").into(),
                );
            }
        }
        let mut selected = BTreeSet::new();
        for id in &self.catalog_capabilities {
            let id = slug(id, "catalog capability reference")?;
            if !selected.insert(id.clone()) {
                return Err(
                    format!("qualification program repeats catalog capability {id}").into(),
                );
            }
            if !view.capabilities.contains_key(&id) {
                return Err(format!("unknown catalog capability {id}").into());
            }
        }
        let mut resolved = BTreeSet::new();
        for id in &selected {
            resolve_capability(id, &view.capabilities, &mut resolved);
        }
        for id in resolved {
            let capability = &view.capabilities[&id];
            self.capabilities.push(capability.clone());
            self.catalog_scopes
                .insert(id.clone(), view.scopes[&id].clone());
            let (catalog_id, path) = view.capability_owners[&id].clone();
            origins.insert(
                id,
                CapabilityOrigin::Catalog {
                    id: catalog_id,
                    path,
                },
            );
        }
        self.catalog_sources = view.sources.clone();
        self.direct_catalog_capabilities = selected;
        self.catalog = view;
        self.capability_origins = origins;
        Ok(self)
    }
}

fn resolve_capability(
    id: &str,
    declarations: &BTreeMap<String, CapabilityDocument>,
    resolved: &mut BTreeSet<String>,
) {
    if !resolved.insert(id.to_owned()) {
        return;
    }
    for dependency in &declarations[id].depends_on {
        if declarations.contains_key(dependency) {
            resolve_capability(dependency, declarations, resolved);
        }
    }
}

pub(super) fn validate_catalog_dependencies(
    declarations: &BTreeMap<String, CapabilityDocument>,
    allowed_external: &BTreeSet<String>,
) -> Result<()> {
    fn visit(
        id: &str,
        declarations: &BTreeMap<String, CapabilityDocument>,
        allowed_external: &BTreeSet<String>,
        active: &mut BTreeSet<String>,
        complete: &mut BTreeSet<String>,
    ) -> Result<()> {
        if complete.contains(id) {
            return Ok(());
        }
        if !active.insert(id.to_owned()) {
            return Err(format!("catalog capability dependency cycle reaches {id}").into());
        }
        for dependency in &declarations[id].depends_on {
            let dependency = slug(dependency, "dependency")?;
            if dependency == id {
                return Err(format!("catalog capability {id} depends on itself").into());
            }
            if declarations.contains_key(&dependency) {
                visit(
                    &dependency,
                    declarations,
                    allowed_external,
                    active,
                    complete,
                )?;
            } else if !allowed_external.contains(&dependency) {
                return Err(
                    format!("catalog capability {id} depends on missing {dependency}").into(),
                );
            }
        }
        active.remove(id);
        complete.insert(id.to_owned());
        Ok(())
    }
    let mut active = BTreeSet::new();
    let mut complete = BTreeSet::new();
    for id in declarations.keys() {
        visit(
            id,
            declarations,
            allowed_external,
            &mut active,
            &mut complete,
        )?;
    }
    Ok(())
}

fn validate_regular_reference(root: &Path, relative: &Path, kind: &str) -> Result<()> {
    validate_relative_path(relative)?;
    read_contained_file(root, relative, kind).map(|_| ())
}

fn read_contained_file(root: &Path, relative: &Path, kind: &str) -> Result<String> {
    let mut path = fs::canonicalize(root)?;
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Err(format!("{kind} must be a contained repository-relative path").into());
        };
        path.push(name);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect {kind} {}: {error}", relative.display()))?;
        let final_component = index + 1 == components.len();
        if metadata.file_type().is_symlink()
            || (final_component && !metadata.file_type().is_file())
            || (!final_component && !metadata.file_type().is_dir())
        {
            return Err(format!(
                "{kind} path must contain only regular directories and end in a regular file: {}",
                relative.display()
            )
            .into());
        }
    }
    fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {kind} {}: {error}", relative.display()).into())
}

fn display_path(root: &Path, path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    absolute
        .strip_prefix(root)
        .map_or_else(|_| absolute.clone(), Path::to_path_buf)
}

fn sha256(input: &[u8]) -> String {
    format!("{:x}", Sha256::digest(input))
}

#[cfg(test)]
mod tests;
