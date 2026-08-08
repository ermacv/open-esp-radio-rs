//! Direct trace extraction command.

use super::super::*;

pub(super) fn run(arguments: TraceInputArgs, svd: &MmioMap) -> Result<bool> {
    let input = ArtifactSymbolSelector {
        artifact: arguments
            .artifact
            .ok_or("missing --artifact")
            .map_err(crate::Error::invalid)?,
        member: arguments.member,
        symbol: arguments
            .symbol
            .ok_or("missing --symbol")
            .map_err(crate::Error::invalid)?,
    };
    let trace = extract(&input, svd)?;
    let document = trace_document(&trace);
    crate::cli::output::render_report(&document, || print_trace(&trace), || print_trace(&trace));
    Ok(trace.is_exact())
}
