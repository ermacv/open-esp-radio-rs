//! Reviewed interface declarations. These are conditional contracts, not runtime facts.
use crate::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceIndexDomain {
    pub word: u8,
    /// Inclusive bounds are declared preconditions, not inferred runtime values.
    pub min: u32,
    pub max: u32,
    pub reason: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InterfaceGuard {
    CapturedPayload {
        payload: ArtifactId,
    },
    /// Relative to the final table address; requires runtime evidence before use.
    RuntimeValue {
        offset: u32,
        width: u8,
        mask: u32,
        value: u32,
        purpose: String,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceSlot {
    pub offset: u32,
    pub name: String,
    pub signature: Option<CallSignature>,
    /// Reviewed semantic identity. It does not select an execution model or callee.
    pub semantic: Option<SubjectId>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceContract {
    pub root: AccessRoot,
    pub path: Vec<AccessStep>,
    pub layout_version: String,
    pub layout_bytes: u32,
    pub pointer_bytes: u8,
    pub abi: CallAbi,
    pub index_domains: Vec<InterfaceIndexDomain>,
    pub guards: Vec<InterfaceGuard>,
    pub slots: Vec<InterfaceSlot>,
    pub purpose: String,
    pub applicability: String,
}
impl InterfaceContract {
    /// Variable storage of this declaration; the owning claim accounts for its Box.
    pub fn allocated_bytes(&self) -> u64 {
        let mut bytes = (self.path.capacity() * std::mem::size_of::<AccessStep>()
            + self.index_domains.capacity() * std::mem::size_of::<InterfaceIndexDomain>()
            + self.guards.capacity() * std::mem::size_of::<InterfaceGuard>()
            + self.slots.capacity() * std::mem::size_of::<InterfaceSlot>()
            + self.layout_version.capacity()
            + self.purpose.capacity()
            + self.applicability.capacity()) as u64;
        bytes += match &self.root {
            AccessRoot::Symbol { symbol, .. } => symbol.object.artifact.allocated_bytes(),
            AccessRoot::EntryWord { function, .. } => function.object().artifact.allocated_bytes(),
            AccessRoot::Address { .. } | AccessRoot::Section { .. } => 0,
        };
        for domain in &self.index_domains {
            bytes += domain.reason.capacity() as u64;
        }
        for guard in &self.guards {
            bytes += match guard {
                InterfaceGuard::CapturedPayload { payload } => payload.allocated_bytes(),
                InterfaceGuard::RuntimeValue { purpose, .. } => purpose.capacity() as u64,
            };
        }
        for slot in &self.slots {
            bytes += slot.name.capacity() as u64
                + slot.semantic.as_ref().map_or(0, SubjectId::allocated_bytes)
                + (slot
                    .signature
                    .as_ref()
                    .map_or(0, |s| s.arguments.capacity())
                    * std::mem::size_of::<CallArgument>()) as u64;
            if let Some(signature) = &slot.signature {
                for arg in &signature.arguments {
                    bytes += arg.role.allocated_bytes();
                }
            }
        }
        bytes
    }
}
