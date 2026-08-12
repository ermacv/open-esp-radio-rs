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
    replacements: Vec<ReplacementEvidence>,
    reviewed_effects: Vec<ReviewedEffectRuleEvidence>,
    feature_qualifications: Vec<crate::qualification::FunctionQualificationEvidence>,
}

pub(super) fn run(arguments: InspectFunctionArgs, project: &ProjectSpec) -> Result<bool> {
    let full = arguments.full;
    let focused_calls = arguments.calls || arguments.call.is_some();
    let call_filter = arguments.call.clone();
    let (source, symbol) = arguments
        .selector
        .split_once(':')
        .ok_or_else(|| crate::Error::invalid("function selector must be SOURCE:SYMBOL"))?;
    if source.is_empty() || symbol.is_empty() || symbol.contains(':') {
        return Err(crate::Error::invalid(
            "function selector must contain one non-empty SOURCE and SYMBOL",
        ));
    }
    let (symbol, runtime_address) = parse_exact_symbol(symbol)?;
    if arguments.replacement {
        let report = ReplacementInvestigationReport {
            schema_version: 2,
            command: "inspect function replacement",
            source: source.to_owned(),
            symbol: symbol.to_owned(),
            replacements: replacement_evidence(source, &symbol, project)?,
            reviewed_effects: reviewed_effect_rules(source, &symbol, project)?,
            feature_qualifications: crate::qualification::evidence_for_function(
                project, source, &symbol,
            )?,
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
            inventory: arguments.inventory.as_deref(),
            member: arguments.member.as_deref(),
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
    use super::parse_exact_symbol;

    #[test]
    fn exact_identity_keeps_the_symbol_and_selects_the_linked_address() {
        assert_eq!(
            parse_exact_symbol("ppTask@0x10067fa0").unwrap(),
            ("ppTask", Some(0x1006_7fa0))
        );
        assert_eq!(parse_exact_symbol("ppTask").unwrap(), ("ppTask", None));
        assert!(parse_exact_symbol("ppTask@0xnot-hex").is_err());
    }
}
