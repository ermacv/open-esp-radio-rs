//! Artifact selection and user-facing trace extraction helpers.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::{ReferenceResolver, trace_binary_symbol};
use crate::{
    ArtifactSymbolIdentity, FunctionAnalysis, MmioRegisterMap, ObservableEvent, Result, artifact,
};

pub(crate) fn list_code_symbols(
    artifact: &Path,
    prefix: &str,
) -> Result<Vec<ArtifactSymbolIdentity>> {
    Ok(artifact::load_symbols(artifact, prefix)?
        .into_iter()
        .map(|symbol| ArtifactSymbolIdentity {
            member: symbol.member,
            name: symbol.name,
        })
        .collect())
}

#[derive(Clone, Debug)]
pub(crate) struct ArtifactSymbolSelector {
    pub(crate) artifact: PathBuf,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
}

pub(crate) fn take_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value").into())
}

pub(crate) fn parse_input(
    arguments: &mut impl Iterator<Item = String>,
    prefix: &str,
) -> Result<ArtifactSymbolSelector> {
    let mut artifact = None;
    let mut member = None;
    let mut symbol = None;
    while let Some(argument) = arguments.next() {
        let plain = prefix.is_empty();
        let artifact_option = if plain {
            "--artifact".to_owned()
        } else {
            format!("--{prefix}-artifact")
        };
        let member_option = if plain {
            "--member".to_owned()
        } else {
            format!("--{prefix}-member")
        };
        let symbol_option = if plain {
            "--symbol".to_owned()
        } else {
            format!("--{prefix}-symbol")
        };
        if argument == artifact_option {
            artifact = Some(PathBuf::from(take_value(arguments, &artifact_option)?));
        } else if argument == member_option {
            member = Some(take_value(arguments, &member_option)?);
        } else if argument == symbol_option {
            symbol = Some(take_value(arguments, &symbol_option)?);
        } else {
            return Err(format!("unknown {prefix} input option: {argument}").into());
        }
        if artifact.is_some() && symbol.is_some() && (!plain || argument == symbol_option) {
            break;
        }
    }
    Ok(ArtifactSymbolSelector {
        artifact: artifact.ok_or_else(|| format!("missing --{prefix}-artifact"))?,
        member,
        symbol: symbol.ok_or_else(|| format!("missing --{prefix}-symbol"))?,
    })
}

pub(crate) fn extract(
    input: &ArtifactSymbolSelector,
    svd: &MmioRegisterMap,
) -> Result<FunctionAnalysis> {
    let symbols = artifact::load_symbols(&input.artifact, &input.symbol)?;
    let symbol = symbols
        .iter()
        .find(|candidate| {
            candidate.name == input.symbol
                && input
                    .member
                    .as_deref()
                    .is_none_or(|member| candidate.member.as_deref() == Some(member))
        })
        .ok_or_else(|| {
            format!(
                "symbol {} in member {:?} was not found",
                input.symbol, input.member
            )
        })?;
    trace_binary_symbol(symbol, svd, &BTreeMap::new(), &BTreeMap::new(), None)
}

pub(crate) fn extract_reference(
    input: &ArtifactSymbolSelector,
    companions: &[PathBuf],
    svd: &MmioRegisterMap,
) -> Result<FunctionAnalysis> {
    ReferenceResolver::load(&input.artifact, companions)?.trace(
        input.member.as_deref(),
        &input.symbol,
        svd,
    )
}

pub(crate) fn print_trace(trace: &FunctionAnalysis) {
    println!("TRACE\t{}\texact={}", trace.symbol, trace.is_exact());
    for (index, event) in trace.events.iter().enumerate() {
        println!("{index}\t{}", event.canonical());
    }
    for blocker in &trace.blockers {
        println!("BLOCKER\t{blocker}");
    }
}

pub(crate) fn returns_equal(left: &FunctionAnalysis, right: &FunctionAnalysis) -> bool {
    left.return_value.is_resolved()
        && right.return_value.is_resolved()
        && left.return_value.canonical() == right.return_value.canonical()
}

pub(crate) fn traces_equal(left: &FunctionAnalysis, right: &FunctionAnalysis) -> bool {
    left.events.len() == right.events.len()
        && left
            .events
            .iter()
            .zip(&right.events)
            .all(|(left, right)| left.equivalent(right))
}

pub(crate) fn print_uncovered(symbol: &str, side: &str, trace: &FunctionAnalysis) -> usize {
    let mut count = 0;
    for blocker in &trace.blockers {
        println!("UNCOVERED\t{symbol}\t{side}\t{blocker}");
        count += 1;
    }
    for address in trace
        .events
        .iter()
        .filter_map(ObservableEvent::unmapped_address)
    {
        println!(
            "UNCOVERED\t{symbol}\t{side}\tunmapped-register {:#010x}",
            address
        );
        count += 1;
    }
    count
}
