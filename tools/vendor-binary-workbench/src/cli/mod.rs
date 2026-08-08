//! Command parsing and dispatch for the Vendor Binary Workbench.

mod args;
mod arguments;
mod commands;
mod dispatch;
mod generated_output;
mod output;
mod resolver;
mod ui;
mod values;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use crate::*;
use args::{Command, CommandArguments, ParsedInvocation};
pub(crate) use arguments::*;
pub(crate) use values::*;

pub(crate) fn output_line(arguments: std::fmt::Arguments<'_>) {
    output::line(arguments);
}

pub(crate) fn run() -> Result<bool> {
    let invocation = ParsedInvocation::parse(std::env::args().skip(1))?;
    ui::init(&invocation.ui)?;
    output::init(invocation.ui.format);
    dispatch::run(resolver::resolve(invocation)?)
}

pub(crate) fn finish_output() -> Result<()> {
    output::finish()
}
