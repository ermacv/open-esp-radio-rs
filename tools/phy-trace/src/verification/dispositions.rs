//! Machine-readable implementation disposition for the vendor PHY inventory.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use crate::{ArtifactSymbolIdentity, Result};

use super::effect_contract::{
    EffectComparison, EffectDisposition, EffectPolicy, EffectSelector, parse_effect_rule,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Protocol {
    Shared,
    Wifi,
    Bluetooth,
    Ble,
    Coex,
    Ieee802154,
    Unknown,
}

impl Protocol {
    fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "shared" => Ok(Self::Shared),
            "wifi" => Ok(Self::Wifi),
            "bluetooth" => Ok(Self::Bluetooth),
            "ble" => Ok(Self::Ble),
            "coex" => Ok(Self::Coex),
            "ieee802154" => Ok(Self::Ieee802154),
            "unknown" => Ok(Self::Unknown),
            _ => Err(format!("invalid protocol {value:?} at line {line}").into()),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Wifi => "wifi",
            Self::Bluetooth => "bluetooth",
            Self::Ble => "ble",
            Self::Coex => "coex",
            Self::Ieee802154 => "ieee802154",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Disposition {
    Direct,
    StateTransition,
    ReplacedByComposition,
    NotYetPorted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::enum_variant_names,
    reason = "chip-qualified variants prevent future cross-chip contract ambiguity"
)]
pub enum SemanticContract {
    Esp32s31Channel,
    Esp32s31RfInit,
    Esp32s31BluetoothTxDc,
    Esp32s31BluetoothTxDcPwdet,
    Esp32s31BluetoothTxPower,
}

impl SemanticContract {
    fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "esp32s31-channel" => Ok(Self::Esp32s31Channel),
            "esp32s31-rf-init" => Ok(Self::Esp32s31RfInit),
            "esp32s31-bluetooth-txdc" => Ok(Self::Esp32s31BluetoothTxDc),
            "esp32s31-bluetooth-txdc-pwdet" => Ok(Self::Esp32s31BluetoothTxDcPwdet),
            "esp32s31-bluetooth-tx-power" => Ok(Self::Esp32s31BluetoothTxPower),
            _ => Err(format!("invalid semantic contract {value:?} at line {line}").into()),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Esp32s31Channel => "esp32s31-channel",
            Self::Esp32s31RfInit => "esp32s31-rf-init",
            Self::Esp32s31BluetoothTxDc => "esp32s31-bluetooth-txdc",
            Self::Esp32s31BluetoothTxDcPwdet => "esp32s31-bluetooth-txdc-pwdet",
            Self::Esp32s31BluetoothTxPower => "esp32s31-bluetooth-tx-power",
        }
    }
}

impl Disposition {
    fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "direct" => Ok(Self::Direct),
            "state-transition" => Ok(Self::StateTransition),
            "replaced-by-composition" => Ok(Self::ReplacedByComposition),
            "not-yet-ported" => Ok(Self::NotYetPorted),
            _ => Err(format!("invalid disposition {value:?} at line {line}").into()),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::StateTransition => "state-transition",
            Self::ReplacedByComposition => "replaced-by-composition",
            Self::NotYetPorted => "not-yet-ported",
        }
    }

    pub const fn is_implemented(self) -> bool {
        matches!(
            self,
            Self::Direct | Self::StateTransition | Self::ReplacedByComposition
        )
    }
}

#[derive(Clone, Debug)]
struct ProtocolPrefix {
    source: String,
    prefix: String,
    protocol: Protocol,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub source: String,
    pub symbol: String,
    pub disposition: Disposition,
    pub protocol: Option<Protocol>,
    pub rust_component: Option<String>,
    pub hil_evidence: Option<String>,
    pub semantic_contract: Option<SemanticContract>,
    pub effect_contract: Option<EffectPolicy>,
    pub qualification_blockers: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
struct EntryBuilder {
    source: String,
    symbol: String,
    disposition: Option<Disposition>,
    protocol: Option<Protocol>,
    rust_component: Option<String>,
    hil_evidence: Option<String>,
    semantic_contract: Option<SemanticContract>,
    effect_comparison: Option<EffectComparison>,
    effect_rules: Vec<(EffectSelector, EffectDisposition)>,
    qualification_blockers: Vec<(String, String)>,
    line: usize,
}

impl EntryBuilder {
    fn finish(self) -> Result<Entry> {
        let disposition = self.disposition.ok_or_else(|| {
            format!(
                "function {} {} has no disposition (started at line {})",
                self.source, self.symbol, self.line
            )
        })?;
        if disposition.is_implemented() && self.rust_component.is_none() {
            return Err(format!(
                "implemented function {} {} has no rust-component",
                self.source, self.symbol
            )
            .into());
        }
        if self.semantic_contract.is_some() && !disposition.is_implemented() {
            return Err(format!(
                "unimplemented function {} {} cannot have a semantic-contract",
                self.source, self.symbol
            )
            .into());
        }
        if self.effect_comparison.is_some() && !disposition.is_implemented() {
            return Err(format!(
                "unimplemented function {} {} cannot have an effect-contract",
                self.source, self.symbol
            )
            .into());
        }
        if self.semantic_contract.is_some() && self.effect_comparison.is_some() {
            return Err(format!(
                "function {} {} cannot combine semantic-contract and effect-contract",
                self.source, self.symbol
            )
            .into());
        }
        if self.effect_comparison.is_none() && !self.effect_rules.is_empty() {
            return Err(format!(
                "function {} {} has effect rules but no effect-contract",
                self.source, self.symbol
            )
            .into());
        }
        if !self.qualification_blockers.is_empty() && !disposition.is_implemented() {
            return Err(format!(
                "unimplemented function {} {} cannot have qualification blockers",
                self.source, self.symbol
            )
            .into());
        }
        if !self.qualification_blockers.is_empty()
            && (self.semantic_contract.is_some() || self.effect_comparison.is_some())
        {
            return Err(format!(
                "qualified function {} {} cannot have qualification blockers",
                self.source, self.symbol
            )
            .into());
        }
        let effect_contract = self
            .effect_comparison
            .map(|comparison| EffectPolicy::new(comparison, self.effect_rules))
            .transpose()?;
        Ok(Entry {
            source: self.source,
            symbol: self.symbol,
            disposition,
            protocol: self.protocol,
            rust_component: self.rust_component,
            hil_evidence: self.hil_evidence,
            semantic_contract: self.semantic_contract,
            effect_contract,
            qualification_blockers: self.qualification_blockers,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedDisposition<'a> {
    pub disposition: Disposition,
    pub protocol: Protocol,
    pub entry: Option<&'a Entry>,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    default_disposition: Disposition,
    default_protocol: Protocol,
    protocol_prefixes: Vec<ProtocolPrefix>,
    entries: BTreeMap<(String, String), Entry>,
}

fn directive_value(line: &str, line_number: usize) -> Result<(&str, &str)> {
    line.split_once(char::is_whitespace)
        .map(|(directive, value)| (directive, value.trim()))
        .filter(|(_, value)| !value.is_empty())
        .ok_or_else(|| format!("disposition directive needs a value at line {line_number}").into())
}

fn parse_source(value: &str, line: usize) -> Result<&str> {
    if matches!(value, "rom" | "archive") {
        Ok(value)
    } else {
        Err(format!("invalid vendor source {value:?} at line {line}").into())
    }
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
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
                parse_source(source, line_number)?;
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
                    parse_source(source, line_number)?;
                    protocol_prefixes.push(ProtocolPrefix {
                        source: source.to_owned(),
                        prefix: prefix.to_owned(),
                        protocol: Protocol::parse(protocol, line_number)?,
                    });
                }
                "disposition" | "protocol" | "rust-component" | "hil-evidence"
                | "semantic-contract" | "effect-contract" | "effect" | "blocked-by" => {
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
                        "effect" => {
                            builder
                                .effect_rules
                                .push(parse_effect_rule(value, line_number)?);
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
                            parse_source(source, line_number)?;
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
#[path = "dispositions_tests.rs"]
mod tests;
