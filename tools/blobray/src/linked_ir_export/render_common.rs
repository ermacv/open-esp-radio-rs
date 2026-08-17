//! Shared formatting and guard-link traversal for all IR report views.

use std::collections::BTreeSet;

use crate::{LinkedCallGuardMmioSource, LinkedCallGuardPath, LinkedDirectMmioPredicateSource};

pub(crate) fn format_site_path(site_path: &[Option<u32>]) -> String {
    site_path
        .iter()
        .map(|site| site.map_or_else(|| "unknown".to_owned(), |site| format!("{site:#010x}")))
        .collect::<Vec<_>>()
        .join(" -> ")
}

pub(crate) fn optional_hex_text(value: Option<u32>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| format!("{value:#010x}"))
}

pub(crate) type ProducerMmioGuardLink = (
    u32,
    String,
    &'static str,
    bool,
    String,
    &'static str,
    Option<u32>,
    Option<u32>,
    LinkedCallGuardMmioSource,
);

pub(crate) fn guard_mmio_links(paths: &[LinkedCallGuardPath]) -> Vec<ProducerMmioGuardLink> {
    paths
        .iter()
        .flat_map(|path| &path.guards)
        .flat_map(|guard| {
            guard.result_sources.iter().flat_map(|source| {
                source.mmio_sources.iter().cloned().map(|mmio| {
                    (
                        guard.site,
                        guard.condition.clone(),
                        guard.operation,
                        guard.taken,
                        source
                            .target
                            .clone()
                            .unwrap_or_else(|| "unknown".to_owned()),
                        source.operand,
                        source.comparison_value,
                        source.source_comparison_value,
                        mmio,
                    )
                })
            })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) type DirectMmioGuardLink = (
    u32,
    String,
    &'static str,
    bool,
    LinkedDirectMmioPredicateSource,
);

pub(crate) fn guard_direct_mmio_links(paths: &[LinkedCallGuardPath]) -> Vec<DirectMmioGuardLink> {
    paths
        .iter()
        .flat_map(|path| &path.guards)
        .flat_map(|guard| {
            guard.direct_mmio_sources.iter().cloned().map(|source| {
                (
                    guard.site,
                    guard.condition.clone(),
                    guard.operation,
                    guard.taken,
                    source,
                )
            })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
