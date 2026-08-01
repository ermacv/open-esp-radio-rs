//! Concrete vendor/Rust comparison command.

use super::super::*;

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    let mut vendor_artifact = None;
    let mut vendor_companion = None;
    let mut vendor_symbol = None;
    let mut rust_artifact = None;
    let mut rust_companion = None;
    let mut rust_symbol = None;
    let mut compare_return = false;
    let mut scenarios = Vec::new();
    let mut current_scenario: Option<NamedScenario> = None;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--vendor-artifact" => {
                vendor_artifact = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--vendor-artifact",
                )?));
            }
            "--vendor-symbol" => {
                vendor_symbol = Some(take_value(&mut arguments, "--vendor-symbol")?);
            }
            "--vendor-companion" => {
                vendor_companion = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--vendor-companion",
                )?));
            }
            "--rust-artifact" => {
                rust_artifact = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--rust-artifact",
                )?));
            }
            "--rust-symbol" => {
                rust_symbol = Some(take_value(&mut arguments, "--rust-symbol")?);
            }
            "--rust-companion" => {
                rust_companion = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--rust-companion",
                )?));
            }
            "--compare-return" => compare_return = true,
            "--case" => {
                if let Some(scenario) = current_scenario.take() {
                    scenarios.push(scenario);
                }
                current_scenario = Some(NamedScenario::new(take_value(&mut arguments, "--case")?));
            }
            "--arg" => {
                let value = take_value(&mut arguments, "--arg")?;
                current_scenario
                    .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                    .scenario
                    .arguments
                    .push(parse_u32(&value).ok_or("invalid --arg value")?);
            }
            "--mmio" => {
                let assignment = take_value(&mut arguments, "--mmio")?;
                let (address, value) = parse_assignment(&assignment, "--mmio")?;
                current_scenario
                    .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                    .scenario
                    .mmio_initial
                    .insert(address, value);
            }
            "--read" => {
                let assignment = take_value(&mut arguments, "--read")?;
                let (address, value) = parse_assignment(&assignment, "--read")?;
                current_scenario
                    .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                    .scenario
                    .mmio_reads
                    .entry(address)
                    .or_default()
                    .push_back(value);
            }
            "--ram" => {
                let assignment = take_value(&mut arguments, "--ram")?;
                let (address, value) = parse_assignment(&assignment, "--ram")?;
                let scenario = &mut current_scenario
                    .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                    .scenario;
                seed_ram_word(scenario, address, value);
            }
            "--vendor-ram-symbol" => {
                let assignment = take_value(&mut arguments, "--vendor-ram-symbol")?;
                current_scenario
                    .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                    .vendor_symbol_words
                    .push(parse_symbol_word(&assignment, "--vendor-ram-symbol")?);
            }
            "--rust-ram-symbol" => {
                let assignment = take_value(&mut arguments, "--rust-ram-symbol")?;
                current_scenario
                    .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                    .rust_symbol_words
                    .push(parse_symbol_word(&assignment, "--rust-ram-symbol")?);
            }
            "--observe" => {
                let assignment = take_value(&mut arguments, "--observe")?;
                let (address, length) = parse_assignment(&assignment, "--observe")?;
                let scenario = &mut current_scenario
                    .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                    .scenario;
                observe_memory(scenario, address, length)?;
            }
            "--max-steps" => {
                let value = take_value(&mut arguments, "--max-steps")?;
                current_scenario
                    .get_or_insert_with(|| NamedScenario::new("default".to_owned()))
                    .scenario
                    .max_steps = value.parse()?;
            }
            _ => return Err(format!("unknown execute-compare option: {argument}").into()),
        }
    }
    if let Some(scenario) = current_scenario {
        scenarios.push(scenario);
    }
    if scenarios.is_empty() {
        scenarios.push(NamedScenario::new("default".to_owned()));
    }

    let vendor_artifact = vendor_artifact.ok_or("missing --vendor-artifact")?;
    let vendor_symbol = vendor_symbol.ok_or("missing --vendor-symbol")?;
    let rust_artifact = rust_artifact.ok_or("missing --rust-artifact")?;
    let rust_symbol = rust_symbol.ok_or("missing --rust-symbol")?;
    Ok(compare_execution_scenarios(
        svd,
        ExecutionInput {
            artifact: &vendor_artifact,
            companion: vendor_companion.as_deref(),
            symbol: &vendor_symbol,
        },
        ExecutionInput {
            artifact: &rust_artifact,
            companion: rust_companion.as_deref(),
            symbol: &rust_symbol,
        },
        compare_return,
        &scenarios,
    )? == ComparisonVerdict::Match)
}
