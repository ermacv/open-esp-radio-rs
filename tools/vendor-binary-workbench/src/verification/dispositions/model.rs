//! Disposition manifest data model and entry invariants.

use std::collections::BTreeMap;

use super::super::bindings::{Binding, BindingVersion, DriverAdapter};
use super::super::effect_contract::{
    EffectComparison, EffectDisposition, EffectPolicy, EffectSelector,
};
use crate::Result;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Disposition {
    Direct,
    StateTransition,
    ReplacedByComposition,
    GenerationCandidate,
    NotYetPorted,
}

impl Disposition {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::StateTransition => "state-transition",
            Self::ReplacedByComposition => "replaced-by-composition",
            Self::GenerationCandidate => "generation-candidate",
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticContract(String);

impl SemanticContract {
    pub(super) fn parse(value: &str, line: usize) -> Result<Self> {
        if value.is_empty()
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            })
        {
            return Err(crate::Error::invalid(format!(
                "invalid semantic contract id {value:?} at line {line}"
            )));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn label(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub(super) struct ProtocolPrefix {
    pub(super) source: String,
    pub(super) prefix: String,
    pub(super) protocol: Protocol,
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
    pub binding: Option<Binding>,
    pub qualification_blockers: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub(super) struct EntryBuilder {
    pub(super) source: String,
    pub(super) symbol: String,
    pub(super) disposition: Option<Disposition>,
    pub(super) protocol: Option<Protocol>,
    pub(super) rust_component: Option<String>,
    pub(super) hil_evidence: Option<String>,
    pub(super) semantic_contract: Option<SemanticContract>,
    pub(super) effect_comparison: Option<EffectComparison>,
    pub(super) effect_rules: Vec<(EffectSelector, EffectDisposition)>,
    pub(super) binding_version: Option<BindingVersion>,
    pub(super) rust_probe: Option<String>,
    pub(super) compare_return: Option<bool>,
    pub(super) driver_adapter: Option<DriverAdapter>,
    pub(super) qualification_blockers: Vec<(String, String)>,
    pub(super) line: usize,
}

impl EntryBuilder {
    pub(super) fn finish(self) -> Result<Entry> {
        let line = self.line;
        (|| {
            let disposition = self
                .disposition
                .ok_or_else(|| {
                    format!(
                        "function {} {} has no disposition (started at line {})",
                        self.source, self.symbol, self.line
                    )
                })
                .map_err(crate::Error::invalid)?;
            if disposition.is_implemented() && self.rust_component.is_none() {
                return Err(crate::Error::invalid(format!(
                    "implemented function {} {} has no rust-component",
                    self.source, self.symbol
                )));
            }
            if self.semantic_contract.is_some() && !disposition.is_implemented() {
                return Err(crate::Error::invalid(format!(
                    "unimplemented function {} {} cannot have a semantic-contract",
                    self.source, self.symbol
                )));
            }
            if self.effect_comparison.is_some()
                && !disposition.is_implemented()
                && disposition != Disposition::GenerationCandidate
            {
                return Err(crate::Error::invalid(format!(
                    "unimplemented function {} {} cannot have an effect-contract",
                    self.source, self.symbol
                )));
            }
            if self.semantic_contract.is_some() && self.effect_comparison.is_some() {
                return Err(crate::Error::invalid(format!(
                    "function {} {} cannot combine semantic-contract and effect-contract",
                    self.source, self.symbol
                )));
            }
            if self.effect_comparison.is_none() && !self.effect_rules.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "function {} {} has effect rules but no effect-contract",
                    self.source, self.symbol
                )));
            }
            if !self.qualification_blockers.is_empty() && !disposition.is_implemented() {
                return Err(crate::Error::invalid(format!(
                    "unimplemented function {} {} cannot have qualification blockers",
                    self.source, self.symbol
                )));
            }
            if !self.qualification_blockers.is_empty()
                && (self.semantic_contract.is_some() || self.effect_comparison.is_some())
            {
                return Err(crate::Error::invalid(format!(
                    "qualified function {} {} cannot have qualification blockers",
                    self.source, self.symbol
                )));
            }
            let effect_contract = self
                .effect_comparison
                .map(|comparison| EffectPolicy::new(comparison, self.effect_rules))
                .transpose()?;
            let has_binding_fields = self.rust_probe.is_some()
                || self.compare_return.is_some()
                || self.driver_adapter.is_some();
            let binding = match self.binding_version {
                Some(version) => Some(Binding::new(
                    version,
                    self.rust_probe
                        .ok_or_else(|| {
                            format!("binding {} {} has no rust-probe", self.source, self.symbol)
                        })
                        .map_err(crate::Error::invalid)?,
                    self.compare_return.unwrap_or(false),
                    self.driver_adapter,
                )?),
                None if has_binding_fields => {
                    return Err(crate::Error::invalid(format!(
                        "function {} {} has binding fields but no binding version",
                        self.source, self.symbol
                    )));
                }
                None => None,
            };
            if effect_contract.is_some()
                && binding.is_none()
                && disposition != Disposition::GenerationCandidate
            {
                return Err(crate::Error::invalid(format!(
                    "effect contract {} {} has no executable binding",
                    self.source, self.symbol
                )));
            }
            if binding.is_some() && effect_contract.is_none() && self.semantic_contract.is_none() {
                return Err(crate::Error::invalid(format!(
                    "binding {} {} has no registered effect or semantic contract",
                    self.source, self.symbol
                )));
            }
            if binding
                .as_ref()
                .is_some_and(|binding| binding.driver_adapter.is_some())
                && effect_contract.is_none()
            {
                return Err(crate::Error::invalid(format!(
                    "driver adapter {} {} requires an effect-contract",
                    self.source, self.symbol
                )));
            }
            Ok(Entry {
                source: self.source,
                symbol: self.symbol,
                disposition,
                protocol: self.protocol,
                rust_component: self.rust_component,
                hil_evidence: self.hil_evidence,
                semantic_contract: self.semantic_contract,
                effect_contract,
                binding,
                qualification_blockers: self.qualification_blockers,
            })
        })()
        .map_err(|error: crate::error::WorkbenchError| error.at_line(line))
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
    pub(super) default_disposition: Disposition,
    pub(super) default_protocol: Protocol,
    pub(super) protocol_prefixes: Vec<ProtocolPrefix>,
    pub(super) entries: BTreeMap<(String, String), Entry>,
}
