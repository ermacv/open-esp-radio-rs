//! Executable vendor-to-Rust binding identity.
//!
//! A binding selects one compiled Rust symbol used for a generic comparison.
//! It deliberately does not authenticate input artifacts. The
//! caller owns artifact provenance and may enforce any digest/signature policy
//! before invoking Blobray.

use crate::{ArtifactSymbolIdentity, Result};
use open_radio_vendor_semantics::RustBindingKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BindingVersion {
    V2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComparisonPlan(String);

impl ComparisonPlan {
    pub(crate) fn parse(value: &str, line: usize) -> Result<Self> {
        let valid = !value.is_empty()
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            });
        if !valid {
            return Err(crate::Error::invalid(format!(
                "invalid comparison plan id {value:?} at line {line}"
            )));
        }
        if value != "direct-effects-v1" {
            return Err(crate::Error::invalid(format!(
                "unsupported comparison plan {value:?} at line {line}; only generic direct-effects-v1 is available"
            )));
        }
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn label(&self) -> &str {
        &self.0
    }
}

impl BindingVersion {
    pub(crate) fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "v2" => Ok(Self::V2),
            _ => Err(crate::Error::invalid(format!(
                "unknown binding version {value:?} at line {line}"
            ))),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::V2 => "v2",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Binding {
    pub(crate) version: BindingVersion,
    pub(crate) rust_kind: RustBindingKind,
    pub(crate) rust_probe: String,
    pub(crate) comparison_plan: ComparisonPlan,
    pub(crate) compare_return: bool,
}

impl Binding {
    pub(crate) fn new(
        version: BindingVersion,
        rust_kind: RustBindingKind,
        rust_probe: String,
        comparison_plan: ComparisonPlan,
        compare_return: bool,
    ) -> Result<Self> {
        if rust_probe.is_empty() || rust_probe.chars().any(char::is_whitespace) {
            return Err(crate::Error::invalid(
                "binding rust-probe must be one non-empty symbol",
            ));
        }
        Ok(Self {
            version,
            rust_kind,
            rust_probe,
            comparison_plan,
            compare_return,
        })
    }

    pub(crate) fn validate(&self, rust_symbols: &[ArtifactSymbolIdentity]) -> Result<()> {
        if !rust_symbols
            .iter()
            .any(|symbol| symbol.name == self.rust_probe)
        {
            return Err(crate::Error::invalid(format!(
                "binding refers to missing Rust probe {}",
                self.rust_probe
            )));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn canonical(&self) -> String {
        format!(
            "binding {}\nrust-binding {}\nrust-probe {}\ncomparison-plan {}\ncompare-return {}\n",
            self.version.label(),
            self.rust_kind.label(),
            self.rust_probe,
            self.comparison_plan.label(),
            self.compare_return,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> Binding {
        Binding::new(
            BindingVersion::V2,
            RustBindingKind::ExactProductionEntry,
            "open_phy_trace_leaf".to_owned(),
            ComparisonPlan::parse("direct-effects-v1", 1).unwrap(),
            false,
        )
        .unwrap()
    }

    #[test]
    fn binding_validates_the_exact_probe_symbol() {
        let symbols = [ArtifactSymbolIdentity {
            member: None,
            name: "open_phy_trace_leaf".to_owned(),
        }];
        binding().validate(&symbols).unwrap();
        assert!(binding().validate(&[]).is_err());
    }

    #[test]
    fn return_comparison_is_an_explicit_evidence_bound_property() {
        let return_binding = Binding::new(
            BindingVersion::V2,
            RustBindingKind::ExactProductionEntry,
            "open_custom_trace_leaf".to_owned(),
            ComparisonPlan::parse("direct-effects-v1", 1).unwrap(),
            true,
        )
        .unwrap();

        assert!(return_binding.compare_return);
        assert!(return_binding.compare_return);
        assert!(!binding().compare_return);
    }
}
