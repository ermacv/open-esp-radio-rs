//! Provenance of executable adapters, separate from declarative knowledge.
//!
//! This metadata records what a host selected. It is not a reviewed fact,
//! artifact authentication token, or assertion that a model is equivalent to
//! the analyzed program. Implementations must enforce their own applicability.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionModelKind {
    /// Executable interpretation of public runtime/ABI boundary contracts.
    RuntimeSemantics,
    /// Temporary handwritten reconstruction of a present function body.
    ManualReconstruction,
}

impl ExecutionModelKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::RuntimeSemantics => "runtime-semantics",
            Self::ManualReconstruction => "manual-reconstruction",
        }
    }
}

/// One independently versioned executable provider selected by the host.
///
/// `applicability` documents checks/preconditions implemented by its hooks;
/// `evidence` identifies their review source. Registry validation checks only
/// metadata completeness and cache revision coupling, never evidence truth.
#[derive(Clone, Copy, Debug)]
pub struct ExecutionModelProviderSpec {
    pub id: &'static str,
    pub revision: u32,
    pub kind: ExecutionModelKind,
    pub applicability: &'static str,
    pub evidence: &'static str,
}
