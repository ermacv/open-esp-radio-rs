//! Command parsing and dispatch for the Vendor Binary Workbench.

macro_rules! outputln {
    ($($argument:tt)*) => {{
        crate::cli::output_line(format_args!($($argument)*));
    }};
}

mod args;
mod arguments;
pub(crate) mod commands;
mod dispatch;
mod generated_output;
mod output;
mod progress;
pub(crate) mod render;
mod resolver;
mod table;
mod ui;
mod values;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use crate::*;
use args::ParsedInvocation;
pub(crate) use arguments::*;
pub(crate) use values::*;

pub(crate) fn output_line(arguments: std::fmt::Arguments<'_>) {
    output::line(arguments);
}

pub(crate) fn run() -> Result<bool> {
    let invocation = ParsedInvocation::parse(std::env::args().skip(1))?;
    ui::init(&invocation.ui)?;
    output::init(invocation.ui.format);
    let progress = progress::command_span(&invocation.command);
    let _entered = progress.as_ref().map(tracing::Span::enter);
    dispatch::run(resolver::resolve(invocation)?)
}

pub(crate) fn finish_output() -> Result<()> {
    output::finish()
}
