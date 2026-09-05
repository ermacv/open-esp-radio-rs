//! Failures of the host/target exchange, distinct from scenario assertions.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ErrorKind {
    Transport,
    Protocol,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct LinkError {
    pub(crate) kind: ErrorKind,
    message: String,
}

impl LinkError {
    pub(super) fn transport(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Transport,
            message: message.into(),
        }
    }

    pub(super) fn protocol(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Protocol,
            message: message.into(),
        }
    }
}

impl fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LinkError {}

/// Both causes remain available when teardown also fails. The primary cause
/// determines the scenario outcome; the secondary cause stays in the report.
#[derive(Debug)]
pub(super) struct FinalizationError {
    pub(super) primary: Box<dyn std::error::Error + Send + Sync>,
    pub(super) finalization: Box<dyn std::error::Error + Send + Sync>,
}

impl fmt::Display for FinalizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}; capture finalization also failed: {}",
            self.primary, self.finalization
        )
    }
}

impl std::error::Error for FinalizationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.primary)
    }
}
