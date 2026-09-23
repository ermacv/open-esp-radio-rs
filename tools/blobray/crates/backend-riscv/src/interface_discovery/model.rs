//! Architecture-neutral evidence records emitted by the RV32 discovery pass.

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum InterfaceSymbolAddressing {
    Absolute,
    PcRelative,
    Got,
}

impl InterfaceSymbolAddressing {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Absolute => "absolute",
            Self::PcRelative => "pc-relative",
            Self::Got => "got",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InterfaceRoot {
    RelocatedSymbol {
        reference: crate::SymbolReference,
        member: Option<String>,
        symbol: String,
        addend: i64,
        addressing: InterfaceSymbolAddressing,
    },
    FunctionArgument {
        owner: crate::artifact::CodeIdentity,
        index: u8,
    },
    /// Original numeric base with all range candidates for the observed access.
    /// A containing symbol is an association, not proof of ownership or bounds.
    AbsoluteAddress {
        address: u32,
        data_address: crate::DataAddressResolution,
    },
}

impl InterfaceRoot {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::RelocatedSymbol { .. } => "relocated-symbol",
            Self::FunctionArgument { .. } => "function-argument",
            Self::AbsoluteAddress { .. } => "absolute-address",
        }
    }

    pub fn canonical(&self) -> String {
        match self {
            Self::RelocatedSymbol {
                member,
                symbol,
                addend,
                ..
            } => format!(
                "{}::{symbol}{addend:+#x}",
                member.as_deref().unwrap_or("<elf>")
            ),
            Self::FunctionArgument { index, .. } => format!("arg{index}"),
            Self::AbsoluteAddress {
                address,
                data_address,
            } => format!("{address:#010x} [data-address:{data_address:?}]"),
        }
    }

    pub const fn addressing(&self) -> Option<InterfaceSymbolAddressing> {
        match self {
            Self::RelocatedSymbol { addressing, .. } => Some(*addressing),
            Self::FunctionArgument { .. } | Self::AbsoluteAddress { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceLoad {
    pub site: u32,
    pub offset: i32,
    pub width: u8,
    pub selector: Option<InterfaceSlotSelector>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceSlotSelector {
    pub argument: u8,
    pub scale: u32,
    pub addend: i32,
}

impl InterfaceSlotSelector {
    pub fn canonical(&self) -> String {
        format!("arg{}*{}{:+#x}", self.argument, self.scale, self.addend)
    }

    pub fn selects_offset(&self, offset: i32) -> bool {
        let delta = i64::from(offset) - i64::from(self.addend);
        delta >= 0 && self.scale != 0 && delta % i64::from(self.scale) == 0
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfacePointer {
    pub root: InterfaceRoot,
    pub loads: Vec<InterfaceLoad>,
    pub post_offset: i32,
}

/// One statically observed store with possible pointer provenance.
///
/// Both sides retain provenance. A target may be a relocated function, a
/// function argument supplied by a runtime registration call, or a linked
/// numeric address with explicit range candidates or an unknown association.
/// Numeric values may also be scalars; this record does not prove a pointer
/// type or a valid interface slot. It says only that
/// the producer can perform the store; it does not claim that the producer
/// has executed, that an argument is executable code, that static data has
/// its initial contents, or that the assignment is the active runtime value.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct InterfaceSlotAssignment {
    pub owner: crate::artifact::CodeIdentity,
    pub member: Option<String>,
    pub function: String,
    pub function_address: u32,
    pub site: u32,
    pub root: InterfaceRoot,
    pub container_loads: Vec<InterfaceLoad>,
    pub offset: i32,
    pub width: u8,
    pub target: InterfaceRoot,
    pub target_loads: Vec<InterfaceLoad>,
    pub target_offset: i32,
}

impl InterfacePointer {
    pub fn canonical(&self) -> String {
        let mut value = self.root.canonical();
        for load in &self.loads {
            let selector = load
                .selector
                .as_ref()
                .map(|selector| format!("+{}", selector.canonical()))
                .unwrap_or_default();
            value = format!("load{}({value}{:+#x}{selector})", load.width, load.offset);
        }
        if self.post_offset != 0 {
            value.push_str(&format!("{:+#x}", self.post_offset));
        }
        value
    }

    pub fn slot(&self) -> Option<&InterfaceLoad> {
        self.loads.last()
    }

    pub fn fixed_slot(&self) -> Option<&InterfaceLoad> {
        self.slot().filter(|load| load.selector.is_none())
    }

    pub fn container_loads(&self) -> &[InterfaceLoad] {
        self.loads
            .split_last()
            .map_or(&[], |(_, container)| container)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum InterfaceArgumentValue {
    Unknown,
    Alternatives(Vec<InterfaceArgumentValue>),
    Selector(InterfaceSlotSelector),
    IndexedPointer {
        pointer: InterfacePointer,
        selector: InterfaceSlotSelector,
    },
    GotAddress(InterfacePointer),
    Constant(u32),
    Pointer(InterfacePointer),
}

impl InterfaceArgumentValue {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Constant(_) => "constant",
            Self::Pointer(_) => "pointer-provenance",
            Self::Alternatives(_) => "alternatives",
            Self::Selector(_) => "selector",
            Self::IndexedPointer { .. } => "indexed-pointer",
            Self::GotAddress(_) => "got-address",
        }
    }

    pub fn canonical(&self) -> String {
        match self {
            Self::Unknown => "?".to_owned(),
            Self::Alternatives(values) => format!(
                "alternatives[{}]",
                values
                    .iter()
                    .map(Self::canonical)
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            Self::Selector(selector) => selector.canonical(),
            Self::IndexedPointer { pointer, selector } => {
                format!("{}+{}", pointer.canonical(), selector.canonical())
            }
            Self::GotAddress(pointer) => format!("GOT({})", pointer.canonical()),
            Self::Constant(value) => format!("{value:#010x}"),
            Self::Pointer(pointer) => pointer.canonical(),
        }
    }
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum InterfaceCallKind {
    Call,
    TailJump,
    LinkedJump(u8),
}

impl InterfaceCallKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::TailJump => "tail-jump",
            Self::LinkedJump(_) => "linked-jump",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct InterfaceCallCandidate {
    pub owner: crate::artifact::CodeIdentity,
    pub member: Option<String>,
    pub function: String,
    pub function_address: u32,
    pub site: u32,
    pub kind: InterfaceCallKind,
    pub target: InterfacePointer,
    pub jalr_offset: i32,
    pub arguments: Vec<InterfaceArgumentValue>,
}

/// Limits stop propagation without replacing the retained frontier with unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDiscoveryLimits {
    pub max_state_updates: usize,
    pub max_value_alternatives: usize,
}

impl Default for InterfaceDiscoveryLimits {
    fn default() -> Self {
        Self {
            max_state_updates: 4096,
            max_value_alternatives: 64,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InterfaceGapReason {
    StateUpdateLimit { limit: usize, processed: usize },
    ValueAlternativeLimit { limit: usize, observed: usize },
    UnresolvedCallTarget,
    UnsupportedInstruction,
    UnmodeledValueTransform { registers: Vec<u8> },
    UnresolvedAssignmentLocation,
    UnresolvedAssignmentValue,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceRegisterValue {
    pub register: u8,
    pub value: InterfaceArgumentValue,
}

/// A concrete unresolved use or an unprocessed analysis frontier. The complete
/// captured register state is retained; values are candidates, not path proofs.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceAnalysisGap {
    pub owner: crate::CodeIdentity,
    pub member: Option<String>,
    pub function: String,
    pub site: u32,
    pub reason: InterfaceGapReason,
    pub registers: Vec<InterfaceRegisterValue>,
}
