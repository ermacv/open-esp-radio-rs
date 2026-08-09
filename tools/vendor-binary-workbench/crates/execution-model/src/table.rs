//! Scenario-owned runtime function-table placement and lifecycle evidence.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum TableSlotTarget {
    Null,
    Address(u32),
    Symbol(String),
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TableInstanceSlot {
    pub offset: u32,
    pub target: TableSlotTarget,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TableInstance {
    pub layout_id: String,
    pub base_address: u32,
    pub layout_size: u32,
    pub pointer_cells: Vec<u32>,
    pub slots: Vec<TableInstanceSlot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TableLifecycleEvent {
    SlotInitialized {
        layout_id: String,
        offset: u32,
        target: u32,
    },
    SlotWritten {
        layout_id: String,
        offset: u32,
        width: u8,
        value: u32,
        site: u32,
    },
    PointerInstalled {
        layout_id: String,
        address: u32,
        base_address: u32,
    },
    IndirectCall {
        layout_id: Option<String>,
        slot_offset: Option<u32>,
        site: u32,
        target: u32,
        symbol: String,
    },
}
