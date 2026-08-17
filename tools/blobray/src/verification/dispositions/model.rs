//! Disposition manifest data model and entry invariants.

use std::{collections::BTreeMap, ops::Deref};

use super::super::bindings::{Binding, BindingVersion, ComparisonPlan};
use super::super::effect_contract::{
    EffectComparison, EffectDisposition, EffectPolicy, EffectSelector,
};
use crate::Result;
use open_radio_vendor_semantics::RustBindingKind;
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
    BoundedFeature,
    GenerationCandidate,
    NotYetPorted,
}

/// Stable identity of the Rust item that owns a vendor replacement.
///
/// The canonical Rust module/item path is deliberately used as the component
/// id. This keeps the reviewed manifest reproducible while preventing an
/// arbitrary human description from becoming the join key for project-wide
/// verification reports.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RustComponentId(String);

impl RustComponentId {
    pub(super) fn parse(value: &str, line: usize) -> Result<Self> {
        let valid = !value.is_empty()
            && value.split("::").all(|segment| {
                let mut characters = segment.chars();
                characters
                    .next()
                    .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
                    && characters
                        .all(|character| character == '_' || character.is_ascii_alphanumeric())
            });
        if !valid {
            return Err(crate::Error::invalid(format!(
                "invalid Rust component path {value:?} at line {line}"
            )));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn label(&self) -> &str {
        &self.0
    }
}

impl Deref for RustComponentId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.label()
    }
}

impl Disposition {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::StateTransition => "state-transition",
            Self::ReplacedByComposition => "replaced-by-composition",
            Self::BoundedFeature => "bounded-feature",
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

    pub const fn is_bounded_feature(self) -> bool {
        matches!(self, Self::BoundedFeature)
    }

    pub const fn has_production_owner(self) -> bool {
        self.is_implemented() || self.is_bounded_feature()
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
    pub rust_component: Option<RustComponentId>,
    pub hil_evidence: Option<String>,
    pub effect_contract: Option<EffectPolicy>,
    pub binding: Option<Binding>,
    pub release_blockers: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub(super) struct EntryBuilder {
    pub(super) source: String,
    pub(super) symbol: String,
    pub(super) disposition: Option<Disposition>,
    pub(super) protocol: Option<Protocol>,
    pub(super) rust_component: Option<RustComponentId>,
    pub(super) hil_evidence: Option<String>,
    pub(super) effect_comparison: Option<EffectComparison>,
    pub(super) effect_rules: Vec<(EffectSelector, EffectDisposition)>,
    pub(super) binding_version: Option<BindingVersion>,
    pub(super) rust_binding: Option<RustBindingKind>,
    pub(super) rust_probe: Option<String>,
    pub(super) comparison_plan: Option<ComparisonPlan>,
    pub(super) compare_return: Option<bool>,
    pub(super) release_blockers: Vec<(String, String)>,
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
            if disposition.has_production_owner() && self.rust_component.is_none() {
                return Err(crate::Error::invalid(format!(
                    "production-owned function or feature {} {} has no rust-component",
                    self.source, self.symbol
                )));
            }
            if self.effect_comparison.is_some()
                && !disposition.has_production_owner()
                && disposition != Disposition::GenerationCandidate
            {
                return Err(crate::Error::invalid(format!(
                    "unimplemented function {} {} cannot have an effect-contract",
                    self.source, self.symbol
                )));
            }
            if self.effect_comparison.is_none() && !self.effect_rules.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "function {} {} has effect rules but no effect-contract",
                    self.source, self.symbol
                )));
            }
            if !self.release_blockers.is_empty() && !disposition.is_implemented() {
                return Err(crate::Error::invalid(format!(
                    "unimplemented function {} {} cannot have release blockers",
                    self.source, self.symbol
                )));
            }
            if !self.release_blockers.is_empty() && self.effect_comparison.is_some() {
                return Err(crate::Error::invalid(format!(
                    "verified function {} {} cannot have release blockers",
                    self.source, self.symbol
                )));
            }
            let effect_contract = self
                .effect_comparison
                .map(|comparison| EffectPolicy::new(comparison, self.effect_rules))
                .transpose()?;
            let has_binding_fields = self.rust_probe.is_some()
                || self.rust_binding.is_some()
                || self.comparison_plan.is_some()
                || self.compare_return.is_some();
            let binding = match self.binding_version {
                Some(version) => Some(Binding::new(
                    version,
                    self.rust_binding
                        .ok_or_else(|| {
                            format!(
                                "binding {} {} has no rust-binding trust classification",
                                self.source, self.symbol
                            )
                        })
                        .map_err(crate::Error::invalid)?,
                    self.rust_probe
                        .ok_or_else(|| {
                            format!("binding {} {} has no rust-probe", self.source, self.symbol)
                        })
                        .map_err(crate::Error::invalid)?,
                    self.comparison_plan
                        .ok_or_else(|| {
                            format!(
                                "binding {} {} has no comparison-plan",
                                self.source, self.symbol
                            )
                        })
                        .map_err(crate::Error::invalid)?,
                    self.compare_return.unwrap_or(false),
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
            if binding.is_some() && effect_contract.is_none() {
                return Err(crate::Error::invalid(format!(
                    "binding {} {} has no registered effect contract",
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
                effect_contract,
                binding,
                release_blockers: self.release_blockers,
            })
        })()
        .map_err(|error: crate::error::BlobrayError| error.at_line(line))
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
