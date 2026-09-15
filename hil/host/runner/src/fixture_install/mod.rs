//! Versioned Linux fixture provisioning shared by the runner and root apply owner.

mod admission;
pub mod launcher;
mod model;
mod prepare;
mod transaction;

pub use admission::{OperationalLease, admit_system};
pub use model::{
    Artifact, ArtifactRole, ArtifactState, Bundle, InstallPlan, InstallResult, InstallState,
    Provider, SourceIdentity,
};
pub use prepare::{build_plan, prepare};
pub use transaction::apply_system;

pub const INSTALLER_BINARY: &str = "open-radio-fixture-install";
pub const BUNDLE_SCHEMA: u32 = 2;
pub const RECEIPT_SCHEMA: u32 = 1;

#[cfg(test)]
mod tests;
