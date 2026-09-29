//! JSON documents printed by the `blobray` CLI with `--format json`.
//!
//! The renderer emits these shapes and typed clients decode them; payloads are
//! the application/domain records themselves, not copies of their schemas.
use blobray_domain::{
    ArtifactId, CheckVerdict, ForbiddenTargetRange, RegisterAccess, RegisterAccessSummary,
    TargetAuditRecord, TargetAuditSummary,
};
use serde::{Deserialize, Serialize};

/// Schema of the `audit-targets` document.
pub const TARGET_AUDIT_SCHEMA: u32 = 1;

/// Every finding of one final-image audit and its verdict: `pass` only
/// without a forbidden target or a coverage gap.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetAuditDocument {
    pub schema: u32,
    /// SHA-256 of the audited image.
    pub artifact: ArtifactId,
    pub decoder: String,
    pub semantics: String,
    pub ranges: Vec<ForbiddenTargetRange>,
    pub records: Vec<TargetAuditRecord>,
    pub summary: TargetAuditSummary,
    pub verdict: CheckVerdict,
}

/// Schema of the `register-accesses` document
/// (`{"schema":1,"inputs":[...],"records":[...],"summary":{...}}`).
pub const REGISTER_ACCESSES_SCHEMA: u32 = 1;

/// One analyzed input of a `register-accesses` document, by position.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAccessInput {
    pub role: String,
    pub sha256: ArtifactId,
}

/// Every register access of the analyzed inputs; see `REGISTER_ACCESSES_SCHEMA`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAccessDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    pub records: Vec<RegisterAccess>,
    pub summary: RegisterAccessSummary,
}
