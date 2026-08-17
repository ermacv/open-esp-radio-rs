//! Structured evidence identities with reviewable component provenance.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Result, error::BlobrayError};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvidenceIdentity {
    pub(crate) kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) digest: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) components: BTreeMap<String, String>,
}

impl EvidenceIdentity {
    pub(crate) fn plain(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            digest: None,
            components: BTreeMap::new(),
        }
    }

    pub(crate) fn composed(
        kind: impl Into<String>,
        domain: &str,
        components: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self> {
        let kind = kind.into();
        let components = components.into_iter().collect::<BTreeMap<_, _>>();
        if components.is_empty() {
            return Err(crate::Error::invalid(format!(
                "digested evidence {kind:?} requires at least one component"
            )));
        }
        for (name, digest) in &components {
            validate_component(name, digest)?;
        }
        let mut aggregate = Sha256::new();
        aggregate.update(domain.as_bytes());
        aggregate.update([0]);
        aggregate.update(kind.as_bytes());
        for (name, digest) in &components {
            aggregate.update([0]);
            aggregate.update(name.as_bytes());
            aggregate.update([0]);
            aggregate.update(digest.as_bytes());
        }
        Ok(Self {
            kind,
            digest: Some(format!("{:x}", aggregate.finalize())),
            components,
        })
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.kind.is_empty() {
            return Err(crate::Error::invalid("evidence kind must not be empty"));
        }
        match &self.digest {
            Some(digest) => {
                validate_sha256("evidence digest", digest)?;
                if self.components.is_empty() {
                    return Err(crate::Error::invalid(format!(
                        "digested evidence {:?} requires component provenance",
                        self.kind
                    )));
                }
            }
            None if !self.components.is_empty() => {
                return Err(crate::Error::invalid(format!(
                    "plain evidence {:?} cannot contain digested components",
                    self.kind
                )));
            }
            None => {}
        }
        for (name, digest) in &self.components {
            validate_component(name, digest)?;
        }
        Ok(())
    }

    pub(crate) fn label(&self) -> String {
        self.digest.as_ref().map_or_else(
            || self.kind.clone(),
            |digest| format!("{}/sha256:{digest}", self.kind),
        )
    }
}

impl fmt::Display for EvidenceIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label())
    }
}

pub(crate) fn component(name: impl Into<String>, contents: impl AsRef<[u8]>) -> (String, String) {
    (
        name.into(),
        format!("{:x}", Sha256::digest(contents.as_ref())),
    )
}

pub(crate) fn combined_component<'a>(
    name: impl Into<String>,
    parts: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> (String, String) {
    let mut digest = Sha256::new();
    for (part_name, contents) in parts {
        digest.update(part_name.as_bytes());
        digest.update([0]);
        digest.update(contents.as_bytes());
        digest.update([0]);
    }
    (name.into(), format!("{:x}", digest.finalize()))
}

fn validate_component(name: &str, digest: &str) -> Result<()> {
    if name.is_empty() {
        return Err(crate::Error::invalid(
            "evidence component name must not be empty",
        ));
    }
    validate_sha256(&format!("evidence component {name:?}"), digest)
}

fn validate_sha256(context: &str, digest: &str) -> Result<()> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(BlobrayError::invalid(format!(
            "{context} must be a 64-digit SHA-256 value"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composed_identity_is_order_independent_and_reviewable() {
        let left = EvidenceIdentity::composed(
            "scenario/profile:test",
            "test-v1",
            [component("profile", "a"), component("executor", "b")],
        )
        .unwrap();
        let right = EvidenceIdentity::composed(
            "scenario/profile:test",
            "test-v1",
            [component("executor", "b"), component("profile", "a")],
        )
        .unwrap();
        assert_eq!(left, right);
        assert!(left.label().starts_with("scenario/profile:test/sha256:"));
    }

    #[test]
    fn digested_identity_requires_component_provenance() {
        let identity = EvidenceIdentity {
            kind: "scenario/profile:test".to_owned(),
            digest: Some("11".repeat(32)),
            components: BTreeMap::new(),
        };
        assert!(
            identity
                .validate()
                .unwrap_err()
                .to_string()
                .contains("component provenance")
        );
    }
}
