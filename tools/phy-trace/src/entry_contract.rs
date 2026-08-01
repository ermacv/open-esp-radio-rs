//! Explicit runtime entry contracts for mutable pointer cells.
//!
//! ELF bytes prove immutable data and the load/store sites, but a mutable
//! pointer cell needs a lifecycle precondition. These contracts are selected
//! explicitly by the caller; the structural resolver never assumes that PHY
//! registration has happened merely because the relevant symbols exist.

use crate::Result;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum EntryContract {
    #[default]
    None,
    Esp32s31PhyCold,
    Esp32s31PhyRegistered,
}

impl EntryContract {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "none" => Self::None,
            "esp32s31-phy-cold" => Self::Esp32s31PhyCold,
            "esp32s31-phy-registered" => Self::Esp32s31PhyRegistered,
            _ => return Err(format!("unsupported entry contract {value:?}").into()),
        })
    }

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Esp32s31PhyCold => "esp32s31-phy-cold",
            Self::Esp32s31PhyRegistered => "esp32s31-phy-registered",
        }
    }

    pub(crate) const fn function_table(self) -> Option<FunctionTable> {
        match self {
            Self::None => None,
            Self::Esp32s31PhyCold => Some(FunctionTable::Esp32s31PhyCold),
            Self::Esp32s31PhyRegistered => Some(FunctionTable::Esp32s31PhyRegistered),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum FunctionTable {
    Esp32s31PhyCold,
    Esp32s31PhyRegistered,
}

impl FunctionTable {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Esp32s31PhyCold => "esp32s31-rom-phy-functions-cold",
            Self::Esp32s31PhyRegistered => "esp32s31-rom-phy-functions-registered",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FunctionTarget {
    Address(u32),
    Symbol(&'static str),
}

pub(crate) const ROM_PHY_FUNCTION_TABLE: u32 = 0x2f07_f944;
pub(crate) const ROM_PHY_FUNCTION_TABLE_POINTER_SYMBOL: &str = "rom_phyFuns";
pub(crate) const LINKED_PHY_FUNCTION_TABLE_POINTER_SYMBOL: &str = "g_phyFuns";
pub(crate) const ROM_PHY_PARAM_POINTER_SYMBOL: &str = "phy_param_rom";
pub(crate) const LINKED_PHY_PARAM_SYMBOL: &str = "phy_param";

// Pinned ESP32-S31 rev0 ROM g_phyFuns_instance image. This is the same table
// used by semantic qualification scenarios; keeping it here gives structural
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

pub(crate) fn function_targets(
    table: FunctionTable,
) -> impl Iterator<Item = (u32, FunctionTarget)> {
    let targets = match table {
        FunctionTable::Esp32s31PhyCold => COLD_TARGETS,
        FunctionTable::Esp32s31PhyRegistered => REGISTERED_TARGETS,
    };
    targets
        .into_iter()
        .enumerate()
        .map(|(index, target)| (index as u32 * 4, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_phy_table_contracts_cover_exactly_thirteen_aligned_slots() {
        for table in [
            FunctionTable::Esp32s31PhyCold,
            FunctionTable::Esp32s31PhyRegistered,
        ] {
            assert_eq!(
                function_targets(table)
                    .map(|(offset, _)| offset)
                    .collect::<Vec<_>>(),
                (0..13).map(|index| index * 4).collect::<Vec<_>>()
            );
        }
    }
}
