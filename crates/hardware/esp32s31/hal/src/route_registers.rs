//! The Wi-Fi register set paired with the shared radio owner.
//!
//! The PAC keeps protocol partitions and the shared radio partitions in
//! separate owners. While Wi-Fi runs exclusively, it holds both side by side
//! through this crate-private pair: MAC transactions reach the MAC owner, and
//! every radio-PHY, coexistence or shared baseband access names the shared
//! owner explicitly. Concurrent routes borrow the shared half through the
//! shared radio arbiter instead.

use core::ops::{Deref, DerefMut};

use oer_esp32s31_pac::{
    RadioPhyRegisters, SharedRadioRegisters, WifiRadioRegisters,
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

    pub(crate) const fn shared(&self) -> &SharedRadioRegisters {
        &self.shared
    }

    pub(crate) const fn radio_phy(&self) -> &RadioPhyRegisters {
        self.shared.radio_phy()
    }

    pub(crate) fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
        self.shared.radio_phy_mut()
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
