//! CLI filtering and presentation for the stored symbol inventory.

use serde::Serialize;

use super::super::*;
use crate::{
    artifacts::{
        SymbolInventoryDocument, build_symbol_inventory_document, render_symbol_inventory,
    },
    run_spec::RunSpec,
};

type Options = SymbolInventoryArgs;

impl Options {
    fn includes(&self, symbol: &LinkageSymbol) -> bool {
        self.name_prefix
            .as_ref()
            .is_none_or(|prefix| symbol.fact.name.starts_with(prefix))
            && (!self.undefined_only
                || symbol.fact.definition == artifact::ArtifactSymbolDefinitionState::Undefined)
    }
}

#[derive(Serialize)]
struct CommandDocument<'a> {
    #[serde(flatten)]
    artifact: &'a SymbolInventoryDocument,
    #[serde(skip_serializing_if = "Option::is_none")]
    publication: Option<crate::cli::output::Publication>,
}

fn optional_human(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn print_report_human(inventory: &ProjectLinkageInventory, options: &Options) {
    outputln!("Symbol inventory");
    outputln!(
        "Artifacts:\n{}",
        crate::cli::table::render(
            [
                "#",
                "Container",
                "Objects",
                "Skipped",
                "Roles",
                "Sources",
                "Path"
            ],
            inventory
                .artifacts
                .iter()
                .enumerate()
                .map(|(index, artifact)| [
                    index.to_string(),
                    artifact.container.label().to_owned(),
                    artifact.objects.to_string(),
                    artifact.skipped_members.to_string(),
                    artifact.roles.join(", "),
                    artifact.sources.join(", "),
                    artifact.path.display().to_string(),
                ]),
        )
    );

    let symbols = inventory
        .symbols
        .iter()
        .filter(|symbol| options.includes(symbol))
        .collect::<Vec<_>>();
    outputln!(
        "Symbols:\n{}",
        crate::cli::table::render(
            [
                "Artifact",
                "Member",
                "Name",
                "Definition",
                "Address",
                "Size",
                "Resolution",
                "Candidates",
            ],
            symbols.iter().map(|symbol| [
                symbol.artifact.to_string(),
                optional_human(symbol.member.as_deref()).to_owned(),
                symbol.fact.name.clone(),
                symbol.fact.definition.label().to_owned(),
                format!("{:#x}", symbol.fact.address),
                symbol.fact.size.to_string(),
                symbol.resolution.label().to_owned(),
                symbol.candidates.len().to_string(),
            ]),
        )
    );

    let code_sections = inventory
        .artifacts
        .iter()
        .enumerate()
        .flat_map(|(artifact_index, artifact)| {
            artifact.code_sections.iter().map(move |section| {
                let coverage = &section.coverage;
                let coverage_percent = if coverage.size == 0 {
                    100.0
                } else {
                    coverage.symbol_covered_bytes as f64 * 100.0 / coverage.size as f64
                };
                [
                    artifact_index.to_string(),
                    optional_human(section.member.as_deref()).to_owned(),
                    coverage.name.clone(),
                    coverage.size.to_string(),
                    format!("{} ({coverage_percent:.1}%)", coverage.symbol_covered_bytes),
                    (coverage.size - coverage.symbol_covered_bytes).to_string(),
                    coverage.named_zero_sized_symbols.to_string(),
                    coverage.uncovered_ranges.len().to_string(),
                    coverage.function_candidates.len().to_string(),
                    coverage.recovery_blockers.len().to_string(),
                ]
            })
        })
        .collect::<Vec<_>>();
    if !code_sections.is_empty() {
        outputln!(
            "Executable code coverage by named, sized symbols:\n{}",
            crate::cli::table::render(
                [
                    "Artifact",
                    "Member",
                    "Section",
                    "Exec bytes",
                    "Symbol-covered",
                    "Uncovered",
                    "Zero-sized",
                    "Gaps",
                    "Candidates",
                    "Blockers",
                ],
                code_sections,
            )
        );
    }

    let function_candidates = inventory
        .artifacts
        .iter()
        .enumerate()
        .flat_map(|(artifact_index, artifact)| {
            artifact.code_sections.iter().flat_map(move |section| {
                section
                    .coverage
                    .function_candidates
                    .iter()
                    .map(move |candidate| {
                        let evidence = candidate
                            .direct_control_flow
                            .iter()
                            .map(|call| {
                                format!(
                                    "{}:{}@{:#x}",
                                    call.kind.label(),
                                    call.caller,
                                    call.site_offset
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        [
                            artifact_index.to_string(),
                            optional_human(section.member.as_deref()).to_owned(),
                            section.coverage.name.clone(),
                            format!("{:#x}", candidate.entry_offset),
                            format!("{:#x}", candidate.end_limit_offset),
                            if candidate.symbol_names.is_empty() {
                                "-".to_owned()
                            } else {
                                candidate.symbol_names.join(", ")
                            },
                            if evidence.is_empty() {
                                "-".to_owned()
                            } else {
                                evidence
                            },
                        ]
                    })
            })
        })
        .collect::<Vec<_>>();
    if !function_candidates.is_empty() {
        outputln!(
            "Unreviewed function-boundary candidates:\n{}",
            crate::cli::table::render(
                [
                    "Artifact",
                    "Member",
                    "Section",
                    "Entry",
                    "End limit",
                    "Symbols",
                    "Direct control flow",
                ],
                function_candidates,
            )
        );
    }

    let candidates = symbols
        .iter()
        .flat_map(|symbol| {
            symbol.candidates.iter().map(move |candidate| {
                [
                    symbol.fact.name.clone(),
                    candidate.artifact.to_string(),
                    optional_human(candidate.member.as_deref()).to_owned(),
                    format!("{:#x}", candidate.address),
                    candidate.kind.label().to_owned(),
                ]
            })
        })
        .collect::<Vec<_>>();
    if !candidates.is_empty() {
        outputln!(
            "Resolution candidates:\n{}",
            crate::cli::table::render(
                ["Symbol", "Artifact", "Member", "Address", "Kind"],
                candidates,
            )
        );
    }

    let undefined = inventory
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.fact.definition == artifact::ArtifactSymbolDefinitionState::Undefined
        })
        .count();
    let exported = inventory
        .symbols
        .iter()
        .filter(|symbol| symbol.fact.is_exported_definition())
        .count();
    let unresolved = inventory
        .symbols
        .iter()
        .filter(|symbol| symbol.resolution.is_unresolved())
        .count();
    let executable_bytes = inventory
        .artifacts
        .iter()
        .flat_map(|artifact| &artifact.code_sections)
        .map(|section| section.coverage.size)
        .sum::<u64>();
    let symbol_covered_bytes = inventory
        .artifacts
        .iter()
        .flat_map(|artifact| &artifact.code_sections)
        .map(|section| section.coverage.symbol_covered_bytes)
        .sum::<u64>();
    let function_boundary_candidates = inventory
        .artifacts
        .iter()
        .flat_map(|artifact| &artifact.code_sections)
        .map(|section| section.coverage.function_candidates.len())
        .sum::<usize>();
    let code_recovery_blockers = inventory
        .artifacts
        .iter()
        .flat_map(|artifact| &artifact.code_sections)
        .map(|section| section.coverage.recovery_blockers.len())
        .sum::<usize>();
    outputln!(
        "Summary: artifacts={} symbol-facts={} shown={} exported={} undefined={} unresolved-or-associated={} executable-bytes={} symbol-covered-bytes={} uncovered-executable-bytes={} function-boundary-candidates={} code-recovery-blockers={}",
        inventory.artifacts.len(),
        inventory.symbols.len(),
        symbols.len(),
        exported,
        undefined,
        unresolved,
        executable_bytes,
        symbol_covered_bytes,
        executable_bytes - symbol_covered_bytes,
        function_boundary_candidates,
        code_recovery_blockers,
    );
}

pub(super) fn run(options: SymbolInventoryArgs, run_spec: &RunSpec) -> Result<bool> {
    if options.check && options.json_report.is_none() {
        return Err(crate::Error::invalid(
            "symbols inventory --check requires --json-report or project [analysis.symbols]",
        ));
    }
    let inputs = run_spec
        .inputs()
        .iter()
        .map(|input| (input.role.to_string(), input.path.clone()))
        .collect::<Vec<_>>();
    let inventory = build_project_linkage_inventory(&inputs)?;
    let artifact = build_symbol_inventory_document(&inventory, |symbol| options.includes(symbol))?;
    let publication = options.json_report.as_deref().map(|path| {
        crate::cli::output::Publication::new(
            path,
            if options.check { "verified" } else { "written" },
        )
    });
    if let Some(path) = options.json_report.as_deref() {
        crate::application::generated_file::write_or_check(
            path,
            &render_symbol_inventory(&artifact)?,
            options.check,
            "symbol inventory",
        )?;
    }
    let document = CommandDocument {
        artifact: &artifact,
        publication: publication.clone(),
    };
    if !crate::cli::output::structured(&document) {
        print_report_human(&inventory, &options);
        if let Some(publication) = publication {
            outputln!("Publication: {} — {}", publication.status, publication.path);
        }
    }
    Ok(true)
}
