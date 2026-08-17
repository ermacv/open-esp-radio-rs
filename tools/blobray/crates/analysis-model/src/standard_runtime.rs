//! Architecture-neutral identities for standardized runtime boundaries.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandardMemoryFunction {
    Copy,
    Move,
    Set,
}

impl StandardMemoryFunction {
    pub const fn operation(self) -> &'static str {
        match self {
            Self::Copy => "memory.copy",
            Self::Move => "memory.move",
            Self::Set => "memory.set",
        }
    }

    pub const fn contract_id(self) -> &'static str {
        match self {
            Self::Copy => "standard.memcpy",
            Self::Move => "standard.memmove",
            Self::Set => "standard.memset",
        }
    }
}
