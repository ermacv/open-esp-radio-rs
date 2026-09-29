//! JSON documents printed by the `blobray` CLI with `--format json`.
//!
//! The renderer emits these shapes and typed clients decode them; payloads are
//! the application/domain records themselves, not copies of their schemas.
use blobray_application::{QuerySummary, RunRecord};
use blobray_domain::{ResultAssessment, Revision, RevisionId};
use serde::{Deserialize, Serialize};

/// Schema of streamed record documents (`{"schema":2,"records":[...],...}`).
pub const RECORDS_SCHEMA: u32 = 2;
/// Schema of the inventory document.
pub const INVENTORY_SCHEMA: u32 = 2;

/// One streamed record. `kind` names the payload type selected by the query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record<T> {
    pub kind: String,
    pub value: T,
}

/// Complete record document. Callers choose `T` from the query they issued;
/// use `serde_json::Value` only to inspect mixed or unknown record kinds.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordDocument<T> {
    pub schema: u32,
    pub records: Vec<Record<T>>,
    pub summary: QuerySummary,
    pub assessment: ResultAssessment,
}

/// Terminal run of a durable or supervised operation. Its schema is chosen by
/// the command; the run record carries its own format version. The renderer
/// serializes a borrowed record; clients decode the owned default.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunDocument<R = RunRecord> {
    pub schema: u32,
    pub run: R,
}

/// Summary of a query whose records were exported to files in `output`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportDocument<S = QuerySummary, P = std::path::PathBuf> {
    pub schema: u32,
    pub summary: S,
    pub output: P,
}

/// Schema of the `register-accesses` document
/// (`{"schema":1,"inputs":[...],"records":[...],"summary":{...}}`).
pub const REGISTER_ACCESSES_SCHEMA: u32 = 1;

/// One analyzed input of a `register-accesses` document, by position.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAccessInput {
    pub role: String,
    pub sha256: blobray_domain::ArtifactId,
}

/// Every register access of the analyzed inputs; see `REGISTER_ACCESSES_SCHEMA`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAccessDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    pub records: Vec<blobray_domain::RegisterAccess>,
    pub summary: blobray_domain::RegisterAccessSummary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventorySnapshot {
    pub revision_id: RevisionId,
    pub revision: Revision,
}

/// Captured revision inventory, including its completeness assessment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryDocument {
    pub schema: u32,
    pub assessment: ResultAssessment,
    pub complete: bool,
    pub snapshot: InventorySnapshot,
}
