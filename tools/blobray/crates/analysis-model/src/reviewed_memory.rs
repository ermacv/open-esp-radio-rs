//! Reviewed memory facts shared by knowledge providers and analysis backends.
//!
//! These declarations neither decode instructions nor install executable models.

use serde::Serialize;

/// Operation performed by one exact reviewed vendor-code memory access.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewedMemoryAccessOperation {
    Load,
    Store,
}

/// Whether one exact unresolved RAM access belongs to a hardware-facing
/// object or to ordinary vendor software state.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewedMemoryAccessRole {
    HardwareShared,
    SoftwareOnly,
}

impl ReviewedMemoryAccessRole {
    pub const fn label(self) -> &'static str {
        match self {
            Self::HardwareShared => "hardware-shared",
            Self::SoftwareOnly => "software-only",
        }
    }
}

/// Sparse reviewed classification for one exact artifact-local memory access.
///
/// The full linked-IR function identity and instruction site deliberately
/// make this fail closed when a vendor artifact changes. `object` is a stable
/// reviewer-owned semantic label; it is never inferred from symbol spelling,
/// addresses or diagnostic text.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ReviewedMemoryAccessOccurrence {
    pub artifact_source: &'static str,
    pub artifact_sha256: &'static str,
    pub function: &'static str,
    pub site: u32,
    pub operation: ReviewedMemoryAccessOperation,
}

impl ReviewedMemoryAccessOccurrence {
    pub const fn new(
        artifact_source: &'static str,
        artifact_sha256: &'static str,
        function: &'static str,
        site: u32,
        operation: ReviewedMemoryAccessOperation,
    ) -> Self {
        Self {
            artifact_source,
            artifact_sha256,
            function,
            site,
            operation,
        }
    }

    fn validate(self) -> std::result::Result<(), String> {
        for (field, value) in [
            ("artifact source", self.artifact_source),
            ("function", self.function),
        ] {
            if value.is_empty() {
                return Err(format!("reviewed memory-access {field} is empty"));
            }
        }
        if self.artifact_sha256.len() != 64
            || !self
                .artifact_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!(
                "reviewed memory-access artifact SHA-256 {:?} is not 64 lower-case hex digits",
                self.artifact_sha256
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ReviewedMemoryAccessClassification {
    pub id: &'static str,
    pub occurrence: ReviewedMemoryAccessOccurrence,
    pub role: ReviewedMemoryAccessRole,
    pub object: &'static str,
    pub evidence: &'static str,
}

impl ReviewedMemoryAccessClassification {
    pub const fn new(
        id: &'static str,
        occurrence: ReviewedMemoryAccessOccurrence,
        role: ReviewedMemoryAccessRole,
        object: &'static str,
        evidence: &'static str,
    ) -> Self {
        Self {
            id,
            occurrence,
            role,
            object,
            evidence,
        }
    }

    pub fn validate(self) -> std::result::Result<(), String> {
        for (field, value) in [
            ("id", self.id),
            ("object", self.object),
            ("evidence", self.evidence),
        ] {
            if value.is_empty() {
                return Err(format!("reviewed memory-access {field} is empty"));
            }
        }
        self.occurrence.validate()
    }
}

/// Immutable reviewed encoding of a 32-bit compressed pointer field.
///
/// The encoded value is interpreted as
/// `address_base | ((field & ((1 << field_bits) - 1)) << address_shift)`.
/// This descriptor declares the reviewed layout. Recognizing instruction-derived
/// bit provenance remains the analysis backend's responsibility.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReviewedCompressedPointerEncoding {
    id: &'static str,
    address_base: u32,
    field_bits: u8,
    address_shift: u8,
}

impl ReviewedCompressedPointerEncoding {
    pub const fn new(
        id: &'static str,
        address_base: u32,
        field_bits: u8,
        address_shift: u8,
    ) -> Self {
        Self {
            id,
            address_base,
            field_bits,
            address_shift,
        }
    }

    pub const fn id(self) -> &'static str {
        self.id
    }

    pub const fn address_base(self) -> u32 {
        self.address_base
    }

    pub const fn field_bits(self) -> u8 {
        self.field_bits
    }

    pub const fn address_shift(self) -> u8 {
        self.address_shift
    }
}
