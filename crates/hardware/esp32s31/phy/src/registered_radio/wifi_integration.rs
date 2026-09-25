//! Wi-Fi handoff edges of the coupled registered radio owner.
//!
//! The physical radio, registration proof and client set remain private in
//! the ancestor owner; this child may perform only the existing typed Wi-Fi
//! transfer and cold/retained receive-enable edges.

use super::RegisteredPhyRadio;

impl<P> RegisteredPhyRadio<P> {
    /// Complete the protocol wrapper's post-wake Wi-Fi RX-enable edge.
    ///
    /// Retained PHY wake itself returns a protocol-neutral powered owner. The
    /// Wi-Fi runtime calls this only after reacquiring the Wi-Fi client and
    /// before exposing runtime register ownership again.
    #[doc(hidden)]
    pub fn enable_wifi_rx_after_retained_wake(&mut self) {
        self.radio.enable_wifi_rx();
    }

    /// Transfer the registered PHY and scheduler to the Wi-Fi runtime context
    /// together with its matching physical and interrupt owners.
    pub fn into_wifi_runtime_parts(
        self,
    ) -> (
        P,
        oer_esp32s31_hal::owner::RadioRuntimeOwner,
        oer_esp32s31_hal::owner::MacInterruptSetup,
        crate::RegisteredWifiPhy,
    ) {
        let (platform, registers, interrupt) = self.radio.into_running().into_runtime_parts();
        (
            platform,
            registers,
            interrupt,
            crate::RegisteredWifiPhy {
                registered: self.phy,
                clients: self.clients,
            },
        )
    }

    /// Select the cold Wi-Fi channel without releasing the registration owner.
    pub async fn initialize_wifi_channel<D: crate::PhyAsyncDelay, O: crate::PhyTargetObserver>(
        &mut self,
        channel: u16,
        cbw: u8,
        observer: &mut O,
    ) -> Result<(), crate::PhyTargetPortError> {
        self.radio.enable_wifi_rx();
        let mut hardware = self.radio.channel_hal();
        crate::target_port::select_phy_channel_with_hal::<D, _, _>(
            self.phy.target_state_mut(),
            channel,
            cbw,
            &mut hardware,
            observer,
        )
        .await
    }
}
