//! Executable vendor-to-Rust binding identity.

use std::collections::BTreeSet;

use crate::{ArtifactSymbolIdentity, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BindingVersion {
    V1,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VendorRevision {
    Esp32s31Eco0Rom,
    Esp32s31Rev0Libphy,
}

impl VendorRevision {
    pub(crate) fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "esp32s31-eco0-rom" => Ok(Self::Esp32s31Eco0Rom),
            "esp32s31-rev0-libphy" => Ok(Self::Esp32s31Rev0Libphy),
            _ => Err(format!("unknown vendor revision {value:?} at line {line}").into()),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Esp32s31Eco0Rom => "esp32s31-eco0-rom",
            Self::Esp32s31Rev0Libphy => "esp32s31-rev0-libphy",
        }
    }

    pub(crate) const fn source(self) -> &'static str {
        match self {
            Self::Esp32s31Eco0Rom => "rom",
            Self::Esp32s31Rev0Libphy => "archive",
        }
    }
}

pub(crate) fn parse_sha256(value: &str, directive: &str, line: usize) -> Result<String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(
            format!("{directive} requires a lowercase 64-digit SHA-256 at line {line}").into(),
        );
    }
    Ok(value.to_owned())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Binding {
    pub(crate) version: BindingVersion,
    pub(crate) revision: VendorRevision,
    artifact_digests: BTreeSet<String>,
    inventory_digests: BTreeSet<String>,
    pub(crate) rust_probe: String,
}

impl Binding {
    pub(crate) fn new(
        version: BindingVersion,
        revision: VendorRevision,
        artifact_digests: BTreeSet<String>,
        inventory_digests: BTreeSet<String>,
        rust_probe: String,
    ) -> Result<Self> {
        if artifact_digests.is_empty() {
            return Err("binding has no vendor-artifact-sha256".into());
        }
        if rust_probe.is_empty() || rust_probe.chars().any(char::is_whitespace) {
            return Err("binding rust-probe must be one non-empty symbol".into());
        }
        if revision == VendorRevision::Esp32s31Rev0Libphy && inventory_digests.is_empty() {
            return Err("libphy binding has no vendor-inventory-sha256".into());
        }
        if revision == VendorRevision::Esp32s31Eco0Rom && !inventory_digests.is_empty() {
            return Err("ROM binding cannot have vendor-inventory-sha256".into());
        }
        Ok(Self {
            version,
            revision,
            artifact_digests,
            inventory_digests,
            rust_probe,
        })
    }

    pub(crate) fn validate(
        &self,
        source: &str,
        artifact_digest: &str,
        inventory_digest: Option<&str>,
        rust_symbols: &[ArtifactSymbolIdentity],
    ) -> Result<()> {
        if source != self.revision.source() {
            return Err(format!(
                "binding revision {} belongs to {}, not {source}",
                self.revision.label(),
                self.revision.source()
            )
            .into());
        }
        if !self.artifact_digests.contains(artifact_digest) {
            return Err(format!(
                "binding revision {} rejects vendor artifact sha256 {artifact_digest}",
                self.revision.label()
            )
            .into());
        }
        match (self.inventory_digests.is_empty(), inventory_digest) {
            (true, None) => {}
            (false, Some(digest)) if self.inventory_digests.contains(digest) => {}
            (false, Some(digest)) => {
                return Err(format!(
                    "binding revision {} rejects vendor inventory sha256 {digest}",
                    self.revision.label()
                )
                .into());
            }
            (false, None) => {
                return Err(format!(
                    "binding revision {} requires a vendor inventory",
                    self.revision.label()
                )
                .into());
            }
            (true, Some(_)) => {
                return Err(format!(
                    "binding revision {} does not permit a separate vendor inventory",
                    self.revision.label()
                )
                .into());
            }
        }
        if !rust_symbols
            .iter()
            .any(|symbol| symbol.name == self.rust_probe)
        {
            return Err(format!(
                "binding revision {} refers to missing Rust probe {}",
                self.revision.label(),
                self.rust_probe
            )
            .into());
        }
        Ok(())
    }

    pub(crate) fn canonical(&self) -> String {
        let mut output = format!(
            "binding {}\nvendor-revision {}\n",
            self.version.label(),
            self.revision.label()
        );
        for digest in &self.artifact_digests {
            output.push_str("vendor-artifact-sha256 ");
            output.push_str(digest);
            output.push('\n');
        }
        for digest in &self.inventory_digests {
            output.push_str("vendor-inventory-sha256 ");
            output.push_str(digest);
            output.push('\n');
        }
        output.push_str("rust-probe ");
        output.push_str(&self.rust_probe);
        output.push('\n');
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn rom_binding() -> Binding {
        Binding::new(
            BindingVersion::V1,
            VendorRevision::Esp32s31Eco0Rom,
            BTreeSet::from([DIGEST.to_owned()]),
            BTreeSet::new(),
            "open_phy_trace_leaf".to_owned(),
        )
        .unwrap()
    }

    #[test]
    fn binding_validates_revision_digest_and_exact_probe_symbol() {
        let symbols = [ArtifactSymbolIdentity {
            member: None,
            name: "open_phy_trace_leaf".to_owned(),
        }];
        rom_binding()
            .validate("rom", DIGEST, None, &symbols)
            .unwrap();
        assert!(
            rom_binding()
                .validate("archive", DIGEST, None, &symbols)
                .is_err()
        );
        assert!(
            rom_binding()
                .validate("rom", &"f".repeat(64), None, &symbols)
                .is_err()
        );
        assert!(rom_binding().validate("rom", DIGEST, None, &[]).is_err());
    }

    #[test]
    fn digest_parser_rejects_uppercase_short_and_non_hex_values() {
        assert!(parse_sha256(DIGEST, "digest", 1).is_ok());
        assert!(parse_sha256("abcd", "digest", 1).is_err());
        assert!(parse_sha256(&"A".repeat(64), "digest", 1).is_err());
        assert!(parse_sha256(&"z".repeat(64), "digest", 1).is_err());
    }
}
