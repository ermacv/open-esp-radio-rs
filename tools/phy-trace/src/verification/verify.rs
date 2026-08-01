//! Inventory matching, verification gates and probe accounting.

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
};

use crate::qualification::{
    qualify_esp32s31_bluetooth_tx_power, qualify_esp32s31_bluetooth_txdc,
    qualify_esp32s31_bluetooth_txdc_pwdet, qualify_esp32s31_channel, qualify_esp32s31_rf_init,
};
use crate::*;

#[derive(Clone, Copy)]
pub(crate) struct VerifySource<'a> {
    pub(crate) name: &'a str,
    pub(crate) artifact: &'a Path,
    pub(crate) inventory: Option<&'a Path>,
    pub(crate) companion: Option<&'a Path>,
    pub(crate) prefix: &'a str,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct VerifySummary {
    pub(crate) vendor_functions: usize,
    pub(crate) matched: usize,
    pub(crate) symbolic_matches: usize,
    pub(crate) scenario_matches: usize,
    pub(crate) state_matches: usize,
    pub(crate) composition_matches: usize,
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
            ("completion", Some(_)) => Err("--match-floor requires --gate regression".into()),
            ("regression", Some(match_floor)) => Ok(Self::Regression { match_floor }),
            ("regression", None) => Err("--gate regression requires --match-floor".into()),
            _ => Err(format!("unsupported verification gate {name:?}").into()),
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

    pub(crate) fn report(self, passed: bool) {
        let result = if passed { "PASS" } else { "FAIL" };
        match self {
            Self::Completion => println!("GATE\tcompletion\t{result}"),
            Self::Regression { match_floor } => {
                println!("GATE\tregression\t{result}\tmatch-floor={match_floor}");
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
        self.scenario_matches += other.scenario_matches;
        self.state_matches += other.state_matches;
        self.composition_matches += other.composition_matches;
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

pub(crate) fn print_protocol_inventory(
    manifest: &dispositions::Manifest,
    sources: &[(&str, &[ArtifactSymbolIdentity])],
) {
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
    println!(
        "PROTOCOL-INVENTORY\tshared={shared}\twifi={wifi}\tbluetooth={bluetooth}\tble={ble}\tcoex={coex}\tieee802154={ieee802154}\tunknown={unknown}\texact-dispositions={}",
        manifest.entries().count()
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "source verification keeps all artifact and policy inputs explicit"
)]
pub(crate) fn verify_source(
    svd: &MmioRegisterMap,
    source: VerifySource<'_>,
    rust_artifact: &Path,
    rust_companion: Option<&Path>,
    rust_prefix: &str,
    execution_profiles: &[profiles::Profile],
    disposition_manifest: Option<&dispositions::Manifest>,
    evidence: &mut EvidenceSet,
) -> Result<VerifySummary> {
    let vendor_digest = pinned_vendor_digest(source.artifact)?;
    println!(
        "ORACLE\t{}\t{}\tsha256={vendor_digest}",
        source.name,
        source.artifact.display()
    );
    if let Some(inventory) = source.inventory.filter(|path| *path != source.artifact) {
        let inventory_digest = pinned_vendor_digest(inventory)?;
        println!(
            "ORACLE\t{}-inventory\t{}\tsha256={inventory_digest}",
            source.name,
            inventory.display()
        );
    }
    if let Some(companion) = source.companion {
        let companion_digest = pinned_vendor_digest(companion)?;
        println!(
            "ORACLE\t{}-companion\t{}\tsha256={companion_digest}",
            source.name,
            companion.display()
        );
    }
    let vendor_symbols = vendor_symbols(source)?;
    let rust_symbols = list_code_symbols(rust_artifact, rust_prefix)?;
    let mut profiled_vendor_symbols = BTreeSet::new();
    for profile in execution_profiles {
        if profile.vendor_source != source.name && source.name != "vendor" {
            return Err(format!(
                "profile {} targets {}, but was routed to {}",
                profile.name, profile.vendor_source, source.name
            )
            .into());
        }
        if !profiled_vendor_symbols.insert(profile.vendor_symbol.as_str()) {
            return Err(format!(
                "multiple execution profiles target {} in {}",
                profile.vendor_symbol, source.name
            )
            .into());
        }
        if !vendor_symbols
            .iter()
            .any(|symbol| symbol.name == profile.vendor_symbol)
        {
            return Err(format!(
                "profile {} refers to missing {} vendor symbol {}",
                profile.name, source.name, profile.vendor_symbol
            )
            .into());
        }
        if !rust_symbols
            .iter()
            .any(|symbol| symbol.name == profile.rust_symbol)
        {
            return Err(format!(
                "profile {} refers to missing Rust symbol {}",
                profile.name, profile.rust_symbol
            )
            .into());
        }
    }
    let mut rust_by_suffix = HashMap::new();
    for symbol in &rust_symbols {
        let Some(suffix) = symbol.name.strip_prefix(rust_prefix) else {
            continue;
        };
        let (suffix, compare_return) = suffix
            .strip_prefix("ret_")
            .map_or((suffix, false), |suffix| (suffix, true));
        if let Some((previous, _)) = rust_by_suffix.insert(suffix, (symbol, compare_return)) {
            return Err(format!(
                "Rust probe suffix {suffix:?} is ambiguous between {} and {}",
                previous.name, symbol.name
            )
            .into());
        }
    }

    let mut summary = VerifySummary {
        vendor_functions: vendor_symbols.len(),
        ..VerifySummary::default()
    };
    for vendor in &vendor_symbols {
        let suffix = vendor
            .name
            .strip_prefix(source.prefix)
            .expect("symbol was filtered by vendor prefix");
        let source_qualified_suffix = format!("{}_{suffix}", source.name);
        let Some((rust, compare_return)) = rust_by_suffix
            .get(source_qualified_suffix.as_str())
            .or_else(|| rust_by_suffix.get(suffix))
        else {
            if let Some(manifest) = disposition_manifest {
                let resolved = manifest.resolve(source.name, &vendor.name);
                if resolved.disposition.is_implemented() {
                    let entry = resolved
                        .entry
                        .expect("implemented disposition must be an exact function entry");
                    if let Some(contract) = entry.semantic_contract {
                        let matched = match contract {
                            dispositions::SemanticContract::Esp32s31Channel => {
                                if source.name != "archive" || vendor.name != "phy_chip_set_chan" {
                                    return Err(format!(
                                        "semantic contract {} cannot qualify {} {}",
                                        contract.label(),
                                        source.name,
                                        vendor.name,
                                    )
                                    .into());
                                }
                                let companion = source.companion.ok_or_else(|| {
                                    format!(
                                        "semantic contract {} requires an archive companion",
                                        contract.label()
                                    )
                                })?;
                                qualify_esp32s31_channel(svd, source.artifact, companion, false)?
                            }
                            dispositions::SemanticContract::Esp32s31RfInit => {
                                if source.name != "archive" || vendor.name != "phy_rf_init" {
                                    return Err(format!(
                                        "semantic contract {} cannot qualify {} {}",
                                        contract.label(),
                                        source.name,
                                        vendor.name,
                                    )
                                    .into());
                                }
                                let companion = source.companion.ok_or_else(|| {
                                    format!(
                                        "semantic contract {} requires an archive companion",
                                        contract.label()
                                    )
                                })?;
                                qualify_esp32s31_rf_init(svd, source.artifact, companion, false)?
                            }
                            dispositions::SemanticContract::Esp32s31BluetoothTxDc => {
                                if source.name != "archive" || vendor.name != "phy_bt_txdc_cal_new"
                                {
                                    return Err(format!(
                                        "semantic contract {} cannot qualify {} {}",
                                        contract.label(),
                                        source.name,
                                        vendor.name,
                                    )
                                    .into());
                                }
                                let companion = source.companion.ok_or_else(|| {
                                    format!(
                                        "semantic contract {} requires an archive companion",
                                        contract.label()
                                    )
                                })?;
                                qualify_esp32s31_bluetooth_txdc(
                                    svd,
                                    source.artifact,
                                    companion,
                                    false,
                                )?
                            }
                            dispositions::SemanticContract::Esp32s31BluetoothTxDcPwdet => {
                                if source.name != "archive"
                                    || vendor.name != "phy_txdc_cal_pwdet_init"
                                {
                                    return Err(format!(
                                        "semantic contract {} cannot qualify {} {}",
                                        contract.label(),
                                        source.name,
                                        vendor.name,
                                    )
                                    .into());
                                }
                                let companion = source.companion.ok_or_else(|| {
                                    format!(
                                        "semantic contract {} requires an archive companion",
                                        contract.label()
                                    )
                                })?;
                                qualify_esp32s31_bluetooth_txdc_pwdet(
                                    svd,
                                    source.artifact,
                                    companion,
                                    false,
                                )?
                            }
                            dispositions::SemanticContract::Esp32s31BluetoothTxPower => {
                                if source.name != "archive"
                                    || vendor.name != "phy_bt_tx_pwctrl_init"
                                {
                                    return Err(format!(
                                        "semantic contract {} cannot qualify {} {}",
                                        contract.label(),
                                        source.name,
                                        vendor.name,
                                    )
                                    .into());
                                }
                                let companion = source.companion.ok_or_else(|| {
                                    format!(
                                        "semantic contract {} requires an archive companion",
                                        contract.label()
                                    )
                                })?;
                                qualify_esp32s31_bluetooth_tx_power(
                                    svd,
                                    source.artifact,
                                    companion,
                                    false,
                                )?
                            }
                        };
                        if matched {
                            summary.matched += 1;
                            summary.composition_matches += 1;
                            record_evidence(
                                evidence,
                                source.name,
                                &vendor.name,
                                semantic_contract_evidence(contract.label()),
                            )?;
                            println!(
                                "FUNCTION\t{}\t{}\tMATCH\trust-component={}\tevidence=composition-state-scenario\tcontract={}\thil-evidence={}",
                                source.name,
                                vendor.name,
                                entry
                                    .rust_component
                                    .as_deref()
                                    .expect("implemented entry has a Rust component"),
                                contract.label(),
                                entry.hil_evidence.as_deref().unwrap_or("none"),
                            );
                        } else {
                            summary.mismatched += 1;
                            println!(
                                "FUNCTION\t{}\t{}\tMISMATCH\trust-component={}\tevidence=composition-state-scenario\tcontract={}",
                                source.name,
                                vendor.name,
                                entry
                                    .rust_component
                                    .as_deref()
                                    .expect("implemented entry has a Rust component"),
                                contract.label(),
                            );
                        }
                    } else {
                        summary.missing += 1;
                        summary.implemented_unqualified += 1;
                        let qualification_blockers = entry
                            .qualification_blockers
                            .iter()
                            .map(|(source, symbol)| format!("{source}:{symbol}"))
                            .collect::<Vec<_>>()
                            .join(",");
                        println!(
                            "FUNCTION\t{}\t{}\tIMPLEMENTED-UNQUALIFIED\tdisposition={}\tprotocol={}\trust-component={}\thil-evidence={}\tqualification-blockers={}\tmissing-semantic-contract",
                            source.name,
                            vendor.name,
                            resolved.disposition.label(),
                            resolved.protocol.label(),
                            entry
                                .rust_component
                                .as_deref()
                                .expect("implemented entry has a Rust component"),
                            entry.hil_evidence.as_deref().unwrap_or("none"),
                            if qualification_blockers.is_empty() {
                                "none"
                            } else {
                                &qualification_blockers
                            },
                        );
                    }
                } else {
                    summary.missing += 1;
                    summary.not_yet_ported += 1;
                    println!(
                        "FUNCTION\t{}\t{}\tUNCOVERED\tdisposition={}\tprotocol={}\tmissing-rust-probe {}{suffix} or {}{source_qualified_suffix}",
                        source.name,
                        vendor.name,
                        resolved.disposition.label(),
                        resolved.protocol.label(),
                        rust_prefix,
                        rust_prefix,
                    );
                }
            } else {
                summary.missing += 1;
                println!(
                    "FUNCTION\t{}\t{}\tUNCOVERED\tmissing-rust-probe {}{suffix} or {}{source_qualified_suffix}",
                    source.name, vendor.name, rust_prefix, rust_prefix
                );
            }
            continue;
        };
        let vendor_trace = extract(
            &ArtifactSymbolSelector {
                artifact: source.artifact.to_path_buf(),
                member: source
                    .inventory
                    .map_or_else(|| vendor.member.clone(), |_| None),
                symbol: vendor.name.clone(),
            },
            svd,
        )?;
        let rust_trace = extract(
            &ArtifactSymbolSelector {
                artifact: rust_artifact.to_path_buf(),
                member: rust.member.clone(),
                symbol: rust.name.clone(),
            },
            svd,
        )?;
        if let Some(profile) = execution_profiles
            .iter()
            .find(|profile| profile.vendor_symbol == vendor.name)
        {
            println!("PROFILE\t{}\t{}\tBEGIN", source.name, profile.name);
            let verdict = compare_execution_scenarios(
                svd,
                ExecutionInput {
                    artifact: source.artifact,
                    companion: source.companion,
                    symbol: &profile.vendor_symbol,
                },
                ExecutionInput {
                    artifact: rust_artifact,
                    companion: rust_companion,
                    symbol: &profile.rust_symbol,
                },
                profile.compare_return,
                &profile.scenarios,
            )?;
            match verdict {
                ComparisonVerdict::Match => {
                    summary.matched += 1;
                    match profile.contract {
                        profiles::ProfileContract::Scenario => summary.scenario_matches += 1,
                        profiles::ProfileContract::State => summary.state_matches += 1,
                    }
                    record_evidence(
                        evidence,
                        source.name,
                        &vendor.name,
                        profile_evidence(profile),
                    )?;
                }
                ComparisonVerdict::Mismatch => summary.mismatched += 1,
                ComparisonVerdict::Incomplete => summary.incomplete += 1,
            }
            println!(
                "FUNCTION\t{}\t{}\t{}\trust={}\tevidence={}\tbranch-outcomes=complete\tprofile={}",
                source.name,
                vendor.name,
                verdict.label(),
                rust.name,
                profile.contract.evidence(),
                profile.name
            );
            continue;
        }
        if !vendor_trace.is_exact()
            || !rust_trace.is_exact()
            || (*compare_return
                && (!vendor_trace.return_value.is_resolved()
                    || !rust_trace.return_value.is_resolved()))
        {
            summary.incomplete += 1;
            let mut uncovered = print_uncovered(&vendor.name, source.name, &vendor_trace)
                + print_uncovered(&vendor.name, "rust", &rust_trace);
            if *compare_return && !vendor_trace.return_value.is_resolved() {
                println!(
                    "UNCOVERED\t{}\t{}\tvendor\tunresolved-return",
                    source.name, vendor.name
                );
                uncovered += 1;
            }
            if *compare_return && !rust_trace.return_value.is_resolved() {
                println!(
                    "UNCOVERED\t{}\t{}\trust\tunresolved-return",
                    source.name, vendor.name
                );
                uncovered += 1;
            }
            println!(
                "FUNCTION\t{}\t{}\tINCOMPLETE\trust={}\tuncovered={uncovered}",
                source.name, vendor.name, rust.name
            );
        } else if traces_equal(&vendor_trace, &rust_trace)
            && (!*compare_return || returns_equal(&vendor_trace, &rust_trace))
        {
            summary.matched += 1;
            summary.symbolic_matches += 1;
            record_evidence(evidence, source.name, &vendor.name, "symbolic")?;
            println!(
                "FUNCTION\t{}\t{}\tMATCH\trust={}\tevidence=symbolic\tevents={}\treturn={}",
                source.name,
                vendor.name,
                rust.name,
                vendor_trace.events.len(),
                if *compare_return { "checked" } else { "void" }
            );
        } else {
            summary.mismatched += 1;
            println!(
                "FUNCTION\t{}\t{}\tMISMATCH\trust={}\tvendor-events={}\trust-events={}",
                source.name,
                vendor.name,
                rust.name,
                vendor_trace.events.len(),
                rust_trace.events.len()
            );
        }
    }
    println!(
        "SOURCE-SUMMARY\t{}\tvendor-functions={}\tmatch={}\tsymbolic-match={}\tscenario-match={}\tstate-match={}\tcomposition-match={}\tmismatch={}\tincomplete={}\tmissing-rust-probe={}\timplemented-unqualified={}\tnot-yet-ported={}",
        source.name,
        summary.vendor_functions,
        summary.matched,
        summary.symbolic_matches,
        summary.scenario_matches,
        summary.state_matches,
        summary.composition_matches,
        summary.mismatched,
        summary.incomplete,
        summary.missing,
        summary.implemented_unqualified,
        summary.not_yet_ported,
    );
    Ok(summary)
}

pub(crate) fn orphan_probe_count(
    rust_artifact: &Path,
    rust_prefix: &str,
    sources: &[(VerifySource<'_>, &[ArtifactSymbolIdentity])],
) -> Result<usize> {
    let rust_symbols = list_code_symbols(rust_artifact, rust_prefix)?;
    Ok(rust_symbols
        .iter()
        .filter(|rust| {
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
