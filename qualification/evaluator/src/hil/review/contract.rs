//! Strict review declarations and the current property/input fingerprint.
use super::*;

pub(crate) fn validate(
    root: &Path,
    document: &CapabilityDocument,
    catalog: &ScenarioCatalog,
) -> Result<()> {
    let mut paths = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut scenarios = BTreeSet::new();
    for path in &document.hil_reviews {
        if !paths.insert(path) {
            return Err("duplicate HIL applicability review path".into());
        }
        let review = read(root, path)?;
        if review.capability != document.id
            || !ids.insert(review.id.clone())
            || !scenarios.insert(review.scenario.clone())
            || !document.hil_requirements.iter().any(|r| {
                r.scenario == review.scenario
                    || catalog.control_for(&r.scenario) == Some(&review.scenario)
            })
        {
            return Err(
                "HIL review must uniquely bind a declared capability scenario or its control"
                    .into(),
            );
        }
    }
    Ok(())
}

pub(super) fn read(root: &Path, path: &Path) -> Result<Document> {
    regular(root, path)?;
    let document: Document = toml_edit::de::from_str(&fs::read_to_string(root.join(path))?)?;
    if document.schema != 1
        || !valid_id(&document.id)
        || !valid_id(&document.capability)
        || !valid_id(&document.scenario)
        || !valid_sha256(&document.property_sha256)
        || document.reviewer.trim().is_empty()
        || document.reason.trim().is_empty()
        || document.inputs.is_empty()
    {
        return Err("invalid HIL applicability review identity, rationale or inputs".into());
    }
    if document.source.build_record.is_some() {
        return Err("review source must be an actual observation".into());
    }
    for reference in [&document.source, &document.destination] {
        if !valid_sha256(&reference.id)
            || !valid_id(&reference.image)
            || !valid_sha256(&reference.application_sha256)
            || reference
                .build_record
                .as_ref()
                .is_some_and(|p| !safe_relative(p))
        {
            return Err("invalid HIL review observation/build binding".into());
        }
    }
    let mut roots = BTreeSet::new();
    if document.dependency_roots.iter().any(|name| {
        (name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
            || !roots.insert(name)
    }) {
        return Err("invalid or duplicate dependency root".into());
    }
    let mut paths = BTreeSet::new();
    for input in &document.inputs {
        if !safe_relative(&input.path)
            || !valid_sha256(&input.sha256)
            || !paths.insert(&input.path)
            || (input.kind != InputKind::Bytes
                && input.reason.as_deref().is_none_or(|s| s.trim().is_empty()))
        {
            return Err("invalid or duplicate HIL review source binding".into());
        }
    }
    let mut failures = BTreeSet::new();
    for failure in &document.failures {
        if !valid_sha256(&failure.observation)
            || !failures.insert(&failure.observation)
            || failure.reason.trim().is_empty()
            || (failure.disposition == Disposition::Fixed
                && failure.resolving_observation.is_none())
            || (failure.disposition == Disposition::NotApplicable
                && failure.resolving_observation.is_some())
            || failure
                .resolving_observation
                .as_ref()
                .is_some_and(|id| !valid_sha256(id))
        {
            return Err("invalid HIL failure disposition or resolving observation".into());
        }
    }
    Ok(document)
}

pub(crate) fn property(
    document: &CapabilityDocument,
    declarations: &BTreeMap<String, CapabilityDocument>,
    requirement: &HilRequirement,
    catalog: &ScenarioCatalog,
    root: &Path,
) -> Result<PropertyBinding> {
    let mut pending = vec![document.id.clone()];
    let mut visited = BTreeSet::new();
    let mut owners = BTreeSet::new();
    let mut scopes = BTreeMap::new();
    let mut unmapped = Vec::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let d = if id == document.id {
            document
        } else {
            declarations
                .get(&id)
                .ok_or("missing dependency while binding a HIL property")?
        };
        if d.source_contracts.iter().all(|c| c.source_paths.is_empty()) {
            unmapped.push(id.clone());
        }
        for contract in &d.source_contracts {
            owners.extend(contract.source_paths.iter().cloned());
        }
        scopes.insert(id, serde_json::json!({"scope":d.scope,"contracts":d.source_contracts,"dependencies":d.depends_on,"catalog_scope":d.catalog_scope}));
        pending.extend(d.depends_on.iter().cloned());
    }
    let mut implicit_build_inputs = Vec::new();
    for path in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"] {
        if root.join(path).try_exists()?
            && owners.insert(PathBuf::from(path))
            && path != "rust-toolchain.toml"
        {
            implicit_build_inputs.push(PathBuf::from(path));
        }
    }
    unmapped.sort();
    let mut identity = serde_json::json!({
        "format":"oer-hil-property-v3", "image_sensitive":image_sensitive(requirement, catalog), "capability":document.id, "scopes":scopes,
        "scenario":requirement.scenario,"checks":requirement.checks,"minimum_repetitions":requirement.minimum_repetitions,
        "procedure":catalog.definitions.get(&requirement.scenario).map(crate::hil::procedure::normalize),
    });
    let bytes = serde_json::to_vec(&identity)?;
    identity["format"] = serde_json::json!("oer-hil-property-v2");
    identity["procedure"] = catalog
        .definitions
        .get(&requirement.scenario)
        .map(procedure)
        .unwrap_or_default();
    let previous_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(&identity)?));
    identity["format"] = serde_json::json!("oer-hil-property-v1");
    identity["procedure"] = catalog
        .definitions
        .get(&requirement.scenario)
        .cloned()
        .unwrap_or_default();
    let legacy_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(&identity)?));
    let current_inputs = owners
        .iter()
        .map(|path| {
            regular(root, path)?;
            Ok(InputBinding {
                kind: InputKind::Bytes,
                reason: None,
                path: path.clone(),
                sha256: sha256_file(&root.join(path))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(PropertyBinding {
        current_inputs,
        implicit_build_inputs,
        legacy_sha256,
        previous_sha256,
        procedure_sha256: catalog
            .definitions
            .get(&requirement.scenario)
            .map(|v| {
                serde_json::to_vec(&crate::hil::procedure::normalize(v))
                    .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
            })
            .transpose()?,
        image_sensitive: image_sensitive(requirement, catalog),
        sha256: format!("{:x}", Sha256::digest(bytes)),
        required_inputs: owners.into_iter().collect(),
        unmapped_capabilities: unmapped,
    })
}

pub(super) fn image_sensitive(requirement: &HilRequirement, catalog: &ScenarioCatalog) -> bool {
    if requirement.checks.is_empty()
        && catalog
            .definitions
            .get(&requirement.scenario)
            .is_some_and(checks::whole_scenario_image_sensitive)
    {
        return true;
    }
    catalog
        .checks
        .get(&requirement.scenario)
        .is_some_and(|checks| {
            checks.iter().any(|(name, c)| {
                (requirement.checks.is_empty() || requirement.checks.contains(name))
                    && c.image_sensitive()
            })
        })
}

/// Only top-level display metadata is excluded. Workload, criteria, evidence,
/// fixture interventions and unknown future execution fields remain bound.
fn procedure(definition: &serde_json::Value) -> serde_json::Value {
    let mut value = definition.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("description");
        object.remove("tags");
    }
    value
}
