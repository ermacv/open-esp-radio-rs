//! Add operation context without erasing the typed cause.

use std::{error::Error, fmt};

#[derive(Debug)]
struct Context {
    message: String,
    source: Box<dyn Error + Send + Sync>,
}

impl fmt::Display for Context {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for Context {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.source)
    }
}

pub(crate) fn context(
    operation: impl Into<String>,
    source: Box<dyn Error + Send + Sync>,
) -> Box<dyn Error + Send + Sync> {
    with_message(format!("{}: {source}", operation.into()), source)
}

/// Attach a complete diagnostic without discarding the original typed cause.
pub(crate) fn with_message(
    message: String,
    source: Box<dyn Error + Send + Sync>,
) -> Box<dyn Error + Send + Sync> {
    Box::new(Context { message, source })
}
