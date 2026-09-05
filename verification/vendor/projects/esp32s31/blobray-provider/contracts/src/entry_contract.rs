//! Investigation-local registered PHY entry contract.
//!
//! The cold ROM table is reusable rev0 chip knowledge. The registered table
//! is an overlay because eleven targets are resolved from this project's
//! authenticated `archive` source and its linked mutable pointer symbols.

use open_radio_vendor_contracts::{
    DataPointerBinding, EntryContractRef, EntryContractSpec, FunctionTableRef, FunctionTableSpec,
    FunctionTarget,
};

pub use open_radio_vendor_chip_contracts_esp32s31_rev0::entry_contract::{
    NONE, PHY_COLD, PHY_COLD_TABLE, ROM_PHY_FUNCTION_TABLE, ROM_PHY_FUNCTION_TABLE_POINTER_SYMBOL,
    ROM_PHY_PARAM_POINTER_SYMBOL,
};

pub const LINKED_PHY_FUNCTION_TABLE_POINTER_SYMBOL: &str = "g_phyFuns";
pub const LINKED_PHY_PARAM_SYMBOL: &str = "phy_param";

// phy_get_romfunc_addr replaces eleven slots in this vendor-library lineage
// and retains the ROM debug and tone-SAR callbacks at offsets 0x10 and 0x2c.
const REGISTERED_TARGETS: [FunctionTarget; 13] = [
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_i2c_enter_critical",
    },
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_i2c_exit_critical",
    },
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_get_i2c_read_mask_new",
    },
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_get_i2c_hostid_new",
    },
    FunctionTarget::Address(0x2f82_44fe),
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_set_rx_comp_new",
    },
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_set_tsens_power",
    },
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_set_tsens_range",
    },
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_get_tsens_value",
    },
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_wifi_get_tx_tab_new",
    },
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_bt_get_tx_tab_new",
    },
    FunctionTarget::Address(0x2f82_66da),
    FunctionTarget::SourceSymbol {
        source: "archive",
        symbol: "phy_txgain_comp_pacfg_new",
    },
];

const REGISTERED_TABLE_SPEC: FunctionTableSpec = FunctionTableSpec {
    id: "esp32s31-rom-phy-functions-registered",
    targets: &REGISTERED_TARGETS,
};

pub const PHY_REGISTERED_TABLE: FunctionTableRef = FunctionTableRef::new(&REGISTERED_TABLE_SPEC);

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

pub const PHY_REGISTERED: EntryContractRef = EntryContractRef::new(&PHY_REGISTERED_SPEC);
pub const ALL: &[EntryContractRef] = &[NONE, PHY_COLD, PHY_REGISTERED];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_contract_extends_cold_rom_with_archive_targets() {
        assert_eq!(
            PHY_REGISTERED_TABLE
                .targets()
                .map(|(offset, _)| offset)
                .collect::<Vec<_>>(),
            (0..13).map(|index| index * 4).collect::<Vec<_>>()
        );
        assert_eq!(
            PHY_COLD_TABLE.targets().nth(3).unwrap().1,
            FunctionTarget::Address(0x2f82_9fc0)
        );
        assert_eq!(
            PHY_REGISTERED_TABLE.targets().nth(3).unwrap().1,
            FunctionTarget::SourceSymbol {
                source: "archive",
                symbol: "phy_get_i2c_hostid_new",
            }
        );
        assert_eq!(
            PHY_REGISTERED_TABLE.targets().nth(5).unwrap().1,
            FunctionTarget::SourceSymbol {
                source: "archive",
                symbol: "phy_set_rx_comp_new",
            }
        );
    }
}
