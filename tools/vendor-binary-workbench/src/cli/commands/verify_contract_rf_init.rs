//! ESP32-S31 RF-init verification command.

use super::super::*;

pub(super) fn run(
    arguments: VerifyContractArgs,
    svd: &MmioRegisterMap,
    harness: &str,
) -> Result<bool> {
    let vendor_artifact = arguments
        .vendor_artifact
        .ok_or("missing --vendor-artifact")?;
    let vendor_companion = arguments
        .vendor_companion
        .ok_or("missing --vendor-companion")?;
    crate::harnesses::verify_named_contract(
        harness,
        "rf-init",
        svd,
        &vendor_artifact,
        &vendor_companion,
    )
}
