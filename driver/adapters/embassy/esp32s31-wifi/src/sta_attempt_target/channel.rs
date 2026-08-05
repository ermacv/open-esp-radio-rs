use super::*;

/// Channel-switch capability accepted by the concrete attempt owner.
pub trait Esp32s31StaAttemptChannel<H> {
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut H,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> impl Future<Output = Result<(), PhyTargetPortError>> + 'a;
}

impl<P, O, D> Esp32s31StaAttemptChannel<RadioRegisters> for Esp32s31ScanPhy<'_, P, O, D>
where
    P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut RadioRegisters,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> impl Future<Output = Result<(), PhyTargetPortError>> + 'a {
        Esp32s31ScanPhy::switch_channel(self, channel_or_frequency, cbw, hardware)
    }
}

impl<'cell, 'registers, P, O, D>
    Esp32s31StaAttemptChannel<CooperativeRadioHardware<'cell, 'registers>>
    for Esp32s31ScanPhy<'_, P, O, D>
where
    P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut CooperativeRadioHardware<'cell, 'registers>,
        channel_or_frequency: u16,
        cbw: u8,
    ) -> impl Future<Output = Result<(), PhyTargetPortError>> + 'a {
        async move {
            let mut registers = hardware.register_cell().borrow_mut();
            Esp32s31ScanPhy::switch_channel(self, channel_or_frequency, cbw, &mut registers).await
        }
    }
}
