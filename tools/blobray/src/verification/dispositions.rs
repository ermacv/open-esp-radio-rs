//! Machine-readable implementation disposition parsing and inventory validation.
//!
//! Dispositions own reviewed binding and claim declarations. They do not own
//! vendor/Rust observations and cannot change the generic engine's verdict.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use crate::{ArtifactSymbolIdentity, Result};
use open_radio_vendor_semantics::RustBindingKind;
use serde::Deserialize;

use super::bindings::{BindingVersion, ComparisonPlan};
use super::effect_contract::{EffectComparison, EffectDisposition, EffectSelector};

mod model;

pub use model::{Disposition, Entry, Manifest, Protocol, ResolvedDisposition, RustComponentId};
use model::{EntryBuilder, ProtocolPrefix};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct DispositionDocument {
    schema: u32,
    default_disposition: Disposition,
    default_protocol: Protocol,
    #[serde(default)]
    protocol_prefixes: Vec<ProtocolPrefixInput>,
    #[serde(default)]
    functions: Vec<FunctionInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolPrefixInput {
    source: String,
    prefix: String,
    protocol: Protocol,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectRuleInput {
    selector: EffectSelector,
    disposition: EffectDisposition,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockerInput {
    source: String,
    symbol: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct FunctionInput {
    source: String,
    symbol: String,
    disposition: Disposition,
    #[serde(default)]
    protocol: Option<Protocol>,
    #[serde(default)]
    rust_component: Option<String>,
    #[serde(default)]
    hil_evidence: Option<String>,
    #[serde(default)]
    effect_contract: Option<EffectComparison>,
    #[serde(default)]
    effects: Vec<EffectRuleInput>,
    #[serde(default)]
    binding: Option<String>,
    #[serde(default)]
    rust_binding: Option<RustBindingKind>,
    #[serde(default)]
    rust_probe: Option<String>,
    #[serde(default)]
    comparison_plan: Option<String>,
    #[serde(default)]
    compare_return: Option<bool>,
    #[serde(default)]
    blocked_by: Vec<BlockerInput>,
}

pub(crate) fn validate_source_id(value: &str, line: usize) -> Result<&str> {
    crate::source_id::validate_source_id(value).map_err(|_| {
        crate::Error::invalid(format!("invalid vendor source id {value:?} at line {line}"))
    })
}

impl Manifest {
    #[tracing::instrument(name = "load_disposition_manifest", fields(path = %path.display()))]
    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let document: DispositionDocument = toml_edit::de::from_str(&input).map_err(|error| {
            crate::error::BlobrayError::manifest_source(
                "disposition TOML",
                path,
                &input,
                &error,
                error.span(),
            )
        })?;
        Self::finish(document).map_err(|error| {
            crate::error::BlobrayError::manifest_document("disposition TOML", path, &input, error)
        })
    }

    pub(crate) fn load_all(paths: &[std::path::PathBuf]) -> Result<Option<Self>> {
        let mut manifests = paths.iter();
        let Some(first) = manifests.next() else {
            return Ok(None);
        };
        let mut merged = Self::load(first)?;
        for path in manifests {
            merged.merge(Self::load(path)?, path)?;
        }
        Ok(Some(merged))
    }

    fn merge(&mut self, other: Self, path: &Path) -> Result<()> {
        if self.default_disposition != other.default_disposition
            || self.default_protocol != other.default_protocol
        {
            return Err(crate::Error::invalid(format!(
                "disposition fragment {} has different defaults",
                path.display()
            )));
        }
        let mut prefix_keys = self
            .protocol_prefixes
            .iter()
            .map(|prefix| (prefix.source.clone(), prefix.prefix.clone()))
            .collect::<BTreeSet<_>>();
        for prefix in other.protocol_prefixes {
            let key = (prefix.source.clone(), prefix.prefix.clone());
            if !prefix_keys.insert(key.clone()) {
                return Err(crate::Error::invalid(format!(
                    "disposition fragment {} repeats protocol prefix {} {}",
                    path.display(),
                    key.0,
                    key.1
                )));
            }
            self.protocol_prefixes.push(prefix);
        }
        for (key, entry) in other.entries {
            if self.entries.insert(key.clone(), entry).is_some() {
                return Err(crate::Error::invalid(format!(
                    "disposition fragment {} repeats function {} {}",
                    path.display(),
                    key.0,
                    key.1
                )));
            }
        }
        Ok(())
    }

    fn finish(document: DispositionDocument) -> Result<Self> {
        if document.schema != 3 {
            return Err(crate::Error::invalid(
                "disposition TOML requires schema = 3",
            ));
        }
        let mut prefix_identities = BTreeSet::new();
        let mut protocol_prefixes = Vec::new();
        for prefix in document.protocol_prefixes {
            validate_source_id(&prefix.source, 1)?;
            if !prefix_identities.insert((prefix.source.clone(), prefix.prefix.clone())) {
                return Err(crate::Error::invalid(format!(
                    "duplicate protocol prefix {} {}",
                    prefix.source, prefix.prefix
                )));
            }
            protocol_prefixes.push(ProtocolPrefix {
                source: prefix.source,
                prefix: prefix.prefix,
                protocol: prefix.protocol,
            });
        }
        let mut entries = BTreeMap::new();
        for (index, function) in document.functions.into_iter().enumerate() {
            validate_source_id(&function.source, index + 1)?;
            let rust_component = function
                .rust_component
                .as_deref()
                .map(|value| RustComponentId::parse(value, index + 1))
                .transpose()?;
            let binding_version = function
                .binding
                .as_deref()
                .map(|value| BindingVersion::parse(value, index + 1))
                .transpose()?;
            let comparison_plan = function
                .comparison_plan
                .as_deref()
                .map(|value| ComparisonPlan::parse(value, index + 1))
                .transpose()?;
            let entry = EntryBuilder {
                source: function.source,
                symbol: function.symbol,
                disposition: Some(function.disposition),
                protocol: function.protocol,
                rust_component,
                hil_evidence: function.hil_evidence,
                effect_comparison: function.effect_contract,
                effect_rules: function
                    .effects
                    .into_iter()
                    .map(|rule| (rule.selector, rule.disposition))
                    .collect(),
                binding_version,
                rust_binding: function.rust_binding,
                rust_probe: function.rust_probe,
                comparison_plan,
                compare_return: function.compare_return,
                release_blockers: function
                    .blocked_by
                    .into_iter()
                    .map(|blocker| (blocker.source, blocker.symbol))
                    .collect(),
                line: index + 1,
            }
            .finish()?;
            let key = (entry.source.clone(), entry.symbol.clone());
            if entries.insert(key, entry).is_some() {
                return Err(crate::Error::invalid(
                    "duplicate disposition function entry",
                ));
            }
        }
        Ok(Self {
            default_disposition: document.default_disposition,
            default_protocol: document.default_protocol,
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
                return Err(crate::Error::invalid(format!(
                    "disposition refers to missing {} vendor symbol {}",
                    key.0, key.1
                )));
            }
        }
        for entry in self.entries.values() {
            for blocker in &entry.release_blockers {
                if !inventory.contains(blocker) {
                    return Err(crate::Error::invalid(format!(
                        "release blocker for {} {} refers to missing {} vendor symbol {}",
                        entry.source, entry.symbol, blocker.0, blocker.1
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }
}
