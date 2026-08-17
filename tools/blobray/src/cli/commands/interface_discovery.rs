//! Generic pointer-table and indirect-call discovery over project artifacts.

use super::super::*;
use super::interface_discovery_options::{resolve_options, selected_inputs};
use crate::{
    analysis::{
        ProjectInterfaceDiscovery as Discovery, ProjectInterfaceDiscoveryOptions,
        discover_project_interfaces,
    },
    interface_discovery::{InterfaceArgumentValue, InterfaceCallCandidate},
    run_spec::RunSpec,
};

fn signed_hex(value: i32) -> String {
    if value < 0 {
        format!("-{:#x}", value.unsigned_abs())
    } else {
        format!("+{:#x}", value as u32)
    }
}

fn compact_arguments(call: &InterfaceCallCandidate) -> String {
    let values = call
        .arguments
        .iter()
        .enumerate()
        .filter(|(_, value)| !matches!(value, InterfaceArgumentValue::Unknown))
        .map(|(index, value)| format!("a{index}={}", value.canonical()))
        .collect::<Vec<_>>();
    let rendered = if values.is_empty() {
        "-".to_owned()
    } else {
        values.join(",")
    };
    crate::cli::table::compact(rendered, 64)
}

fn artifact_label(path: &std::path::Path) -> String {
    let label = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| path.as_os_str().to_string_lossy());
    crate::cli::table::compact(label, 52)
}

fn print_report(discovery: &Discovery) {
    const MAX_CALLS: usize = 32;
    const MAX_CALLS_PER_ARTIFACT: usize = 8;

    // A global `take(32)` made the first large ROM image monopolize the human
    // sample and hid calls from the archive being investigated. Keep the
    // machine report complete, but show a small representative slice of every
    // input in the terminal.
    let visible_calls = (0..discovery.linkage.artifacts.len())
        .flat_map(|artifact| {
            discovery
                .calls
                .iter()
                .filter(move |call| call.artifact == artifact)
                .take(MAX_CALLS_PER_ARTIFACT)
        })
        .take(MAX_CALLS)
        .collect::<Vec<_>>();

    outputln!("Interface discovery");
    outputln!(
        "Artifacts:\n{}",
        crate::cli::table::render(
            ["#", "Functions", "Roles", "Sources", "Path"],
            discovery
                .linkage
                .artifacts
                .iter()
                .enumerate()
                .map(|(index, artifact)| [
                    index.to_string(),
                    discovery.functions[index].to_string(),
                    artifact.roles.join(", "),
                    artifact.sources.join(", "),
                    artifact_label(&artifact.path),
                ]),
        )
    );
    outputln!(
        "Representative interface calls ({} of {}):\n{}",
        visible_calls.len(),
        discovery.calls.len(),
        crate::cli::table::render(
            ["Artifact", "Function", "Site", "Root", "Slot", "Arguments"],
            visible_calls.into_iter().map(|discovered| {
                let call = &discovered.call;
                let slot = call
                    .target
                    .slot()
                    .map(|load| signed_hex(load.offset))
                    .unwrap_or_else(|| "-".to_owned());
                [
                    discovered.artifact.to_string(),
                    crate::cli::table::compact(&call.function, 40),
                    format!("{:#010x}", call.site),
                    crate::cli::table::compact(call.target.root.canonical(), 24),
                    slot,
                    compact_arguments(call),
                ]
            }),
        )
    );
    let table_calls = discovery
        .calls
        .iter()
        .filter(|call| !call.call.target.loads.is_empty())
        .count();
    outputln!(
        "Summary: artifacts={} functions={} indirect-candidates={} table-slot-candidates={} decode-blockers={} analysis-failures={} semantic-claims=false completeness-claim=false",
        discovery.linkage.artifacts.len(),
        discovery.functions.iter().sum::<usize>(),
        discovery.calls.len(),
        table_calls,
        discovery.decode_blockers.len(),
        discovery.failures.len(),
    );
    if !discovery.decode_blockers.is_empty() || !discovery.failures.is_empty() {
        outputln!(
            "Decode blockers are retained with instruction provenance in the JSON report; usable findings from other instructions and functions remain available."
        );
    }
}

#[tracing::instrument(name = "discover_interfaces", skip_all)]
pub(super) fn run(
    arguments: InterfaceDiscoverArgs,
    run_spec: &RunSpec,
    project: Option<&crate::project::ProjectSpec>,
) -> Result<bool> {
    let options = resolve_options(arguments);
    if options.check && options.output.is_none() {
        return Err(crate::Error::invalid(
            "interfaces discover --check requires --output PATH",
        ));
    }
    let inputs = selected_inputs(run_spec, &options)?;
    if inputs.is_empty() {
        return Err(crate::Error::invalid(
            "run spec has no artifact or inventory inputs for interface discovery",
        ));
    }
    tracing::debug!(inputs = inputs.len(), "resolved interface discovery inputs");
    let effective_code = project
        .map(crate::analysis::EffectiveCodeCatalog::load)
        .transpose()?;
    let discovery = discover_project_interfaces(
        &inputs,
        &ProjectInterfaceDiscoveryOptions {
            name_prefix: options.name_prefix.clone(),
            tables_only: options.tables_only,
        },
        effective_code.as_ref(),
    )?;
    let document = crate::artifacts::build_interface_facts(&discovery)?;
    if !crate::cli::output::structured(&document) {
        print_report(&discovery);
    }
    if let Some(path) = options.output.as_deref() {
        crate::application::generated_file::write_or_check_json(
            path,
            &document,
            options.check,
            "interface discovery report",
            false,
        )?;
        tracing::info!(
            status = if options.check { "verified" } else { "written" },
            path = %path.display(),
            "interface discovery JSON report"
        );
    }
    if !discovery.decode_blockers.is_empty() || !discovery.failures.is_empty() {
        tracing::warn!(
            decode_blockers = discovery.decode_blockers.len(),
            analysis_failures = discovery.failures.len(),
            "interface discovery retained partial findings"
        );
    }
    // Interface discovery explicitly makes no completeness claim. Per-PC
    // blockers remain typed evidence and do not discard usable calls.
    Ok(true)
}
