use core::cell::RefCell;
use std::vec::Vec;

use super::*;
use crate::power::clock::{ModemClockModule, ModemClockPlannerIdentity};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Device(ModemClockDevice, bool),
    AcquirePll,
    ReleasePll,
    AcquireAnalogI2c,
    ReleaseAnalogI2c,
}

/// One ordered log shared by the modem port and the platform provider.
#[derive(Default)]
struct Log {
    operations: Vec<Operation>,
    refuse: Option<Operation>,
}

struct Port<'log>(&'log RefCell<Log>);

impl ModemClockPort for Port<'_> {
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool) {
        self.0
            .borrow_mut()
            .operations
            .push(Operation::Device(device, enable));
    }
}

struct Platform<'log>(&'log RefCell<Log>);

impl Platform<'_> {
    fn request(&mut self, operation: Operation) -> Result<(), PlatformClockError> {
        let mut log = self.0.borrow_mut();
        if log.refuse == Some(operation) {
            return Err(PlatformClockError);
        }
        log.operations.push(operation);
        Ok(())
    }
}

impl PlatformClockProvider for Platform<'_> {
    fn acquire_pll_f160m(&mut self) -> Result<(), PlatformClockError> {
        self.request(Operation::AcquirePll)
    }
    fn release_pll_f160m(&mut self) -> Result<(), PlatformClockError> {
        self.request(Operation::ReleasePll)
    }
    fn acquire_analog_i2c_clock(&mut self) -> Result<(), PlatformClockError> {
        self.request(Operation::AcquireAnalogI2c)
    }
    fn release_analog_i2c_clock(&mut self) -> Result<(), PlatformClockError> {
        self.request(Operation::ReleaseAnalogI2c)
    }
}

#[test]
fn the_pll_source_brackets_the_modem_gate_and_the_analog_clock_goes_to_the_platform() {
    let log = RefCell::new(Log::default());
    let mut identity = ModemClockPlannerIdentity::new();
    let planner = ModemClockPlanner::managed_for_test(&mut identity);
    let prepared = planner
        .prepare_module_acquire(ModemClockModule::Phy)
        .unwrap_or_else(|_| panic!("PHY"));
    let (planner, lease) = execute_acquire(prepared, &mut Port(&log), &mut Platform(&log))
        .unwrap_or_else(|_| panic!("acquire"));
    assert_eq!(
        log.borrow().operations,
        [
            Operation::Device(ModemClockDevice::ModemAdcCommonFe, true),
            Operation::Device(ModemClockDevice::ModemPrivateFe, true),
            Operation::AcquirePll,
            Operation::Device(ModemClockDevice::PllSourceGate, true),
            Operation::AcquireAnalogI2c,
            Operation::Device(ModemClockDevice::WifiBaseband80x1, true),
        ]
    );

    log.borrow_mut().operations.clear();
    let prepared = planner
        .prepare_release(lease)
        .unwrap_or_else(|_| panic!("release"));
    let _planner = execute_release(prepared, &mut Port(&log), &mut Platform(&log))
        .unwrap_or_else(|_| panic!("release"));
    assert_eq!(
        log.borrow().operations,
        [
            Operation::Device(ModemClockDevice::ModemAdcCommonFe, false),
            Operation::Device(ModemClockDevice::ModemPrivateFe, false),
            Operation::Device(ModemClockDevice::PllSourceGate, false),
            Operation::ReleasePll,
            Operation::ReleaseAnalogI2c,
            Operation::Device(ModemClockDevice::WifiBaseband80x1, false),
        ]
    );
}

#[test]
fn a_refused_platform_request_poisons_the_transaction_before_the_gate() {
    let log = RefCell::new(Log {
        refuse: Some(Operation::AcquirePll),
        ..Log::default()
    });
    let mut identity = ModemClockPlannerIdentity::new();
    let planner = ModemClockPlanner::managed_for_test(&mut identity);
    let prepared = planner
        .prepare_module_acquire(ModemClockModule::Coexistence)
        .unwrap_or_else(|_| panic!("coexistence"));
    let Err(poisoned) = execute_acquire(prepared, &mut Port(&log), &mut Platform(&log)) else {
        panic!("a refused PLL request must poison the acquisition");
    };
    assert_eq!(poisoned.edge(), ModemClockAcquireEdge::Pll160AndModemSource);
    // The modem gate was not opened without its upstream source.
    assert!(log.borrow().operations.is_empty());
}
