//! Per-command parsing and execution.

mod analyze;
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

use super::{Command, MmioRegisterMap, Result};

pub(super) fn run(command: Command, arguments: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    match command {
        Command::QualifyEsp32s31Channel => qualify_channel::run(arguments, svd),
        Command::QualifyEsp32s31RfInit => qualify_rf_init::run(arguments, svd),
        Command::Execute => execute::run(arguments, svd),
        Command::ExecuteCompare => execute_compare::run(arguments, svd),
        Command::VerifyProfiles => verify_profiles::run(arguments, svd),
        Command::GenerateReference => generate_reference::run(arguments, svd),
        Command::GenerateReferenceBatch => generate_reference_batch::run(arguments, svd),
        Command::Analyze => analyze::run(arguments, svd),
        Command::VerifyAll => verify_all::run(arguments, svd),
        Command::Verify => verify::run(arguments, svd),
        Command::Extract => extract::run(arguments, svd),
        Command::Compare => compare::run(arguments, svd),
    }
}
