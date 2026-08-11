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
    object: crate::artifacts::StoredDataObject,
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
                object,
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
    outputln!("{}", crate::cli::output::heading("Memory object"));
    outputln!("Object:       {}:{}", report.source, report.symbol);
    outputln!("Observations: {}", report.observations.len());
    for observation in &report.observations {
        let object = &observation.object;
        outputln!("\n{}", crate::cli::output::heading(&observation.profile));
        outputln!(
            "Member:  {}",
            object.member.as_deref().unwrap_or("<linked-image>")
        );
        outputln!(
            "Address: {}",
            object.address.as_deref().unwrap_or("unresolved")
        );
        outputln!("Size:    {} byte(s)", object.size);
        outputln!("Uses:    {}", object.xrefs.len());
        if crate::cli::output::details() {
            outputln!("Report:  {}", observation.report);
        }
        if !object.xrefs.is_empty() {
            outputln!(
                "{}",
                crate::cli::table::render(
                    ["Function", "Reads", "Writes", "Offsets"],
                    object.xrefs.iter().take(50).map(|xref| [
                        xref.function.clone(),
                        xref.reads.to_string(),
                        xref.writes.to_string(),
                        xref.offsets.join(", "),
                    ]),
                )
            );
        }
    }
}
