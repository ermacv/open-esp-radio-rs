//! Artifact inventory analysis command.

use super::super::*;

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    let mut artifact = None;
    let mut companions = Vec::new();
    let mut prefix = "phy_".to_owned();
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--artifact" => {
                artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
            }
            "--companion" => {
                companions.push(PathBuf::from(take_value(&mut arguments, "--companion")?));
            }
            "--symbol-prefix" => {
                prefix = take_value(&mut arguments, "--symbol-prefix")?;
            }
            _ => return Err(format!("unknown analyze option: {argument}").into()),
        }
    }
    let artifact = artifact.ok_or("missing --artifact")?;
    let symbols = list_code_symbols(&artifact, &prefix)?;
    if symbols.is_empty() {
        return Err(format!("no external code symbols start with {prefix:?}").into());
    }
    let reference_catalog = ReferenceResolver::load(&artifact, &companions)?;

    let mut exact = 0usize;
    let mut incomplete = 0usize;
    let mut reference_codegen_eligible = 0usize;
    let mut reasons = BTreeMap::<String, usize>::new();
    let mut reference_reasons = BTreeMap::<String, usize>::new();
    for symbol in &symbols {
        let input = ArtifactSymbolSelector {
            artifact: artifact.clone(),
            member: symbol.member.clone(),
            symbol: symbol.name.clone(),
        };
        let trace = extract(&input, svd)?;
        let reference_trace =
            reference_catalog.trace(symbol.member.as_deref(), &symbol.name, svd)?;
        let owner = symbol.member.as_deref().unwrap_or("-");
        let reference_status = if reference_trace.is_reference_eligible() {
            reference_codegen_eligible += 1;
            "eligible"
        } else {
            "blocked"
        };
        if trace.is_exact() {
            exact += 1;
            println!(
                "FUNCTION\t{}\t{owner}\tDIRECT-TRACE-EXACT\tevents={}\treference-codegen={reference_status}\treference-dependencies={}\tindexed-mmio={}",
                symbol.name,
                trace.events.len(),
                reference_trace.reference_dependencies.len(),
                reference_trace.reference_indexed_mmio_count(),
            );
        } else {
            incomplete += 1;
            println!(
                "FUNCTION\t{}\t{owner}\tINCOMPLETE\tevents={}\tuncovered={}\treference-codegen={reference_status}\treference-dependencies={}\tindexed-mmio={}",
                symbol.name,
                trace.events.len(),
                trace.blockers.len()
                    + trace
                        .events
                        .iter()
                        .filter_map(ObservableEvent::unmapped_address)
                        .count(),
                reference_trace.reference_dependencies.len(),
                reference_trace.reference_indexed_mmio_count(),
            );
            for blocker in &trace.blockers {
                let kind = blocker
                    .split_once(' ')
                    .map_or(blocker.as_str(), |pair| pair.0);
                *reasons.entry(kind.to_owned()).or_default() += 1;
                println!("UNCOVERED\t{}\t{blocker}", symbol.name);
            }
            for address in trace
                .events
                .iter()
                .filter_map(ObservableEvent::unmapped_address)
            {
                *reasons.entry("unmapped-register".to_owned()).or_default() += 1;
                println!(
                    "UNCOVERED\t{}\tunmapped-register {:#010x}",
                    symbol.name, address
                );
            }
        }
        for blocker in &reference_trace.reference_blockers {
            let kind = blocker
                .split_once(' ')
                .map_or(blocker.as_str(), |pair| pair.0);
            *reference_reasons.entry(kind.to_owned()).or_default() += 1;
        }
    }
    println!(
        "SUMMARY\tfunctions={}\tdirect_trace_exact={exact}\tincomplete={incomplete}\treference_codegen_eligible={reference_codegen_eligible}\treference_codegen_blocked={}",
        symbols.len(),
        symbols.len() - reference_codegen_eligible,
    );
    for (reason, count) in reasons {
        println!("SUMMARY-UNCOVERED\t{reason}\t{count}");
    }
    for (reason, count) in reference_reasons {
        println!("SUMMARY-REFERENCE-BLOCKED\t{reason}\t{count}");
    }
    Ok(incomplete == 0)
}
