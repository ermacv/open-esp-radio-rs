//! Typed local semantics. No machine state, memory environment or repository access.
use crate::*;

/// RV32 operations; register operands are always in 0..32.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operand {
    Register(u8),
    Immediate(u32),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntegerOp {
    Add,
    Sub,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Sar,
    Lt,
    Ltu,
    Mul,
    Mulh,
    Mulhsu,
    Mulhu,
    Div,
    Divu,
    Rem,
    Remu,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryKind {
    Load,
    Store,
    LoadReserved,
    StoreConditional,
    Atomic,
}
/// An instruction's bounded local effect, before abstract interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticOp {
    Integer {
        op: IntegerOp,
        dest: u8,
        left: Operand,
        right: Operand,
    },
    Upper {
        dest: u8,
        value: u32,
        pc_relative: bool,
    },
    Link {
        dest: u8,
    },
    Memory {
        kind: MemoryKind,
        base: u8,
        displacement: i32,
        width: u8,
        dest: Option<u8>,
        source: Option<u8>,
        swap: bool,
        signed: bool,
    },
    None,
    Unsupported,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueRelocation {
    Ignore,
    UpperAbsolute,
    UpperPcRelative,
    CallUpper,
    LowerAbsolute,
    LowerPcRelative,
    Unsupported,
}
/// Extends structural decoding without adding filesystem or execution authority.
pub trait FunctionSemantics: FunctionDecoder {
    fn branch(&self, _bytes: &[u8]) -> Option<(BranchTest, Operand, Operand)> {
        None
    }
    fn semantic_identity(&self) -> &'static str;
    fn lift(&self, bytes: &[u8]) -> SemanticOp;
    fn value_relocation(
        &self,
        relocation: &FunctionRelocation,
        operation: SemanticOp,
    ) -> ValueRelocation;
}
/// Values are relative to this recipe's input/object unless an exact symbol is supplied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AbstractValue {
    Unknown,
    Expression {
        id: u32,
    },
    Constant {
        value: u32,
    },
    ImageAddress {
        address: u32,
    },
    ScopedAddress {
        source: FunctionSource,
        object: ObjectId,
        address: u32,
    },
    Section {
        section: u32,
        offset: i64,
    },
    Symbol {
        symbol: SymbolId,
        addend: i64,
    },
    EntryStack {
        offset: i64,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelocationSite {
    pub section: u32,
    pub index: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticGapReason {
    UnsupportedInstruction,
    OpaqueCall,
    ConflictingBoundary,
    UnresolvedRelocation,
    UnexpandedControlFlow,
}
/// None in an old manifest means analysis was not performed, not zero coverage.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticSummary {
    pub complete: bool,
    pub values: u64,
    pub known_values: u64,
    pub accesses: u64,
    pub known_addresses: u64,
    pub gaps: u64,
}

/// Explicit assumption about integer calling conventions; not inferred from ELF flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallAbi {
    RiscvInteger,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchOptions {
    #[serde(default)]
    pub companions: Vec<PublicationId>,
    pub publication: PublicationId,
    pub abi: Option<CallAbi>,
    pub knowledge: Option<KnowledgeRevisionId>,
}
/// Flat DAG. References must point to earlier records in the same function.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Expression {
    EntryRegister {
        register: u8,
    },
    Integer {
        op: IntegerOp,
        left: AbstractValue,
        right: AbstractValue,
    },
    Load {
        address: AbstractValue,
        width: u8,
        signed: bool,
    },
    CallResult {
        callsite: u64,
        register: u8,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioField {
    pub name: String,
    pub lsb: u8,
    pub width: u8,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioRegister {
    pub name: String,
    pub address: u32,
    pub width: u8,
    pub fields: Vec<MmioField>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BranchTest {
    Eq,
    Ne,
    Lt,
    Ge,
    Ltu,
    Geu,
}

/// Reviewed classification of a physical MMIO interval; never supplies load values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioRegion {
    pub name: String,
    pub range: ImageRegion,
}
