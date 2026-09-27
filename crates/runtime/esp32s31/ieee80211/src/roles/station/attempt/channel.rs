use super::*;

use oer_esp32s31_hal::shared_radio::PlatformClockProvider;
use oer_esp32s31_phy::ConcurrentWifiChannelError;

use crate::roles::radio_channel::RadioChannel;

/// Channel-switch capability accepted by the concrete attempt owner.
pub trait StaAttemptChannel<H> {
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut H,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> impl Future<Output = Result<(), ConcurrentWifiChannelError>> + 'a;

    /// Publish the station's activity to the coexistence schedule.
    fn publish_coex_activity(&self, activity: WifiCoexActivity) -> impl Future<Output = ()> + '_;
}

impl<P, C, O, D> StaAttemptChannel<RadioRuntimeOwner> for RadioChannel<'_, P, C, O, D>
where
    C: PlatformClockProvider,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut RadioRuntimeOwner,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> impl Future<Output = Result<(), ConcurrentWifiChannelError>> + 'a {
        RadioChannel::switch_channel(self, channel_or_frequency, cbw, hardware)
    }

    fn publish_coex_activity(&self, activity: WifiCoexActivity) -> impl Future<Output = ()> + '_ {
        RadioChannel::publish_coex_activity(self, activity)
    }
}

impl<'arena, P, C, O, D> StaAttemptChannel<CooperativeRadioHardware<'arena>>
    for RadioChannel<'_, P, C, O, D>
where
    C: PlatformClockProvider,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    async fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut CooperativeRadioHardware<'arena>,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let access = hardware.register_access();
        self.switch_published_channel(channel_or_frequency, cbw, access)
            .await
    }

    fn publish_coex_activity(&self, activity: WifiCoexActivity) -> impl Future<Output = ()> + '_ {
        RadioChannel::publish_coex_activity(self, activity)
    }
}
