//! Direct trace comparison command.

use super::super::*;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum TraceComparisonVerdict {
    Match,
    Mismatch,
    Incomplete,
}

#[derive(Serialize)]
struct TraceComparisonDocument<'a> {
    schema_version: u32,
    command: &'static str,
    left: TraceDocument<'a>,
    right: TraceDocument<'a>,
    verdict: TraceComparisonVerdict,
}

pub(super) fn run(arguments: InspectCompareArgs, svd: &MmioRegisterMap) -> Result<bool> {
    let left = ArtifactSymbolSelector {
        artifact: arguments.artifact.ok_or("missing --artifact")?,
        member: arguments.member,
        symbol: arguments.symbol.ok_or("missing --symbol")?,
    };
    let right = ArtifactSymbolSelector {
        artifact: arguments.right_artifact.ok_or("missing --right-artifact")?,
        member: arguments.right_member,
        symbol: arguments.right_symbol.ok_or("missing --right-symbol")?,
    };
    let left_trace = extract(&left, svd)?;
    let right_trace = extract(&right, svd)?;
    let complete = left_trace.is_exact() && right_trace.is_exact();
    let equal = complete && traces_equal(&left_trace, &right_trace);
    let verdict = if !complete {
        TraceComparisonVerdict::Incomplete
    } else if equal {
        TraceComparisonVerdict::Match
    } else {
        TraceComparisonVerdict::Mismatch
    };
    let document = TraceComparisonDocument {
        schema_version: 1,
        command: "inspect compare",
        left: trace_document(&left_trace),
        right: trace_document(&right_trace),
        verdict,
    };
    if !crate::cli::output::structured("trace-comparison", &document) {
        print_trace(&left_trace);
        print_trace(&right_trace);
        outputln!(
            "VERDICT\t{}",
            if !complete {
                "INCOMPLETE"
            } else if equal {
                "MATCH"
            } else {
                "MISMATCH"
            }
        );
    }
    Ok(equal)
}
