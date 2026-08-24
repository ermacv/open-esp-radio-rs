//! Architecture-neutral evidence records emitted by the RV32 discovery pass.

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum InterfaceRoot {
    RelocatedSymbol {
        member: Option<String>,
        symbol: String,
        addend: i64,
        addressing: InterfaceSymbolAddressing,
    },
    FunctionArgument {
        index: u8,
    },
    /// Exact linked address proven to lie inside one sized data symbol.
    ///
    /// This preserves the bound that distinguishes an observed static-data
    /// pointer from an arbitrary numeric constant. It does not claim that the
    /// pointed object has been initialized or that a producer has executed.
    BoundedDataAddress {
        member: Option<String>,
        symbol: String,
        symbol_address: u32,
        symbol_size: u32,
        address: u32,
    },
    AbsoluteAddress {
        address: u32,
    },
}

impl InterfaceRoot {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::RelocatedSymbol { .. } => "relocated-symbol",
            Self::FunctionArgument { .. } => "function-argument",
            Self::BoundedDataAddress { .. } => "bounded-data-address",
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
            Self::FunctionArgument { index } => format!("arg{index}"),
            Self::BoundedDataAddress {
                member,
                symbol,
                symbol_address,
                address,
                ..
            } => format!(
                "{}::{symbol}{:+#x}",
                member.as_deref().unwrap_or("<elf>"),
                address.wrapping_sub(*symbol_address)
            ),
            Self::AbsoluteAddress { address } => format!("{address:#010x}"),
        }
    }

    pub const fn addressing(&self) -> Option<InterfaceSymbolAddressing> {
        match self {
            Self::RelocatedSymbol { addressing, .. } => Some(*addressing),
            Self::FunctionArgument { .. }
            | Self::BoundedDataAddress { .. }
            | Self::AbsoluteAddress { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InterfaceLoad {
    pub site: u32,
    pub offset: i32,
    pub width: u8,
    pub selector: Option<InterfaceSlotSelector>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InterfacePointer {
    pub root: InterfaceRoot,
    pub loads: Vec<InterfaceLoad>,
    pub post_offset: i32,
}

/// One statically evidenced pointer store into a table slot or pointer cell.
///
/// Both sides retain provenance. A target may be a relocated function, a
/// function argument supplied by a runtime registration call, or a linked
/// address bounded by a sized static-data symbol. This record says only that
/// the producer can perform the store; it does not claim that the producer
/// has executed, that an argument is executable code, that static data has
/// its initial contents, or that the assignment is the active runtime value.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InterfaceSlotAssignment {
    pub member: Option<String>,
    pub function: String,
    pub function_address: u32,
    pub site: u32,
    pub root: InterfaceRoot,
    pub container_loads: Vec<InterfaceLoad>,
    pub offset: i32,
    pub width: u8,
    pub target: InterfaceRoot,
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum InterfaceArgumentValue {
    Unknown,
    Constant(u32),
    Pointer(InterfacePointer),
}

impl InterfaceArgumentValue {
    pub fn canonical(&self) -> String {
        match self {
            Self::Unknown => "?".to_owned(),
            Self::Constant(value) => format!("{value:#010x}"),
            Self::Pointer(pointer) => pointer.canonical(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InterfaceCallCandidate {
    pub member: Option<String>,
    pub function: String,
    pub function_address: u32,
    pub site: u32,
    pub kind: InterfaceCallKind,
    pub target: InterfacePointer,
    pub jalr_offset: i32,
    pub arguments: Vec<InterfaceArgumentValue>,
}
