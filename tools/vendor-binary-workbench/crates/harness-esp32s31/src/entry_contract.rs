//! Explicit runtime entry contracts for mutable pointer cells.
//!
//! ELF bytes prove immutable data and the load/store sites, but a mutable
//! pointer cell needs a lifecycle precondition. These contracts are selected
//! explicitly by the caller; the structural resolver never assumes that PHY
//! registration has happened merely because the relevant symbols exist.

use open_radio_vendor_contracts::{
    DataPointerBinding, EntryContractRef, EntryContractSpec, FunctionTableRef, FunctionTableSpec,
    FunctionTarget,
};

pub const ROM_PHY_FUNCTION_TABLE: u32 = 0x2f07_f944;
pub const ROM_PHY_FUNCTION_TABLE_POINTER_SYMBOL: &str = "rom_phyFuns";
pub const LINKED_PHY_FUNCTION_TABLE_POINTER_SYMBOL: &str = "g_phyFuns";
pub const ROM_PHY_PARAM_POINTER_SYMBOL: &str = "phy_param_rom";
pub const LINKED_PHY_PARAM_SYMBOL: &str = "phy_param";

// Pinned ESP32-S31 rev0 ROM g_phyFuns_instance image. This is the same table
// used by semantic verification scenarios; keeping it here gives structural
// analysis and concrete execution one reviewed source of truth.
const COLD_TARGETS: [FunctionTarget; 13] = [
    FunctionTarget::Address(0x2f82_9f18),
    FunctionTarget::Address(0x2f82_9f1a),
    FunctionTarget::Address(0x2f82_9f84),
    FunctionTarget::Address(0x2f82_9fc0),
    FunctionTarget::Address(0x2f82_44fe),
    FunctionTarget::Address(0x2f82_78b0),
    FunctionTarget::Address(0x2f82_5dc8),
    FunctionTarget::Address(0x2f82_5ecc),
    FunctionTarget::Address(0x2f82_5f7c),
    FunctionTarget::Address(0x2f82_711c),
    FunctionTarget::Address(0x2f82_7392),
    FunctionTarget::Address(0x2f82_66da),
    FunctionTarget::Address(0x2f82_88de),
];

// phy_get_romfunc_addr replaces eleven slots in the linked image and retains
// the ROM debug and tone-SAR callbacks at offsets 0x10 and 0x2c.
const REGISTERED_TARGETS: [FunctionTarget; 13] = [
    FunctionTarget::Symbol("phy_i2c_enter_critical"),
    FunctionTarget::Symbol("phy_i2c_exit_critical"),
    FunctionTarget::Symbol("phy_get_i2c_read_mask_new"),
    FunctionTarget::Symbol("phy_get_i2c_hostid_new"),
    FunctionTarget::Address(0x2f82_44fe),
    FunctionTarget::Symbol("phy_set_rx_comp_new"),
    FunctionTarget::Symbol("phy_set_tsens_power"),
    FunctionTarget::Symbol("phy_set_tsens_range"),
    FunctionTarget::Symbol("phy_get_tsens_value"),
    FunctionTarget::Symbol("phy_wifi_get_tx_tab_new"),
    FunctionTarget::Symbol("phy_bt_get_tx_tab_new"),
    FunctionTarget::Address(0x2f82_66da),
    FunctionTarget::Symbol("phy_txgain_comp_pacfg_new"),
];

const COLD_TABLE_SPEC: FunctionTableSpec = FunctionTableSpec {
    id: "esp32s31-rom-phy-functions-cold",
    targets: &COLD_TARGETS,
};
const REGISTERED_TABLE_SPEC: FunctionTableSpec = FunctionTableSpec {
    id: "esp32s31-rom-phy-functions-registered",
    targets: &REGISTERED_TARGETS,
};

pub const PHY_COLD_TABLE: FunctionTableRef = FunctionTableRef::new(&COLD_TABLE_SPEC);
pub const PHY_REGISTERED_TABLE: FunctionTableRef = FunctionTableRef::new(&REGISTERED_TABLE_SPEC);

const NONE_SPEC: EntryContractSpec = EntryContractSpec {
    id: "none",
    function_table: None,
    pointer_symbols: &[],
    data_pointer_binding: None,
};
const PHY_COLD_SPEC: EntryContractSpec = EntryContractSpec {
    id: "esp32s31-phy-cold",
    function_table: Some(PHY_COLD_TABLE),
    pointer_symbols: &[ROM_PHY_FUNCTION_TABLE_POINTER_SYMBOL],
    data_pointer_binding: None,
};
const PHY_REGISTERED_SPEC: EntryContractSpec = EntryContractSpec {
    id: "esp32s31-phy-registered",
    function_table: Some(PHY_REGISTERED_TABLE),
    pointer_symbols: &[
        ROM_PHY_FUNCTION_TABLE_POINTER_SYMBOL,
        LINKED_PHY_FUNCTION_TABLE_POINTER_SYMBOL,
    ],
    data_pointer_binding: Some(DataPointerBinding {
        pointer_symbol: ROM_PHY_PARAM_POINTER_SYMBOL,
        target_symbol: LINKED_PHY_PARAM_SYMBOL,
    }),
};

pub const NONE: EntryContractRef = EntryContractRef::new(&NONE_SPEC);
pub const PHY_COLD: EntryContractRef = EntryContractRef::new(&PHY_COLD_SPEC);
pub const PHY_REGISTERED: EntryContractRef = EntryContractRef::new(&PHY_REGISTERED_SPEC);
pub const ALL: &[EntryContractRef] = &[NONE, PHY_COLD, PHY_REGISTERED];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_phy_table_contracts_cover_exactly_thirteen_aligned_slots() {
        for table in [PHY_COLD_TABLE, PHY_REGISTERED_TABLE] {
            assert_eq!(
                table
                    .targets()
                    .map(|(offset, _)| offset)
                    .collect::<Vec<_>>(),
                (0..13).map(|index| index * 4).collect::<Vec<_>>()
            );
        }
    }
}
