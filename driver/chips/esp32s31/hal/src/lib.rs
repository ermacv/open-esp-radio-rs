#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

use core::future::Future;

use open_esp_radio_esp32s31_pac::{
    ColdRadioRegisters, MacInterruptRegisters as PacMacInterruptRegisters,
    MacInterruptSetup as PacMacInterruptSetup,
    MacPowerInterruptRegisters as PacMacPowerInterruptRegisters, RadioRegisters,
};
pub mod analog_i2c;
pub mod channel;
pub mod coex;
pub mod pbus;
pub mod phy_agc;
pub mod phy_baseband;
pub mod phy_clock;
pub mod phy_frequency;
pub mod phy_i2c;
pub mod phy_iq_estimator;
pub mod phy_memory;
pub mod phy_power_detector;
pub mod phy_prelude;
pub mod phy_rx_dco;
pub mod phy_temperature;
pub mod power;
pub mod power_detector_platform;
pub mod radio_arena;
pub mod types;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;
pub mod wifi_bb;
pub mod wifi_mac;
pub use open_esp_radio_esp32s31_pac::{
    MacPowerWakeCause, MacTsfTimerIndex, StaBeaconMissLimit, StaBeaconMissTimeoutRaw,
    StaModemSleepLimit, StaModemWakeConfig, StaModemWakePrepareError, StaModemWakeRestore,
    StaModemWakeRestoreError, StaModemWakeRestoreFailure, StaTbttAutoPeriod,
    StaTbttWakePrepareError, StaTbttWakeRestore, StaTbttWakeRestoreError,
    StaTbttWakeRestoreFailure, StaWakeProtectEarlyTimeRaw,
};
pub use power::{PowerCheckpoint, PowerClockControl, PowerClockImages, PowerError};
pub use types::{
    CfrValue, ForcedRxGain, MacInterruptEvents, MacInterruptMask, MacInterruptSnapshot,
    MacPowerInterruptSnapshot,
};

/// Powered-lifecycle PHY capability.
///
/// The PAC owner remains private to HAL. PHY code can pass this value only to
/// named HAL operations; it cannot dereference or recover a generic register
/// block from it.
pub struct PhyHal {
    registers: ColdRadioRegisters,
}

mod sealed {
    use super::RadioRegisters;

    pub trait PhyAccess {
        fn pac(&self) -> &RadioRegisters;
        fn pac_mut(&mut self) -> &mut RadioRegisters;
    }
}

/// Sealed marker accepted by named PHY HAL operations.
///
/// External crates can use an acquired [`PhyHal`] but cannot implement this
/// trait for an arbitrary owner or use it to recover the underlying PAC.
pub trait PhyAccess: sealed::PhyAccess {}

impl sealed::PhyAccess for PhyHal {
    fn pac(&self) -> &RadioRegisters {
        self.registers.radio()
    }

    fn pac_mut(&mut self) -> &mut RadioRegisters {
        self.registers.radio_mut()
    }
}

impl PhyAccess for PhyHal {}

impl sealed::PhyAccess for RadioRegisters {
    fn pac(&self) -> &RadioRegisters {
        self
    }

    fn pac_mut(&mut self) -> &mut RadioRegisters {
        self
    }
}

impl PhyAccess for RadioRegisters {}

#[cfg(target_arch = "riscv32")]
pub(crate) fn phy_pac(access: &(impl PhyAccess + ?Sized)) -> &RadioRegisters {
    sealed::PhyAccess::pac(access)
}

pub(crate) fn phy_pac_mut(access: &mut (impl PhyAccess + ?Sized)) -> &mut RadioRegisters {
    sealed::PhyAccess::pac_mut(access)
}

impl PhyHal {
    pub fn set_phy_calibration_clock(&mut self, enabled: bool) {
        self.registers
            .radio_mut()
            .set_phy_calibration_clock(enabled);
    }

    pub fn set_rx_gain_dc_calibration(&mut self, enabled: bool) {
        self.registers
            .radio_mut()
            .set_rx_gain_dc_calibration(enabled);
    }

    pub fn configure_power_control_tone(&mut self, selector: u16, step: u8) {
        self.registers
            .radio_mut()
            .configure_power_control_tone(selector, step);
    }

    pub fn configure_calibration_tone(&mut self, enabled: bool, selector: u16, step: u8) {
        self.registers
            .radio_mut()
            .configure_calibration_tone(enabled, selector, step);
    }

    pub fn configure_tx_iq_correction(&mut self, begin: bool) {
        self.registers.radio_mut().configure_tx_iq_correction(begin);
    }

    pub fn txiq_tone_control(&mut self) -> u32 {
        self.registers.radio_mut().txiq_tone_control()
    }

    pub fn restore_txiq_tone_control(&mut self, saved: u32) {
        self.registers.radio_mut().restore_txiq_tone_control(saved);
    }

    pub fn configure_txiq_mismatch_power(
        &mut self,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    ) {
        self.registers.radio_mut().configure_txiq_mismatch_power(
            first,
            polarity,
            attenuation,
            selector,
        );
    }

    pub fn set_tx_iq_gain_coefficient(&mut self, coefficient: i8) {
        self.registers
            .radio_mut()
            .set_tx_iq_gain_coefficient(coefficient);
    }

    pub fn set_tx_iq_phase_coefficient(&mut self, coefficient: i8) {
        self.registers
            .radio_mut()
            .set_tx_iq_phase_coefficient(coefficient);
    }

    pub fn set_rx_iq_gain_coefficient(&mut self, coefficient: i8) {
        self.registers
            .radio_mut()
            .set_rx_iq_gain_coefficient(coefficient);
    }

    pub fn set_rx_iq_phase_coefficient(&mut self, coefficient: i8) {
        self.registers
            .radio_mut()
            .set_rx_iq_phase_coefficient(coefficient);
    }

    pub fn configure_rx_iq_calibration_mode(&mut self) {
        self.registers
            .radio_mut()
            .configure_rx_iq_calibration_mode();
    }

    pub fn configure_adc_rate(&mut self, rate: u32) {
        self.registers.radio_mut().configure_adc_rate(rate);
    }

    pub fn set_power_detector_tone_armed(&mut self, armed: bool) {
        self.registers
            .radio_mut()
            .set_power_detector_tone_armed(armed);
    }

    pub fn stop_power_detector_tone(&mut self) {
        self.registers.radio_mut().stop_power_detector_tone();
    }

    pub fn trigger_tx_dc_measurement(&mut self) {
        self.registers.radio_mut().trigger_tx_dc_measurement();
    }

    pub fn tx_dc_measurement_is_ready(&mut self) -> bool {
        self.registers.radio_mut().tx_dc_measurement_is_ready()
    }

    pub fn sample_tx_dc_comparators(&mut self) -> [bool; 2] {
        self.registers.radio_mut().sample_tx_dc_comparators()
    }

    pub fn clear_tx_dc_measurement(&mut self) {
        self.registers.radio_mut().clear_tx_dc_measurement();
    }

    pub fn open_frontend_baseband_internal_clocks(&mut self) {
        self.registers
            .radio_mut()
            .open_frontend_baseband_internal_clocks();
    }
}

/// Type states for the coarse radio power lifecycle.
pub mod state {
    use super::{ColdRadioRegisters, MacInterruptSetup, PhyHal, RadioRuntimeOwner};

    /// The application uniquely owns the peripheral, but the open driver has
    /// not yet established its clock/reset prerequisites.
    pub struct Owned {
        pub(super) registers: ColdRadioRegisters,
    }

    /// The radio clock/reset prerequisites have been established and finite
    /// PHY register operations may access MMIO.
    pub struct Powered {
        pub(super) registers: PhyHal,
    }

    /// Cold initialization has completed and task/ISR authority is disjoint.
    pub struct Running {
        pub(super) registers: RadioRuntimeOwner,
        pub(super) interrupts: MacInterruptSetup,
    }
}

/// Unique application-visible owner of an ESP32-S31 radio peripheral.
///
/// `P` is the integration layer's singleton token (for example,
/// `esp_hal::peripherals::WIFI`). Keeping it inside this value ties the open
/// driver's register capability to the safe peripheral owner.
pub struct Radio<P, State = state::Owned> {
    peripheral: P,
    state: State,
}

/// Opaque task-side owner of the running radio register partition.
///
/// This value can be moved between lifecycle owners and the runtime arena, but
/// it exposes no PAC owner, dereference operation, or generic register
/// callback. Finite hardware transactions are borrowed through HAL
/// capabilities.
pub struct RadioRuntimeOwner {
    registers: RadioRegisters,
}

impl RadioRuntimeOwner {
    /// Construct the running register owner inside an isolated validation
    /// image without exposing the underlying PAC owner.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn claim_for_validation() -> Self {
        Self::from_pac(open_esp_radio_esp32s31_pac::validation::radio_registers())
    }

    pub fn wifi_mac_hal(&mut self) -> wifi_mac::WifiMacHal<'_> {
        wifi_mac::WifiMacHal::from_owned(&mut self.registers)
    }

    pub fn channel_hal<'owner, P>(
        &'owner mut self,
        platform: &'owner mut P,
    ) -> channel::RadioChannelHal<'owner, P> {
        channel::RadioChannelHal::from_owned(platform, &mut self.registers)
    }

    /// Read the calibrated baseband observation without exposing the PAC
    /// owner or its register encoding.
    pub fn read_noise_floor_dbm(&self) -> i8 {
        self.registers.read_noise_floor_dbm()
    }

    pub fn access_point_receive_policy_snapshot(&self) -> wifi_mac::MacApReceivePolicySnapshot {
        self.registers.ap_receive_policy_snapshot()
    }

    pub fn receive_statistics_snapshot(&self) -> wifi_mac::MacRxStatisticsSnapshot {
        self.registers.rx_statistics_snapshot()
    }

    pub fn receive_dma_snapshot(&self) -> wifi_mac::MacRxDmaSnapshot {
        self.registers.mac_rx_dma_snapshot()
    }

    /// Latch one indirect SoftAP receive-BA bank through the reviewed HAL
    /// projection without exposing its PAC owner.
    pub fn extra_softap_rx_block_ack_entry_snapshot(
        &mut self,
        index: u8,
    ) -> Option<wifi_mac::ExtraSoftApRxBlockAckEntrySnapshot> {
        let index = types::MacExtraSoftApRxBlockAckEntryIndex::new(u32::from(index))?;
        Some(
            self.wifi_mac_hal()
                .extra_softap_rx_block_ack_entry_snapshot(index),
        )
    }

    /// Sample one ordinary direct receive-BA bank without exposing its PAC
    /// owner.
    pub fn rx_block_ack_entry_snapshot(
        &mut self,
        index: u8,
    ) -> Option<wifi_mac::RxBlockAckEntrySnapshot> {
        let index = types::MacRxBlockAckEntryIndex::new(u32::from(index))?;
        self.wifi_mac_hal().rx_block_ack_entry_snapshot(index)
    }

    /// Copy the reviewed HE transmit-vector readback for one queue.
    pub fn he_tx_vector_snapshot(&self, queue: u8) -> types::MacHeTxVectorSnapshot {
        self.registers.he_mac_tx_vector_snapshot(queue)
    }

    /// Copy the reviewed Trigger-queue readback for one reservation.
    pub fn he_trigger_based_queue_snapshot(
        &self,
        reservation: types::MacHeTbLinkReservation,
    ) -> types::MacHeTriggerTxQueueSnapshot {
        self.registers.he_trigger_based_queue_snapshot(reservation)
    }

    pub fn he_trigger_receive_diagnostics(&self) -> types::MacHeTriggerRxDiagnostics {
        self.registers.he_trigger_receive_diagnostics()
    }

    pub(crate) fn from_pac(registers: RadioRegisters) -> Self {
        Self { registers }
    }

    pub(crate) fn pac(&self) -> &RadioRegisters {
        &self.registers
    }

    pub(crate) fn pac_mut(&mut self) -> &mut RadioRegisters {
        &mut self.registers
    }
}

/// Task-side setup authority for one finite MAC interrupt epoch.
pub struct MacInterruptSetup {
    inner: PacMacInterruptSetup,
}

/// Proof that connected-STA interrupt policy was applied before activation.
pub struct ConnectedStaInterruptPrepared {
    _private: (),
}

/// Disjoint HAL capability installed in the hard MAC interrupt handler.
pub struct MacInterruptRegisters {
    inner: PacMacInterruptRegisters,
}

impl MacInterruptRegisters {
    pub fn mac_interrupt_status(&self) -> MacInterruptSnapshot {
        self.inner.mac_interrupt_status()
    }

    pub fn acknowledge_mac_interrupts(&mut self, snapshot: MacInterruptSnapshot) {
        self.inner.acknowledge_mac_interrupts(snapshot);
    }

    pub fn mask_rx_delivery_interrupts(&mut self) {
        self.inner.mask_rx_delivery_interrupts();
    }

    pub fn unmask_rx_delivery_interrupts(&mut self) {
        self.inner.unmask_rx_delivery_interrupts();
    }

    pub fn deactivate(self, power: MacPowerInterruptRegisters) -> MacInterruptSetup {
        MacInterruptSetup {
            inner: self.inner.deactivate(power.inner),
        }
    }
}

/// Construct the task-side interrupt setup owner inside an isolated probe.
///
/// The returned value is the same finite capability consumed by production
/// connected-STA setup; only its singleton acquisition is validation-only.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_interrupt_setup() -> MacInterruptSetup {
    MacInterruptSetup {
        inner: open_esp_radio_esp32s31_pac::validation::mac_interrupt_setup(),
    }
}

/// Construct the hard-MAC interrupt capability inside an isolated validation
/// image without exposing the PAC partition.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_interrupt_registers() -> MacInterruptRegisters {
    MacInterruptRegisters {
        inner: open_esp_radio_esp32s31_pac::validation::mac_interrupt_registers(),
    }
}

/// Construct the hard power-interrupt capability inside an isolated probe.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_power_interrupt_registers() -> MacPowerInterruptRegisters {
    MacPowerInterruptRegisters {
        inner: open_esp_radio_esp32s31_pac::validation::mac_power_interrupt_registers(),
    }
}

/// Disjoint HAL capability installed in the hard power interrupt handler.
pub struct MacPowerInterruptRegisters {
    inner: PacMacPowerInterruptRegisters,
}

impl MacPowerInterruptRegisters {
    pub fn mask_and_acknowledge_wake_cause(&mut self, cause: MacPowerWakeCause) {
        self.inner.mask_and_acknowledge_wake_cause(cause);
    }

    pub fn acknowledge_wake_cause(&mut self, cause: MacPowerWakeCause) {
        self.inner.acknowledge_wake_cause(cause);
    }

    pub fn power_interrupt_status(&self) -> MacPowerInterruptSnapshot {
        self.inner.power_interrupt_status()
    }

    pub fn acknowledge_power_interrupts(&mut self, snapshot: MacPowerInterruptSnapshot) {
        self.inner.acknowledge_power_interrupts(snapshot);
    }
}

impl MacInterruptSetup {
    pub fn prepare_connected_sta_without_power_save(
        &mut self,
        radio: &mut RadioRuntimeOwner,
    ) -> ConnectedStaInterruptPrepared {
        let _ = self
            .inner
            .prepare_connected_sta_without_power_save(&mut radio.registers);
        ConnectedStaInterruptPrepared { _private: () }
    }

    pub(crate) fn prepare_connected_sta_with_pac(
        &mut self,
        registers: &mut RadioRegisters,
    ) -> ConnectedStaInterruptPrepared {
        let _ = self
            .inner
            .prepare_connected_sta_without_power_save(registers);
        ConnectedStaInterruptPrepared { _private: () }
    }

    pub fn activate(
        self,
        event_mask: MacInterruptMask,
    ) -> (MacInterruptRegisters, MacPowerInterruptRegisters) {
        let (mac, power) = self.inner.activate(event_mask);
        (
            MacInterruptRegisters { inner: mac },
            MacPowerInterruptRegisters { inner: power },
        )
    }
}

/// Failed power transition retaining the unique unpowered radio owner.
pub struct PowerUpFailure<P> {
    radio: Radio<P, state::Owned>,
    error: PowerError,
}

impl<P> PowerUpFailure<P> {
    /// Inspect the exact failed read-back checkpoint.
    pub const fn error(&self) -> PowerError {
        self.error
    }

    /// Recover the owner for diagnostics, reset, or a controlled retry.
    pub fn into_radio(self) -> Radio<P, state::Owned> {
        self.radio
    }

    /// Recover both the owner and the checkpoint error.
    pub fn into_parts(self) -> (Radio<P, state::Owned>, PowerError) {
        (self.radio, self.error)
    }
}

impl<P> Radio<P, state::Owned> {
    /// Bind the integration layer's unique peripheral token to the open
    /// driver's register capability.
    ///
    /// The platform token and custom radio PAC singleton must both be free.
    /// A second claim returns the platform token unchanged.
    pub fn claim(peripheral: P) -> Result<Self, P> {
        #[cfg(not(test))]
        let Some(registers) = ColdRadioRegisters::take() else {
            return Err(peripheral);
        };
        #[cfg(test)]
        let registers = ColdRadioRegisters::for_validation();
        Ok(Self {
            peripheral,
            state: state::Owned { registers },
        })
    }

    /// Construct the complete owner inside an isolated validation image.
    ///
    /// This bypasses only the process-wide singleton acquisition required by
    /// production. The returned ownership and lifecycle API is identical.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn claim_for_validation(peripheral: P) -> Self {
        Self {
            peripheral,
            state: state::Owned {
                registers: ColdRadioRegisters::for_validation(),
            },
        }
    }

    /// Release a radio that has not crossed into the powered state.
    pub fn release(self) -> P {
        self.peripheral
    }

    /// Adopt radio clocks and resets established by an external PHY oracle.
    ///
    /// This is intentionally separate from [`Self::power_up`]: a comparison
    /// HIL may first run the vendor cold initializer and must not pulse the
    /// already calibrated radio reset merely to obtain the Rust type state.
    ///
    /// The caller is responsible for completing the modem/PHY clock, power and
    /// reset prerequisites before choosing this explicit external-init path.
    /// This is a hardware protocol precondition rather than a Rust memory-
    /// safety contract.
    pub fn assume_powered_after_external_initialization(mut self) -> Radio<P, state::Powered>
    where
        P: wifi_bb::PhyWifiBbControl,
    {
        self.state
            .registers
            .radio_mut()
            .set_wifi_baseband_enabled_image(self.peripheral.wifi_baseband_is_enabled());
        Radio {
            peripheral: self.peripheral,
            state: state::Powered {
                registers: PhyHal {
                    registers: self.state.registers,
                },
            },
        }
    }
}

impl<P: PowerClockControl> Radio<P, state::Owned> {
    /// Execute the finite modem/PHY clock and reset prerequisites.
    ///
    /// Register fields come from the official ESP32-S31 PAC. The exact
    /// operation order and field values reproduce the qualified S31 `esp-hal`
    /// clock path; the ROM-only frontend gates are a
    /// later owned PHY transition and are not folded into this type-state
    /// change.
    ///
    /// `P` owns the official platform capability. A successful read-back is
    /// the only safe path into `Radio<P, Powered>`.
    pub fn power_up(mut self) -> Result<Radio<P, state::Powered>, PowerUpFailure<P>> {
        if let Err(error) = power::execute_owned(&mut self.peripheral) {
            return Err(PowerUpFailure { radio: self, error });
        }
        Ok(Radio {
            peripheral: self.peripheral,
            state: state::Powered {
                registers: PhyHal {
                    registers: self.state.registers,
                },
            },
        })
    }
}

impl<P> Radio<P, state::Powered> {
    /// Borrow the platform and the narrow cold-MAC capability together for
    /// one lifecycle transition.
    pub fn cold_mac_parts(&mut self) -> (&mut P, wifi_mac::WifiMacColdHal<'_>) {
        (
            &mut self.peripheral,
            wifi_mac::WifiMacColdHal::from_owned(&mut self.state.registers.registers),
        )
    }

    /// Close the cold polling interrupt phase before constructing disjoint
    /// task/ISR runtime capabilities.
    pub fn close_cold_interrupt_phase(&mut self) -> u32 {
        let mask = self.state.registers.registers.mac_interrupt_enable();
        self.state
            .registers
            .registers
            .mask_and_clear_all_mac_interrupts();
        mask
    }

    /// Borrow the integration token without releasing register ownership.
    pub const fn peripheral(&self) -> &P {
        &self.peripheral
    }

    /// Borrow a narrow channel capability for one transaction.
    pub fn channel_hal(&mut self) -> channel::RadioChannelHal<'_, P> {
        channel::RadioChannelHal::from_owned(
            &mut self.peripheral,
            phy_pac_mut(&mut self.state.registers),
        )
    }

    /// Borrow the platform and PHY capability independently.
    ///
    /// The two mutable borrows are tied to this unique powered owner and refer
    /// to disjoint fields, allowing a lifecycle function to coordinate an
    /// official system operation with internal Wi-Fi MMIO.
    pub fn phy_hal_parts(&mut self) -> (&mut P, &mut PhyHal) {
        (&mut self.peripheral, &mut self.state.registers)
    }

    /// Enable the Wi-Fi RX/baseband path after the PHY transition completes.
    ///
    /// Espressif's `enable_phy_with_wifi_rx` lifecycle wrapper performs this
    /// operation after `register_chipv7_phy` or `phy_wakeup_init`.  Keeping it
    /// on the powered owner makes that final lifecycle edge explicit and
    /// prevents application code from writing `WIFI_BB_CFG` without owning the
    /// radio peripheral.
    /// Internal PHY capability used by source-owned target bindings.
    ///
    /// The returned borrow cannot outlive the unique powered radio owner.
    pub fn phy_hal_mut(&mut self) -> &mut PhyHal {
        &mut self.state.registers
    }
}

impl<P> Radio<P, state::Powered> {
    /// Complete the one-way ownership transition after cold MAC setup.
    pub fn into_running(self) -> Radio<P, state::Running> {
        let (registers, interrupts) = self.state.registers.registers.into_running();
        Radio {
            peripheral: self.peripheral,
            state: state::Running {
                registers: RadioRuntimeOwner::from_pac(registers),
                interrupts: MacInterruptSetup { inner: interrupts },
            },
        }
    }
}

impl<P> Radio<P, state::Running> {
    pub const fn peripheral(&self) -> &P {
        &self.peripheral
    }

    pub fn channel_hal(&mut self) -> channel::RadioChannelHal<'_, P> {
        self.state.registers.channel_hal(&mut self.peripheral)
    }

    pub fn wifi_mac_hal(&mut self) -> wifi_mac::WifiMacHal<'_> {
        self.state.registers.wifi_mac_hal()
    }

    /// Split only at the role-epoch ownership boundary. Both returned
    /// capabilities remain opaque and can be recombined only through
    /// [`Self::from_runtime_parts`].
    pub fn into_runtime_parts(self) -> (P, RadioRuntimeOwner, MacInterruptSetup) {
        (self.peripheral, self.state.registers, self.state.interrupts)
    }

    pub fn from_runtime_parts(
        peripheral: P,
        registers: RadioRuntimeOwner,
        interrupts: MacInterruptSetup,
    ) -> Self {
        Self {
            peripheral,
            state: state::Running {
                registers,
                interrupts,
            },
        }
    }
}

impl<P: wifi_bb::PhyWifiBbControl> Radio<P, state::Powered> {
    /// Enable the Wi-Fi RX/baseband path after the PHY transition completes.
    ///
    /// The official system register and the PBus-visible owned state are
    /// updated together under the unique radio owner.
    #[cfg(target_arch = "riscv32")]
    pub fn enable_wifi_rx(&mut self) {
        let (platform, registers) = self.phy_hal_parts();
        phy_frequency::set_wifi_enabled(platform, registers, true);
    }
}

/// Executor-neutral source of asynchronous deadlines.
pub trait AsyncDelay {
    type Error;

    fn delay_micros(&mut self, micros: u32) -> impl Future<Output = Result<(), Self::Error>> + '_;
}

/// Executor-neutral interrupt/event edge.
pub trait AsyncEvent {
    type Event;
    type Error;

    fn wait(&mut self) -> impl Future<Output = Result<Self::Event, Self::Error>> + '_;
}

#[cfg(test)]
mod tests {
    use super::{PowerClockControl, PowerClockImages, Radio, state};

    #[derive(Debug, Eq, PartialEq)]
    struct TestPeripheral {
        id: u8,
        ready: bool,
    }

    fn require_owned(_: &Radio<TestPeripheral, state::Owned>) {}
    fn require_powered(_: &Radio<TestPeripheral, state::Powered>) {}

    impl PowerClockControl for TestPeripheral {
        fn set_wifi_baseband_and_mac_reset(&mut self, _asserted: bool) {}
        fn select_hp_active_modem_icg(&mut self) {}
        fn apply_modem_icg_selection(&mut self) {}
        fn apply_sleep_icg_selection(&mut self) {}
        fn enable_modem_register_bus_clock(&mut self) {}
        fn configure_hp_active_modem_clock_map(&mut self) {}
        fn configure_shared_modem_clock_map(&mut self) {}
        fn configure_modem_source_clocks(&mut self) {}
        fn set_wifi_baseband_reset(&mut self, _asserted: bool) {}
        fn enable_phy_calibration_clocks(&mut self) {}
        fn select_phy_i2c_160mhz_source(&mut self) {}
        fn enable_phy_i2c_master_clock(&mut self) {}

        fn power_clock_images(&self) -> PowerClockImages {
            PowerClockImages {
                reset_released: self.ready,
                hp_active_icg_selected: self.ready,
                modem_bus_clock_enabled: self.ready,
                hp_active_clock_map_configured: self.ready,
                shared_clock_map_configured: self.ready,
                modem_source_clocks_configured: self.ready,
                phy_calibration_clocks_enabled: self.ready,
                phy_i2c_160mhz_selected: self.ready,
                phy_i2c_master_clock_enabled: self.ready,
            }
        }
    }

    impl crate::wifi_bb::PhyWifiBbControl for TestPeripheral {
        fn clear_cold_start_wifi_control(&mut self) {}
        fn wifi_baseband_is_enabled(&self) -> bool {
            false
        }
        fn set_wifi_baseband_enabled(&mut self, _enabled: bool) {}
        fn set_bss_cbw_40_digital(&mut self, _enabled: bool) {}
        fn set_bb_agc_update_encoding(&mut self, _encoding: u8) {}
        fn set_mac_baseband_enabled(&mut self, _enabled: bool) {}
    }

    #[test]
    fn peripheral_token_follows_the_type_state_owner() {
        let owned = Radio::claim(TestPeripheral { id: 7, ready: true })
            .unwrap_or_else(|_| panic!("test radio claim failed"));
        require_owned(&owned);

        let powered = owned
            .power_up()
            .unwrap_or_else(|_| panic!("fake prerequisite sequence failed"));
        require_powered(&powered);
        assert_eq!(powered.peripheral(), &TestPeripheral { id: 7, ready: true });
    }

    #[test]
    fn external_initialization_bridge_preserves_the_unique_owner() {
        let owned = Radio::claim(TestPeripheral { id: 8, ready: true })
            .unwrap_or_else(|_| panic!("test radio claim failed"));
        require_owned(&owned);

        let powered = owned.assume_powered_after_external_initialization();
        require_powered(&powered);
        assert_eq!(powered.peripheral(), &TestPeripheral { id: 8, ready: true });
    }

    #[test]
    fn channel_capability_is_a_temporary_borrow_not_a_consuming_split() {
        let owned = Radio::claim(TestPeripheral {
            id: 10,
            ready: true,
        })
        .unwrap_or_else(|_| panic!("test radio claim failed"));
        let mut powered = owned.assume_powered_after_external_initialization();
        let channel = powered.channel_hal();
        drop(channel);
        assert_eq!(powered.peripheral().id, 10);
    }

    #[test]
    fn unpowered_owner_can_release_the_original_token() {
        let owned = Radio::claim(TestPeripheral { id: 9, ready: true })
            .unwrap_or_else(|_| panic!("test radio claim failed"));
        assert_eq!(owned.release(), TestPeripheral { id: 9, ready: true });
    }

    #[test]
    fn failed_power_transition_returns_the_unique_owned_radio() {
        let owned = Radio::claim(TestPeripheral {
            id: 11,
            ready: false,
        })
        .unwrap_or_else(|_| panic!("test radio claim failed"));
        let failure = match owned.power_up() {
            Ok(_) => panic!("stuck reset unexpectedly powered the radio"),
            Err(failure) => failure,
        };
        let recovered = failure.into_radio();
        require_owned(&recovered);
        assert_eq!(
            recovered.release(),
            TestPeripheral {
                id: 11,
                ready: false
            }
        );
    }
}
