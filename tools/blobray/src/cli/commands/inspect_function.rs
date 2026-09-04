//! Lossless, project-aware function investigation command.

mod calls;
mod document;
mod render;
mod replacement;

use serde::Serialize;

use super::super::*;
use crate::function_investigation::{
    CallKnowledgeEvidence, FunctionInvestigationRequest, ReplacementEvidence,
    ReviewedEffectRuleEvidence, investigate, replacement_evidence, reviewed_effect_rules,
};

#[derive(Serialize)]
pub(super) struct CallsiteInvestigationReport<'a> {
    schema_version: u32,
    command: &'static str,
    source: &'a str,
    symbol: &'a str,
    filter: Option<&'a str>,
    calls: Vec<ProfiledCallsite<'a>>,
}

#[derive(Serialize)]
pub(super) struct ProfiledCallsite<'a> {
    profile: &'a str,
    #[serde(flatten)]
    call: &'a CallKnowledgeEvidence,
}

#[derive(Serialize)]
pub(super) struct ReplacementInvestigationReport {
    schema_version: u32,
    command: &'static str,
    source: String,
    symbol: String,
    requested_case: Option<String>,
    replacements: Vec<ReplacementEvidence>,
    vendor_effects: Vec<VendorEffectEvidence>,
    reviewed_effects: Vec<ReviewedEffectRuleEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct VendorEffectEvidence {
    profile: String,
    address: u64,
    block: Option<usize>,
    kind: &'static str,
    access: String,
    width: u8,
    targets: Vec<String>,
    guards: Vec<String>,
    value: Option<String>,
}

pub(super) fn run(arguments: InspectFunctionArgs, project: &ProjectSpec) -> Result<bool> {
    let full = arguments.full;
    let focused_calls = arguments.calls || arguments.call.is_some();
    let call_filter = arguments.call.clone();
    let (source, selector) = arguments
        .selector
        .split_once(':')
        .ok_or_else(|| crate::Error::invalid("function selector must be SOURCE:SYMBOL"))?;
    if source.is_empty() || selector.is_empty() {
        return Err(crate::Error::invalid(
            "function selector must contain one non-empty SOURCE and SYMBOL",
        ));
    }
    let resolved = resolve_semantic_selector(project, source, selector)?;
    let (symbol, runtime_address, resolved_member) = match &resolved {
        Some(resolved) => (
            resolved.symbol.as_str(),
            resolved.address.map(u64::from),
            resolved.member.as_deref(),
        ),
        None => {
            let (symbol, runtime_address) = parse_exact_symbol(selector)?;
            (symbol, runtime_address, None)
        }
    };
    if let (Some(requested), Some(resolved)) = (arguments.member.as_deref(), resolved_member)
        && requested != resolved
    {
        return Err(crate::Error::invalid(format!(
            "semantic function selector {selector:?} resolves to archive member {resolved:?}, not requested member {requested:?}"
        )));
    }
    let member = arguments.member.as_deref().or(resolved_member);
    if arguments.replacement {
        let mut replacements = replacement_evidence(source, symbol, project)?;
        if let Some(requested) = arguments.case.as_deref() {
            let found = replacements.iter().any(|replacement| {
                replacement.proofs.iter().any(|proof| {
                    proof
                        .execution_cases
                        .iter()
                        .any(|case| case.name == requested)
                })
            });
            if !found {
                return Err(crate::Error::invalid(format!(
                    "replacement evidence for {source}:{symbol} has no execution case {requested:?}"
                )));
            }
            for replacement in &mut replacements {
                for proof in &mut replacement.proofs {
                    proof.execution_cases.retain(|case| case.name == requested);
                    proof.adapter_cases.retain(|case| case.name == requested);
                }
            }
        }
        let vendor_effects = match arguments.artifact.as_deref() {
            Some(artifact) => {
                let investigation = investigate(
                    FunctionInvestigationRequest {
                        source,
                        symbol,
                        runtime_address,
                        artifact,
                        inventories: &arguments.inventory,
                        member,
                        origin_member: arguments.origin_member.as_deref(),
                        graph_depth: 0,
                        include_callers: false,
                        cfg_path: None,
                        include_linked_ir_record: false,
                    },
                    project,
                )?;
                direct_vendor_effects(&investigation)
            }
            None => Vec::new(),
        };
        let report = ReplacementInvestigationReport {
            schema_version: 5,
            command: "inspect function replacement",
            source: source.to_owned(),
            symbol: symbol.to_owned(),
            requested_case: arguments.case,
            replacements,
            vendor_effects,
            reviewed_effects: reviewed_effect_rules(source, symbol, project)?,
        };
        let found = !report.replacements.is_empty();
        crate::cli::output::render_report(&report, || replacement::render(&report));
        return Ok(found);
    }
    let artifact = arguments.artifact.as_deref().ok_or_else(|| {
        crate::Error::invalid(format!(
            "run spec does not define source-artifact:{source}; pass --artifact"
        ))
    })?;
    let report = investigate(
        FunctionInvestigationRequest {
            source,
            symbol,
            runtime_address,
            artifact,
            inventories: &arguments.inventory,
            member,
            origin_member: arguments.origin_member.as_deref(),
            graph_depth: arguments.depth,
            include_callers: arguments.callers,
            cfg_path: arguments.path.as_deref(),
            include_linked_ir_record: full,
        },
        project,
    )?;
    if focused_calls {
        let callsites = calls::report(&report, call_filter.as_deref());
        crate::cli::output::render_report(&callsites, || {
            calls::render(&report, call_filter.as_deref());
        });
    } else {
        if full {
            crate::cli::output::render_report(&report, || render::human(&report, true));
        } else {
            let document = document::CompactFunctionInvestigation::from_report(&report);
            crate::cli::output::render_report(&document, || render::human(&report, false));
        }
    }
    Ok(report.runtime.accounted_bytes == report.runtime.size)
}

struct ResolvedSemanticFunction {
    symbol: String,
    member: Option<String>,
    address: Option<u32>,
}

fn resolve_semantic_selector(
    project: &ProjectSpec,
    source: &str,
    selector: &str,
) -> Result<Option<ResolvedSemanticFunction>> {
    if !selector.contains(':') {
        return Ok(None);
    }
    parse_semantic_selector(selector)?
        .expect("a selector containing a colon must be a semantic identity");
    let mut matches = std::collections::BTreeSet::new();
    for profile in project
        .ir_profiles
        .iter()
        .filter(|profile| profile.sources.iter().any(|candidate| candidate == source))
        .filter(|profile| profile.output.is_dir())
    {
        let reader = crate::artifacts::LinkedIrReader::open(&profile.output)?;
        if let Some(function) = reader.get_function_by_identity(selector)?
            && function.source == source
        {
            matches.insert((function.symbol, function.member, function.address));
        }
    }
    let (symbol, member, address) = match matches.len() {
        0 => {
            return Err(crate::Error::invalid(format!(
                "no generated linked-IR profile resolves {source}:{selector}; rebuild project IR after accepting the pin"
            )));
        }
        1 => matches.into_iter().next().expect("one match was counted"),
        count => {
            return Err(crate::Error::invalid(format!(
                "generated linked-IR profiles resolve {source}:{selector} to {count} different raw functions"
            )));
        }
    };
    Ok(Some(ResolvedSemanticFunction {
        symbol,
        member,
        address,
    }))
}

fn parse_semantic_selector(
    selector: &str,
) -> Result<Option<open_radio_vendor_contracts::SemanticEntityId>> {
    if !selector.contains(':') {
        return Ok(None);
    }
    let semantic = selector
        .parse::<open_radio_vendor_contracts::SemanticEntityId>()
        .map_err(|error| {
            crate::Error::invalid(format!(
                "function selector after SOURCE is neither a raw symbol nor a canonical semantic identity: {error}"
            ))
        })?;
    if semantic.domain() != open_radio_vendor_contracts::EntityDomain::Function {
        return Err(crate::Error::invalid(format!(
            "inspect function requires a function semantic identity, got {semantic}"
        )));
    }
    Ok(Some(semantic))
}

fn direct_vendor_effects(
    report: &crate::function_investigation::FunctionInvestigationReport,
) -> Vec<VendorEffectEvidence> {
    type Key = (
        String,
        u64,
        Option<usize>,
        &'static str,
        String,
        u8,
        Option<String>,
    );
    let mut grouped = std::collections::BTreeMap::<
        Key,
        (
            std::collections::BTreeSet<String>,
            std::collections::BTreeSet<String>,
        ),
    >::new();
    for semantic in &report.semantics {
        for instruction in &semantic.instruction_evidence {
            for effect in &instruction.effects {
                let key = (
                    semantic.profile.clone(),
                    instruction.address,
                    instruction.block,
                    effect.kind,
                    effect.access.clone(),
                    effect.width,
                    effect.value.clone(),
                );
                let (targets, guards) = grouped.entry(key).or_default();
                targets.insert(effect.target.clone());
                guards.extend(effect.guards.iter().cloned());
            }
        }
    }
    grouped
        .into_iter()
        .map(
            |((profile, address, block, kind, access, width, value), (targets, guards))| {
                VendorEffectEvidence {
                    profile,
                    address,
                    block,
                    kind,
                    access,
                    width,
                    targets: targets.into_iter().collect(),
                    guards: guards.into_iter().collect(),
                    value,
                }
            },
        )
        .collect()
}

fn parse_exact_symbol(input: &str) -> Result<(&str, Option<u64>)> {
    let Some((symbol, address)) = input.rsplit_once("@0x") else {
        return Ok((input, None));
    };
    if symbol.is_empty() || address.is_empty() {
        return Err(crate::Error::invalid(
            "exact function identity must be SYMBOL@0xADDRESS",
        ));
    }
    let address = u64::from_str_radix(address, 16).map_err(|_| {
        crate::Error::invalid(format!("invalid linked function address in {input:?}"))
    })?;
    Ok((symbol, Some(address)))
}

#[cfg(test)]
mod tests {
    use super::{parse_exact_symbol, parse_semantic_selector};

    #[test]
    fn exact_identity_keeps_the_symbol_and_selects_the_linked_address() {
        assert_eq!(
            parse_exact_symbol("ppTask@0x10067fa0").unwrap(),
            ("ppTask", Some(0x1006_7fa0))
        );
        assert_eq!(parse_exact_symbol("ppTask").unwrap(), ("ppTask", None));
        assert!(parse_exact_symbol("ppTask@0xnot-hex").is_err());
    }

    #[test]
    fn semantic_selector_accepts_only_function_identities() {
        let semantic = parse_semantic_selector("function:esp-idf/ble/controller/start")
            .unwrap()
            .expect("semantic selector");
        assert_eq!(
            semantic.to_string(),
            "function:esp-idf/ble/controller/start"
        );
        assert!(parse_semantic_selector("raw_symbol").unwrap().is_none());
        assert!(parse_semantic_selector("memory-object:esp-idf/ble/state").is_err());
        assert!(parse_semantic_selector("function:").is_err());
    }
}
