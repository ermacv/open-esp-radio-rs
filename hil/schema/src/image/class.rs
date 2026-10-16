//! HIL image identity and reproducible build feature recipes.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageClass {
    SystemWatchdog,
    /// The radio-free image with the product panic entry alone, without the
    /// HIL panic report: a panic records itself and resets the chip.
    SystemPanicReset,
    BluetoothDtm,
    BluetoothGatt,
    BluetoothSecureGatt,
    BootSmoke,
    /// The plaintext Trouble GATT image recording the Bluetooth LE trace
    /// (`bluetooth-trace`), for the scenario that checks the trace.
    DiagnosticBluetoothGattTrace,
    /// The system watchdog image that, on request, switches its USB
    /// Serial/JTAG off its pads: the board leaves USB, for the stand's
    /// recovery scenario.
    DiagnosticUsbJtagOff,
}

impl ImageClass {
    /// Effective Cargo feature selection used by both the builder and
    /// provenance, with `network`, the Cargo feature of the image's network
    /// integration.
    pub fn build_features(self, network: &str) -> String {
        if matches!(
            self,
            Self::SystemWatchdog
                | Self::SystemPanicReset
                | Self::DiagnosticUsbJtagOff
                | Self::BluetoothGatt
                | Self::DiagnosticBluetoothGattTrace
                | Self::BluetoothSecureGatt
                | Self::BluetoothDtm
        ) {
            self.runtime_features().to_owned()
        } else {
            format!("{},{}", self.runtime_features(), network)
        }
    }

    pub const ALL: [Self; 8] = [
        Self::BluetoothSecureGatt,
        Self::BluetoothGatt,
        Self::SystemWatchdog,
        Self::SystemPanicReset,
        Self::BluetoothDtm,
        Self::BootSmoke,
        Self::DiagnosticBluetoothGattTrace,
        Self::DiagnosticUsbJtagOff,
    ];

    /// Whether the image adds observers or probes that the product does
    /// not carry. Its observations can never qualify a product.
    pub const fn diagnostic(self) -> bool {
        matches!(
            self,
            Self::DiagnosticBluetoothGattTrace | Self::DiagnosticUsbJtagOff
        )
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::SystemWatchdog => "system-watchdog",
            Self::SystemPanicReset => "system-panic-reset",
            Self::BluetoothDtm => "bluetooth-dtm",
            Self::BluetoothGatt => "bluetooth-gatt",
            Self::BluetoothSecureGatt => "bluetooth-secure-gatt",
            Self::BootSmoke => "boot-smoke",
            Self::DiagnosticBluetoothGattTrace => "diagnostic-bluetooth-gatt-trace",
            Self::DiagnosticUsbJtagOff => "diagnostic-usb-jtag-off",
        }
    }

    pub const fn runtime_features(self) -> &'static str {
        match self {
            Self::BluetoothGatt => "bluetooth-gatt",
            Self::DiagnosticBluetoothGattTrace => "bluetooth-gatt,bluetooth-trace",
            Self::BluetoothSecureGatt => "bluetooth-secure-gatt",
            Self::SystemWatchdog => "system-watchdog",
            Self::SystemPanicReset => "system-panic-reset",
            Self::DiagnosticUsbJtagOff => "system-watchdog,usb-jtag-off",
            Self::BluetoothDtm => "bluetooth-hil,phy-rx-hot-sram",
            Self::BootSmoke => "boot-smoke",
        }
    }

    /// The one stage-two placement every image uses: code, data and task
    /// stacks in PSRAM, interrupt stacks in SRAM.
    pub const fn runtime_profile(self) -> &'static str {
        "psram-code-psram-data-psram-stack"
    }
}

impl std::str::FromStr for ImageClass {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|class| class.id() == value)
            .ok_or_else(|| format!("unknown image class `{value}`"))
    }
}
