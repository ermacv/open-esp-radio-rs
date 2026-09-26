use core::sync::atomic::{AtomicUsize, Ordering};

use crate::controller_time::ControllerTimeWorkerPhase;

use oer_esp32s31_hal::owner::SharedPhyAccess;

use super::{
    BluetoothRadioHardware, BluetoothStopped, TeardownPendingPlatform, separate_interrupt_owner,
};

static PLATFORM_DROPS: AtomicUsize = AtomicUsize::new(0);

struct PlatformDropCounter;

impl Drop for PlatformDropCounter {
    fn drop(&mut self) {
        PLATFORM_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn pending_phy_teardown_suppresses_implicit_platform_release() {
    PLATFORM_DROPS.store(0, Ordering::Relaxed);
    drop(TeardownPendingPlatform::new(PlatformDropCounter));
    assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
}

#[test]
fn task_and_interrupt_owners_reunite_into_the_same_radio_root() {
    let stopped = BluetoothStopped::from_hardware((), BluetoothRadioHardware::for_validation());
    let (registers, ()) = stopped.into_parts();
    let (task, setup) = separate_interrupt_owner(registers);
    assert_eq!(task.controller_time_retirement_ready(), Ok(()));
    assert_eq!(
        task.controller_time_phase(),
        ControllerTimeWorkerPhase::Idle
    );
    let hardware = task
        .reunite(setup)
        .expect("untouched owners remain cold-reunitable")
        .release()
        .expect("an untouched Bluetooth route can be released");

    // Re-entering Wi-Fi proves that every inactive protocol and shared
    // owner survived the complete Bluetooth ownership roundtrip.
    let _wifi = oer_esp32s31_hal::owner::Radio::from_hardware((), hardware);
}

#[test]
fn task_retirement_keeps_a_controller_time_ownership_fault() {
    let stopped = BluetoothStopped::from_hardware((), BluetoothRadioHardware::for_validation());
    let (registers, ()) = stopped.into_parts();
    let (mut task, setup) = separate_interrupt_owner(registers);
    assert!(
        task.controller_time
            .cancel_owned(crate::controller_time::ControllerTimeRequest::for_validation(1))
            .is_err()
    );
    assert_eq!(
        task.controller_time_retirement_ready(),
        Err(crate::controller_time::ControllerTimeRetirementError::Faulted)
    );
    let _retained = (task, setup);
}

#[test]
fn mutable_shared_phy_borrow_arms_fail_stop_reunion() {
    fn accepts_shared_phy(_: &mut impl SharedPhyAccess) {}

    let stopped = BluetoothStopped::from_hardware((), BluetoothRadioHardware::for_validation());
    let (registers, ()) = stopped.into_parts();
    let (mut task, setup) = separate_interrupt_owner(registers);
    {
        let mut phy = task.shared_phy_hal();
        accepts_shared_phy(&mut phy);
    }

    let failure = match task.reunite(setup) {
        Ok(_) => panic!("a mutable shared-PHY borrow requires verified rollback"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        oer_esp32s31_hal::bluetooth::TaskOwnerReuniteError::HardwareLifecycleNotRestored
    );
}
