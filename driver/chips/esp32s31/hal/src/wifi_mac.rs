//! Wi-Fi-specific MAC operations reached from the vendor PHY ABI.
//!
//! These leaves are deliberately separate from the shared PHY modules: their
//! physical registers belong to the 802.11 MAC and are not reusable by
//! Bluetooth, BLE or IEEE 802.15.4 PHY paths.

use open_esp_radio_esp32s31_pac::RadioRegisters;
pub use open_esp_radio_esp32s31_pac::{
    MacInterface, MacItwtClearIndex, MacPti, MacRoleReceivePolicy, MacStaApReceivePlan,
    MacStaPolicyMode, MacTxPtiCount, MacTxPtiProgram, MacTxQueueIndex,
};

/// Complete identity of one hardware MAC interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacInterfaceIdentity {
    pub interface: MacInterface,
    pub address: [u8; 6],
    pub bssid: [u8; 6],
}

/// Closed HAL authority for reviewed runtime Wi-Fi MAC transactions.
///
/// This type intentionally exposes neither the contained [`RadioRegisters`]
/// nor `Deref`. LMAC code can request only the finite operations defined here.
pub struct WifiMacHal<'registers> {
    registers: &'registers mut RadioRegisters,
}

impl<'registers> WifiMacHal<'registers> {
    pub fn new(registers: &'registers mut RadioRegisters) -> Self {
        Self { registers }
    }

    /// Publish the receive address and BSSID using two complete vendor leaves.
    pub fn program_interface_identity(&mut self, identity: MacInterfaceIdentity) {
        self.registers
            .program_receive_interface_address(identity.interface, identity.address);
        self.registers
            .program_interface_bssid(identity.interface, identity.bssid);
    }

    /// Apply one exact reviewed role-policy transaction.
    pub fn configure_role_receive_policy(&mut self, policy: MacRoleReceivePolicy) {
        self.registers.apply_role_receive_policy(policy);
    }

    pub fn configure_station_receive_policy(&mut self, bssid: [u8; 6]) {
        self.registers.apply_sta_link_receive_policy(bssid);
    }

    /// Apply only the exact vendor policy-six register transaction.
    /// Connected-STA entry normally uses [`Self::configure_station_receive_policy`]
    /// so the open scan/sniffer frontier is closed first.
    pub fn configure_station_policy_six(&mut self, bssid: [u8; 6], mode: MacStaPolicyMode) {
        self.configure_role_receive_policy(MacRoleReceivePolicy::Station { bssid, mode });
    }

    pub fn configure_access_point_receive_policy(&mut self, address: [u8; 6]) {
        self.configure_role_receive_policy(MacRoleReceivePolicy::AccessPoint { address });
    }

    /// Program both reviewed receive contexts as one register composition.
    ///
    /// This does not claim simultaneous runtime ownership or select a channel.
    pub fn configure_sta_ap_receive_plan(&mut self, plan: MacStaApReceivePlan) {
        self.registers.apply_sta_ap_receive_plan(plan);
    }

    /// Stop the access-point TSF using the complete reviewed vendor leaf.
    pub fn stop_access_point_tsf(&mut self) {
        self.registers.stop_softap_tsf();
    }

    /// Start a new access-point TSF epoch through the reviewed selector-zero
    /// transaction. No raw timestamp word or register image crosses the HAL.
    pub fn reset_and_start_access_point_tsf(&mut self) {
        self.registers.reset_and_start_softap_tsf();
    }

    /// Publish the complete two-edge receive-beacon PTI transaction.
    pub fn set_rx_beacon_pti(&mut self, beacon: MacPti, shared: MacPti) {
        self.registers.set_rx_beacon_pti(beacon, shared);
    }

    /// Publish the complete receive-beacon PTI clear edge.
    pub fn clear_rx_beacon_pti(&mut self) {
        self.registers.clear_rx_beacon_pti();
    }

    /// Publish the complete two-edge individual-TWT PTI transaction.
    pub fn set_itwt_pti(&mut self, argument_is_zero: bool, shared: MacPti) {
        self.registers.set_itwt_pti(argument_is_zero, shared);
    }

    /// Publish one bounded individual-TWT clear request.
    pub fn clear_itwt_pti(&mut self, index: MacItwtClearIndex) {
        self.registers.clear_itwt_pti(index);
    }

    /// Publish the complete scheduler and queue-vector PTI transaction.
    pub fn set_tx_pti(&mut self, queue: MacTxQueueIndex, program: MacTxPtiProgram) {
        self.registers.set_tx_pti(queue, program);
    }
}

/// Apply complete rev0 ROM `phy_enable_cca` or `phy_disable_cca`.
#[cfg(target_arch = "riscv32")]
pub fn set_cca_enabled(registers: &mut RadioRegisters, enabled: bool) {
    registers.set_phy_wifi_cca_enabled(enabled);
}

/// Apply complete rev0 ROM `phy_sifs_reg_init`.
#[cfg(target_arch = "riscv32")]
pub fn initialize_sifs(registers: &mut RadioRegisters) {
    registers.initialize_phy_wifi_sifs();
}
