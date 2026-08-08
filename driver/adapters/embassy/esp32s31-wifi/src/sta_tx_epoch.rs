//! Embassy control-TX construction for the executor-independent STA TX epoch.

use core::pin::Pin;

use open_esp_radio_esp32s31_wifi_mac::{tx::TxSlot, tx_runtime::StaTxRuntimePolicy};
use open_esp_radio_esp32s31_wifi_sta::{
    control_tx::Esp32s31ControlTx,
    tx::{ControlTxConfig, WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer},
    tx_epoch::{Esp32s31StaTxEpoch, Esp32s31StaTxEpochError},
};

/// Runtime construction methods for a STA epoch whose control owner is the
/// Embassy-composed ordinary transmitter.
pub trait Esp32s31StaTxEpochExt<'slot, P, E, T, const BUFFER_SIZE: usize>: Sized {
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
            Esp32s31StaTxEpochError,
            Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
        ),
    >;
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> Esp32s31StaTxEpochExt<'slot, P, E, T, BUFFER_SIZE>
    for Esp32s31StaTxEpoch<Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: ControlTxConfig,
    ) -> Self {
        Self::from_control(Esp32s31ControlTx::new(resources, config), config)
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
                policy: StaTxRuntimePolicy::vendor_defaults(),
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
            Esp32s31StaTxEpochError,
            Esp32s31ControlTx<'slot, P, E, T, BUFFER_SIZE>,
        ),
    > {
        self.restore_control(Esp32s31ControlTx::new(resources, self.config()))
    }
}
