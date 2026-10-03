//! JSON documents printed by the `blobray` CLI with `--format json`.
//!
//! The renderer emits these shapes and typed clients decode them; payloads are
//! the application/domain records themselves, not copies of their schemas.
use blobray_domain::{
    CheckVerdict, ForbiddenTargetRange, LibraryFunction, RegisterAccess, RegisterAccessSummary,
    TargetAuditRecord, TargetAuditSummary,
};
use oer_riscv_model::{
    ArtifactId, CallAbi, Error, FunctionCoverage, FunctionRecord, SemanticSummary,
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
/// (`{"schema":2,"inputs":[...],"abi":...,"records":[...],"summary":{...}}`).
pub const REGISTER_ACCESSES_SCHEMA: u32 = 2;

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
    /// The calling convention the analysis assumed, if any.
    pub abi: Option<CallAbi>,
    pub records: Vec<RegisterAccess>,
    pub summary: RegisterAccessSummary,
}

/// Schema of the `function-records` document
/// (`{"schema":2,"inputs":[...],"abi":...,"functions":[...],"missing":[...]}`).
pub const FUNCTION_RECORDS_SCHEMA: u32 = 2;
pub const FIELD_ACCESSES_SCHEMA: u32 = 2;
pub const CALLERS_SCHEMA: u32 = 1;

/// Every memory access of the analyzed functions that lands on one field
/// offset, as a field of its root ([`crate::field`]), and the functions no
/// analysis could read.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldAccessesDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    /// The calling convention the analysis assumed, if any.
    pub abi: Option<CallAbi>,
    /// The field's displacement from the last loaded pointer or the root.
    pub offset: i64,
    /// The access width the request selects, if any.
    pub width: Option<u8>,
    /// The functions with at least one such access, in input, object and
    /// symbol order.
    pub functions: Vec<FieldAccessFunction>,
    /// Functions that cannot be analyzed: their accesses are unknown.
    pub blocked: Vec<LibraryFunction>,
    /// Analyzed functions whose coverage or value semantics is incomplete:
    /// some of their accesses may be missing above.
    pub partial: u64,
    /// Code no function covers, whose accesses are unknown too.
    pub gaps: u64,
    /// Accesses of the analyzed functions whose address is not known at
    /// all, so no field selection sees them.
    pub unknown_addresses: u64,
}

/// One function's accesses of the requested field.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldAccessFunction {
    pub function: LibraryFunction,
    pub accesses: Vec<crate::field::FieldAccess>,
}

/// The complete analysis of every function the request names, in input,
/// object and symbol order, and the names no input defines.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionRecordsDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    /// The calling convention the analysis assumed, if any.
    pub abi: Option<CallAbi>,
    pub functions: Vec<NamedFunction>,
    /// Requested names that no analyzed or blocked function carries.
    pub missing: Vec<String>,
}

/// One function a `function-records` request names.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NamedFunction {
    /// Its records: every fact, expression and value the analysis derived.
    Analyzed {
        function: LibraryFunction,
        /// Coverage and value semantics are both complete.
        complete: bool,
        coverage: FunctionCoverage,
        semantics: SemanticSummary,
        records: Vec<FunctionRecord>,
    },
    /// It cannot be analyzed; its behavior is unknown.
    Blocked {
        function: LibraryFunction,
        error: Error,
    },
}

/// Every reference the analyzed functions make to the requested symbols:
/// each is a relocation, as every call, jump or address of another symbol
/// is in a relocatable object.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallersDocument {
    pub schema: u32,
    pub inputs: Vec<RegisterAccessInput>,
    /// The calling convention the analysis assumed, if any.
    pub abi: Option<CallAbi>,
    /// The requested symbol names.
    pub symbols: Vec<String>,
    /// The referencing functions, in input, object and symbol order.
    pub callers: Vec<Caller>,
    /// Functions that cannot be analyzed: their references are unknown.
    pub blocked: Vec<LibraryFunction>,
    /// Code no function covers.
    pub gaps: u64,
}

/// One function's references to the requested symbols.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caller {
    pub function: LibraryFunction,
    pub references: Vec<crate::listing::SymbolReference>,
}
