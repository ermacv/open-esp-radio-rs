//! Exact data observations and reviewed integer interpretations. No runtime-memory claim.
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
    /// A defined static symbol; a zero-sized symbol requires an explicit length.
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
    pub occurrence: KnowledgeOccurrence,
    pub ranges: Vec<DataSelector>,
    /// Retained analyses from the same object. Records preserve unknown calls and gaps.
    pub analyses: Vec<FunctionAnalysisId>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DataByteOrder {
    Little,
    Big,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerEncoding {
    pub width: u8,
    pub signed: bool,
    pub byte_order: DataByteOrder,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerTable {
    pub encoding: IntegerEncoding,
    pub count: u64,
    pub stride: u64,
}
impl IntegerTable {
    pub fn byte_length(&self) -> Option<u64> {
        if !matches!(self.encoding.width, 1 | 2 | 4 | 8)
            || self.count == 0
            || self.stride < u64::from(self.encoding.width)
        {
            return None;
        }
        (self.count - 1)
            .checked_mul(self.stride)?
            .checked_add(u64::from(self.encoding.width))
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ConstantOperand {
    Value,
    WriteValue,
    Address,
    CallArgument { index: u8 },
    ReturnLow,
    ReturnHigh,
}
impl ConstantOperand {
    pub fn select<'a>(&self, r: &'a FunctionRecord) -> Option<&'a AbstractValue> {
        match (self, r) {
            (Self::Value, FunctionRecord::Value { value, .. }) => Some(value),
            (Self::WriteValue, FunctionRecord::MemoryAccess { value, .. }) => value.as_ref(),
            (Self::Address, FunctionRecord::MemoryAccess { address, .. }) => Some(address),
            (Self::CallArgument { index }, FunctionRecord::CallInputs { registers, .. }) => {
                registers.get(usize::from(*index))
            }
            (Self::ReturnLow, FunctionRecord::ReturnValue { low, .. }) => Some(low),
            (Self::ReturnHigh, FunctionRecord::ReturnValue { high, .. }) => Some(high),
            _ => None,
        }
    }
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
    pub knowledge: Option<KnowledgeRevisionId>,
    pub accepted: Option<KnowledgeEntry>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DataRecord {
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
    Integer {
        index: u64,
        offset: u64,
        bits: u64,
        signed: Option<i64>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataProposalRequest {
    pub analyses: Vec<FunctionAnalysisId>,
    pub occurrence: KnowledgeOccurrence,
    pub subject: SubjectId,
    pub selector: DataSelector,
    pub layout: IntegerTable,
    pub purpose: String,
    pub applicability: String,
    pub expected_base: Option<KnowledgeRevisionId>,
    pub actor: String,
    pub reason: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstantProposalRequest {
    pub analysis: FunctionAnalysisId,
    pub record: u64,
    pub operand: ConstantOperand,
    pub value: u32,
    pub subject: SubjectId,
    pub purpose: String,
    pub applicability: String,
    pub expected_base: Option<KnowledgeRevisionId>,
    pub actor: String,
    pub reason: String,
}
