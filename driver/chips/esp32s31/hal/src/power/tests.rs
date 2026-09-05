use std::{cell::RefCell, rc::Rc, vec::Vec};

use open_esp_radio_esp32s31_pac::{PlatformClockPowerObservation, SharedModemClockObservation};

use super::{PowerCheckpoint, PowerError, PowerSequenceBackend, execute_owned};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    ResetWifi(bool),
    SelectHpActiveIcg,
    ApplyModemIcg,
    ApplySleepIcg,
    EnableModemBus,
    ConfigureHpActiveMap,
    PrepareSharedMap,
    ConfigureModemSource,
    ResetBaseband(bool),
    EnablePhyClocks,
    SelectI2c160Mhz,
    RetainI2cClock,
}

struct FakeShared {
    operations: Rc<RefCell<Vec<Operation>>>,
    prepare_calls: u8,
    retain_calls: u8,
    platform: PlatformClockPowerObservation,
    modem: open_esp_radio_esp32s31_pac::ModemSysconPowerObservation,
    observation: SharedModemClockObservation,
}

impl FakeShared {
    fn ready(operations: Rc<RefCell<Vec<Operation>>>) -> Self {
        Self {
            operations,
            prepare_calls: 0,
            retain_calls: 0,
            platform: PlatformClockPowerObservation {
                hp_active_icg_selected: true,
                modem_register_bus_clock_enabled: true,
                ref_160m_clock_enabled: true,
                modem_source_clocks_configured: true,
            },
            modem: open_esp_radio_esp32s31_pac::ModemSysconPowerObservation {
                wifi_reset_released: true,
                active_clock_map_configured: true,
                phy_calibration_clocks_enabled: true,
                phy_i2c_160mhz_selected: true,
            },
            observation: SharedModemClockObservation {
                power_state_map_configured: true,
                coexistence_clock_enabled: false,
                phy_i2c_master_clock_enabled: true,
                low_power_timer_clock_enabled: false,
            },
        }
    }
}

impl PowerSequenceBackend for FakeShared {
    fn select_hp_active_modem_icg(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::SelectHpActiveIcg);
    }

    fn apply_modem_icg_selection(&mut self) {
        self.operations.borrow_mut().push(Operation::ApplyModemIcg);
    }

    fn apply_sleep_icg_selection(&mut self) {
        self.operations.borrow_mut().push(Operation::ApplySleepIcg);
    }

    fn enable_modem_register_bus_clock(&mut self) {
        self.operations.borrow_mut().push(Operation::EnableModemBus);
    }

    fn configure_modem_source_clocks(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::ConfigureModemSource);
    }

    fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        self.platform
    }

    fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        self.operations
            .borrow_mut()
            .push(Operation::ResetWifi(asserted));
    }

    fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        self.operations
            .borrow_mut()
            .push(Operation::ResetBaseband(asserted));
    }

    fn configure_wifi_power_clock_map(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::ConfigureHpActiveMap);
    }

    fn enable_phy_calibration_clocks(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::EnablePhyClocks);
    }

    fn select_phy_i2c_160mhz_source(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::SelectI2c160Mhz);
    }

    fn modem_syscon_power_observation(
        &self,
    ) -> open_esp_radio_esp32s31_pac::ModemSysconPowerObservation {
        self.modem
    }

    fn prepare_shared_modem_clock_map(&mut self) {
        self.operations
            .borrow_mut()
            .push(Operation::PrepareSharedMap);
        self.prepare_calls += 1;
    }

    fn retain_phy_i2c_master_clock(&mut self) {
        self.operations.borrow_mut().push(Operation::RetainI2cClock);
        self.retain_calls += 1;
    }

    fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        self.observation
    }
}

#[test]
fn exact_semantic_sequence_is_finite_and_ordered() {
    let operations = Rc::new(RefCell::new(Vec::new()));
    let mut shared = FakeShared::ready(operations.clone());
    assert_eq!(execute_owned(&mut shared), Ok(()));
    assert_eq!(
        operations.borrow().as_slice(),
        [
            Operation::ResetWifi(true),
            Operation::ResetWifi(false),
            Operation::SelectHpActiveIcg,
            Operation::ApplyModemIcg,
            Operation::ApplySleepIcg,
            Operation::EnableModemBus,
            Operation::ConfigureHpActiveMap,
            Operation::PrepareSharedMap,
            Operation::ConfigureModemSource,
            Operation::ResetBaseband(true),
            Operation::ResetBaseband(false),
            Operation::EnablePhyClocks,
            Operation::SelectI2c160Mhz,
            Operation::RetainI2cClock,
        ]
    );
    assert_eq!((shared.prepare_calls, shared.retain_calls), (1, 1));
}

#[test]
fn failed_semantic_readback_names_the_exact_checkpoint() {
    let operations = Rc::new(RefCell::new(Vec::new()));
    let mut shared = FakeShared::ready(operations);
    shared.platform.modem_source_clocks_configured = false;

    assert_eq!(
        execute_owned(&mut shared),
        Err(PowerError {
            checkpoint: PowerCheckpoint::ModemClockSource,
            expected: true,
            observed: false,
        })
    );
}
