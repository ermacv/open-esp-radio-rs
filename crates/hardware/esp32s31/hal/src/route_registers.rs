//! Protocol register sets paired with the shared radio owner.
//!
//! The PAC keeps protocol partitions and the shared radio partitions in
//! separate owners. While a protocol route runs exclusively, it holds both
//! side by side through these crate-private pairs: protocol transactions
//! reach the protocol owner, and every radio-PHY, coexistence or shared
//! baseband access names the shared owner explicitly. Keeping the pairing in
//! one place lets a later concurrent route replace the owned shared half
//! without touching protocol transactions.

use core::ops::{Deref, DerefMut};

use oer_esp32s31_pac::{
    BluetoothTaskRegisters, CoexistenceLowPowerClockObservation, RadioPhyRegisters,
    SharedRadioRegisters, WifiRadioRegisters,
};

/// Wi-Fi MAC register set and the shared radio owner of one exclusive route.
pub(crate) struct WifiRegisters {
    mac: WifiRadioRegisters,
    shared: SharedRadioRegisters,
}

impl WifiRegisters {
    pub(crate) const fn new(mac: WifiRadioRegisters, shared: SharedRadioRegisters) -> Self {
        Self { mac, shared }
    }

    pub(crate) fn into_parts(self) -> (WifiRadioRegisters, SharedRadioRegisters) {
        (self.mac, self.shared)
    }

    /// Borrow both halves for a transaction touching MAC and shared registers.
    pub(crate) fn parts_mut(&mut self) -> (&mut WifiRadioRegisters, &mut SharedRadioRegisters) {
        (&mut self.mac, &mut self.shared)
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn shared_mut(&mut self) -> &mut SharedRadioRegisters {
        &mut self.shared
    }

    pub(crate) const fn radio_phy(&self) -> &RadioPhyRegisters {
        self.shared.radio_phy()
    }

    pub(crate) fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
        self.shared.radio_phy_mut()
    }

    pub(crate) fn sample_coexistence_low_power_clock(
        &self,
    ) -> Option<CoexistenceLowPowerClockObservation> {
        self.shared.sample_coexistence_low_power_clock()
    }
}

impl Deref for WifiRegisters {
    type Target = WifiRadioRegisters;

    fn deref(&self) -> &WifiRadioRegisters {
        &self.mac
    }
}

impl DerefMut for WifiRegisters {
    fn deref_mut(&mut self) -> &mut WifiRadioRegisters {
        &mut self.mac
    }
}

/// Bluetooth controller register set and the shared radio owner of one
/// exclusive route.
pub(crate) struct BluetoothRegisters {
    task: BluetoothTaskRegisters,
    shared: SharedRadioRegisters,
}

impl BluetoothRegisters {
    pub(crate) const fn new(task: BluetoothTaskRegisters, shared: SharedRadioRegisters) -> Self {
        Self { task, shared }
    }

    pub(crate) fn into_parts(self) -> (BluetoothTaskRegisters, SharedRadioRegisters) {
        (self.task, self.shared)
    }

    /// Borrow both halves for a transaction touching controller and shared
    /// registers.
    pub(crate) fn parts_mut(&mut self) -> (&mut BluetoothTaskRegisters, &mut SharedRadioRegisters) {
        (&mut self.task, &mut self.shared)
    }

    pub(crate) const fn radio_phy(&self) -> &RadioPhyRegisters {
        self.shared.radio_phy()
    }

    pub(crate) fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
        self.shared.radio_phy_mut()
    }

    pub(crate) fn bluetooth_shared_clock_observation(
        &self,
    ) -> (
        oer_esp32s31_pac::SharedModemClockObservation,
        oer_esp32s31_pac::BluetoothLowPowerClockObservation,
    ) {
        self.task.bluetooth_shared_clock_observation(&self.shared)
    }

    pub(crate) fn reset_controller_domains(&mut self) {
        self.task.reset_controller_domains(&mut self.shared);
    }

    pub(crate) fn controller_resets_released(&self) -> bool {
        self.task.controller_resets_released(&self.shared)
    }
}

impl Deref for BluetoothRegisters {
    type Target = BluetoothTaskRegisters;

    fn deref(&self) -> &BluetoothTaskRegisters {
        &self.task
    }
}

impl DerefMut for BluetoothRegisters {
    fn deref_mut(&mut self) -> &mut BluetoothTaskRegisters {
        &mut self.task
    }
}
