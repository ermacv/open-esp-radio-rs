//! Architecture-neutral contracts supplied by a platform harness.
//!
//! The analysis layers carry opaque references to these immutable specs. They
//! does not know which chip, SDK revision, or runtime lifecycle produced them.

use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalReturnModel {
    Constant(u32),
    SymbolicU32,
    PrivateStackOutputU8 {
        pointer_argument: u8,
    },
    /// The ABI identity and human semantics are known, but observable effects
    /// and return propagation are not modeled for validation.
    Unmodeled,
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

/// Reviewed meaning attached to a directly linked vendor function.
///
/// The platform semantic harness is responsible for returning this spec only
/// after it has matched the exact artifact identity it reviewed. The contract carries
/// the opaque operation vocabulary and typed ABI without knowing a chip,
/// vendor library or instruction set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectSemanticFunctionSpec {
    pub id: &'static str,
    pub c_name: &'static str,
    pub argument_count: u8,
    pub semantic: ExternalSemanticSpec,
    pub evidence: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalFunctionSpec {
    pub id: &'static str,
    pub offset: u32,
    pub c_name: &'static str,
    pub argument_count: u8,
    pub return_model: ExternalReturnModel,
    pub semantic: ExternalSemanticSpec,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalTableSpec {
    pub id: &'static str,
    pub pointer_symbol: &'static str,
    pub backing_symbol: &'static str,
    pub version: u32,
    pub magic: u32,
    pub size: u32,
    pub magic_offset: u32,
    pub functions: &'static [ExternalFunctionSpec],
}

#[derive(Clone, Copy, Debug)]
pub struct ExternalTableRef(&'static ExternalTableSpec);

impl ExternalTableRef {
    pub const fn new(spec: &'static ExternalTableSpec) -> Self {
        Self(spec)
    }

    pub const fn spec(self) -> &'static ExternalTableSpec {
        self.0
    }

    pub fn function_at(self, offset: u32) -> Option<ExternalFunctionRef> {
        self.0
            .functions
            .iter()
            .find(|function| function.offset == offset)
            .map(ExternalFunctionRef::new)
    }
}

impl PartialEq for ExternalTableRef {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for ExternalTableRef {}

impl PartialOrd for ExternalTableRef {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ExternalTableRef {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.id.cmp(other.0.id)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExternalFunctionRef(&'static ExternalFunctionSpec);

impl ExternalFunctionRef {
    pub const fn new(spec: &'static ExternalFunctionSpec) -> Self {
        Self(spec)
    }

    pub const fn spec(self) -> &'static ExternalFunctionSpec {
        self.0
    }
}

impl PartialEq for ExternalFunctionRef {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for ExternalFunctionRef {}

impl PartialOrd for ExternalFunctionRef {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ExternalFunctionRef {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.id.cmp(other.0.id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionTarget {
    Address(u32),
    Symbol(&'static str),
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
pub struct HarnessContractSpec {
    pub external_tables: &'static [ExternalTableRef],
    pub entry_contracts: &'static [EntryContractRef],
    pub diagnostic_calls: &'static [DiagnosticCallSpec],
}

impl HarnessContractSpec {
    pub fn entry_contract(self, id: &str) -> Option<EntryContractRef> {
        self.entry_contracts
            .iter()
            .copied()
            .find(|contract| contract.id() == id)
    }
}
