//! Shared reviewed ABI values for direct functions and interface slots.
use crate::*;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AbiValueType {
    Void,
    Integer { bits: u8, signed: bool },
    Pointer { nullable: bool },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallArgument {
    pub role: SubjectId,
    pub value_type: AbiValueType,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallSignature {
    pub arguments: Vec<CallArgument>,
    pub result: AbiValueType,
    pub variadic: bool,
}

impl CallSignature {
    pub fn allocated_bytes(&self) -> u64 {
        (self.arguments.capacity() * std::mem::size_of::<CallArgument>()) as u64
            + self
                .arguments
                .iter()
                .map(|a| a.role.allocated_bytes())
                .sum::<u64>()
    }
}
