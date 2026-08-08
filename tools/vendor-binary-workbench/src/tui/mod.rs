//! Read-only project browser built on the public application facade.

mod runtime;
mod state;
mod view;
mod worker;

pub(crate) use runtime::run;
