//! Concrete vendor/Rust comparison command.

use super::super::*;

pub(super) fn run(arguments: ExecuteCompareArgs, svd: &MmioMap) -> Result<bool> {
    let scenarios = if arguments.case.is_empty() {
        vec![NamedScenario::new("default".to_owned())]
    } else {
        arguments
            .case
            .into_iter()
            .map(|arguments| {
                let mut scenario = NamedScenario::new(arguments.name);
                scenario.scenario = super::execute_run::resolve_scenario(arguments.scenario)?;
                scenario.vendor_symbol_words = arguments
                    .vendor_ram_symbol
                    .iter()
                    .map(|value| parse_symbol_word(value, "vendor-ram-symbol case clause"))
                    .collect::<Result<Vec<_>>>()?;
                scenario.rust_symbol_words = arguments
                    .rust_ram_symbol
                    .iter()
                    .map(|value| parse_symbol_word(value, "rust-ram-symbol case clause"))
                    .collect::<Result<Vec<_>>>()?;
                Ok(scenario)
            })
            .collect::<Result<Vec<_>>>()?
    };

    let vendor_artifact = arguments
        .vendor_artifact
        .ok_or("missing --vendor-artifact")
        .map_err(crate::Error::invalid)?;
    let vendor_symbol = arguments
        .vendor_symbol
        .ok_or("missing --vendor-symbol")
        .map_err(crate::Error::invalid)?;
    let rust_artifact = arguments
        .rust_artifact
        .ok_or("missing --rust-artifact")
        .map_err(crate::Error::invalid)?;
    let rust_symbol = arguments
        .rust_symbol
        .ok_or("missing --rust-symbol")
        .map_err(crate::Error::invalid)?;
    let unconstrained_coverage = [crate::verification::profiles::ProfileCoverageConstraint {
        arguments: [None; 8],
        stable_words: std::collections::BTreeMap::new(),
    }];
    let report = compare_execution_scenarios(
        svd,
        ExecutionInput {
            artifact: &vendor_artifact,
            companion: arguments.vendor_companion.as_deref(),
            symbol: &vendor_symbol,
        },
        ExecutionInput {
            artifact: &rust_artifact,
            companion: arguments.rust_companion.as_deref(),
            symbol: &rust_symbol,
        },
        crate::ExecutionComparisonPolicy {
            compare_return: arguments.compare_return,
            transaction_comparison:
                crate::verification::profiles::TransactionComparison::Observables,
            call_equivalences: &[],
            coverage_domain: &unconstrained_coverage,
        },
        &scenarios,
    )?;
    let matched = report.verdict == EquivalenceVerdict::Match;
    crate::cli::output::render_report(&report, || {
        crate::cli::render::print_execution_comparison(&report)
    });
    Ok(matched)
}
