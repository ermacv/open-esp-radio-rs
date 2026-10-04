//! Reviewed summaries of companion functions, such as the chip's ROM, that
//! the machine-code analysis cannot bound alone: each one's frame and the
//! functions it calls, checked against the companion where the analysis can.
use crate::{Analysis, FrameSource, Transfer, TransferKind};
use oer_riscv_model::{Error, ErrorCode, Result};
use serde::Deserialize;
use std::collections::BTreeSet;

/// One reviewed function.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub name: String,
    /// The function's start, which must carry `name` in its ELF.
    pub address: u32,
    /// Bytes below the entry `sp`.
    pub frame: u64,
    /// The functions it calls or jumps to, by name.
    #[serde(default)]
    pub calls: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Summaries {
    function: Vec<Summary>,
}

/// The summaries of a `[[function]]` TOML document.
pub fn parse(text: &str) -> Result<Vec<Summary>> {
    toml::from_str::<Summaries>(text)
        .map(|summaries| summaries.function)
        .map_err(|error| Error::new(ErrorCode::InvalidRequest, format!("summaries: {error}")))
}

fn integrity(message: String) -> Error {
    Error::new(ErrorCode::Integrity, message)
}

/// Apply `summaries` to `analysis`, whose functions at `image` are the
/// image's own. Every summary must name its function's address. A summary of
/// a function the image reaches applies: where the analysis bounds the
/// function itself, the two must agree, and otherwise the summary stands for
/// it. The names of the summaries that applied are returned: a summary no
/// image of the platform uses is stale, which the platform's audit checks
/// over all its images.
pub(crate) fn apply(
    analysis: &mut Analysis,
    image: &BTreeSet<u32>,
    summaries: &[Summary],
) -> Result<BTreeSet<String>> {
    let address_of = |analysis: &Analysis, name: &str| {
        analysis
            .functions
            .values()
            .find(|facts| facts.function.names.iter().any(|n| n == name))
            .map(|facts| facts.function.address)
    };
    for summary in summaries {
        let facts = analysis.functions.get(&summary.address).ok_or_else(|| {
            integrity(format!(
                "summary of {} at {:#010x}: no function starts there",
                summary.name, summary.address
            ))
        })?;
        if !facts.function.names.contains(&summary.name) {
            return Err(integrity(format!(
                "summary of {} at {:#010x}: the function there is {}",
                summary.name,
                summary.address,
                facts.function.label()
            )));
        }
    }
    // Every function the image's own functions reach, as the machine code
    // and the summaries that apply say.
    let mut reached = BTreeSet::new();
    let mut used = BTreeSet::new();
    let mut pending: Vec<u32> = image.iter().copied().collect();
    while let Some(address) = pending.pop() {
        if let Some(summary) = summaries.iter().find(|s| s.address == address)
            && used.insert(summary.name.clone())
        {
            let mut calls = BTreeSet::new();
            for name in &summary.calls {
                calls.insert(address_of(analysis, name).ok_or_else(|| {
                    integrity(format!("summary of {}: no function {name}", summary.name))
                })?);
            }
            let facts = analysis.functions.get_mut(&address).expect("checked above");
            let bounded =
                facts.frame.is_some() && facts.transfers.iter().all(|t| t.target.is_some());
            if bounded {
                let targets: BTreeSet<u32> =
                    facts.transfers.iter().filter_map(|t| t.target).collect();
                if facts.frame != Some(summary.frame) || targets != calls {
                    return Err(integrity(format!(
                        "summary of {} disagrees with its machine code: frame {:?} and calls \
                         {targets:x?}, summarized as {} and {calls:x?}",
                        summary.name, facts.frame, summary.frame
                    )));
                }
            } else {
                facts.frame = Some(summary.frame);
                facts.source = Some(FrameSource::Summary);
                facts.transfers = calls
                    .into_iter()
                    .map(|target| Transfer {
                        site: summary.address,
                        target: Some(target),
                        kind: TransferKind::Call,
                        source: None,
                        table: None,
                        load: None,
                    })
                    .collect();
            }
        }
        let Some(facts) = analysis.functions.get(&address) else {
            continue;
        };
        for target in facts.transfers.iter().filter_map(|t| t.target) {
            if !image.contains(&target) && reached.insert(target) {
                pending.push(target);
            }
        }
    }
    Ok(used)
}
