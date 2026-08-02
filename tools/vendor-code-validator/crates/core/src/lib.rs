//! Architecture-neutral contracts supplied by a platform harness.
//!
//! The analysis core carries opaque references to these immutable specs.  It
//! does not know which chip, SDK revision, or runtime lifecycle produced them.

use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalReturnModel {
    Constant(u32),
    SymbolicU32,
    PrivateStackOutputU8 { pointer_argument: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalFunctionSpec {
    pub id: &'static str,
    pub offset: u32,
    pub c_name: &'static str,
    pub argument_count: u8,
    pub return_model: ExternalReturnModel,
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
