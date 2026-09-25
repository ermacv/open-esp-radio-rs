use super::*;

/// Channel-switch capability accepted by the concrete attempt owner.
pub trait StaAttemptChannel<H> {
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut H,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> impl Future<Output = Result<(), PhyTargetPortError>> + 'a;
}

impl<P, O, D> StaAttemptChannel<RadioRuntimeOwner> for ScanPhy<'_, P, O, D>
where
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut RadioRuntimeOwner,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> impl Future<Output = Result<(), PhyTargetPortError>> + 'a {
        ScanPhy::switch_channel(self, channel_or_frequency, cbw, hardware)
    }
}

impl<'arena, P, O, D> StaAttemptChannel<CooperativeRadioHardware<'arena>> for ScanPhy<'_, P, O, D>
where
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    async fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut CooperativeRadioHardware<'arena>,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> Result<(), PhyTargetPortError> {
        let access = hardware.register_access();
        self.switch_published_channel(channel_or_frequency, cbw, access)
            .await
    }
}
