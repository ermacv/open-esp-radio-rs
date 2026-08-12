//! Verification inputs, aggregate gates, protocol inventory, and probe accounting.

use std::{collections::BTreeSet, path::Path};

use crate::*;

use super::super::ProtocolInventoryReport;

#[derive(Clone, Copy)]
pub(crate) struct VerifySource<'a> {
    pub(crate) name: &'a str,
    pub(crate) artifact: &'a Path,
    pub(crate) inventory: Option<&'a Path>,
    pub(crate) companion: Option<&'a Path>,
    pub(crate) prefix: &'a str,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(crate) struct VerifySummary {
    pub(crate) vendor_functions: usize,
    pub(crate) matched: usize,
    pub(crate) symbolic_matches: usize,
    pub(crate) effect_contract_matches: usize,
    pub(crate) scenario_matches: usize,
    pub(crate) state_matches: usize,
    pub(crate) composition_matches: usize,
    pub(crate) bounded_matches: usize,
    pub(crate) mismatched: usize,
    pub(crate) incomplete: usize,
    pub(crate) missing: usize,
    pub(crate) implemented_unqualified: usize,
    pub(crate) not_yet_ported: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VerificationGate {
    Completion,
    Regression { match_floor: usize },
}

impl VerificationGate {
    pub(crate) fn parse(name: &str, match_floor: Option<usize>) -> Result<Self> {
        match (name, match_floor) {
            ("completion", None) => Ok(Self::Completion),
            ("completion", Some(_)) => Err(crate::Error::invalid(
                "--match-floor requires --gate regression",
            )),
            ("regression", Some(match_floor)) => Ok(Self::Regression { match_floor }),
            ("regression", None) => Err(crate::Error::invalid(
                "--gate regression requires --match-floor",
            )),
            _ => Err(crate::Error::invalid(format!(
                "unsupported verification gate {name:?}"
            ))),
        }
    }

    pub(crate) const fn passes(self, summary: VerifySummary, orphan_probes: usize) -> bool {
        match self {
            Self::Completion => summary.is_complete() && orphan_probes == 0,
            Self::Regression { match_floor } => {
                summary.mismatched == 0
                    && summary.incomplete == 0
                    && summary.matched >= match_floor
                    && orphan_probes == 0
            }
        }
    }
}

impl VerifySummary {
    const fn is_complete(self) -> bool {
        self.mismatched == 0 && self.incomplete == 0 && self.missing == 0
    }

    pub(crate) fn add(&mut self, other: Self) {
        self.vendor_functions += other.vendor_functions;
        self.matched += other.matched;
        self.symbolic_matches += other.symbolic_matches;
        self.effect_contract_matches += other.effect_contract_matches;
        self.scenario_matches += other.scenario_matches;
        self.state_matches += other.state_matches;
        self.composition_matches += other.composition_matches;
        self.bounded_matches += other.bounded_matches;
        self.mismatched += other.mismatched;
        self.incomplete += other.incomplete;
        self.missing += other.missing;
        self.implemented_unqualified += other.implemented_unqualified;
        self.not_yet_ported += other.not_yet_ported;
    }
}

pub(crate) fn vendor_symbols(source: VerifySource<'_>) -> Result<Vec<ArtifactSymbolIdentity>> {
    list_code_symbols(source.inventory.unwrap_or(source.artifact), source.prefix)
}

pub(crate) fn protocol_inventory(
    manifest: &dispositions::Manifest,
    sources: &[(&str, &[ArtifactSymbolIdentity])],
) -> ProtocolInventoryReport {
    let mut shared = 0;
    let mut wifi = 0;
    let mut bluetooth = 0;
    let mut ble = 0;
    let mut coex = 0;
    let mut ieee802154 = 0;
    let mut unknown = 0;
    for (source, symbols) in sources {
        for symbol in *symbols {
            match manifest.resolve(source, &symbol.name).protocol {
                dispositions::Protocol::Shared => shared += 1,
                dispositions::Protocol::Wifi => wifi += 1,
                dispositions::Protocol::Bluetooth => bluetooth += 1,
                dispositions::Protocol::Ble => ble += 1,
                dispositions::Protocol::Coex => coex += 1,
                dispositions::Protocol::Ieee802154 => ieee802154 += 1,
                dispositions::Protocol::Unknown => unknown += 1,
            }
        }
    }
    ProtocolInventoryReport {
        shared,
        wifi,
        bluetooth,
        ble,
        coex,
        ieee802154,
        unknown,
        exact_dispositions: manifest.entries().count(),
        executable_bindings: manifest
            .entries()
            .filter(|entry| entry.binding.is_some())
            .count(),
    }
}

pub(crate) fn orphan_probe_count(
    rust_artifact: &Path,
    rust_prefix: &str,
    sources: &[(VerifySource<'_>, &[ArtifactSymbolIdentity])],
    explicitly_bound_probes: &BTreeSet<String>,
) -> Result<usize> {
    let rust_symbols = list_code_symbols(rust_artifact, rust_prefix)?;
    Ok(rust_symbols
        .iter()
        .filter(|rust| {
            if explicitly_bound_probes.contains(&rust.name) {
                return false;
            }
            let suffix = rust
                .name
                .strip_prefix(rust_prefix)
                .expect("symbol was filtered by Rust prefix");
            let suffix = suffix.strip_prefix("ret_").unwrap_or(suffix);
            !sources.iter().any(|(source, symbols)| {
                symbols.iter().any(|vendor| {
                    vendor
                        .name
                        .strip_prefix(source.prefix)
                        .is_some_and(|vendor_suffix| {
                            rust_probe_suffix_matches(source.name, vendor_suffix, suffix)
                        })
                })
            })
        })
        .count())
}

pub(crate) fn rust_probe_suffix_matches(
    source: &str,
    vendor_suffix: &str,
    rust_suffix: &str,
) -> bool {
    rust_suffix == vendor_suffix
        || rust_suffix
            .strip_prefix(source)
            .and_then(|suffix| suffix.strip_prefix('_'))
            == Some(vendor_suffix)
}
