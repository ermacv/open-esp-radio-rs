//! ESP32-S31 channel qualification command.

use super::super::*;

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap, harness: &str) -> Result<bool> {
    let mut vendor_artifact = None;
    let mut vendor_companion = None;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--vendor-artifact" => {
                vendor_artifact = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--vendor-artifact",
                )?));
            }
            "--vendor-companion" => {
                vendor_companion = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--vendor-companion",
                )?));
            }
            _ => {
                return Err(format!("unknown channel contract option: {argument}").into());
            }
        }
    }
    let vendor_artifact = vendor_artifact.ok_or("missing --vendor-artifact")?;
    let vendor_companion = vendor_companion.ok_or("missing --vendor-companion")?;
    crate::harnesses::qualify_named_contract(
        harness,
        "channel",
        svd,
        &vendor_artifact,
        &vendor_companion,
    )
}
