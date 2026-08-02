//! Versioned external ABI descriptions used by the structural reference tracer.
//!
//! These tables describe interfaces published by the platform. They do not
//! turn arbitrary indirect calls into trusted calls: a load must be rooted in
//! the named pointer cell and use an exact registered slot offset.

use open_radio_vendor_validator_core::{
    ExternalFunctionRef, ExternalFunctionSpec, ExternalReturnModel, ExternalTableRef,
    ExternalTableSpec,
};

const ESP32S31_WIFI_OSI_V9_FUNCTIONS: &[ExternalFunctionSpec] = &[
    ExternalFunctionSpec {
        id: "env-is-chip",
        offset: 0x004,
        c_name: "_env_is_chip",
        argument_count: 0,
        // The reference profile is explicitly the real ESP32-S31 target, not
        // the FPGA/emulation branch selected by a false callback result.
        return_model: ExternalReturnModel::Constant(1),
    },
    ExternalFunctionSpec {
        id: "rand",
        offset: 0x0bc,
        c_name: "_rand",
        argument_count: 0,
        return_model: ExternalReturnModel::SymbolicU32,
    },
    ExternalFunctionSpec {
        id: "random",
        offset: 0x144,
        c_name: "_random",
        argument_count: 0,
        return_model: ExternalReturnModel::SymbolicU32,
    },
    ExternalFunctionSpec {
        id: "slow-clock-calibration-get",
        offset: 0x148,
        c_name: "_slowclk_cal_get",
        argument_count: 0,
        return_model: ExternalReturnModel::SymbolicU32,
    },
    ExternalFunctionSpec {
        id: "coex-pti-get",
        offset: 0x1a8,
        c_name: "_coex_pti_get",
        argument_count: 2,
        return_model: ExternalReturnModel::PrivateStackOutputU8 {
            pointer_argument: 1,
        },
    },
];

const ESP32S31_WIFI_OSI_V9: ExternalTableSpec = ExternalTableSpec {
    id: "esp32s31-wifi-osi-v9",
    pointer_symbol: "g_osi_funcs_p",
    backing_symbol: "g_wifi_osi_funcs",
    version: 0x0000_0009,
    magic: 0xdead_beaf,
    size: 0x200,
    magic_offset: 0x1fc,
    functions: ESP32S31_WIFI_OSI_V9_FUNCTIONS,
};

pub const WIFI_OSI_V9: ExternalTableRef = ExternalTableRef::new(&ESP32S31_WIFI_OSI_V9);
pub const ENV_IS_CHIP: ExternalFunctionRef =
    ExternalFunctionRef::new(&ESP32S31_WIFI_OSI_V9_FUNCTIONS[0]);
pub const RAND: ExternalFunctionRef = ExternalFunctionRef::new(&ESP32S31_WIFI_OSI_V9_FUNCTIONS[1]);
pub const RANDOM: ExternalFunctionRef =
    ExternalFunctionRef::new(&ESP32S31_WIFI_OSI_V9_FUNCTIONS[2]);

#[cfg(test)]
pub fn slots(table: ExternalTableRef) -> impl Iterator<Item = ExternalFunctionRef> {
    table.spec().functions.iter().map(ExternalFunctionRef::new)
}

#[cfg(test)]
mod tests;
