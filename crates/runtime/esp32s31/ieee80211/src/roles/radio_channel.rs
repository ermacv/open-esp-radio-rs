//! Wi-Fi channel transactions leased from the shared radio.
//!
//! Every retune takes the arbiter lease for exactly one transaction, as the
//! vendor `phy_lock` scope does, so another radio client can use the shared
//! domain between the channels of one scan or hop sequence.

use oer_esp32s31_hal::{
    ieee80211::arena::RadioAccess, owner::RadioRuntimeOwner, shared_radio::PlatformClockProvider,
};
use oer_esp32s31_phy::{ConcurrentWifiChannelError, PhyAsyncDelay, PhyTargetObserver};
use oer_esp32s31_radio_runtime::RadioSystem;

use oer_esp32s31_ieee80211_sta::hardware::channel::ScanPhy;

/// A Wi-Fi role's channel authority on the shared radio.
pub struct RadioChannel<'radio, P, C, O, D> {
    radio: &'radio RadioSystem<P, C>,
    phy: ScanPhy<O, D>,
}

impl<'radio, P, C, O, D> RadioChannel<'radio, P, C, O, D>
where
    C: PlatformClockProvider,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    pub const fn new(radio: &'radio RadioSystem<P, C>, observer: O) -> Self {
        Self {
            radio,
            phy: ScanPhy::new(observer),
        }
    }

    /// Retune while the caller still owns a cold, stopped MAC.
    pub async fn select_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        owner: &mut RadioRuntimeOwner,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        self.phy
            .select_channel(lease, platform, channel_or_frequency, cbw, owner)
            .await
    }

    /// Stop the MAC, retune and restore the qualified REGDMA link.
    pub async fn switch_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        owner: &mut RadioRuntimeOwner,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        self.phy
            .switch_channel(lease, platform, channel_or_frequency, cbw, owner)
            .await
    }

    /// Switch through the arena's serialized channel-only capability.
    pub async fn switch_published_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        access: RadioAccess<'_>,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        self.phy
            .switch_published_channel(lease, platform, channel_or_frequency, cbw, access)
            .await
    }

    /// Return the observer once the role no longer retunes.
    pub fn into_observer(self) -> O {
        self.phy.into_observer()
    }
}
