//! Test-only compatibility helpers for legacy textual-disassembly fixtures.

use std::path::PathBuf;

mod legacy_disassembly;

pub(crate) use legacy_disassembly::trace_disassembly;

/// Resolves a private integration-test input only when the caller explicitly
/// supplies it. Unit tests never infer a repository-local oracle directory.
pub(crate) fn private_input(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable).map(PathBuf::from)
}
