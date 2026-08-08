//! Direct trace extraction command.

use super::super::*;

pub(super) fn run(arguments: TraceInputArgs, svd: &MmioRegisterMap) -> Result<bool> {
    let input = ArtifactSymbolSelector {
        artifact: arguments.artifact.ok_or("missing --artifact")?,
        member: arguments.member,
        symbol: arguments.symbol.ok_or("missing --symbol")?,
    };
    let trace = extract(&input, svd)?;
    let document = trace_document(&trace);
    crate::cli::output::render_report(
        "direct-trace",
        &document,
        || print_trace(&trace),
        || print_trace(&trace),
    );
    Ok(trace.is_exact())
}
