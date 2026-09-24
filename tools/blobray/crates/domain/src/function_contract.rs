//! Conditional reviewed function and argument-context interpretations.
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextAccess {
    Read,
    Write,
    ReadWrite,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextField {
    pub offset: i32,
    pub width: u8,
    pub name: String,
    pub value_type: Option<AbiValueType>,
    /// Declared role, not a measured access or an execution permission.
    pub access: ContextAccess,
    pub role: Option<SubjectId>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgumentContext {
    pub argument: u8,
    pub name: String,
    /// Byte extent relative to the argument pointer; negative prefixes are explicit.
    pub start: i32,
    pub length: u32,
    pub fields: Vec<ContextField>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FunctionPrecondition {
    /// Inclusive unsigned raw-bit-pattern bounds, even for a signed ABI type.
    ArgumentRange {
        argument: u8,
        min: u64,
        max: u64,
        reason: String,
    },
    ArgumentBits {
        argument: u8,
        mask: u64,
        value: u64,
        reason: String,
    },
    /// Little-endian bytes within an explicitly declared argument context.
    ContextBits {
        argument: u8,
        offset: i32,
        width: u8,
        mask: u64,
        value: u64,
        reason: String,
    },
    /// Attributed text; no evaluator or implicit truth value.
    Assumption {
        id: SubjectId,
        statement: String,
        reason: String,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionContract {
    pub selector: FunctionSelector,
    pub abi: CallAbi,
    pub signature: Option<CallSignature>,
    pub name: String,
    pub role: Option<SubjectId>,
    pub return_role: Option<SubjectId>,
    pub summary: String,
    pub contexts: Vec<ArgumentContext>,
    pub preconditions: Vec<FunctionPrecondition>,
    pub applicability: String,
}
impl FunctionContract {
    /// Variable capacities; the owner separately accounts for the boxed contract.
    pub fn allocated_bytes(&self) -> u64 {
        let mut bytes = self.selector.object().artifact.allocated_bytes()
            + self
                .signature
                .as_ref()
                .map_or(0, CallSignature::allocated_bytes)
            + self.role.as_ref().map_or(0, SubjectId::allocated_bytes)
            + self
                .return_role
                .as_ref()
                .map_or(0, SubjectId::allocated_bytes)
            + (self.name.capacity()
                + self.summary.capacity()
                + self.applicability.capacity()
                + self.contexts.capacity() * std::mem::size_of::<ArgumentContext>()
                + self.preconditions.capacity() * std::mem::size_of::<FunctionPrecondition>())
                as u64;
        for context in &self.contexts {
            bytes += (context.name.capacity()
                + context.fields.capacity() * std::mem::size_of::<ContextField>())
                as u64;
            for field in &context.fields {
                bytes += field.name.capacity() as u64
                    + field.role.as_ref().map_or(0, SubjectId::allocated_bytes);
            }
        }
        for predicate in &self.preconditions {
            bytes += match predicate {
                FunctionPrecondition::ArgumentRange { reason, .. }
                | FunctionPrecondition::ArgumentBits { reason, .. }
                | FunctionPrecondition::ContextBits { reason, .. } => reason.capacity() as u64,
                FunctionPrecondition::Assumption {
                    id,
                    statement,
                    reason,
                } => id.allocated_bytes() + (statement.capacity() + reason.capacity()) as u64,
            };
        }
        bytes
    }
}
