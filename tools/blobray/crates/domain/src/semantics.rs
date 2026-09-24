//! Typed local semantics. No machine state, memory environment or repository access.
use crate::*;

/// RV32 operations; register operands are always in 0..32.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operand {
    Register(u8),
    Immediate(u32),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
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
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AbstractValue {
    Unknown,
    Alternatives {
        values: ValueAlternatives,
    },
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
/// Maximum exact alternatives retained by the finite-value profile.
pub const MAX_VALUE_ALTERNATIVES: usize = 8;
/// Nonrecursive exact leaves. Alternatives never contain expressions or other sets.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ValueAlternative {
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
impl ValueAlternative {
    pub fn as_value(&self) -> AbstractValue {
        match self {
            Self::Constant { value } => AbstractValue::Constant { value: *value },
            Self::ImageAddress { address } => AbstractValue::ImageAddress { address: *address },
            Self::ScopedAddress {
                source,
                object,
                address,
            } => AbstractValue::ScopedAddress {
                source: source.clone(),
                object: object.clone(),
                address: *address,
            },
            Self::Section { section, offset } => AbstractValue::Section {
                section: *section,
                offset: *offset,
            },
            Self::Symbol { symbol, addend } => AbstractValue::Symbol {
                symbol: symbol.clone(),
                addend: *addend,
            },
            Self::EntryStack { offset } => AbstractValue::EntryStack { offset: *offset },
        }
    }
    pub fn from_value(value: &AbstractValue) -> Option<Self> {
        Some(match value {
            AbstractValue::Constant { value } => Self::Constant { value: *value },
            AbstractValue::ImageAddress { address } => Self::ImageAddress { address: *address },
            AbstractValue::ScopedAddress {
                source,
                object,
                address,
            } => Self::ScopedAddress {
                source: source.clone(),
                object: object.clone(),
                address: *address,
            },
            AbstractValue::Section { section, offset } => Self::Section {
                section: *section,
                offset: *offset,
            },
            AbstractValue::Symbol { symbol, addend } => Self::Symbol {
                symbol: symbol.clone(),
                addend: *addend,
            },
            AbstractValue::EntryStack { offset } => Self::EntryStack { offset: *offset },
            _ => return None,
        })
    }
}
/// Canonical, bounded set; serde rejects oversized, singleton and noncanonical sets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Hash)]
#[serde(transparent)]
pub struct ValueAlternatives(Vec<ValueAlternative>);
impl ValueAlternatives {
    pub fn new(mut values: Vec<ValueAlternative>) -> Result<Self> {
        if values.len() > MAX_VALUE_ALTERNATIVES {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "too many value alternatives",
            ));
        }
        values.sort_unstable();
        values.dedup();
        if values.len() < 2 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "alternatives require at least two distinct values",
            ));
        }
        Ok(Self(values))
    }
    pub fn values(&self) -> &[ValueAlternative] {
        &self.0
    }
    pub fn allocated_bytes(&self) -> u64 {
        (self.0.capacity() * std::mem::size_of::<ValueAlternative>()) as u64
            + self
                .0
                .iter()
                .map(|v| match v {
                    ValueAlternative::ScopedAddress { source, object, .. } => {
                        source.allocated_bytes() + object.artifact.allocated_bytes()
                    }
                    ValueAlternative::Symbol { symbol, .. } => {
                        symbol.object.artifact.allocated_bytes()
                    }
                    _ => 0,
                })
                .sum::<u64>()
    }
}
impl<'de> Deserialize<'de> for ValueAlternatives {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ValueAlternatives;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("2..=8 sorted distinct exact alternatives")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::with_capacity(MAX_VALUE_ALTERNATIVES);
                while let Some(value) = seq.next_element::<ValueAlternative>()? {
                    if values.len() == MAX_VALUE_ALTERNATIVES
                        || values.last().is_some_and(|v| v >= &value)
                    {
                        return Err(serde::de::Error::custom(
                            "oversized or noncanonical alternatives",
                        ));
                    }
                    values.push(value);
                }
                if values.len() < 2 {
                    return Err(serde::de::Error::custom("alternatives require two values"));
                }
                Ok(ValueAlternatives(values))
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}
impl AbstractValue {
    /// Membership is a may-match, never a proof that this address is selected at runtime.
    pub fn contains_address(&self, expected: u32) -> bool {
        match self {
            Self::Constant { value } | Self::ImageAddress { address: value } => *value == expected,
            Self::Alternatives { values } => values.values().iter().any(|v| matches!(v, ValueAlternative::Constant { value } | ValueAlternative::ImageAddress { address: value } if *value == expected)),
            _ => false,
        }
    }
    pub fn contains_symbol(&self, expected: &SymbolId) -> bool {
        match self {
            Self::Symbol { symbol, .. } => symbol == expected,
            Self::Alternatives { values } => values.values().iter().any(
                |v| matches!(v, ValueAlternative::Symbol { symbol, .. } if symbol == expected),
            ),
            _ => false,
        }
    }
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
    AlternativeLimit,
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Hash)]
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
