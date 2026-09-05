//! Reviewed semantic summaries for effects that cannot yet be reconstructed
//! from the structural instruction trace alone.
//!
//! These are temporary executable models, not declarative facts or generated
//! reconstruction. Each entry point enforces its body/context applicability;
//! a mismatch returns None and preserves structural analysis fallback.

use crate::*;

mod body_identity;
mod direct_semantic;
pub(crate) use direct_semantic::direct_external_semantic_function;
mod fail_stop;
mod i2c;
mod intrinsics;
mod rf;

pub(super) use direct_semantic::direct_semantic_function;
use i2c::{
    HOST_ID_SUMMARIES, PHY_CHIP_I2C_READ_REG_ORG_BODY, PHY_CHIP_I2C_WRITE_REG_BODY,
    chip_i2c_read_reg_org_trace, chip_i2c_write_reg_trace, host_id_trace,
};
pub(super) use intrinsics::wide_signed_divide_intrinsic;
use rf::{
    exact_iq_estimator_poll, exact_rf_frequency_offset_scratch_wrapper,
    exact_rfpll_calibration_poll, exact_rfpll_cap_calibration_search, iq_estimator_poll_trace,
    rf_frequency_offset_scratch_trace, rfpll_calibration_poll_trace,
    rfpll_cap_calibration_search_trace,
};

pub(super) fn reference_intrinsic_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioMap,
    pointer_context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    let intrinsic_arguments: Rv32CallArguments =
        core::array::from_fn(|index| SymbolicValue::input(index as u8));
    if let Some((return_value, _)) = wide_signed_divide_intrinsic(symbol, &intrinsic_arguments) {
        return Some(FunctionAnalysis {
            symbol: symbol.name.clone(),
            events: Vec::new(),
            located_events: Vec::new(),
            located_reference_events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value,
            reference_flow: None,
            unresolved_branch: None,
        });
    }

    if fail_stop::exact_btdm_assert(symbol) {
        return Some(fail_stop::btdm_assert_trace(symbol));
    }

    if exact_rfpll_calibration_poll(symbol) {
        return Some(rfpll_calibration_poll_trace(symbol));
    }

    if exact_rfpll_cap_calibration_search(symbol) {
        return Some(rfpll_cap_calibration_search_trace(symbol));
    }

    if exact_rf_frequency_offset_scratch_wrapper(symbol) {
        return Some(rf_frequency_offset_scratch_trace(symbol));
    }

    if exact_iq_estimator_poll(symbol) {
        return iq_estimator_poll_trace(symbol, svd, pointer_context);
    }

    if symbol.name == "phy_chip_i2c_readReg_org"
        && symbol.address == 0x2f82_9ffa
        && symbol.bytes == PHY_CHIP_I2C_READ_REG_ORG_BODY
    {
        return chip_i2c_read_reg_org_trace(symbol, svd);
    }

    if symbol.name == "phy_chip_i2c_writeReg"
        && symbol.address == 0x2f82_a30e
        && symbol.bytes == PHY_CHIP_I2C_WRITE_REG_BODY
    {
        return chip_i2c_write_reg_trace(symbol, svd, pointer_context);
    }

    HOST_ID_SUMMARIES
        .iter()
        .find(|summary| {
            symbol.name == summary.name
                && symbol.address == u64::from(summary.address)
                && symbol.bytes == summary.body
        })
        .map(|summary| host_id_trace(symbol, svd, *summary))
}

#[cfg(test)]
mod tests;
