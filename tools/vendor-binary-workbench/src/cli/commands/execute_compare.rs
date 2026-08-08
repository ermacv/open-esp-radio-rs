//! Concrete vendor/Rust comparison command.

use super::super::*;

pub(super) fn run(arguments: ExecuteCompareArgs, svd: &MmioRegisterMap) -> Result<bool> {
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
        .ok_or("missing --vendor-artifact")?;
    let vendor_symbol = arguments.vendor_symbol.ok_or("missing --vendor-symbol")?;
    let rust_artifact = arguments.rust_artifact.ok_or("missing --rust-artifact")?;
    let rust_symbol = arguments.rust_symbol.ok_or("missing --rust-symbol")?;
    let unconstrained_arguments = [[None; 8]];
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
        arguments.compare_return,
        &unconstrained_arguments,
        &scenarios,
    )?;
    let matched = report.verdict == ComparisonVerdict::Match;
    crate::cli::output::render_report(
        &report,
        || print_execution_comparison(&report),
        || print_execution_comparison(&report),
    );
    Ok(matched)
}
