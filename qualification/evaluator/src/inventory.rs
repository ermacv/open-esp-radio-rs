//! Deterministic ignored catalog and program views.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    Result,
    hil::HilRequirement,
    model::{
        CapabilityOrigin, CatalogView, HilRequirementDocument, Qualification, VendorEvidenceRef,
    },
};

static INVENTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn write_static(
    catalog: &CatalogView,
    output_directory: &Path,
    root: &Path,
) -> Result<()> {
    fs::create_dir_all(output_directory)?;
    write_file(
        output_directory,
        "domain-inventory.md",
        &render_domain(catalog, output_directory, root)?,
    )?;
    write_file(
        output_directory,
        "capability-catalog.md",
        &render_capabilities(catalog, output_directory, root)?,
    )?;
    write_file(
        output_directory,
        "migration-map.md",
        &render_mapping(catalog, output_directory, root)?,
    )?;
    write_file(
        output_directory,
        "project-status.md",
        &crate::engineering::ProjectMap::from_catalog(catalog, None)?
            .markdown(output_directory, root)?,
    )?;
    println!("CATALOG-INVENTORY\t{}", output_directory.display());
    Ok(())
}

pub(crate) fn write(
    qualification: &Qualification,
    output_directory: &Path,
    root: &Path,
) -> Result<()> {
    write_static(&qualification.catalog, output_directory, root)?;
    write_file(
        output_directory,
        "program-inventory.md",
        &render_program(qualification, output_directory, root)?,
    )?;
    write_file(
        output_directory,
        "program-status.md",
        &crate::engineering::ProjectMap::from_program(qualification, None)?
            .markdown(output_directory, root)?,
    )?;
    println!(
        "PROGRAM-INVENTORY\t{}",
        output_directory.join("program-inventory.md").display()
    );
    Ok(())
}

fn write_file(directory: &Path, name: &str, contents: &str) -> Result<()> {
    let output = directory.join(name);
    let temporary = directory.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        INVENTORY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, &output)?;
        File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn render_domain(catalog: &CatalogView, output: &Path, root: &Path) -> Result<String> {
    let mut text = String::from("# Source capability inventory\n\n");
    text.push_str("This static view is generated from the canonical catalog. It reports source ownership, not qualification readiness. `IMPLEMENTED`, `PARTIAL`, `FAIL-CLOSED`, `ABSENT`, `HOST-ONLY`, and `DIAGNOSTIC` are distinct source-facet states; level describes whether the declaration is a native capability, lower primitive, or composed product.\n\n");
    render_sources(&mut text, catalog, output, root)?;
    let mut counts = BTreeMap::<(String, String), usize>::new();
    for item in &catalog.items {
        let domain = catalog
            .sections
            .iter()
            .find(|section| section.id == item.section)
            .expect("validated inventory section")
            .domain
            .clone();
        *counts
            .entry((domain, item.status.label().to_owned()))
            .or_default() += 1;
    }
    text.push_str(
        "## Coverage counts\n\n| Domain | Source status | Items |\n| --- | --- | ---: |\n",
    );
    for ((domain, status), count) in counts {
        text.push_str(&format!(
            "| {} | {} | {count} |\n",
            escape_table(&domain),
            status
        ));
    }
    let unique_facts = catalog
        .items
        .iter()
        .map(|item| item.source_fact.as_deref().unwrap_or(&item.id))
        .chain(catalog.source_facts.keys().map(String::as_str))
        .collect::<BTreeSet<_>>();
    let projections = catalog
        .items
        .iter()
        .filter(|item| item.source_fact.is_some())
        .count();
    text.push_str(&format!(
        "\nDisplayed rows: {}. Unique source facts: {}. Explicit projections: {}.\n",
        catalog.items.len(),
        unique_facts.len(),
        projections
    ));
    let mut domains = BTreeMap::<&str, Vec<_>>::new();
    for section in &catalog.sections {
        domains
            .entry(section.domain.as_str())
            .or_default()
            .push(section);
    }
    for (domain, sections) in domains {
        text.push_str(&format!("\n## Domain: `{}`\n\n", escape_heading(domain)));
        for section in sections {
            text.push_str(&format!("### {}\n\n", escape_heading(&section.id)));
            text.push_str(&format!("**{}**\n\n", escape_text(&section.title)));
            if !section.overview.is_empty() {
                text.push_str(&rewrite_links(
                    &section.overview,
                    &section.source_document,
                    output,
                    root,
                )?);
                text.push_str("\n\n");
            }
            for item in catalog
                .items
                .iter()
                .filter(|item| item.section == section.id)
            {
                text.push_str(&format!("#### {}\n\n", escape_heading(&item.id)));
                text.push_str(&format!("**{}**\n\n", escape_text(&item.title)));
                text.push_str(&format!(
                    "- Source status: `{}`\n- Level: `{}`\n- Legacy owner: `{}`\n\n",
                    item.status.label(),
                    item.level.label(),
                    section.source_document.display()
                ));
                if let Some(fact) = &item.source_fact {
                    text.push_str(&format!("- Canonical source fact: `{fact}`\n\n"));
                }
                text.push_str(&rewrite_links(
                    &item.scope_and_limitations,
                    &section.source_document,
                    output,
                    root,
                )?);
                text.push_str("\n\n");
                if !item.source_paths.is_empty() {
                    text.push_str("Source-owner links: ");
                    for (index, path) in item.source_paths.iter().enumerate() {
                        if index > 0 {
                            text.push_str(", ");
                        }
                        text.push_str(&format!(
                            "[{}]({})",
                            escape_text(&path.display().to_string()),
                            link_to(path, output, root)?
                        ));
                    }
                    text.push_str(".\n\n");
                }
            }
            for reference in catalog
                .references
                .iter()
                .filter(|reference| reference.section == section.id)
            {
                text.push_str(&format!("#### {}\n\n", escape_heading(&reference.id)));
                text.push_str("**");
                text.push_str(&rewrite_links(
                    &reference.title,
                    &section.source_document,
                    output,
                    root,
                )?);
                text.push_str("**\n\n");
                text.push_str(&format!(
                    "- Reference kind: `{}`\n\n",
                    reference.kind.label()
                ));
                text.push_str(&rewrite_links(
                    &reference.details,
                    &section.source_document,
                    output,
                    root,
                )?);
                text.push_str("\n\n");
                if !reference.source_paths.is_empty() {
                    text.push_str("Source-owner links: ");
                    for (index, path) in reference.source_paths.iter().enumerate() {
                        if index > 0 {
                            text.push_str(", ");
                        }
                        text.push_str(&format!(
                            "[{}]({})",
                            escape_text(&path.display().to_string()),
                            link_to(path, output, root)?
                        ));
                    }
                    text.push_str(".\n\n");
                }
            }
        }
    }
    Ok(text)
}

fn render_capabilities(catalog: &CatalogView, output: &Path, root: &Path) -> Result<String> {
    let mut text = String::from("# Qualification capability catalog\n\n");
    text.push_str("This is a static declaration view. Every catalog capability is validated, but none receives an evidence-derived readiness verdict here.\n\n");
    render_sources(&mut text, catalog, output, root)?;
    for (id, capability) in &catalog.capabilities {
        let scope = &catalog.scopes[id];
        text.push_str(&format!(
            "## {}\n\n**{}**\n\n{}\n\n",
            escape_heading(id),
            escape_text(&capability.title),
            escape_text(&capability.scope)
        ));
        text.push_str(&format!("- Evaluation: not evaluated in this static view\n- Dependencies: {}\n- Source axes: implementation=`{}`, host=`{}`, async=`{}`\n- Structured scope: chip=`{}`, role=`{}`, PHY=`{}`, security=`{}`, composition=`{}`, level=`{}`\n- Activation boundary: {}\n- Limitations: {}\n",
            code_list(&capability.depends_on), capability.implementation.label(), capability.host.label(), capability.async_proof.label(),
            scope.chip, scope.role, scope.phy, scope.security.join(","), scope.composition, scope.level.label(), escape_text(&scope.activation_boundary), escape_text(&scope.limitations)));
        if !capability.source_fact_refs.is_empty() {
            text.push_str(&format!(
                "- Canonical source facts: {}\n",
                code_list(&capability.source_fact_refs)
            ));
        }
        if !capability.vendor_roots.is_empty() {
            text.push_str("- Vendor roots: ");
            text.push_str(
                &capability
                    .vendor_roots
                    .iter()
                    .map(|root| format!("`{}:{}`", root.source, root.symbol))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            text.push('\n');
        }
        text.push_str("- Explicit vendor evidence references: ");
        text.push_str(&vendor_evidence_list(&capability.vendor_evidence));
        text.push('\n');
        text.push_str(&format!(
            "- Vendor not-applicable reason: {}\n",
            optional_reason(capability.vendor_not_applicable.as_deref())
        ));
        if !capability.vendor_anchors.is_empty() {
            text.push_str("- Vendor/source anchors: ");
            for (index, path) in capability.vendor_anchors.iter().enumerate() {
                if index > 0 {
                    text.push_str(", ");
                }
                text.push_str(&format!(
                    "[{}]({})",
                    path.display(),
                    link_to(path, output, root)?
                ));
            }
            text.push('\n');
        }
        text.push_str("- HIL obligations: ");
        text.push_str(&hil_requirement_document_list(&capability.hil_requirements));
        text.push('\n');
        text.push_str(&format!(
            "- HIL not-applicable reason: {}\n- Async not-applicable reason: {}\n",
            optional_reason(capability.hil_not_applicable.as_deref()),
            optional_reason(capability.async_not_applicable.as_deref())
        ));
        if !capability.gaps.is_empty() {
            text.push_str("- Declared gaps: ");
            text.push_str(
                &capability
                    .gaps
                    .iter()
                    .map(|gap| format!("`{}:{}`", gap.axis.label(), gap.id))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            text.push('\n');
        }
        for contract in &capability.source_contracts {
            text.push_str(&format!("\n### source-contract-{}\n\n- Composition: `{}`\n- Scope: {}\n- Limits: {}\n- Paths: ", escape_heading(&contract.id), contract.composition.label(), escape_text(&contract.scope), escape_text(&contract.limits)));
            for (index, path) in contract.source_paths.iter().enumerate() {
                if index > 0 {
                    text.push_str(", ");
                }
                text.push_str(&format!(
                    "[{}]({})",
                    path.display(),
                    link_to(path, output, root)?
                ));
            }
            text.push('\n');
        }
        text.push('\n');
    }
    Ok(text)
}

fn render_program(qualification: &Qualification, output: &Path, root: &Path) -> Result<String> {
    let mut text = format!("# Qualification program {}\n\n", qualification.target);
    text.push_str("This readiness view is generated by the sole qualification evaluator from the resolved program and current evidence inputs.\n\n");
    text.push_str("## Evaluation identity\n\n");
    text.push_str(&format!("- Repository commit: `{}`\n- Repository dirty: `{}`\n- Program: [{}]({}), schema {}, SHA-256 `{}`\n- Verification project: `{}`\n- Vendor evidence index: `{}` ({} entries, {} current-release)\n- HIL catalog: `{}`\n- HIL runs: `{}` ({} qualifying bundles)\n\n",
        qualification.repository.commit, qualification.repository.dirty,
        qualification.program_source.path.display(), link_to(&qualification.program_source.path, output, root)?, qualification.program_source.schema, qualification.program_source.sha256,
        qualification.evidence_inputs.verification_project.display(), qualification.evidence_inputs.vendor_evidence_index.display(), qualification.evidence_inputs.verification_entries, qualification.evidence_inputs.verification_current_release_entries,
        qualification.evidence_inputs.hil_catalog.display(), qualification.evidence_inputs.hil_runs.display(), qualification.evidence_inputs.hil.qualifying));
    text.push_str("## Resolved capabilities\n\n");
    for capability in qualification.capabilities.values() {
        let directly_selected = qualification
            .direct_catalog_capabilities
            .contains(&capability.id);
        let dependents = qualification
            .capabilities
            .values()
            .filter(|candidate| candidate.dependencies.contains(&capability.id))
            .map(|candidate| candidate.id.as_str())
            .collect::<Vec<_>>();
        let membership = match (directly_selected, dependents.is_empty()) {
            (true, true) => "direct selection".to_owned(),
            (true, false) => format!("direct selection; dependency of {}", dependents.join(", ")),
            (false, false) => format!("dependency of {}", dependents.join(", ")),
            (false, true) => "dependency closure".to_owned(),
        };
        let origin = match &qualification.capability_origins[&capability.id] {
            CapabilityOrigin::Program => "inline program".to_owned(),
            CapabilityOrigin::Catalog { id, path } => {
                format!("catalog `{id}` (`{}`)", path.display())
            }
        };
        let structured_scope = qualification.catalog_scopes.get(&capability.id).map_or_else(
            || "inline program declaration".to_owned(),
            |scope| format!("chip=`{}`, role=`{}`, PHY=`{}`, security=`{}`, composition=`{}`, level=`{}`", scope.chip, scope.role, scope.phy, scope.security.join(","), scope.composition, scope.level.label()),
        );
        text.push_str(&format!("### {}\n\n**{}**\n\n- Membership: {membership}\n- Declaration: {origin}\n- Dependencies: {}\n- Structured scope: {structured_scope}\n- Axes: implementation=`{}`, host=`{}`, vendor=`{}`, HIL=`{}`, async=`{}`\n- Proof-ready: `{}`\n- Effective ready (including dependencies): `{}`\n- Explicit vendor evidence references: {}\n- Vendor not-applicable reason: {}\n- HIL obligations: {}\n- HIL not-applicable reason: {}\n- Async not-applicable reason: {}\n- Evidence: {}\n- Reasons/gaps: {}\n\n",
            escape_heading(&capability.id), escape_text(&capability.title), code_list(&capability.dependencies), capability.implementation.label(), capability.host.label(), capability.vendor.label(), capability.hil.label(), capability.async_proof.label(), capability.proof_ready(), qualification.is_ready(&capability.id), vendor_evidence_list(&capability.vendor_evidence), optional_reason(capability.vendor_not_applicable.as_deref()), hil_requirement_list(&capability.hil_requirements), optional_reason(capability.hil_not_applicable.as_deref()), optional_reason(capability.async_not_applicable.as_deref()), code_list(&capability.evidence), capability.gaps.iter().map(|gap| format!("`{}:{}`", gap.axis.label(), gap.id)).collect::<Vec<_>>().join(", ")));
    }
    let unselected = qualification
        .catalog
        .capabilities
        .keys()
        .filter(|id| !qualification.capabilities.contains_key(*id))
        .collect::<Vec<_>>();
    if !unselected.is_empty() {
        text.push_str("## Unselected catalog declarations\n\nThese declarations are statically validated but are not evaluated and have no readiness boolean in this program view.\n\n");
        for id in unselected {
            text.push_str(&format!("- `{id}`\n"));
        }
    }
    Ok(text)
}

fn render_mapping(catalog: &CatalogView, output: &Path, root: &Path) -> Result<String> {
    let mut text = String::from(
        "# Source inventory migration map\n\nThis ignored generated map demonstrates that each legacy row, mapping, or protocol matrix cell has one canonical catalog item/reference or an explicit projection of a shared source fact.\n\n| Legacy document | Legacy section | Legacy entry | Canonical ID | Fact/reference |\n| --- | --- | --- | --- | --- |\n",
    );
    for item in &catalog.items {
        let section = catalog
            .sections
            .iter()
            .find(|section| section.id == item.section)
            .expect("validated inventory section");
        text.push_str(&format!(
            "| `{}` | {} | {} | [`{}`](domain-inventory.md#{}) | `{}` |\n",
            section.source_document.display(),
            escape_table(&section.title),
            escape_table(&rewrite_links(
                &item.title,
                &section.source_document,
                output,
                root,
            )?),
            item.id,
            item.id,
            item.source_fact.as_deref().unwrap_or(&item.id)
        ));
    }
    for reference in &catalog.references {
        let section = catalog
            .sections
            .iter()
            .find(|section| section.id == reference.section)
            .expect("validated inventory section");
        text.push_str(&format!(
            "| `{}` | {} | {} | [`{}`](domain-inventory.md#{}) | `{}` |\n",
            section.source_document.display(),
            escape_table(&section.title),
            escape_table(&rewrite_links(
                &reference.title,
                &section.source_document,
                output,
                root,
            )?),
            reference.id,
            reference.id,
            reference.kind.label()
        ));
    }
    Ok(text)
}

fn render_sources(
    text: &mut String,
    catalog: &CatalogView,
    output: &Path,
    root: &Path,
) -> Result<()> {
    text.push_str("## Canonical source identity\n\n");
    for source in &catalog.sources {
        text.push_str(&format!(
            "- Catalog `{}`: schema {}, SHA-256 `{}` ([{}]({}))\n",
            source.id,
            source.schema,
            source.sha256,
            source.path.display(),
            link_to(&source.path, output, root)?
        ));
    }
    text.push('\n');
    Ok(())
}

pub(crate) fn rewrite_links(
    value: &str,
    source: &Path,
    output: &Path,
    root: &Path,
) -> Result<String> {
    let mut rendered = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("](") {
        let target_start = start + 2;
        let Some(end_offset) = rest[target_start..].find(')') else {
            break;
        };
        let end = target_start + end_offset;
        rendered.push_str(&rest[..target_start]);
        let target = &rest[target_start..end];
        if target.starts_with('#') || target.contains("://") {
            rendered.push_str(target);
        } else {
            let (path_part, fragment) = target
                .split_once('#')
                .map_or((target, ""), |(path, fragment)| (path, fragment));
            let path = if path_part.is_empty() {
                source.to_owned()
            } else {
                normalize_relative(
                    source.parent().unwrap_or(Path::new("")),
                    Path::new(path_part),
                )?
            };
            rendered.push_str(&link_to(&path, output, root)?);
            if !fragment.is_empty() {
                rendered.push('#');
                rendered.push_str(fragment);
            }
        }
        rendered.push(')');
        rest = &rest[end + 1..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn normalize_relative(base: &Path, relative: &Path) -> Result<PathBuf> {
    let mut parts = base
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err("inventory link escapes repository".into());
                }
            }
            Component::Normal(value) => parts.push(value.to_owned()),
            _ => return Err("inventory link is not repository-relative".into()),
        }
    }
    Ok(parts.into_iter().collect())
}

pub(crate) fn link_to(target: &Path, output: &Path, root: &Path) -> Result<String> {
    let root = fs::canonicalize(root)?;
    let output = if output.is_absolute() {
        output.to_owned()
    } else {
        root.join(output)
    };
    let output = output
        .strip_prefix(&root)
        .map_err(|_| "inventory output must be inside repository root")?;
    let target = if target.is_absolute() {
        target
            .strip_prefix(&root)
            .map_err(|_| "inventory link target is outside repository root")?
            .to_owned()
    } else {
        target.to_owned()
    };
    let depth = output.components().count();
    Ok(format!(
        "{}{}",
        "../".repeat(depth),
        target.to_string_lossy()
    ))
}

fn code_list(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values
            .iter()
            .map(|value| format!("`{}`", value.replace('`', "\\`")))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn vendor_evidence_list(values: &[VendorEvidenceRef]) -> String {
    if values.is_empty() {
        "none declared".to_owned()
    } else {
        values
            .iter()
            .map(|reference| {
                format!(
                    "`{}/{}:{}`",
                    reference.suite, reference.source, reference.symbol
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn hil_requirement_list(values: &[HilRequirement]) -> String {
    if values.is_empty() {
        "none declared".to_owned()
    } else {
        values
            .iter()
            .map(|requirement| {
                format!(
                    "`{}` ×{}{}",
                    requirement.scenario,
                    requirement.minimum_repetitions,
                    named_checks(&requirement.checks)
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn hil_requirement_document_list(values: &[HilRequirementDocument]) -> String {
    if values.is_empty() {
        "none declared".to_owned()
    } else {
        values
            .iter()
            .map(|requirement| {
                format!(
                    "`{}` ×{}{}",
                    requirement.scenario,
                    requirement.minimum_repetitions,
                    named_checks(&requirement.checks)
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn optional_reason(reason: Option<&str>) -> String {
    reason.map_or_else(|| "not declared".to_owned(), |reason| format!("`{reason}`"))
}

fn named_checks(checks: &[String]) -> String {
    if checks.is_empty() {
        String::new()
    } else {
        format!(" checks: {}", code_list(checks))
    }
}

fn escape_heading(value: &str) -> String {
    value.replace(['\r', '\n', '`'], "")
}
fn escape_text(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}
fn escape_table(value: &str) -> String {
    escape_text(value).replace('|', "&#124;")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RenderRoot {
        path: PathBuf,
    }

    impl RenderRoot {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "open-radio-catalog-render-{}-{name}",
                std::process::id()
            ));
            if path.exists() {
                fs::remove_dir_all(&path).unwrap();
            }
            fs::create_dir_all(path.join("catalog")).unwrap();
            fs::create_dir_all(path.join("scenarios")).unwrap();
            fs::write(
                path.join("scenarios/static.toml"),
                "schema = 4\nid = \"static\"\nrepetitions = 1\n",
            )
            .unwrap();
            fs::write(path.join("FEATURES.md"), "# Features\n\n## ownership\n").unwrap();
            fs::write(
                path.join("verification.toml"),
                "id = \"test\"\nverification-addon = \"verification-addon.toml\"\n",
            )
            .unwrap();
            fs::write(path.join("verification-addon.toml"), "").unwrap();
            Self { path }
        }

        fn write_catalog(&self, name: &str, id: &str, body: &str) {
            fs::write(
                self.path.join(format!("catalog/{name}.toml")),
                format!(
                    "schema = 2\nid = \"{id}\"\n\n[validation]\nverification-project = \"verification.toml\"\nhil-catalog = \"scenarios\"\n{body}"
                ),
            )
            .unwrap();
        }
    }

    impl Drop for RenderRoot {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).unwrap();
        }
    }

    #[test]
    fn markdown_cells_escape_structure() {
        assert_eq!(escape_table("a|b\nc"), "a&#124;b c");
    }

    #[test]
    fn relative_paths_cannot_escape_repository() {
        assert!(normalize_relative(Path::new("a"), Path::new("../../b")).is_err());
        assert_eq!(
            normalize_relative(Path::new("a/b"), Path::new("../c")).unwrap(),
            Path::new("a/c")
        );
    }

    #[test]
    fn full_static_render_is_deterministic_and_has_no_runtime_identity() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let catalog = CatalogView::load(
            &root,
            &[PathBuf::from(
                "qualification/catalog/esp32s31/wifi-phy.toml",
            )],
        )
        .unwrap();
        let wifi_items = catalog
            .items
            .iter()
            .filter(|item| item.id.starts_with("wifi-"))
            .count();
        let phy_items = catalog
            .items
            .iter()
            .filter(|item| item.id.starts_with("phy-"))
            .count();
        let matrix_cells = catalog
            .items
            .iter()
            .filter(|item| item.id.starts_with("phy-protocol-consumer-"))
            .count();
        assert_eq!((wifi_items, phy_items, matrix_cells), (60, 78, 27));
        assert_eq!(catalog.items.len(), 138);
        let output = root.join("target/qualification/catalog/test-render");
        let first = render_domain(&catalog, &output, &root).unwrap();
        let second = render_domain(&catalog, &output, &root).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("SoftAP"));
        assert!(first.contains("WPA3-Personal"));
        assert!(first.contains("Resume after RF sleep — Bluetooth"));
        assert!(!first.contains(&root.display().to_string()));
        assert!(!first.contains("Generated at"));
    }

    #[test]
    fn renderer_discovers_every_domain_section_and_item_without_a_whitelist() {
        let root = RenderRoot::new("domains");
        root.write_catalog(
            "bluetooth",
            "bluetooth",
            r#"
[[sections]]
id = "bluetooth-radio"
domain = "bluetooth"
title = "Bluetooth radio"
source-document = "FEATURES.md"
overview = "See [ownership](FEATURES.md#ownership)."

[[sections]]
id = "bluetooth-empty"
domain = "bluetooth"
title = "Bluetooth overview without status rows"
source-document = "FEATURES.md"
overview = "This section deliberately has no rows."

[[items]]
id = "bluetooth-host"
section = "bluetooth-radio"
title = "Host integration"
status = "host-only"
level = "composed-product"
scope-and-limitations = "Host-owned integration only"

[[items]]
id = "bluetooth-diagnostic"
section = "bluetooth-radio"
title = "Diagnostic radio"
status = "diagnostic"
level = "lower-primitive"
scope-and-limitations = "Diagnostic-only path"
"#,
        );
        root.write_catalog(
            "other",
            "other",
            r#"
[[sections]]
id = "new-domain-section"
domain = "new-domain"
title = "Future domain"
source-document = "FEATURES.md"

[[items]]
id = "new-domain-item"
section = "new-domain-section"
title = "Future item"
status = "absent"
level = "native-silicon"
scope-and-limitations = "No composition"
"#,
        );
        let output = root.path.join("target/render");
        let bluetooth =
            CatalogView::load(&root.path, &[PathBuf::from("catalog/bluetooth.toml")]).unwrap();
        let bluetooth_text = render_domain(&bluetooth, &output, &root.path).unwrap();
        assert!(bluetooth_text.contains("## Domain: `bluetooth`"));
        assert!(bluetooth_text.contains("### bluetooth-empty"));
        assert!(bluetooth_text.contains("HOST-ONLY"));
        assert!(bluetooth_text.contains("DIAGNOSTIC"));
        assert!(bluetooth_text.contains("../../FEATURES.md#ownership"));
        assert!(!bluetooth_text.contains("Wi-Fi and shared PHY"));

        let paths = [
            PathBuf::from("catalog/bluetooth.toml"),
            PathBuf::from("catalog/other.toml"),
        ];
        let combined = CatalogView::load(&root.path, &paths).unwrap();
        let reversed =
            CatalogView::load(&root.path, &[paths[1].clone(), paths[0].clone()]).unwrap();
        let combined_text = render_domain(&combined, &output, &root.path).unwrap();
        assert_eq!(
            combined_text,
            render_domain(&reversed, &output, &root.path).unwrap()
        );
        for id in ["bluetooth-host", "bluetooth-diagnostic", "new-domain-item"] {
            assert_eq!(combined_text.matches(&format!("#### {id}\n")).count(), 1);
        }
        assert!(combined_text.contains("## Domain: `new-domain`"));
        let bluetooth_section = combined_text.find("### bluetooth-radio").unwrap();
        let host = combined_text.find("#### bluetooth-host").unwrap();
        let other_domain = combined_text.find("## Domain: `new-domain`").unwrap();
        assert!(bluetooth_section < host && host < other_domain);
        assert!(combined_text.contains("| bluetooth | HOST-ONLY | 1 |"));
        assert!(combined_text.contains("| bluetooth | DIAGNOSTIC | 1 |"));
        assert!(combined_text.contains("| new-domain | ABSENT | 1 |"));
        assert!(combined_text.contains("Displayed rows: 3. Unique source facts: 3."));
    }

    #[test]
    fn migrated_coex_and_whole_radio_views_keep_references_separate_from_status() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let paths = [
            "qualification/catalog/esp32s31/wifi-phy.toml",
            "qualification/catalog/esp32s31/coex.toml",
            "qualification/catalog/esp32s31/bluetooth.toml",
            "qualification/catalog/esp32s31/whole-radio.toml",
        ]
        .map(PathBuf::from);
        let catalog = CatalogView::load(&root, &paths).unwrap();
        let reversed =
            CatalogView::load(&root, &paths.into_iter().rev().collect::<Vec<_>>()).unwrap();
        let output = root.join("target/qualification/catalog/test-whole-radio-render");
        let rendered = render_domain(&catalog, &output, &root).unwrap();
        assert_eq!(rendered, render_domain(&reversed, &output, &root).unwrap());
        assert!(rendered.contains("## Domain: `coexistence`"));
        assert!(rendered.contains("## Domain: `whole-radio`"));
        assert!(
            rendered.contains(
                "Displayed rows: 356. Unique source facts: 349. Explicit projections: 17."
            )
        );
        assert!(rendered.contains("- Canonical source fact: `coex-timer-validation-bridge`"));
        assert!(rendered.contains("- Canonical source fact: `bluetooth-initial-phy-handoff`"));

        let reference_id = "coex-official-coexistence-scenario-scope-wifi-sta-scan-connecting-connected-ble-scan-advertising-connected";
        let reference_start = rendered.find(&format!("#### {reference_id}\n")).unwrap();
        let reference_tail = &rendered[reference_start..];
        let reference_end = reference_tail[5..]
            .find("\n#### ")
            .map_or(reference_tail.len(), |index| index + 5);
        let reference = &reference_tail[..reference_end];
        assert!(reference.contains("- Reference kind: `source-reference`"));
        assert!(reference.contains("Vendor classification: `Y`"));
        assert!(!reference.contains("Source status"));
    }

    #[test]
    fn migrated_ieee802154_view_keeps_host_and_reference_boundaries() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let catalog = CatalogView::load(
            &root,
            &[PathBuf::from(
                "qualification/catalog/esp32s31/ieee802154.toml",
            )],
        )
        .unwrap();
        let output = root.join("target/qualification/catalog/test-ieee802154-render");
        let rendered = render_domain(&catalog, &output, &root).unwrap();
        assert!(rendered.contains("## Domain: `ieee802154`"));
        assert!(rendered.contains("| ieee802154 | HOST-ONLY | 3 |"));
        assert!(
            rendered
                .contains("Displayed rows: 44. Unique source facts: 44. Explicit projections: 2.")
        );
        assert!(rendered.contains("- Canonical source fact: `ieee802154-registered-timing-entry`"));
        assert!(rendered.contains("- Canonical source fact: `ieee802154-mac-operation-subset`"));

        let mapping_id =
            "ieee802154-qualification-scope-mapping-clocks-reset-and-masked-foundation";
        let mapping_start = rendered.find(&format!("#### {mapping_id}\n")).unwrap();
        let mapping_tail = &rendered[mapping_start..];
        let mapping_end = mapping_tail[5..]
            .find("\n#### ")
            .map_or(mapping_tail.len(), |index| index + 5);
        let mapping = &mapping_tail[..mapping_end];
        assert!(mapping.contains("- Reference kind: `qualification-mapping`"));
        assert!(mapping.contains("`clock-reset-foundation`"));
        assert!(!mapping.contains("Source status"));
    }

    #[test]
    fn static_capability_view_preserves_explicit_evidence_and_na_reasons() {
        let root = RenderRoot::new("obligations");
        fs::write(
            root.path.join("disposition.toml"),
            "[[functions]]\nsource = \"archive\"\nsymbol = \"explicit_root\"\nrust-component = \"test::root\"\nsemantic-contract = \"exact\"\n",
        )
        .unwrap();
        fs::write(
            root.path.join("verification-addon.toml"),
            "evidence-index = \"vendor.json\"\n[[suites]]\ndispositions = [\"disposition.toml\"]\n",
        )
        .unwrap();
        root.write_catalog(
            "obligations",
            "obligations",
            r#"
[[capabilities]]
id = "explicit-evidence"
title = "Explicit evidence"
scope = "One source operation"
implementation = "complete"
host = "covered"
async = "bounded"
vendor-roots = [{ source = "archive", symbol = "explicit_root" }]
vendor-evidence = [{ suite = "explicit-suite", source = "archive", symbol = "explicit_root" }]
hil-requirements = [{ scenario = "static", minimum-repetitions = 1 }]

[capabilities.catalog-scope]
chip = "test-chip"
role = "test-role"
phy = "test-phy"
security = ["not-applicable"]
composition = "test-composition"
level = "lower-primitive"
activation-boundary = "One call"
limitations = "Static declaration only"

[[capabilities]]
id = "explicit-na"
title = "Explicit N/A"
scope = "One portable transform"
implementation = "complete"
host = "covered"
async = "not-applicable"
vendor-not-applicable = "portable-source-transform"
hil-not-applicable = "hardware-independent-transform"
async-not-applicable = "pure-finite-transform"

[capabilities.catalog-scope]
chip = "test-chip"
role = "portable-role"
phy = "not-applicable"
security = ["not-applicable"]
composition = "portable-transform"
level = "lower-primitive"
activation-boundary = "One pure call"
limitations = "No hardware boundary"
"#,
        );
        let catalog =
            CatalogView::load(&root.path, &[PathBuf::from("catalog/obligations.toml")]).unwrap();
        let rendered =
            render_capabilities(&catalog, &root.path.join("target/render"), &root.path).unwrap();
        assert!(rendered.contains("`explicit-suite/archive:explicit_root`"));
        assert!(rendered.contains("`static` ×1"));
        assert!(rendered.contains("`portable-source-transform`"));
        assert!(rendered.contains("`hardware-independent-transform`"));
        assert!(rendered.contains("`pure-finite-transform`"));
        assert!(rendered.contains("none declared"));
        assert!(rendered.contains("not declared"));
    }
}
