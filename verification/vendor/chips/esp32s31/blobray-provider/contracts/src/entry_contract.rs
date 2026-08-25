//! ESP32-S31 rev0 cold-ROM entry contracts.

use open_radio_vendor_contracts::{
    EntryContractRef, EntryContractSpec, FunctionTableRef, FunctionTableSpec, FunctionTarget,
};

pub const ROM_PHY_FUNCTION_TABLE: u32 = 0x2f07_f944;
pub const ROM_PHY_FUNCTION_TABLE_POINTER_SYMBOL: &str = "rom_phyFuns";
pub const ROM_PHY_PARAM_POINTER_SYMBOL: &str = "phy_param_rom";

// Reviewed ESP32-S31 rev0 ROM g_phyFuns_instance. Unlike the registered
// table, every target is owned by the ROM revision selected by the chip pack.
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

const COLD_TABLE_SPEC: FunctionTableSpec = FunctionTableSpec {
    id: "esp32s31-rom-phy-functions-cold",
    targets: &COLD_TARGETS,
};

pub const PHY_COLD_TABLE: FunctionTableRef = FunctionTableRef::new(&COLD_TABLE_SPEC);

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

pub const NONE: EntryContractRef = EntryContractRef::new(&NONE_SPEC);
pub const PHY_COLD: EntryContractRef = EntryContractRef::new(&PHY_COLD_SPEC);
pub const ALL: &[EntryContractRef] = &[NONE, PHY_COLD];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_table_is_a_complete_aligned_rom_table() {
        assert_eq!(
            PHY_COLD_TABLE
                .targets()
                .map(|(offset, _)| offset)
                .collect::<Vec<_>>(),
            (0..13).map(|index| index * 4).collect::<Vec<_>>()
        );
        assert!(PHY_COLD_TABLE.targets().all(|(_, target)| {
            matches!(target, FunctionTarget::Address(address) if address >= 0x2f00_0000)
        }));
    }
}
