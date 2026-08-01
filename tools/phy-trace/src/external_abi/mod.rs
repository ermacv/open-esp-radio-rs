//! Versioned external ABI descriptions used by the structural reference tracer.
//!
//! These tables describe interfaces published by the platform. They do not
//! turn arbitrary indirect calls into trusted calls: a load must be rooted in
//! the named pointer cell and use an exact registered slot offset.

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Table {
    Esp32s31WifiOsiV9,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Function {
    EnvIsChip,
    Rand,
    Random,
    SlowClockCalibrationGet,
    CoexPtiGet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReturnModel {
    /// The pinned platform contract fixes the callback result.
    Constant(u32),
    /// The callback produces an unconstrained value supplied by the scenario.
    SymbolicU32,
    /// The callback status is not modeled, but it must publish one byte through
    /// the named pointer argument. The structural tracer currently accepts
    /// only a pointer into the callee's private stack, so this effect cannot be
    /// redirected into MMIO or undeclared RAM.
    PrivateStackOutputU8 { pointer_argument: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TableSpec {
    pub id: &'static str,
    pub pointer_symbol: &'static str,
    pub backing_symbol: &'static str,
    pub version: u32,
    pub magic: u32,
    pub size: u32,
    pub magic_offset: u32,
    pub source_commit: &'static str,
    pub source_header: &'static str,
    pub source_sha256: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SlotSpec {
    pub table: Table,
    pub function: Function,
    pub offset: u32,
    pub c_name: &'static str,
    pub rust_method: &'static str,
    pub argument_count: u8,
    pub return_model: ReturnModel,
}

const ESP32S31_WIFI_OSI_V9: TableSpec = TableSpec {
    id: "esp32s31-wifi-osi-v9",
    pointer_symbol: "g_osi_funcs_p",
    backing_symbol: "g_wifi_osi_funcs",
    version: 0x0000_0009,
    magic: 0xdead_beaf,
    size: 0x200,
    magic_offset: 0x1fc,
    source_commit: "72b97e6fe55307aa92c8c1edf3fdb3f4df816e80",
    source_header: "c/headers/esp32s31/esp_private/wifi_os_adapter.h",
    source_sha256: "08fb73f76da7e6800c42dbff2fb724dff5205e1c8262e969da0f391be21f1eae",
};

const ESP32S31_WIFI_OSI_V9_SLOTS: &[SlotSpec] = &[
    SlotSpec {
        table: Table::Esp32s31WifiOsiV9,
        function: Function::EnvIsChip,
        offset: 0x004,
        c_name: "_env_is_chip",
        rust_method: "wifi_osi_env_is_chip",
        argument_count: 0,
        // The reference profile is explicitly the real ESP32-S31 target, not
        // the FPGA/emulation branch selected by a false callback result.
        return_model: ReturnModel::Constant(1),
    },
    SlotSpec {
        table: Table::Esp32s31WifiOsiV9,
        function: Function::Rand,
        offset: 0x0bc,
        c_name: "_rand",
        rust_method: "wifi_osi_rand",
        argument_count: 0,
        return_model: ReturnModel::SymbolicU32,
    },
    SlotSpec {
        table: Table::Esp32s31WifiOsiV9,
        function: Function::Random,
        offset: 0x144,
        c_name: "_random",
        rust_method: "wifi_osi_random",
        argument_count: 0,
        return_model: ReturnModel::SymbolicU32,
    },
    SlotSpec {
        table: Table::Esp32s31WifiOsiV9,
        function: Function::SlowClockCalibrationGet,
        offset: 0x148,
        c_name: "_slowclk_cal_get",
        rust_method: "wifi_osi_slowclk_cal_get",
        argument_count: 0,
        return_model: ReturnModel::SymbolicU32,
    },
    SlotSpec {
        table: Table::Esp32s31WifiOsiV9,
        function: Function::CoexPtiGet,
        offset: 0x1a8,
        c_name: "_coex_pti_get",
        rust_method: "wifi_osi_coex_pti_get",
        argument_count: 2,
        return_model: ReturnModel::PrivateStackOutputU8 {
            pointer_argument: 1,
        },
    },
];

pub(crate) fn table_spec(table: Table) -> &'static TableSpec {
    match table {
        Table::Esp32s31WifiOsiV9 => &ESP32S31_WIFI_OSI_V9,
    }
}

pub(crate) fn table_for_pointer_symbol(symbol: &str) -> Option<Table> {
    (symbol == ESP32S31_WIFI_OSI_V9.pointer_symbol).then_some(Table::Esp32s31WifiOsiV9)
}

pub(crate) fn slot(table: Table, offset: u32) -> Option<&'static SlotSpec> {
    ESP32S31_WIFI_OSI_V9_SLOTS
        .iter()
        .find(|slot| slot.table == table && slot.offset == offset)
}

pub(crate) fn function(table: Table, function: Function) -> &'static SlotSpec {
    ESP32S31_WIFI_OSI_V9_SLOTS
        .iter()
        .find(|slot| slot.table == table && slot.function == function)
        .expect("every external function enum must have a registered slot")
}

/// Named non-table calls whose effect is diagnostic-only but still retained
/// by generated references. The argument count is deliberately exact for the
/// registered call shape; an unknown symbol or wider live argument set remains
/// unresolved.
pub(crate) fn diagnostic_argument_count(symbol: &str) -> Option<u8> {
    match symbol {
        "wifi_log" => Some(6),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn slots(table: Table) -> impl Iterator<Item = &'static SlotSpec> {
    ESP32S31_WIFI_OSI_V9_SLOTS
        .iter()
        .filter(move |slot| slot.table == table)
}

pub(crate) const fn all_tables() -> [Table; 1] {
    [Table::Esp32s31WifiOsiV9]
}

#[cfg(test)]
mod tests;
