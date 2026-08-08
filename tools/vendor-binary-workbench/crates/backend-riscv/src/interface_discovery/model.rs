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
    AbsoluteAddress {
        address: u32,
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
            Self::FunctionArgument { index } => format!("arg{index}"),
            Self::AbsoluteAddress { address } => format!("{address:#010x}"),
        }
    }

    pub const fn addressing(&self) -> Option<InterfaceSymbolAddressing> {
        match self {
            Self::RelocatedSymbol { addressing, .. } => Some(*addressing),
            Self::FunctionArgument { .. } | Self::AbsoluteAddress { .. } => None,
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
