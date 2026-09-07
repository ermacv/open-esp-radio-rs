//! Semantic routing onto the two active ESP32-S31 RX memory lists.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::types::BluetoothMemoryListSelector;

/// One of the two global RX list classes activated by the current S31 memory
/// manager.
///
/// Complete `update_global_rxlink` bodies inspect scheduler-item byte `+0x4d`:
/// scan kind two selects list one and every other active kind selects list two.
/// Selector three remains outside this semantic type because no current caller
/// publishes it to hardware after reset.
///
/// The DTM allocator writes non-scanner kind five but also selects the memory-
/// manager bypass that prevents this global-insertion function from running.
/// DTM therefore has no selector binding in this API: its private RX graph
/// needs a separately proven hardware-publication path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxMemoryListClass {
    /// Scanner scheduler items use positional selector one.
    Scanning,
    /// Non-scanner items admitted to normal global insertion use selector two.
    NonScanning,
}

impl RxMemoryListClass {
    /// The exact active selector chosen by the complete memory-manager body.
    pub const fn selector(self) -> BluetoothMemoryListSelector {
        match self {
            Self::Scanning => BluetoothMemoryListSelector::One,
            Self::NonScanning => BluetoothMemoryListSelector::Two,
        }
    }
}

#[cfg(test)]
mod tests;
