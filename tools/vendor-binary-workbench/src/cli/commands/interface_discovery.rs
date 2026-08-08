//! Generic pointer-table and indirect-call discovery over project artifacts.

use std::{collections::BTreeSet, path::PathBuf};

use super::super::*;
use super::interface_discovery_options::{Options, resolve_options, selected_inputs};
use crate::{
    analysis::{LinkageSymbolLocation, ProjectLinkageInventory, build_project_linkage_inventory},
    interface_discovery::{
        InterfaceArgumentValue, InterfaceCallCandidate, InterfacePointer, InterfaceRoot,
        discover_interface_calls,
    },
    run_spec::RunSpec,
};

#[derive(Clone)]
pub(super) struct DiscoveredCall {
    pub(super) artifact: usize,
    pub(super) call: InterfaceCallCandidate,
}

#[derive(Clone)]
pub(super) struct DecodeFailure {
    pub(super) artifact: usize,
    pub(super) member: Option<String>,
    pub(super) function: String,
    pub(super) error: String,
}

pub(super) struct Discovery {
    pub(super) linkage: ProjectLinkageInventory,
    pub(super) functions: Vec<usize>,
    pub(super) calls: Vec<DiscoveredCall>,
    pub(super) failures: Vec<DecodeFailure>,
}

fn discover(inputs: &[(String, PathBuf)], options: &Options) -> Result<Discovery> {
    let linkage = build_project_linkage_inventory(inputs)?;
    let mut functions = Vec::with_capacity(linkage.artifacts.len());
    let mut calls = Vec::new();
    let mut failures = Vec::new();
    for (artifact_index, artifact) in linkage.artifacts.iter().enumerate() {
        let symbols = artifact::load_all_code_symbols(&artifact.path, &options.name_prefix)?;
        functions.push(symbols.len());
        for symbol in symbols {
            match discover_interface_calls(&symbol) {
                Ok(discovered) => calls.extend(
                    discovered
                        .into_iter()
                        .filter(|call| !options.tables_only || !call.target.loads.is_empty())
                        .map(|call| DiscoveredCall {
                            artifact: artifact_index,
                            call,
                        }),
                ),
                Err(error) => failures.push(DecodeFailure {
                    artifact: artifact_index,
                    member: symbol.member,
                    function: symbol.name,
                    error: error.to_string(),
                }),
            }
        }
    }
    calls.sort_by(|left, right| (left.artifact, &left.call).cmp(&(right.artifact, &right.call)));
    Ok(Discovery {
        linkage,
        functions,
        calls,
        failures,
    })
}

#[derive(Default)]
pub(super) struct RootLinkage {
    pub(super) symbols: BTreeSet<String>,
    pub(super) resolutions: BTreeSet<&'static str>,
    pub(super) candidates: BTreeSet<LinkageSymbolLocation>,
}

pub(super) fn root_linkage(
    discovery: &Discovery,
    artifact: usize,
    root: &InterfaceRoot,
) -> RootLinkage {
    let mut result = RootLinkage::default();
    for symbol in &discovery.linkage.symbols {
        if symbol.artifact != artifact {
            continue;
        }
        let matches = match root {
            InterfaceRoot::RelocatedSymbol {
                member,
                symbol: name,
                ..
            } => &symbol.member == member && &symbol.fact.name == name,
            InterfaceRoot::AbsoluteAddress { address } => {
                symbol.fact.definition.is_definition() && symbol.fact.address == u64::from(*address)
            }
            InterfaceRoot::FunctionArgument { .. } => false,
        };
        if matches {
            result.symbols.insert(symbol.fact.name.clone());
            result.resolutions.insert(symbol.resolution.label());
            result.candidates.extend(symbol.candidates.iter().cloned());
        }
    }
    result
}

fn signed_hex(value: i32) -> String {
    if value < 0 {
        format!("-{:#x}", value.unsigned_abs())
    } else {
        format!("+{:#x}", value as u32)
    }
}

fn load_chain(pointer: &InterfacePointer) -> String {
    if pointer.loads.is_empty() {
        return "direct-function-pointer".to_owned();
    }
    pointer
        .loads
        .iter()
        .map(|load| format!("load{}({})", load.width, signed_hex(load.offset)))
        .collect::<Vec<_>>()
        .join("->")
}

fn compact_arguments(call: &InterfaceCallCandidate) -> String {
    let values = call
        .arguments
        .iter()
        .enumerate()
        .filter(|(_, value)| !matches!(value, InterfaceArgumentValue::Unknown))
        .map(|(index, value)| format!("a{index}={}", value.canonical()))
        .collect::<Vec<_>>();
    if values.is_empty() {
        "-".to_owned()
    } else {
        values.join(",")
    }
}

fn print_report(discovery: &Discovery) {
    for (index, artifact) in discovery.linkage.artifacts.iter().enumerate() {
        outputln!(
            "ARTIFACT\tindex={index}\tfunctions={}\troles={}\tsources={}\tpath={}",
            discovery.functions[index],
            artifact.roles.join(","),
            artifact.sources.join(","),
            artifact.path.display()
        );
    }
    for discovered in &discovery.calls {
        let call = &discovered.call;
        let linkage = root_linkage(discovery, discovered.artifact, &call.target.root);
        let slot = call
            .target
            .slot()
            .map(|load| signed_hex(load.offset))
            .unwrap_or_else(|| "-".to_owned());
        outputln!(
            "INTERFACE_CALL\tartifact={}\tmember={}\tfunction={}\tfunction-address={:#x}\tsite={:#x}\tkind={}\troot-kind={}\troot-addressing={}\troot={}\tchain={}\tcontainer-depth={}\tslot={}\tjalr-offset={}\troot-symbols={}\tlinkage={}\tcandidates={}\targuments={}",
            discovered.artifact,
            call.member.as_deref().unwrap_or("-"),
            call.function,
            call.function_address,
            call.site,
            call.kind.label(),
            call.target.root.kind(),
            call.target
                .root
                .addressing()
                .map_or("-", |addressing| addressing.label()),
            call.target.root.canonical(),
            load_chain(&call.target),
            call.target.container_loads().len(),
            slot,
            signed_hex(call.jalr_offset),
            linkage.symbols.into_iter().collect::<Vec<_>>().join(","),
            linkage
                .resolutions
                .into_iter()
                .collect::<Vec<_>>()
                .join(","),
            linkage.candidates.len(),
            compact_arguments(call),
        );
    }
    for failure in &discovery.failures {
        outputln!(
            "DECODE_FAILURE\tartifact={}\tmember={}\tfunction={}\terror={}",
            failure.artifact,
            failure.member.as_deref().unwrap_or("-"),
            failure.function,
            failure.error
        );
    }
    let table_calls = discovery
        .calls
        .iter()
        .filter(|call| !call.call.target.loads.is_empty())
        .count();
    outputln!(
        "SUMMARY\tartifacts={}\tfunctions={}\tindirect-candidates={}\ttable-slot-candidates={}\tdecode-failures={}\tsemantic-claims=false\tcompleteness-claim=false",
        discovery.linkage.artifacts.len(),
        discovery.functions.iter().sum::<usize>(),
        discovery.calls.len(),
        table_calls,
        discovery.failures.len(),
    );
}

#[tracing::instrument(name = "discover_interfaces", skip_all)]
pub(super) fn run(arguments: InterfaceDiscoverArgs, run_spec: &RunSpec) -> Result<bool> {
    let options = resolve_options(arguments);
    if options.check && options.json_report.is_none() {
        return Err(crate::Error::invalid(
            "interfaces discover --check requires --json-report PATH",
        ));
    }
    let inputs = selected_inputs(run_spec, &options)?;
    if inputs.is_empty() {
        return Err(crate::Error::invalid(
            "run spec has no artifact or inventory inputs for interface discovery",
        ));
    }
    tracing::debug!(inputs = inputs.len(), "resolved interface discovery inputs");
    let discovery = discover(&inputs, &options)?;
    let document = super::interface_discovery_json::document(&discovery)?;
    if !crate::cli::output::structured(&document) {
        print_report(&discovery);
    }
    if let Some(path) = options.json_report.as_deref() {
        let output = super::interface_discovery_json::render_document(&document)?;
        crate::application::generated_file::write_or_check(
            path,
            &output,
            options.check,
            "interface discovery report",
        )?;
        tracing::info!(
            status = if options.check { "verified" } else { "written" },
            path = %path.display(),
            "interface discovery JSON report"
        );
    }
    Ok(discovery.failures.is_empty())
}
