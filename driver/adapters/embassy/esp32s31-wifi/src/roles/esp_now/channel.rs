//! Exclusive channel owner for bounded standalone ESP-NOW excursions.

use core::future::Future;
#[cfg(target_arch = "riscv32")]
use core::marker::PhantomData;

use open_esp_radio_ieee80211::channel::WifiChannel;

/// Retune capability lent to the standalone scheduler.
///
/// The scheduler calls this only after it has stopped ordinary TX, RX DMA and
/// the MAC interrupt route. A successful implementation must update its
/// `current_channel` observation atomically with the completed PHY transition.
/// An error leaves the physical channel unknown and forces the service into
/// sticky quarantine.
pub trait Esp32s31StandaloneEspNowChannelControl<H, P> {
    type Error;

    fn current_channel(&self) -> WifiChannel;

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut H,
        platform: &'a mut P,
        channel: WifiChannel,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;
}

/// Production S31 PHY/context capability for one standalone ESP-NOW runner.
///
/// This value borrows the role-neutral runtime context instead of duplicating
/// PHY state. It is intentionally supplied only to the opt-in off-channel run
/// method; connected ESP-NOW has no path to construct or consume it.
#[cfg(target_arch = "riscv32")]
pub struct Esp32s31StandaloneEspNowPhyChannelControl<'context, 'observer, D, O> {
    context: &'context mut open_esp_radio_esp32s31_wifi::runtime::Esp32s31WifiRuntimeContext,
    observer: &'observer mut O,
    _delay: PhantomData<D>,
}

#[cfg(target_arch = "riscv32")]
impl<'context, 'observer, D, O>
    Esp32s31StandaloneEspNowPhyChannelControl<'context, 'observer, D, O>
{
    pub fn new(
        context: &'context mut open_esp_radio_esp32s31_wifi::runtime::Esp32s31WifiRuntimeContext,
        observer: &'observer mut O,
    ) -> Self {
        Self {
            context,
            observer,
            _delay: PhantomData,
        }
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.context.current_channel()
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, D, O>
    Esp32s31StandaloneEspNowChannelControl<open_esp_radio_esp32s31_hal::RadioRuntimeOwner, P>
    for Esp32s31StandaloneEspNowPhyChannelControl<'_, '_, D, O>
where
    D: open_esp_radio_esp32s31_phy::PhyAsyncDelay,
    O: open_esp_radio_esp32s31_phy::PhyTargetObserver,
{
    type Error = open_esp_radio_esp32s31_phy::PhyTargetPortError;

    fn current_channel(&self) -> WifiChannel {
        Self::current_channel(self)
    }

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut open_esp_radio_esp32s31_hal::RadioRuntimeOwner,
        platform: &'a mut P,
        channel: WifiChannel,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            open_esp_radio_esp32s31_wifi::switch_esp32s31_wifi_channel::<D, _, _>(
                self.context.phy_mut(),
                channel,
                platform,
                hardware,
                self.observer,
            )
            .await?;
            self.context.set_current_channel(channel);
            Ok(())
        }
    }
}
