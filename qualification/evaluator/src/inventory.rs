//! Deterministic ignored inventory views derived from resolved programs.

use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    Result,
    model::{CapabilityOrigin, Qualification},
};

static INVENTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn write(qualification: &Qualification, output_directory: &Path) -> Result<()> {
    fs::create_dir_all(output_directory)?;
    let output = output_directory.join("capabilities.md");
    let bytes = render(qualification).into_bytes();
    let temporary = output_directory.join(format!(
        ".capabilities.md.tmp-{}-{}",
        std::process::id(),
        INVENTORY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, &output)?;
        File::open(output_directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result?;
    println!("INVENTORY\t{}", output.display());
    Ok(())
}

fn render(qualification: &Qualification) -> String {
    let mut output = String::new();
    output.push_str("# Qualification capability inventory\n\n");
    output.push_str("This ignored view is derived from canonical declarations, the resolved qualification program, and the evaluator's current evidence inputs. It is not a readiness authority.\n\n");
    output.push_str("## Source identity\n\n");
    output.push_str(&format!(
        "- Program `{}`: schema {}, SHA-256 `{}` (`{}`)\n",
        escape_code(&qualification.program_source.id),
        qualification.program_source.schema,
        qualification.program_source.sha256,
        escape_code(&qualification.program_source.path.display().to_string())
    ));
    for source in &qualification.catalog_sources {
        output.push_str(&format!(
            "- Catalog `{}`: schema {}, SHA-256 `{}` (`{}`)\n",
            escape_code(&source.id),
            source.schema,
            source.sha256,
            escape_code(&source.path.display().to_string())
        ));
    }
    output.push_str("\n## Resolved program\n\n");
    output.push_str("| Capability | Declaration | Exact scope | Reviewed source coverage | Program membership | Evidence-derived state | Current limitations |\n");
    output.push_str("| --- | --- | --- | --- | --- | --- | --- |\n");
    for capability in qualification.capabilities.values() {
        let origin = match &qualification.capability_origins[&capability.id] {
            CapabilityOrigin::Program => {
                format!("program `{}`", qualification.program_source.path.display())
            }
            CapabilityOrigin::Catalog { id, path } => {
                format!("catalog `{id}` (`{}`)", path.display())
            }
        };
        let source_coverage = format!(
            "implementation={}; host={}; async={}; source-contracts={}",
            capability.implementation.label(),
            capability.host.label(),
            capability.async_proof.label(),
            capability.source_contracts.len()
        );
        let evidence = format!(
            "vendor={}; HIL={}; ready={}",
            capability.vendor.label(),
            capability.hil.label(),
            qualification.is_ready(&capability.id)
        );
        let declared_scope = qualification.catalog_scopes.get(&capability.id).map_or_else(
            || capability.scope.clone(),
            |scope| {
                format!(
                    "{}; chip={}; role={}; PHY={}; security={}; composition={}; level={}; activation={}",
                    capability.scope,
                    scope.chip,
                    scope.role,
                    scope.phy,
                    scope.security.join(","),
                    scope.composition,
                    scope.level.label(),
                    scope.activation_boundary
                )
            },
        );
        let evidence_gaps = capability
            .gaps
            .iter()
            .map(|gap| format!("{}:{}", gap.axis.label(), gap.id))
            .collect::<Vec<_>>()
            .join("; ");
        let limitations = qualification
            .catalog_scopes
            .get(&capability.id)
            .map_or_else(
                || {
                    if evidence_gaps.is_empty() {
                        "none declared".to_owned()
                    } else {
                        evidence_gaps.clone()
                    }
                },
                |scope| {
                    if evidence_gaps.is_empty() {
                        scope.limitations.clone()
                    } else {
                        format!("{}; evidence gaps: {evidence_gaps}", scope.limitations)
                    }
                },
            );
        output.push_str(&format!(
            "| `{}` | {} | {} | {} | required | {} | {} |\n",
            escape_table(&capability.id),
            escape_table(&origin),
            escape_table(&declared_scope),
            escape_table(&source_coverage),
            escape_table(&evidence),
            escape_table(&limitations)
        ));
    }
    output
}

fn escape_table(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace(['\r', '\n'], " ")
}

fn escape_code(value: &str) -> String {
    value.replace('`', "\\`").replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hil::{HilEvidenceSummary, RepositoryState},
        model::{
            AsyncProof, Capability, CapabilityOrigin, CapabilityScope, EvidenceInputs, HilProof,
            HostProof, ImplementationProof, SourceIdentity, VendorProof,
        },
    };
    use std::{collections::BTreeMap, path::PathBuf};

    #[test]
    fn markdown_cells_escape_structure() {
        assert_eq!(escape_table("a|b\nc"), "a&#124;b c");
        assert_eq!(escape_code("a`b"), "a\\`b");
    }

    #[test]
    fn repeated_render_is_deterministic_and_identifies_sources() {
        let capability = Capability {
            id: "wifi-channel".to_owned(),
            title: "Wi-Fi channel".to_owned(),
            scope: "One switch".to_owned(),
            implementation: ImplementationProof::Complete,
            host: HostProof::Covered,
            vendor: VendorProof::Mapped,
            hil: HilProof::Missing,
            async_proof: AsyncProof::Bounded,
            dependencies: Vec::new(),
            gaps: Vec::new(),
            evidence: Vec::new(),
            source_contracts: Vec::new(),
        };
        let qualification = Qualification {
            target: "wifi".to_owned(),
            repository: RepositoryState {
                commit: "commit".to_owned(),
                dirty: true,
            },
            evidence_inputs: EvidenceInputs {
                verification_entries: 0,
                verification_current_release_entries: 0,
                hil: HilEvidenceSummary::default(),
            },
            capabilities: BTreeMap::from([("wifi-channel".to_owned(), capability)]),
            program_source: SourceIdentity {
                id: "wifi".to_owned(),
                schema: 4,
                path: PathBuf::from("qualification/wifi.toml"),
                sha256: "aa".repeat(32),
            },
            catalog_sources: vec![SourceIdentity {
                id: "wifi-phy".to_owned(),
                schema: 1,
                path: PathBuf::from("qualification/catalog/wifi-phy.toml"),
                sha256: "bb".repeat(32),
            }],
            capability_origins: BTreeMap::from([(
                "wifi-channel".to_owned(),
                CapabilityOrigin::Catalog {
                    id: "wifi-phy".to_owned(),
                    path: PathBuf::from("qualification/catalog/wifi-phy.toml"),
                },
            )]),
            catalog_scopes: BTreeMap::from([(
                "wifi-channel".to_owned(),
                toml_edit::de::from_str::<CapabilityScope>(
                    r#"
chip = "esp32s31"
role = "station"
phy = "wifi-2g4"
security = ["independent-of-link-security"]
composition = "connected-station"
level = "composed-product"
activation-boundary = "After MAC stop"
limitations = "No measured RF claim"
"#,
                )
                .unwrap(),
            )]),
        };
        let first = render(&qualification);
        let second = render(&qualification);
        assert_eq!(first, second);
        assert!(first.contains("Catalog `wifi-phy`: schema 1"));
        assert!(first.contains("vendor=mapped; HIL=missing; ready=false"));
        assert!(first.contains("level=composed-product; activation=After MAC stop"));
        assert!(first.contains("No measured RF claim"));
    }
}
