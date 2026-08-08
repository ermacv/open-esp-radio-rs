//! Machine-readable implementation disposition parsing and inventory validation.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use crate::{ArtifactSymbolIdentity, Result};

use super::bindings::{BindingVersion, DriverAdapter};
use super::effect_contract::{EffectComparison, parse_effect_rule};
#[cfg(test)]
use super::effect_contract::{EffectDisposition, EffectSelector};

mod model;

pub use model::{Disposition, Entry, Manifest, Protocol, ResolvedDisposition, SemanticContract};
use model::{EntryBuilder, ProtocolPrefix};

fn directive_value(line: &str, line_number: usize) -> Result<(&str, &str)> {
    line.split_once(char::is_whitespace)
        .map(|(directive, value)| (directive, value.trim()))
        .filter(|(_, value)| !value.is_empty())
        .ok_or_else(|| format!("disposition directive needs a value at line {line_number}").into())
}

pub(crate) fn validate_source_id(value: &str, line: usize) -> Result<&str> {
    crate::source_id::validate_source_id(value)
        .map_err(|_| format!("invalid vendor source id {value:?} at line {line}").into())
}

impl Manifest {
    #[tracing::instrument(name = "load_disposition_manifest", fields(path = %path.display()))]
    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        Self::parse(&input).map_err(|error| {
            crate::error::WorkbenchError::manifest("disposition manifest", path, error)
        })
    }

    fn parse(input: &str) -> Result<Self> {
        let mut default_disposition = None;
        let mut default_protocol = None;
        let mut protocol_prefixes = Vec::new();
        let mut entries = BTreeMap::new();
        let mut current: Option<EntryBuilder> = None;

        let finish_entry = |builder: EntryBuilder,
                            entries: &mut BTreeMap<(String, String), Entry>|
         -> Result<()> {
            let entry = builder.finish()?;
            let key = (entry.source.clone(), entry.symbol.clone());
            if entries.insert(key, entry).is_some() {
                return Err("duplicate disposition function entry".into());
            }
            Ok(())
        };

        for (index, raw_line) in input.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let (directive, value) = directive_value(line, line_number)?;
            if directive == "function" {
                if let Some(builder) = current.take() {
                    finish_entry(builder, &mut entries)?;
                }
                let mut words = value.split_whitespace();
                let source = words.next().ok_or("function has no source")?;
                let symbol = words.next().ok_or("function has no symbol")?;
                if words.next().is_some() {
                    return Err(format!("function has extra fields at line {line_number}").into());
                }
                validate_source_id(source, line_number)?;
                current = Some(EntryBuilder {
                    source: source.to_owned(),
                    symbol: symbol.to_owned(),
                    disposition: None,
                    protocol: None,
                    rust_component: None,
                    hil_evidence: None,
                    semantic_contract: None,
                    effect_comparison: None,
                    effect_rules: Vec::new(),
                    binding_version: None,
                    rust_probe: None,
                    compare_return: None,
                    driver_adapter: None,
                    qualification_blockers: Vec::new(),
                    line: line_number,
                });
                continue;
            }

            match directive {
                "default-disposition" => {
                    if current.is_some() {
                        return Err(format!(
                            "default-disposition inside function at line {line_number}"
                        )
                        .into());
                    }
                    if default_disposition
                        .replace(Disposition::parse(value, line_number)?)
                        .is_some()
                    {
                        return Err("duplicate default-disposition".into());
                    }
                }
                "default-protocol" => {
                    if current.is_some() {
                        return Err(format!(
                            "default-protocol inside function at line {line_number}"
                        )
                        .into());
                    }
                    if default_protocol
                        .replace(Protocol::parse(value, line_number)?)
                        .is_some()
                    {
                        return Err("duplicate default-protocol".into());
                    }
                }
                "protocol-prefix" => {
                    if current.is_some() {
                        return Err(format!(
                            "protocol-prefix inside function at line {line_number}"
                        )
                        .into());
                    }
                    let mut words = value.split_whitespace();
                    let source = words.next().ok_or("protocol-prefix has no source")?;
                    let prefix = words.next().ok_or("protocol-prefix has no prefix")?;
                    let protocol = words.next().ok_or("protocol-prefix has no protocol")?;
                    if words.next().is_some() {
                        return Err(format!(
                            "protocol-prefix has extra fields at line {line_number}"
                        )
                        .into());
                    }
                    validate_source_id(source, line_number)?;
                    protocol_prefixes.push(ProtocolPrefix {
                        source: source.to_owned(),
                        prefix: prefix.to_owned(),
                        protocol: Protocol::parse(protocol, line_number)?,
                    });
                }
                "disposition" | "protocol" | "rust-component" | "hil-evidence"
                | "semantic-contract" | "effect-contract" | "effect" | "binding" | "rust-probe"
                | "compare-return" | "driver-adapter" | "blocked-by" => {
                    let builder = current.as_mut().ok_or_else(|| {
                        format!("{directive} outside function at line {line_number}")
                    })?;
                    match directive {
                        "disposition" => {
                            if builder
                                .disposition
                                .replace(Disposition::parse(value, line_number)?)
                                .is_some()
                            {
                                return Err(
                                    format!("duplicate disposition at line {line_number}").into()
                                );
                            }
                        }
                        "protocol" => {
                            if builder
                                .protocol
                                .replace(Protocol::parse(value, line_number)?)
                                .is_some()
                            {
                                return Err(
                                    format!("duplicate protocol at line {line_number}").into()
                                );
                            }
                        }
                        "rust-component" => {
                            if builder.rust_component.replace(value.to_owned()).is_some() {
                                return Err(format!(
                                    "duplicate rust-component at line {line_number}"
                                )
                                .into());
                            }
                        }
                        "hil-evidence" => {
                            if builder.hil_evidence.replace(value.to_owned()).is_some() {
                                return Err(format!(
                                    "duplicate hil-evidence at line {line_number}"
                                )
                                .into());
                            }
                        }
                        "semantic-contract" => {
                            if builder
                                .semantic_contract
                                .replace(SemanticContract::parse(value, line_number)?)
                                .is_some()
                            {
                                return Err(format!(
                                    "duplicate semantic-contract at line {line_number}"
                                )
                                .into());
                            }
                        }
                        "effect-contract" => {
                            if builder
                                .effect_comparison
                                .replace(EffectComparison::parse(value, line_number)?)
                                .is_some()
                            {
                                return Err(format!(
                                    "duplicate effect-contract at line {line_number}"
                                )
                                .into());
                            }
                        }
                        "effect" => builder
                            .effect_rules
                            .push(parse_effect_rule(value, line_number)?),
                        "binding" => {
                            if builder
                                .binding_version
                                .replace(BindingVersion::parse(value, line_number)?)
                                .is_some()
                            {
                                return Err(
                                    format!("duplicate binding at line {line_number}").into()
                                );
                            }
                        }
                        "rust-probe" => {
                            if builder.rust_probe.replace(value.to_owned()).is_some() {
                                return Err(
                                    format!("duplicate rust-probe at line {line_number}").into()
                                );
                            }
                        }
                        "compare-return" => {
                            if value != "true" {
                                return Err(format!(
                                    "compare-return must be true at line {line_number}"
                                )
                                .into());
                            }
                            if builder.compare_return.replace(true).is_some() {
                                return Err(format!(
                                    "duplicate compare-return at line {line_number}"
                                )
                                .into());
                            }
                        }
                        "driver-adapter" => {
                            if builder
                                .driver_adapter
                                .replace(DriverAdapter::parse(value, line_number)?)
                                .is_some()
                            {
                                return Err(format!(
                                    "duplicate driver-adapter at line {line_number}"
                                )
                                .into());
                            }
                        }
                        "blocked-by" => {
                            let mut words = value.split_whitespace();
                            let source = words.next().ok_or_else(|| {
                                format!("blocked-by has no source at line {line_number}")
                            })?;
                            let symbol = words.next().ok_or_else(|| {
                                format!("blocked-by has no symbol at line {line_number}")
                            })?;
                            if words.next().is_some() {
                                return Err(format!(
                                    "blocked-by has extra fields at line {line_number}"
                                )
                                .into());
                            }
                            validate_source_id(source, line_number)?;
                            let blocker = (source.to_owned(), symbol.to_owned());
                            if builder.qualification_blockers.contains(&blocker) {
                                return Err(format!(
                                    "duplicate blocked-by {source} {symbol} at line {line_number}"
                                )
                                .into());
                            }
                            builder.qualification_blockers.push(blocker);
                        }
                        _ => unreachable!(),
                    }
                }
                _ => {
                    return Err(format!(
                        "unknown disposition directive {directive:?} at line {line_number}"
                    )
                    .into());
                }
            }
        }
        if let Some(builder) = current {
            finish_entry(builder, &mut entries)?;
        }

        let default_disposition =
            default_disposition.ok_or("disposition manifest has no default-disposition")?;
        let default_protocol =
            default_protocol.ok_or("disposition manifest has no default-protocol")?;
        let mut prefix_identities = BTreeSet::new();
        for prefix in &protocol_prefixes {
            if !prefix_identities.insert((prefix.source.as_str(), prefix.prefix.as_str())) {
                return Err(format!(
                    "duplicate protocol-prefix {} {}",
                    prefix.source, prefix.prefix
                )
                .into());
            }
        }

        Ok(Self {
            default_disposition,
            default_protocol,
            protocol_prefixes,
            entries,
        })
    }

    pub fn resolve(&self, source: &str, symbol: &str) -> ResolvedDisposition<'_> {
        let entry = self.entries.get(&(source.to_owned(), symbol.to_owned()));
        let prefix_protocol = self
            .protocol_prefixes
            .iter()
            .filter(|rule| rule.source == source && symbol.starts_with(&rule.prefix))
            .max_by_key(|rule| rule.prefix.len())
            .map(|rule| rule.protocol);
        ResolvedDisposition {
            disposition: entry.map_or(self.default_disposition, |entry| entry.disposition),
            protocol: entry
                .and_then(|entry| entry.protocol)
                .or(prefix_protocol)
                .unwrap_or(self.default_protocol),
            entry,
        }
    }

    pub fn validate(&self, sources: &[(&str, &[ArtifactSymbolIdentity])]) -> Result<()> {
        let inventory = sources
            .iter()
            .flat_map(|(source, symbols)| {
                symbols
                    .iter()
                    .map(move |symbol| ((*source).to_owned(), symbol.name.clone()))
            })
            .collect::<BTreeSet<_>>();
        for key in self.entries.keys() {
            if !inventory.contains(key) {
                return Err(format!(
                    "disposition refers to missing {} vendor symbol {}",
                    key.0, key.1
                )
                .into());
            }
        }
        for entry in self.entries.values() {
            for blocker in &entry.qualification_blockers {
                if !inventory.contains(blocker) {
                    return Err(format!(
                        "qualification blocker for {} {} refers to missing {} vendor symbol {}",
                        entry.source, entry.symbol, blocker.0, blocker.1
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }
}

#[cfg(test)]
#[path = "../harnesses/esp32s31/dispositions_tests.rs"]
mod tests;
