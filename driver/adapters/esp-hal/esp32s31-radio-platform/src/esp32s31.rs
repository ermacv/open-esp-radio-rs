//! ESP32-S31 official singleton witnesses behind a role-neutral platform owner.
//!
//! The Bluetooth lease follows the pinned ESP-IDF controller lifecycle in
//! `components/bt/controller/esp32s31/bt.c`, `btdm_lp.c`, and the S31 modem
//! clock implementation. These paths define semantics; the PAC stays private.

use esp_hal::{
    efuse,
    peripherals::{
        HP_SYS_CLKRST, I2C_ANA_MST, LP_AON_CLK_RST, LP_PERI, LP_TSENS, MODEM_LPCON, MODEM_SYSCON,
        PMU,
    },
};
use open_esp_radio_bluetooth_hci::BluetoothPublicDeviceAddress;

use crate::{
    bluetooth_address::bluetooth_public_address_from_base,
    coordinator::{BluetoothPlatformBusy, BluetoothPlatformLease, ClockCoordinator},
};

/// Sole safe owner of the ESP32-S31 shared radio-platform singletons.
///
/// Construction consumes the official ESP-HAL singleton tokens. The tokens
/// stay private for the coordinator's whole lifetime; safe code can only issue
/// semantic leases. Existing Wi-Fi code cannot be composed simultaneously
/// until it too consumes a lease from this coordinator, because its current
/// separate adapter requires these same non-duplicable tokens.
pub struct EspHalRadioPlatform {
    _modem_syscon: MODEM_SYSCON<'static>,
    _modem_lpcon: MODEM_LPCON<'static>,
    _hp_sys_clkrst: HP_SYS_CLKRST<'static>,
    _pmu: PMU<'static>,
    _lp_aon_clkrst: LP_AON_CLK_RST<'static>,
    _lp_peri: LP_PERI<'static>,
    _lp_tsens: LP_TSENS<'static>,
    _i2c_ana_mst: I2C_ANA_MST<'static>,
    coordinator: ClockCoordinator,
}

impl EspHalRadioPlatform {
    /// Establish the neutral radio-platform owner after `esp_hal::init`.
    #[allow(
        clippy::too_many_arguments,
        reason = "construction consumes the complete non-groupable ESP-HAL singleton set"
    )]
    pub const fn new(
        modem_syscon: MODEM_SYSCON<'static>,
        modem_lpcon: MODEM_LPCON<'static>,
        hp_sys_clkrst: HP_SYS_CLKRST<'static>,
        pmu: PMU<'static>,
        lp_aon_clkrst: LP_AON_CLK_RST<'static>,
        lp_peri: LP_PERI<'static>,
        lp_tsens: LP_TSENS<'static>,
        i2c_ana_mst: I2C_ANA_MST<'static>,
    ) -> Self {
        Self {
            _modem_syscon: modem_syscon,
            _modem_lpcon: modem_lpcon,
            _hp_sys_clkrst: hp_sys_clkrst,
            _pmu: pmu,
            _lp_aon_clkrst: lp_aon_clkrst,
            _lp_peri: lp_peri,
            _lp_tsens: lp_tsens,
            _i2c_ana_mst: i2c_ana_mst,
            coordinator: ClockCoordinator::new(),
        }
    }

    /// Reserve the only standalone Bluetooth clock lifecycle slot.
    ///
    /// Every clock dependency is retained by the affine custom-PAC route; this
    /// reservation only keeps the official singleton witnesses exclusive.
    pub fn try_bluetooth(&self) -> Result<EspHalBluetoothPlatform<'_>, BluetoothPlatformBusy> {
        self.coordinator
            .try_bluetooth()
            .map(|inner| EspHalBluetoothPlatform { _inner: inner })
    }
}

/// Affine ESP-HAL platform witness consumed by Bluetooth typestate.
///
/// This type deliberately exposes neither peripheral singleton tokens nor PAC
/// register blocks. Dropping it releases only the behavioral reservation;
/// custom-PAC typestate owns all hardware cleanup.
pub struct EspHalBluetoothPlatform<'a> {
    _inner: BluetoothPlatformLease<'a>,
}

impl EspHalBluetoothPlatform<'_> {
    /// Read the factory base identity through ESP-HAL's safe eFuse accessor,
    /// apply the ESP32-S31 second-universal-address policy and retain the
    /// result in canonical EUI-48 order.
    ///
    /// The reviewed ESP32-S31 Controller HCI initializer requests the Bluetooth
    /// interface identity, then reverses those six canonical bytes for its HCI
    /// storage.
    /// [`BluetoothPublicDeviceAddress`] owns that protocol conversion, so this
    /// platform boundary neither exposes eFuse fields nor asks callers to
    /// hand-author HCI byte order.
    pub fn bluetooth_public_address(&self) -> BluetoothPublicDeviceAddress {
        let base = efuse::base_mac_address();
        let bytes = base.as_bytes();
        bluetooth_public_address_from_base([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
        ])
    }
}
