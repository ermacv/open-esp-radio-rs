//! Reports of comparison cases that miss their expected verdict.
//!
//! A failing request writes, per departing case, both sides' ordered events
//! with the contract classification of every concrete effect and the first
//! difference, as a readable report and as JSON beside it. The error names
//! the report, so a scenario never needs a temporary print to see its
//! events. When a side stopped at undeclared memory, the report also lists
//! every undeclared access the request reaches.
use crate::harness::Result;
use blobray_domain::{
    ComparisonVerdict, EffectContract, EffectSelection, EffectTracker, ExecutionEvent,
    ExecutionEvidence, ExecutionRequest, Result as DomainResult, RunControl,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Directory below the run directory that holds the reports.
pub const DIRECTORY: &str = "failures";

/// One event of one side, with the contract classification of a concrete
/// effect.
#[derive(Debug, Serialize)]
pub struct Event {
    pub index: usize,
    pub event: String,
    /// The rule that selected the effect, `unlisted` for an effect no rule
    /// selects, or none for an event that is not a concrete effect.
    pub class: Option<String>,
}

/// One departing case.
#[derive(Debug, Serialize)]
pub struct Case {
    pub case: u32,
    pub name: String,
    pub verdict: String,
    pub difference: String,
    pub vendor: Side,
    pub replacement: Side,
}

#[derive(Debug, Serialize)]
pub struct Side {
    pub stop: String,
    pub events: Vec<Event>,
}

/// Classification without a resource budget: the report is written after
/// the run.
struct Unmetered;

impl RunControl for Unmetered {
    fn checkpoint(&mut self, _units: u64) -> DomainResult<()> {
        Ok(())
    }
}

fn concrete(event: &ExecutionEvent) -> bool {
    matches!(
        event,
        ExecutionEvent::Read { .. }
            | ExecutionEvent::Write { .. }
            | ExecutionEvent::Fence { .. }
            | ExecutionEvent::DelayMicros { .. }
    )
}

/// The events of one side, each concrete effect classified by `contract`.
pub fn classify(
    events: &[ExecutionEvent],
    contract: Option<&EffectContract>,
    replacement: bool,
) -> Result<Vec<Event>> {
    let mut tracker = contract
        .map(|contract| EffectTracker::new(contract, replacement, &mut Unmetered))
        .transpose()?;
    let concrete_indices: Vec<usize> = (0..events.len())
        .filter(|index| concrete(&events[*index]))
        .collect();
    let mut rows = vec![];
    for (index, event) in events.iter().enumerate() {
        let class = match (&mut tracker, concrete(event)) {
            (_, false) => None,
            (None, true) => Some("unlisted".into()),
            (Some(tracker), true) => {
                let position = concrete_indices
                    .iter()
                    .position(|i| *i == index)
                    .expect("a concrete event has a position");
                let next = concrete_indices
                    .get(position + 1)
                    .map(|next| &events[*next]);
                let selection = tracker.observe(event, next, position as u32, &mut Unmetered)?;
                Some(selection_name(
                    selection,
                    contract.expect("a tracker has a contract"),
                ))
            }
        };
        rows.push(Event {
            index,
            event: format!("{event:x?}"),
            class,
        });
    }
    Ok(rows)
}

fn selection_name(selection: EffectSelection, contract: &EffectContract) -> String {
    let rule = |index: u16| {
        contract
            .rules
            .get(usize::from(index))
            .map_or_else(|| format!("rule {index}"), |rule| rule.name.clone())
    };
    match selection {
        EffectSelection::Required(i) => format!("required: {}", rule(i)),
        EffectSelection::Ignored(i) => format!("ignored: {}", rule(i)),
        EffectSelection::Omitted(i) => format!("omitted: {}", rule(i)),
        EffectSelection::Replaced(i) => format!("replaced: {}", rule(i)),
        EffectSelection::Added(i) => format!("added: {}", rule(i)),
        EffectSelection::Unlisted => "unlisted".into(),
        EffectSelection::Unclassified => "unclassified".into(),
        EffectSelection::Violation(violation) => format!("violation: {violation:?}"),
    }
}

/// The departing cases of `records` against `expected`.
pub fn cases(
    request: &ExecutionRequest,
    records: &[ExecutionEvidence],
    expected: Option<ComparisonVerdict>,
    contract: impl Fn(u32) -> Option<EffectContract>,
) -> Result<Vec<Case>> {
    let mut cases = vec![];
    for record in records {
        let ExecutionEvidence::Comparison { case, result } = record else {
            continue;
        };
        if Some(result.verdict) == expected {
            continue;
        }
        let contract = contract(*case);
        let side = |replacement: bool| -> Result<Side> {
            let events = crate::evidence::events(records, *case, replacement);
            Ok(Side {
                stop: format!("{:x?}", crate::evidence::stop(records, *case, replacement)),
                events: classify(&events, contract.as_ref(), replacement)?,
            })
        };
        cases.push(Case {
            case: *case,
            name: request
                .cases
                .get(*case as usize)
                .map_or_else(String::new, |c| c.name.clone()),
            verdict: format!("{:?}", result.verdict),
            difference: difference(request, *case, &result.difference),
            vendor: side(false)?,
            replacement: side(true)?,
        });
    }
    Ok(cases)
}

/// `difference`, naming the memory selection of a memory difference.
fn difference(
    request: &ExecutionRequest,
    case: u32,
    difference: &Option<blobray_domain::ComparisonDifference>,
) -> String {
    let named = match difference {
        Some(blobray_domain::ComparisonDifference::Memory { pair, offset, .. }) => request
            .cases
            .get(case as usize)
            .and_then(|case| {
                let relation = case.relation.as_ref()?;
                let pair = relation.memory.get(usize::from(*pair))?;
                case.vendor.observe_memory.get(usize::from(pair.vendor))
            })
            .map(|selection| {
                format!(
                    " in `{}` at {:#x}",
                    selection.name,
                    u64::from(selection.address) + u64::from(*offset)
                )
            }),
        _ => None,
    };
    format!("{difference:x?}{}", named.unwrap_or_default())
}

/// The readable form of `cases`.
pub fn render(label: &str, expected: Option<ComparisonVerdict>, cases: &[Case]) -> String {
    let mut text = format!("{label}: expected {expected:?}\n");
    for case in cases {
        text.push_str(&format!(
            "\ncase {} `{}`: {}; difference {}\n",
            case.case, case.name, case.verdict, case.difference
        ));
        for (title, side) in [("vendor", &case.vendor), ("production", &case.replacement)] {
            text.push_str(&format!("  {title}: stop {}\n", side.stop));
            for event in &side.events {
                let class = event
                    .class
                    .as_deref()
                    .map_or_else(String::new, |class| format!("  [{class}]"));
                text.push_str(&format!("    {:4} {}{class}\n", event.index, event.event));
            }
        }
    }
    text
}

/// The JSON form of a report.
#[derive(Serialize)]
struct Report<'a> {
    cases: &'a [Case],
    missing: &'a [crate::discovery::Missing],
}

/// Write the report of `cases` and the undeclared accesses `missing` below
/// `run`, as text and JSON; the text path.
pub fn write(
    run: &Path,
    label: &str,
    expected: Option<ComparisonVerdict>,
    cases: &[Case],
    missing: &[crate::discovery::Missing],
) -> Result<PathBuf> {
    let directory = run.join(DIRECTORY);
    std::fs::create_dir_all(&directory)?;
    let stem: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let text = directory.join(format!("{stem}.txt"));
    std::fs::write(
        &text,
        render(label, expected, cases) + &crate::discovery::render(missing),
    )?;
    std::fs::write(
        directory.join(format!("{stem}.json")),
        serde_json::to_vec_pretty(&Report { cases, missing })?,
    )?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blobray_domain::{
        ArtifactId, CallEndpoint, EffectClaimCeiling, EffectDisposition, EffectPattern, EffectRule,
        EffectSelector, EffectValue, FunctionSource, KnowledgeOccurrence, ObjectId, ObjectLocation,
        ReviewedCallBoundary, SymbolId, SymbolTableKind, UnclassifiedEffects,
    };

    fn read(address: u32) -> ExecutionEvent {
        ExecutionEvent::Read {
            address,
            width: 4,
            value: 0,
        }
    }

    fn contract() -> EffectContract {
        let pattern = EffectPattern {
            selector: EffectSelector::MmioRead {
                address: 0x2010_4080,
                width: 4,
            },
            value: EffectValue::Any,
            preceded_by: None,
            occurrence: None,
            followed_by: None,
        };
        let id = ArtifactId::of_bytes(b"failure fixture");
        let object = ObjectId {
            artifact: id.clone(),
            location: ObjectLocation::Standalone,
        };
        let endpoint = CallEndpoint {
            occurrence: KnowledgeOccurrence {
                revision: id.as_str().parse().unwrap(),
                source: FunctionSource::Input { input: 0 },
                object: object.clone(),
                symbol: Some(SymbolId {
                    object,
                    table: SymbolTableKind::Static,
                    table_section: 3,
                    index: 1,
                }),
            },
            boundary: ReviewedCallBoundary::Code { address: 0x1000 },
        };
        EffectContract {
            vendor: endpoint.clone(),
            replacement: endpoint,
            rules: vec![EffectRule {
                name: "settled-check".into(),
                vendor: None,
                replacement: Some(pattern),
                disposition: EffectDisposition::Added,
                min_occurrences: 1,
                max_occurrences: 1,
                reason: "fixture".into(),
            }],
            unclassified: UnclassifiedEffects::Required,
            claim_ceiling: EffectClaimCeiling::ReviewedEffectRefinement,
            applicability: "fixture".into(),
            reason: "fixture".into(),
        }
    }

    #[test]
    fn every_concrete_effect_carries_its_rule_or_unlisted() {
        let contract = contract();
        let events = [
            read(0x2010_4080),
            ExecutionEvent::CallReturn {
                words: [Some(0), None],
            },
            read(0x2010_4088),
        ];
        let rows = classify(&events, Some(&contract), true).unwrap();
        assert_eq!(rows[0].class.as_deref(), Some("added: settled-check"));
        assert_eq!(rows[1].class, None);
        assert_eq!(rows[2].class.as_deref(), Some("unlisted"));
        // Without a contract every concrete effect compares exactly.
        let rows = classify(&events, None, true).unwrap();
        assert_eq!(rows[0].class.as_deref(), Some("unlisted"));
    }

    #[test]
    fn the_report_is_written_as_text_and_json() {
        let directory = tempfile::tempdir().unwrap();
        let cases = [Case {
            case: 3,
            name: "rx-append-1-beyond-00".into(),
            verdict: "Diff".into(),
            difference: "Some(Event { index: 0 })".into(),
            vendor: Side {
                stop: "Returned".into(),
                events: vec![Event {
                    index: 0,
                    event: "Read".into(),
                    class: Some("unlisted".into()),
                }],
            },
            replacement: Side {
                stop: "Returned".into(),
                events: vec![],
            },
        }];
        let text = write(
            directory.path(),
            "rx append/1",
            Some(ComparisonVerdict::Match),
            &cases,
            &[],
        )
        .unwrap();
        assert!(text.ends_with("failures/rx_append_1.txt"));
        let body = std::fs::read_to_string(&text).unwrap();
        assert!(body.contains("case 3 `rx-append-1-beyond-00`: Diff"));
        assert!(body.contains("[unlisted]"));
        assert!(text.with_extension("json").is_file());
    }
}
