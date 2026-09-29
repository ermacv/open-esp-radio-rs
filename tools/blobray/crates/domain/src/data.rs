//! Exact data observations. No runtime-memory claim.
use crate::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DataSelector {
    /// Explicit section-relative bytes, including data without a sized symbol.
    Section {
        section: u32,
        offset: u64,
        length: u64,
    },
    /// A defined physical static or dynamic symbol; a zero-sized symbol requires an explicit length.
    Symbol {
        symbol: SymbolId,
        length: Option<u64>,
    },
    /// Virtual address in an executable ELF, never an object/file offset.
    Image { address: u64, length: u64 },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataRequest {
    pub occurrence: Occurrence,
    pub ranges: Vec<DataSelector>,
    /// Retained analyses from the same object. Records preserve unknown calls and gaps.
    pub analyses: Vec<FunctionAnalysisId>,
    /// Explicit pointer observation profile, applied to exactly one range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer_table: Option<PointerTable>,
}
/// Captured little-endian RV32 absolute-pointer slots; no dynamic loader or ABI inference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerTable {
    pub count: u64,
    pub stride: u64,
}
impl PointerTable {
    pub fn byte_length(&self) -> Option<u64> {
        if self.count == 0 || self.stride < 4 {
            return None;
        }
        (self.count - 1).checked_mul(self.stride)?.checked_add(4)
    }
}
/// Backend interpretation, distinct from the structural width reported by ELF.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerRelocation {
    None,
    Absolute32,
    Unsupported { width: Option<u8> },
}
pub trait PointerDecoder {
    fn pointer_identity(&self) -> Option<&'static str> {
        None
    }
    fn pointer_relocation(&self, _relocation: &FunctionRelocation) -> PointerRelocation {
        PointerRelocation::Unsupported { width: None }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PointerIssue {
    UnknownWriteExtent,
    UnsupportedRelocation,
    OverlappingRelocations,
    PartialSlotWrite,
    ImplicitAddend,
    ArithmeticOverflow,
    UnsupportedSymbol,
    LinkedValueMismatch,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PointerValue {
    Null,
    /// Numeric address only; no code boundary, mapping or callee is inferred.
    Address {
        value: u32,
        image_address: bool,
    },
    DefinedSymbol {
        symbol: SymbolId,
        addend: i64,
    },
    ExternalSymbol {
        symbol: SymbolId,
        addend: i64,
    },
    Unresolved {
        issue: PointerIssue,
    },
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerSummary {
    pub entries: u64,
    pub nulls: u64,
    pub addresses: u64,
    pub defined_symbols: u64,
    pub external_symbols: u64,
    pub unresolved: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataSpan {
    pub selector: DataSelector,
    pub section: u32,
    pub section_range: CodeRange,
    pub file_range: CodeRange,
    pub image_address: Option<u64>,
    /// True means captured initialization bytes, not an immutable runtime value.
    pub writable: bool,
    pub digest: ArtifactId,
    pub export_offset: u64,
    /// All relocations in this section are supplied; none are silently applied.
    pub section_relocations: u64,
    /// Known relocation write extents intersecting the selected byte range.
    pub overlapping_relocations: u64,
    /// Section relocations whose write extent the parser cannot establish.
    /// Such relocations prevent numeric interpretation even outside the range.
    pub unknown_relocation_extents: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataManifest {
    pub schema: u32,
    pub request: DataRequest,
    pub payload: ArtifactId,
    pub spans: Vec<DataSpan>,
    pub analyses: Vec<FunctionManifest>,
    pub pointer_producer: Option<String>,
    pub pointers: Option<PointerSummary>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DataRecord {
    Pointer {
        index: u64,
        offset: u64,
        bits: u32,
        value: PointerValue,
    },
    Unresolved {
        range: u32,
        reason: String,
    },
    Bytes {
        range: u32,
        offset: u64,
        bytes: Vec<u8>,
    },
    Relocation {
        section: u32,
        relocation: FunctionRelocation,
    },
    Analysis {
        analysis: FunctionAnalysisId,
        ordinal: u64,
        record: Box<FunctionRecord>,
        ranges: Vec<u32>,
    },
}

/// A physical object span; absent file backing (e.g. NOBITS) never invents bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataLocation {
    pub selector: DataSelector,
    pub section: u32,
    pub section_range: CodeRange,
    pub file_range: Option<CodeRange>,
    pub image_address: Option<u64>,
    /// ELF section flag only; not a claim about runtime memory permissions.
    pub section_writable: bool,
}
