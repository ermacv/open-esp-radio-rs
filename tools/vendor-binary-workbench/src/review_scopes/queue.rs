//! Stable root-cause grouping and actionable review ordering.

use std::collections::{BTreeMap, BTreeSet};

use super::model::ReviewQueueItem;

#[derive(Debug)]
pub(super) struct Accumulator {
    kind: String,
    priority: u8,
    severity: &'static str,
    occurrences: usize,
    functions: BTreeSet<String>,
    affected_scope_roots: BTreeSet<String>,
    sites: BTreeSet<u32>,
    channels: BTreeSet<String>,
    message: String,
}

pub(super) type Queue = BTreeMap<String, Accumulator>;

pub(super) fn new() -> Queue {
    BTreeMap::new()
}

fn priority(kind: &str) -> (u8, &'static str) {
    match kind {
        "replacement-mismatch" | "replacement-incomplete" => (0, "error"),
        "replacement-unqualified" | "replacement-implemented-unqualified" => (1, "error"),
        "unresolved-call" => (10, "error"),
        "decode" => (20, "error"),
        "indirect-control-flow" | "call-shape" | "call-result-model" => (30, "warning"),
        "control-flow" | "call-boundary" => (40, "warning"),
        "memory-load" | "memory-store" | "memory-intrinsic" => (50, "warning"),
        "analysis-budget" => (60, "warning"),
        "replacement-uncovered" | "replacement-unmapped" => (70, "warning"),
        "replacement-probe-only" => (80, "info"),
        "replacement-bounded" => (85, "info"),
        _ => (65, "warning"),
    }
}

pub(super) fn insert(
    queue: &mut Queue,
    id: String,
    kind: &str,
    function: &str,
    site: Option<u32>,
    channel: &str,
    message: String,
) {
    let (priority, severity) = priority(kind);
    let item = queue.entry(id).or_insert_with(|| Accumulator {
        kind: kind.to_owned(),
        priority,
        severity,
        occurrences: 0,
        functions: BTreeSet::new(),
        affected_scope_roots: BTreeSet::new(),
        sites: BTreeSet::new(),
        channels: BTreeSet::new(),
        message,
    });
    item.occurrences += 1;
    item.functions.insert(function.to_owned());
    item.sites.extend(site);
    item.channels.insert(channel.to_owned());
}

/// Restore one already-grouped structural item before dynamic assurance
/// entries are joined.  Replaying the public fields through `insert` would
/// manufacture a functions × sites cross-product and change occurrence
/// counts.
pub(super) fn insert_existing(queue: &mut Queue, item: ReviewQueueItem) {
    queue.insert(
        item.id,
        Accumulator {
            kind: item.kind,
            priority: item.priority,
            severity: match item.severity.as_str() {
                "error" => "error",
                "warning" => "warning",
                "info" => "info",
                _ => "warning",
            },
            occurrences: item.occurrences,
            functions: item.functions.into_iter().collect(),
            affected_scope_roots: item.affected_scope_roots.into_iter().collect(),
            sites: item.sites.into_iter().collect(),
            channels: item.channels.into_iter().collect(),
            message: item.message,
        },
    );
}

pub(super) fn id(kind: &str, identity: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in kind
        .bytes()
        .chain(std::iter::once(0))
        .chain(identity.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("review-{hash:016x}")
}

pub(super) fn finish(queue: Queue) -> Vec<ReviewQueueItem> {
    let mut queue = queue
        .into_iter()
        .map(|(id, item)| {
            let potentially_unblocked_functions = item.functions.len();
            ReviewQueueItem {
                id,
                kind: item.kind,
                priority: item.priority,
                severity: item.severity.to_owned(),
                occurrences: item.occurrences,
                functions: item.functions.into_iter().collect(),
                affected_scope_roots: item.affected_scope_roots.into_iter().collect(),
                potentially_unblocked_functions,
                sites: item.sites.into_iter().collect(),
                channels: item.channels.into_iter().collect(),
                message: item.message,
            }
        })
        .collect::<Vec<_>>();
    queue.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.id.cmp(&right.id))
    });
    queue
}

pub(super) fn attach_scope_impact(queue: &mut Queue, root_paths: &BTreeMap<String, Vec<String>>) {
    for item in queue.values_mut() {
        for function in &item.functions {
            if let Some(root) = root_paths.get(function).and_then(|path| path.first()) {
                item.affected_scope_roots.insert(root.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_one_root_across_functions_sites_and_channels() {
        let mut queue = new();
        insert(
            &mut queue,
            "root-call-phy-printf".to_owned(),
            "unresolved-call",
            "libphy::first",
            Some(0x1000),
            "reference",
            "unresolved call to phy_printf".to_owned(),
        );
        insert(
            &mut queue,
            "root-call-phy-printf".to_owned(),
            "unresolved-call",
            "libphy::second",
            Some(0x2000),
            "call-graph",
            "unresolved call to phy_printf".to_owned(),
        );

        let item = finish(queue).pop().unwrap();
        assert_eq!(item.occurrences, 2);
        assert_eq!(item.functions.len(), 2);
        assert_eq!(item.sites, [0x1000, 0x2000]);
        assert_eq!(item.channels, ["call-graph", "reference"]);
    }

    #[test]
    fn failures_sort_before_unreviewed_coverage() {
        assert!(priority("replacement-mismatch").0 < priority("replacement-uncovered").0);
        assert!(priority("decode").0 < priority("memory-load").0);
    }
}
