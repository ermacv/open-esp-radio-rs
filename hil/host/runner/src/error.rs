//! Add operation context without erasing the typed cause.

use std::{error::Error, fmt};

#[derive(Debug)]
struct Context {
    operation: String,
    source: Box<dyn Error + Send + Sync>,
}

impl fmt::Display for Context {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.operation, self.source)
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
    Box::new(Context {
        operation: operation.into(),
        source,
    })
}
