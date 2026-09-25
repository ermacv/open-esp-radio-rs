#![expect(
    clippy::result_large_err,
    reason = "TX epoch construction returns the exact DMA owner when preparation fails"
)]

//! Embassy control-TX construction for the executor-independent STA TX epoch.

use core::pin::Pin;

use oer_esp32s31_wifi::tx::{
    ControlTxConfig, WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer,
};

use oer_esp32s31_wifi_mac::tx::{TxSlot, runtime::WifiTxRuntimePolicy};

use oer_esp32s31_wifi_sta::{
    control_tx::ControlTransmitter,
    tx_epoch::{StaTxEpoch, StaTxEpochError},
};

/// Runtime construction methods for a STA epoch whose control owner is the
/// Embassy-composed ordinary transmitter.
pub trait StaTxEpochExt<'slot, P, E, T, const BUFFER_SIZE: usize>: Sized {
    fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: ControlTxConfig,
    ) -> Self;

    fn from_slot(
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        power: P,
        entropy: E,
        timer: T,
        config: ControlTxConfig,
    ) -> Self;

    fn restore_resources(
        &mut self,
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
    ) -> Result<
        (),
        (
            StaTxEpochError,
            ControlTransmitter<'slot, P, E, T, BUFFER_SIZE>,
        ),
    >;
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> StaTxEpochExt<'slot, P, E, T, BUFFER_SIZE>
    for StaTxEpoch<ControlTransmitter<'slot, P, E, T, BUFFER_SIZE>>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: ControlTxConfig,
    ) -> Self {
        Self::from_control(ControlTransmitter::new(resources, config), config)
    }

    fn from_slot(
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        power: P,
        entropy: E,
        timer: T,
        config: ControlTxConfig,
    ) -> Self {
        Self::new(
            WifiTxResources {
                slot,
                policy: WifiTxRuntimePolicy::vendor_defaults(),
                power,
                entropy,
                timer,
            },
            config,
        )
    }

    fn restore_resources(
        &mut self,
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
    ) -> Result<
        (),
        (
            StaTxEpochError,
            ControlTransmitter<'slot, P, E, T, BUFFER_SIZE>,
        ),
    > {
        self.restore_control(ControlTransmitter::new(resources, self.config()))
    }
}
