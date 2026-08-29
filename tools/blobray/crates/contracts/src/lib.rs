//! Architecture-neutral contracts supplied by an optional knowledge provider.
//!
//! The analysis layers carry opaque references to these immutable specs. They
//! does not know which chip, SDK revision, or runtime lifecycle produced them.

#![forbid(unsafe_code)]

use std::cmp::Ordering;

mod fact;
mod identity;

pub use fact::{
    Applicability, ApplicabilityContext, EffectiveFactMetadata, EvidenceReference,
    FactClassification, FactValidationError, RecordMetadata,
};
pub use identity::{
    ArtifactIdentity, EntityDomain, IdentityError, RevisionOccurrenceId, SemanticEntityId,
    SemanticPath,
};

/// Origin of one asserted fact. A hint is navigation metadata only and must
/// never be promoted to a reviewed hardware meaning by generic analysis.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum FactProvenance {
    Observed,
    Derived,
    Imported,
    Hint,
    Reviewed,
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum FactAccuracy {
    Exact,
    Bounded,
    Approximate,
    Unknown,
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum FactCompleteness {
    Complete,
    Partial,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ExternalReturnModel {
    /// The call itself is the modeled observable effect and has no ABI return
    /// value. Consumers must not invent a result merely to keep analysis
    /// moving.
    Void,
    Constant(u32),
    SymbolicU32,
    /// Two-word RV32 ABI result in `a0` (low) and `a1` (high).
    SymbolicU64,
    /// A fresh allocation whose size is supplied in `aN`. The bytes remain
    /// uninitialized: analysis may use the successful result as a writable
    /// memory object, but must not invent their contents or an alias to
    /// another object.
    Allocated {
        size_argument: u8,
    },
    /// A fresh, non-null allocation whose first `aN` bytes are initialized to
    /// zero. The size argument is part of the reviewed ABI model; allocation
    /// identity and lifetime remain explicit in analysis and scenarios.
    AllocatedZeroed {
        size_argument: u8,
    },
    /// A fresh non-null pointer to an opaque runtime-owned object. This
    /// preserves identity and non-nullness without claiming initialized bytes.
    OpaquePointer,
    /// The ABI identity and human semantics are known, but observable effects
    /// and return propagation are not modeled for validation.
    Unmodeled,
}

/// One independently modeled write through a call argument.
///
/// Each variant states the memory-provenance boundary the backend must prove
/// before it may apply the write.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ExternalOutputModel {
    PrivateStack {
        pointer_argument: u8,
        width: u8,
    },
    /// Write one little-endian word through an RV32 pointer argument at a
    /// fixed byte offset. The backend accepts only addresses with proven
    /// writable normal-memory provenance; MMIO and opaque integers fail
    /// closed.
    Memory {
        pointer_argument: u8,
        byte_offset: u16,
        width: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalArgumentDirection {
    Input,
    Output,
    InputOutput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalArgumentSpec {
    pub name: &'static str,
    pub c_type: &'static str,
    pub direction: ExternalArgumentDirection,
}

/// Mechanism-neutral role assigned to one named semantic argument.
///
/// Roles are declarative navigation metadata. They do not model the memory or
/// scheduler effects of the argument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticArgumentRoleSpec {
    pub role: &'static str,
    pub argument: &'static str,
}

/// Reviewed high-level event-dispatch view of a semantic operation.
///
/// The mechanism and execution-context names remain opaque to generic analysis. A receiver
/// may be named only when the platform contract reviewed that relationship;
/// `None` prevents the linked layer from guessing one from pointer values or
/// symbol spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventDispatchSemanticSpec {
    pub mechanism: &'static str,
    pub execution_context: &'static str,
    pub receiver: Option<&'static str>,
    pub argument_roles: &'static [SemanticArgumentRoleSpec],
}

/// Reviewed meaning attached to one external ABI slot.
///
/// Operation and replacement names are deliberately opaque strings. Generic analysis can
/// carry platform knowledge without acquiring an RTOS, NVS or chip-specific
/// semantic enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalSemanticSpec {
    pub operation: &'static str,
    pub arguments: &'static [ExternalArgumentSpec],
    pub return_type: &'static str,
    pub replacement: Option<&'static str>,
    pub event_dispatch: Option<EventDispatchSemanticSpec>,
}

/// Whether generic analysis may descend into a function body after applying
/// its reviewed semantic contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticFunctionBodyPolicy {
    /// The ABI and declared effects are the complete analysis boundary. The
    /// implementation bytes are not a target of transitive analysis.
    OpaqueBoundary,
    /// The contract annotates the call, but the implementation remains part
    /// of transitive analysis.
    AnalyzeBody,
}

impl SemanticFunctionBodyPolicy {
    pub const fn label(self) -> &'static str {
        match self {
            Self::OpaqueBoundary => "opaque-boundary",
            Self::AnalyzeBody => "analyze-body",
        }
    }
}

/// Reviewed meaning attached to a directly linked vendor function.
///
/// The platform semantic harness is responsible for returning this spec only
/// after it has matched the exact artifact identity it reviewed. The contract carries
/// the opaque operation vocabulary and typed ABI without knowing a chip,
/// vendor library or instruction set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectSemanticFunctionSpec {
    pub id: &'static str,
    /// Owning add-on or reviewed-knowledge layer. Generic analysis carries
    /// this provenance verbatim and must not relabel it as generic truth.
    pub source: &'static str,
    pub c_name: &'static str,
    pub argument_count: u8,
    pub body_policy: SemanticFunctionBodyPolicy,
    /// Executable result model for an external boundary. Internal reviewed
    /// summaries use `Unmodeled` because their body, not the ABI boundary,
    /// determines the result.
    pub return_model: ExternalReturnModel,
    pub semantic: ExternalSemanticSpec,
    pub evidence: &'static str,
}

/// Executable behavior supplied by a compiled knowledge provider.
///
/// Layout, slot offsets, names, ABI types and semantic annotations belong to
/// the reviewed interface pack.  The model ID is only a foreign-key target
/// for an explicitly reviewed slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalCallModelSpec {
    pub id: &'static str,
    pub return_model: ExternalReturnModel,
    pub outputs: &'static [ExternalOutputModel],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalCallModelSetSpec {
    pub id: &'static str,
    pub models: &'static [ExternalCallModelSpec],
}

#[derive(Clone, Copy, Debug)]
pub struct ExternalCallModelSetRef(&'static ExternalCallModelSetSpec);

impl ExternalCallModelSetRef {
    pub const fn new(spec: &'static ExternalCallModelSetSpec) -> Self {
        Self(spec)
    }

    pub const fn spec(self) -> &'static ExternalCallModelSetSpec {
        self.0
    }

    pub fn model(self, id: &str) -> Option<ExternalCallModelRef> {
        self.0
            .models
            .iter()
            .find(|model| model.id == id)
            .map(ExternalCallModelRef::new)
    }
}

impl PartialEq for ExternalCallModelSetRef {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for ExternalCallModelSetRef {}

impl PartialOrd for ExternalCallModelSetRef {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ExternalCallModelSetRef {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.id.cmp(other.0.id)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExternalCallModelRef(&'static ExternalCallModelSpec);

impl ExternalCallModelRef {
    pub const fn new(spec: &'static ExternalCallModelSpec) -> Self {
        Self(spec)
    }

    pub const fn spec(self) -> &'static ExternalCallModelSpec {
        self.0
    }
}

impl PartialEq for ExternalCallModelRef {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for ExternalCallModelRef {}

impl PartialOrd for ExternalCallModelRef {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ExternalCallModelRef {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.id.cmp(other.0.id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionTarget {
    Address(u32),
    Symbol(&'static str),
    /// A function supplied by another project source.
    ///
    /// The source qualifier is identity/provenance, not linker precedence.
    /// The selected entry contract still has to resolve `symbol` to one exact
    /// linked address. This is used for runtime-patched ROM tables whose slot
    /// points at code from a static library.
    SourceSymbol {
        source: &'static str,
        symbol: &'static str,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionTableSpec {
    pub id: &'static str,
    pub targets: &'static [FunctionTarget],
}

#[derive(Clone, Copy, Debug)]
pub struct FunctionTableRef(&'static FunctionTableSpec);

impl FunctionTableRef {
    pub const fn new(spec: &'static FunctionTableSpec) -> Self {
        Self(spec)
    }

    pub const fn id(self) -> &'static str {
        self.0.id
    }

    pub fn targets(self) -> impl Iterator<Item = (u32, FunctionTarget)> {
        self.0
            .targets
            .iter()
            .copied()
            .enumerate()
            .map(|(index, target)| (index as u32 * 4, target))
    }
}

impl PartialEq for FunctionTableRef {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for FunctionTableRef {}

impl PartialOrd for FunctionTableRef {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FunctionTableRef {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.id.cmp(other.0.id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataPointerBinding {
    pub pointer_symbol: &'static str,
    pub target_symbol: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntryContractSpec {
    pub id: &'static str,
    pub function_table: Option<FunctionTableRef>,
    pub pointer_symbols: &'static [&'static str],
    pub data_pointer_binding: Option<DataPointerBinding>,
}

#[derive(Clone, Copy, Debug)]
pub struct EntryContractRef(&'static EntryContractSpec);

impl EntryContractRef {
    pub const fn new(spec: &'static EntryContractSpec) -> Self {
        Self(spec)
    }

    pub const fn spec(self) -> &'static EntryContractSpec {
        self.0
    }

    pub const fn id(self) -> &'static str {
        self.0.id
    }
}

impl PartialEq for EntryContractRef {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for EntryContractRef {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticCallSpec {
    pub symbol: &'static str,
    pub argument_count: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KnowledgeContractSpec {
    pub external_call_model_sets: &'static [ExternalCallModelSetRef],
    pub entry_contracts: &'static [EntryContractRef],
    pub diagnostic_calls: &'static [DiagnosticCallSpec],
}

impl KnowledgeContractSpec {
    pub fn entry_contract(self, id: &str) -> Option<EntryContractRef> {
        self.entry_contracts
            .iter()
            .copied()
            .find(|contract| contract.id() == id)
    }
}
