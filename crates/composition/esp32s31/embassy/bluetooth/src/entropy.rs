//! Binding the portable HCI entropy contract to a separately owned SoC service.

use oer_bluetooth_hci::{LeRandomSource, LeRandomUnavailable};
pub use oer_esp32s31_soc_esp_hal::entropy::Entropy;

/// Caller-owned entropy binding, independent of the Bluetooth Controller epoch.
///
/// Retain this service outside Host/Controller resources and install it before
/// polling the Host. The SoC owner, not the radio driver, retains the RNG
/// peripheral and independent LP TRNG source.
pub struct BluetoothEntropy<'d> {
    entropy: Entropy<'d>,
}

impl<'d> BluetoothEntropy<'d> {
    /// Bind an already-owned SoC entropy service without transferring it to radio.
    pub fn new(entropy: Entropy<'d>) -> Self {
        Self { entropy }
    }
}

impl LeRandomSource for BluetoothEntropy<'_> {
    fn random_bytes(&self) -> Result<[u8; 8], LeRandomUnavailable> {
        self.entropy.random_bytes().map_err(|_| LeRandomUnavailable)
    }
}
