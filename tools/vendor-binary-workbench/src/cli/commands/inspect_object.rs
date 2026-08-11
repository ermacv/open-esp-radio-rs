//! Random-access inspection of one recovered global/data object.

use serde::Serialize;

use super::super::*;

#[derive(Serialize)]
struct ObjectInvestigationReport {
    schema_version: u32,
    command: &'static str,
    source: String,
    symbol: String,
    observations: Vec<ObjectObservation>,
}

#[derive(Serialize)]
struct ObjectObservation {
    profile: String,
    report: String,
    object: serde_json::Value,
}

pub(super) fn run(arguments: InspectObjectArgs, project: &ProjectSpec) -> Result<bool> {
    let (source, symbol) = arguments
        .selector
        .split_once(':')
        .ok_or_else(|| crate::Error::invalid("object selector must be SOURCE:SYMBOL"))?;
    if source.is_empty() || symbol.is_empty() || symbol.contains(':') {
        return Err(crate::Error::invalid(
            "object selector must contain one non-empty SOURCE and SYMBOL",
        ));
    }
    let mut observations = Vec::new();
    for profile in project
        .ir_profiles
        .iter()
        .filter(|profile| profile.sources.iter().any(|candidate| candidate == source))
        .filter(|profile| profile.output.is_dir())
    {
        let reader = crate::artifacts::LinkedIrReader::open(&profile.output)?;
        for object in reader.get_data_object(source, symbol)? {
            observations.push(ObjectObservation {
                profile: profile.id.clone(),
                report: profile.output.display().to_string(),
                object: serde_json::to_value(object)?,
            });
        }
    }
    let report = ObjectInvestigationReport {
        schema_version: 1,
        command: "inspect object",
        source: source.to_owned(),
        symbol: symbol.to_owned(),
        observations,
    };
    crate::cli::output::render_report(&report, || render_human(&report));
    Ok(!report.observations.is_empty())
}

fn render_human(report: &ObjectInvestigationReport) {
    crate::cli::output::line(format_args!(
        "OBJECT {}:{}  observations={}",
        report.source,
        report.symbol,
        report.observations.len()
    ));
    for observation in &report.observations {
        let object = &observation.object;
        let member = object
            .get("member")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<linked-image>");
        let address = object
            .get("address")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unresolved");
        let size = object
            .get("size")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let xrefs = object
            .get("xrefs")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        crate::cli::output::line(format_args!(
            "  profile={} member={} address={} size={} xrefs={} report={}",
            observation.profile, member, address, size, xrefs, observation.report
        ));
        if let Some(entries) = object.get("xrefs").and_then(serde_json::Value::as_array) {
            for xref in entries.iter().take(50) {
                crate::cli::output::line(format_args!(
                    "    {} reads={} writes={} offsets={}",
                    xref.get("function")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown"),
                    xref.get("reads")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    xref.get("writes")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    xref.get("offsets")
                        .and_then(serde_json::Value::as_array)
                        .map(|values| values
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default(),
                ));
            }
        }
    }
}
