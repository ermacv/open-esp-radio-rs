//! Deterministic ignored catalog and program views.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    Result,
    model::{CapabilityOrigin, CatalogView, Qualification},
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
        &render_mapping(catalog),
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
    let mut text = String::from("# ESP32-S31 Wi-Fi and shared PHY source inventory\n\n");
    text.push_str("This static view is generated from the canonical catalog. It reports source ownership, not qualification readiness. `IMPLEMENTED`, `PARTIAL`, `FAIL-CLOSED`, and `ABSENT` are source-facet states; level describes whether the declaration is a lower primitive or a composed product.\n\n");
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
    for domain in ["wifi", "phy"] {
        text.push_str(&format!(
            "\n## {}\n\n",
            if domain == "wifi" {
                "Wi-Fi"
            } else {
                "Shared PHY"
            }
        ));
        for section in catalog
            .sections
            .iter()
            .filter(|section| section.domain == domain)
        {
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
        }
    }
    Ok(text)
}

fn render_capabilities(catalog: &CatalogView, output: &Path, root: &Path) -> Result<String> {
    let mut text = String::from("# ESP32-S31 qualification capability catalog\n\n");
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
        if !capability.hil_requirements.is_empty() {
            text.push_str("- HIL obligations: ");
            text.push_str(
                &capability
                    .hil_requirements
                    .iter()
                    .map(|requirement| {
                        format!(
                            "`{}` ×{}",
                            requirement.scenario, requirement.minimum_repetitions
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            text.push('\n');
        }
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
        text.push_str(&format!("### {}\n\n**{}**\n\n- Membership: {membership}\n- Declaration: {origin}\n- Dependencies: {}\n- Structured scope: {structured_scope}\n- Axes: implementation=`{}`, host=`{}`, vendor=`{}`, HIL=`{}`, async=`{}`\n- Proof-ready: `{}`\n- Effective ready (including dependencies): `{}`\n- Evidence: {}\n- Reasons/gaps: {}\n\n",
            escape_heading(&capability.id), escape_text(&capability.title), code_list(&capability.dependencies), capability.implementation.label(), capability.host.label(), capability.vendor.label(), capability.hil.label(), capability.async_proof.label(), capability.proof_ready(), qualification.is_ready(&capability.id), code_list(&capability.evidence), capability.gaps.iter().map(|gap| format!("`{}:{}`", gap.axis.label(), gap.id)).collect::<Vec<_>>().join(", ")));
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

fn render_mapping(catalog: &CatalogView) -> String {
    let mut text = String::from(
        "# Source inventory migration map\n\nThis ignored generated map demonstrates that each legacy row or protocol matrix cell has one canonical catalog item.\n\n| Legacy document | Legacy section | Legacy entry | Canonical ID |\n| --- | --- | --- | --- |\n",
    );
    for item in &catalog.items {
        let section = catalog
            .sections
            .iter()
            .find(|section| section.id == item.section)
            .expect("validated inventory section");
        text.push_str(&format!(
            "| `{}` | {} | {} | [`{}`](domain-inventory.md#{}) |\n",
            section.source_document.display(),
            escape_table(&section.title),
            escape_table(&item.title),
            item.id,
            item.id
        ));
    }
    text
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

fn rewrite_links(value: &str, source: &Path, output: &Path, root: &Path) -> Result<String> {
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

fn link_to(target: &Path, output: &Path, root: &Path) -> Result<String> {
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
        assert_eq!((wifi_items, phy_items, matrix_cells), (60, 75, 24));
        assert_eq!(catalog.items.len(), 135);
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
}
