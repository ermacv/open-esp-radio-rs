//! Test-only helpers for textual-disassembly fixtures.

#[cfg(feature = "esp32s31-harness")]
use std::path::PathBuf;

mod text_disassembly;

pub(crate) use text_disassembly::trace_disassembly;

/// Resolves a private integration-test input only when the caller explicitly
/// supplies it. Unit tests never infer a repository-local oracle directory.
#[cfg(feature = "esp32s31-harness")]
pub(crate) fn private_input(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable).map(PathBuf::from)
}
