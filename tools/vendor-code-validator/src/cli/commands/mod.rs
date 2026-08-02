//! Per-command parsing and execution.

mod analyze;
mod audit_direct_targets;
mod compare;
mod execute;
mod execute_compare;
mod extract;
mod generate_reference;
mod generate_reference_batch;
mod qualify_channel;
mod qualify_rf_init;
mod verify;
mod verify_all;
mod verify_profiles;

use super::{Command, MmioRegisterMap, Result, TargetSpec};

pub(super) fn run(
    command: Command,
    arguments: Vec<String>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    match command {
        Command::AuditDirectTargets => audit_direct_targets::run(arguments),
        Command::QualifyContractChannel => qualify_channel::run(arguments, svd, &target.harness),
        Command::QualifyContractRfInit => qualify_rf_init::run(arguments, svd, &target.harness),
        Command::Execute => execute::run(arguments, svd),
        Command::ExecuteCompare => execute_compare::run(arguments, svd),
        Command::VerifyProfiles => verify_profiles::run(arguments, svd),
        Command::GenerateReference => generate_reference::run(arguments, svd, target),
        Command::GenerateReferenceBatch => generate_reference_batch::run(arguments, svd, target),
        Command::Analyze => analyze::run(arguments, svd, target),
        Command::VerifyAll => verify_all::run(arguments, svd, target),
        Command::Verify => verify::run(arguments, svd, target),
        Command::Extract => extract::run(arguments, svd),
        Command::Compare => compare::run(arguments, svd),
    }
}
