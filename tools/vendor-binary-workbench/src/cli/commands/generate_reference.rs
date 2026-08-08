//! Safe Rust reference generation command.

use super::super::*;

use serde::Serialize;

#[derive(Serialize)]
struct GeneratedReferenceReport {
    schema_version: u32,
    command: &'static str,
    artifact: String,
    artifact_sha256: String,
    member: Option<String>,
    symbol: String,
    output: Option<String>,
    exit_a0_modeled: bool,
    dependencies: Vec<String>,
    source: String,
}

#[tracing::instrument(
    name = "generate_reference",
    skip_all,
    fields(
        artifact = tracing::field::Empty,
        symbol = tracing::field::Empty,
        member = tracing::field::Empty
    )
)]
pub(super) fn run(arguments: ReferenceArgs, svd: &MmioMap, target: &TargetSpec) -> Result<bool> {
    let harness = target.require_available_harness()?;
    let riscv_harness = harnesses::riscv(harness)?;
    let entry_contract = harnesses::entry_contract(harness, &arguments.entry_contract)?;
    let artifact = arguments
        .artifact
        .ok_or("missing --artifact")
        .map_err(crate::Error::invalid)?;
    let symbol = arguments
        .symbol
        .ok_or("missing --symbol")
        .map_err(crate::Error::invalid)?;
    let span = tracing::Span::current();
    span.record("artifact", tracing::field::display(artifact.display()));
    span.record("symbol", symbol.as_str());
    if let Some(member) = arguments.member.as_deref() {
        span.record("member", member);
    }
    let input = ArtifactSymbolSelector {
        artifact: artifact.clone(),
        member: arguments.member.clone(),
        symbol,
    };
    let trace = extract_reference(
        &input,
        &arguments.companion,
        riscv_harness,
        entry_contract,
        svd,
    )?;
    let resolved = ResolvedReferenceProgram::try_from(&trace)
        .map_err(|error| -> Error { crate::Error::invalid(error) })?;
    let digest = artifact_sha256(&artifact)?;
    let companion_provenance = arguments
        .companion
        .iter()
        .map(|companion| Ok((companion.display().to_string(), artifact_sha256(companion)?)))
        .collect::<Result<Vec<_>>>()?;
    let generated = codegen::generate(
        &resolved,
        &artifact.display().to_string(),
        &digest,
        arguments.member.as_deref(),
        &companion_provenance,
    )
    .map_err(|error| -> Error { crate::Error::invalid(error) })?;
    if let Some(output) = arguments.output.as_deref() {
        fs::write(output, &generated.source)?;
    }
    let report = GeneratedReferenceReport {
        schema_version: 1,
        command: "reference generate",
        artifact: artifact.display().to_string(),
        artifact_sha256: digest,
        member: arguments.member,
        symbol: trace.symbol,
        output: arguments.output.map(|path| path.display().to_string()),
        exit_a0_modeled: generated.exit_a0_modeled,
        dependencies: resolved.dependencies,
        source: generated.source,
    };
    if !crate::cli::output::structured(&report) {
        if let Some(output) = &report.output {
            outputln!(
                "GENERATED\t{}\t{}\texit-a0={}",
                report.symbol,
                output,
                if report.exit_a0_modeled {
                    "modeled"
                } else {
                    "unresolved"
                }
            );
        } else {
            crate::cli::output::text(&report.source);
        }
    }
    Ok(true)
}
