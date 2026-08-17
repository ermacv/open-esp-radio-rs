//! Resolution of parsed CLI, project and run-spec inputs into executable invocations.

mod command;
mod defaults;
mod environment;
mod model;
mod needs;
mod register_catalog;
#[cfg(test)]
mod tests;

pub(super) use environment::resolve;
pub(super) use model::{
    CodeWorkspaceCommand, FunctionWorkspaceCommand, InterfaceWorkspaceCommand,
    RegisterWorkspaceCommand, ResolvedInvocation, TargetCommand,
};

#[cfg(test)]
use super::args::{Command, ParsedInvocation};
#[cfg(test)]
use crate::run_spec::RunSpec;
#[cfg(test)]
use defaults::apply_run_spec_defaults;
#[cfg(test)]
use environment::resolve_from;
#[cfg(test)]
use needs::ResolutionNeeds;
#[cfg(test)]
use std::path::PathBuf;
