//! Executable vendor-to-Rust binding identity.
//!
//! A binding selects the Rust probe and optional semantic adapter used for a
//! comparison. It deliberately does not authenticate input artifacts. The
//! caller owns artifact provenance and may enforce any digest/signature policy
//! before invoking the workbench.

use crate::{ArtifactSymbolIdentity, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BindingVersion {
    V1,
}

/// Closed executable bridge between a compiled probe and a Rust architectural
/// replacement whose control flow cannot be compared as one direct leaf.
///
/// This registry is still platform-specific and will move to the ESP32-S31
/// harness. Keeping it here temporarily preserves the existing verifier while
/// the generic engine/harness boundary is introduced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DriverAdapter(String);

impl DriverAdapter {
    pub(crate) fn parse(value: &str, line: usize) -> Result<Self> {
        if !valid_registry_id(value) {
            return Err(format!("invalid driver adapter id {value:?} at line {line}").into());
        }
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn label(&self) -> &str {
        &self.0
    }
}

fn valid_registry_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

impl BindingVersion {
    pub(crate) fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "v1" => Ok(Self::V1),
            _ => Err(format!("unknown binding version {value:?} at line {line}").into()),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::V1 => "v1",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Binding {
    pub(crate) version: BindingVersion,
    pub(crate) rust_probe: String,
    pub(crate) compare_return: bool,
    pub(crate) driver_adapter: Option<DriverAdapter>,
}

impl Binding {
    pub(crate) fn new(
        version: BindingVersion,
        rust_probe: String,
        compare_return: bool,
        driver_adapter: Option<DriverAdapter>,
    ) -> Result<Self> {
        if rust_probe.is_empty() || rust_probe.chars().any(char::is_whitespace) {
            return Err("binding rust-probe must be one non-empty symbol".into());
        }
        Ok(Self {
            version,
            rust_probe,
            compare_return,
            driver_adapter,
        })
    }

    pub(crate) fn validate(&self, rust_symbols: &[ArtifactSymbolIdentity]) -> Result<()> {
        if !rust_symbols
            .iter()
            .any(|symbol| symbol.name == self.rust_probe)
        {
            return Err(format!("binding refers to missing Rust probe {}", self.rust_probe).into());
        }
        Ok(())
    }

    pub(crate) fn canonical(&self) -> String {
        let mut output = format!("binding {}\n", self.version.label());
        output.push_str("rust-probe ");
        output.push_str(&self.rust_probe);
        output.push('\n');
        if self.compare_return {
            output.push_str("compare-return true\n");
        }
        if let Some(adapter) = &self.driver_adapter {
            output.push_str("driver-adapter ");
            output.push_str(adapter.label());
            output.push('\n');
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> Binding {
        Binding::new(
            BindingVersion::V1,
            "open_phy_trace_leaf".to_owned(),
            false,
            None,
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
            BindingVersion::V1,
            "open_custom_trace_leaf".to_owned(),
            true,
            None,
        )
        .unwrap();

        assert!(return_binding.compare_return);
        assert!(return_binding.canonical().contains("compare-return true\n"));
        assert!(!binding().canonical().contains("compare-return"));
    }
}
