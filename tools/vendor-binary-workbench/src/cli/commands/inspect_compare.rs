//! Direct trace comparison command.

use super::super::*;

use serde::Serialize;

#[derive(Serialize)]
struct TraceComparisonDocument<'a> {
    schema_version: u32,
    command: &'static str,
    mode: EquivalenceMode,
    left: TraceDocument<'a>,
    right: TraceDocument<'a>,
    verdict: EquivalenceVerdict,
}

pub(super) fn run(arguments: InspectCompareArgs, svd: &MmioMap) -> Result<bool> {
    let left = ArtifactSymbolSelector {
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
    let right = ArtifactSymbolSelector {
        artifact: arguments
            .right_artifact
            .ok_or("missing --right-artifact")
            .map_err(crate::Error::invalid)?,
        member: arguments.right_member,
        symbol: arguments
            .right_symbol
            .ok_or("missing --right-symbol")
            .map_err(crate::Error::invalid)?,
    };
    let left_trace = extract(&left, svd)?;
    let right_trace = extract(&right, svd)?;
    let complete = left_trace.is_exact() && right_trace.is_exact();
    let equal = complete && traces_equal(&left_trace, &right_trace);
    let verdict = if !complete {
        EquivalenceVerdict::Incomplete
    } else if equal {
        EquivalenceVerdict::Match
    } else {
        EquivalenceVerdict::Diff
    };
    let document = TraceComparisonDocument {
        schema_version: 2,
        command: "inspect compare",
        mode: EquivalenceMode::Physical,
        left: trace_document(&left_trace),
        right: trace_document(&right_trace),
        verdict,
    };
    let render = || {
        print_trace(&left_trace);
        print_trace(&right_trace);
        outputln!(
            "VERDICT\t{}",
            if !complete {
                "INCOMPLETE"
            } else if equal {
                "MATCH"
            } else {
                "DIFF"
            }
        );
    };
    crate::cli::output::render_report(&document, render, render);
    Ok(equal)
}
