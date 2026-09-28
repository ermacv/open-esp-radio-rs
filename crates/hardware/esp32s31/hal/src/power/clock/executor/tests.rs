use std::{cell::RefCell, rc::Rc, vec::Vec};

use super::*;
use crate::power::clock::{ModemClockModule, ModemClockPlannerIdentity};
use crate::power::test_clocks::{ClockEvent, CountingPlatformClocks, held, holding};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Device(ModemClockDevice, bool),
    AcquirePll,
    ReleasePll,
    AcquireAnalogI2c,
    ReleaseAnalogI2c,
    Acquire(PlatformClock),
    Release(PlatformClock),
}

/// One ordered log shared by the modem port and the platform provider.
type Log = Rc<RefCell<Vec<Operation>>>;

struct Port(Log);

impl ModemClockPort for Port {
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool) {
        self.0.borrow_mut().push(Operation::Device(device, enable));
    }
}

fn setup() -> (Log, Port, CountingPlatformClocks) {
    let log = Log::default();
    let sink = log.clone();
    let platform = CountingPlatformClocks::logging(move |event| {
        sink.borrow_mut().push(match event {
            ClockEvent::Acquire(PlatformClock::Pll160m) => Operation::AcquirePll,
            ClockEvent::Acquire(PlatformClock::AnalogI2cMaster) => Operation::AcquireAnalogI2c,
            ClockEvent::Acquire(other) => Operation::Acquire(other),
            ClockEvent::Release(PlatformClock::Pll160m) => Operation::ReleasePll,
            ClockEvent::Release(PlatformClock::AnalogI2cMaster) => Operation::ReleaseAnalogI2c,
            ClockEvent::Release(other) => Operation::Release(other),
        });
    });
    (log.clone(), Port(log), platform)
}

#[test]
fn the_pll_source_brackets_the_modem_gate_and_the_analog_clock_goes_to_the_platform() {
    let (log, mut port, platform) = setup();
    let mut held_clocks = ModemPlatformClocks::default();
    let identity = ModemClockPlannerIdentity::new();
    let planner = ModemClockPlanner::managed(&identity);
    let prepared = planner
        .prepare_acquire(ModemClockModule::Phy.dependencies())
        .unwrap_or_else(|_| panic!("PHY"));
    let (planner, lease) = execute_acquire(prepared, &mut port, &mut held_clocks, &platform)
        .unwrap_or_else(|_| panic!("acquire"));
    assert_eq!(
        held(),
        holding(&[PlatformClock::Pll160m, PlatformClock::AnalogI2cMaster])
    );
    // The owner's own count is what a post-mortem snapshot reports.
    let mut counted = PlatformClockHolds::default();
    held_clocks.count_into(&mut counted);
    assert_eq!(counted, held());
    assert_eq!(
        *log.borrow(),
        [
            Operation::Device(ModemClockDevice::ModemAdcCommonFe, true),
            Operation::Device(ModemClockDevice::ModemPrivateFe, true),
            Operation::AcquirePll,
            Operation::Device(ModemClockDevice::PllSourceGate, true),
            Operation::AcquireAnalogI2c,
            Operation::Device(ModemClockDevice::WifiBaseband80x1, true),
        ]
    );

    log.borrow_mut().clear();
    let prepared = planner
        .prepare_release(lease)
        .unwrap_or_else(|_| panic!("release"));
    let _planner = execute_release(prepared, &mut port, &mut held_clocks)
        .unwrap_or_else(|_| panic!("release"));
    assert!(held().is_empty());
    assert_eq!(
        *log.borrow(),
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
    let (log, mut port, platform) = setup();
    platform.set_refused(PlatformClock::Pll160m, true);
    let mut held_clocks = ModemPlatformClocks::default();
    let identity = ModemClockPlannerIdentity::new();
    let planner = ModemClockPlanner::managed(&identity);
    let prepared = planner
        .prepare_acquire(ModemClockModule::Coexistence.dependencies())
        .unwrap_or_else(|_| panic!("coexistence"));
    let Err(poisoned) = execute_acquire(prepared, &mut port, &mut held_clocks, &platform) else {
        panic!("a refused PLL request must poison the acquisition");
    };
    assert_eq!(poisoned.dependency(), Dependency::Pll160AndModemSource);
    // The modem gate was not opened without its upstream source.
    assert!(log.borrow().is_empty());
    assert!(held().is_empty());
}

#[test]
fn a_poisoned_acquisition_keeps_its_references_until_the_owner_drops() {
    let (_, mut port, platform) = setup();
    platform.set_refused(PlatformClock::AnalogI2cMaster, true);
    let mut held_clocks = ModemPlatformClocks::default();
    let identity = ModemClockPlannerIdentity::new();
    let planner = ModemClockPlanner::managed(&identity);
    let prepared = planner
        .prepare_acquire(ModemClockModule::Phy.dependencies())
        .unwrap_or_else(|_| panic!("PHY"));
    let Err(poisoned) = execute_acquire(prepared, &mut port, &mut held_clocks, &platform) else {
        panic!("a refused analog-I2C request must poison the acquisition");
    };
    assert_eq!(poisoned.dependency(), Dependency::AnalogI2cMaster);
    // The 160 MHz source taken before the refusal stays with the poisoned
    // owner; it is released with the owner, never by a separate call.
    assert_eq!(held(), holding(&[PlatformClock::Pll160m]));
    drop(poisoned);
    assert_eq!(held(), holding(&[PlatformClock::Pll160m]));
    drop(held_clocks);
    assert!(held().is_empty());
}
