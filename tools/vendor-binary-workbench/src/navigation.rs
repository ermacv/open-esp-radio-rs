//! Typed construction and validation of the generated project navigation index.

mod build;
mod inspect;
mod model;
mod reports;
#[cfg(test)]
mod tests;

pub(crate) use build::build;
pub(crate) use inspect::inspect_report;
