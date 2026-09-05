use open_esp_radio_esp32s31_pac::RadioHardware;

use super::{
    BluetoothColdOwner, BluetoothControllerHalBorrow, BluetoothControllerPublicAddress,
    BluetoothControllerRandomAddress, BluetoothRxMemoryListInitialPublication,
    BluetoothTaskOwnerReuniteError, execute_rx_memory_list_initial_publication,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RxListPublicationStep {
    CurrentHead,
    NextHeadCleared,
}

#[derive(Default)]
struct RecordingRxListPublication {
    steps: std::vec::Vec<RxListPublicationStep>,
}

impl BluetoothRxMemoryListInitialPublication for RecordingRxListPublication {
    fn publish_current_head(&mut self) {
        self.steps.push(RxListPublicationStep::CurrentHead);
    }

    fn clear_next_head(&mut self) {
        self.steps.push(RxListPublicationStep::NextHeadCleared);
    }
}

#[test]
fn receive_list_publication_finishes_the_current_head_before_clearing_next() {
    let mut transaction = RecordingRxListPublication::default();

    execute_rx_memory_list_initial_publication(&mut transaction);

    assert_eq!(
        transaction.steps,
        [
            RxListPublicationStep::CurrentHead,
            RxListPublicationStep::NextHeadCleared,
        ]
    );
}

#[test]
fn public_and_hci_random_forms_converge_on_one_controller_identity() {
    let public = BluetoothControllerPublicAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
    let random = BluetoothControllerRandomAddress::from_hci_wire_bytes([6, 5, 4, 3, 2, 1]);

    assert_eq!(
        public.controller_wire_octets(),
        random.controller_wire_octets()
    );
    assert_eq!(public.canonical_bytes(), [1, 2, 3, 4, 5, 6]);
    assert_eq!(random.hci_wire_bytes(), [6, 5, 4, 3, 2, 1]);
}

#[test]
fn untouched_task_owner_reconstructs_the_neutral_root() {
    let cold = BluetoothColdOwner::from_radio_hardware(RadioHardware::for_validation());
    let (task, interrupts) = cold.separate_interrupt_owner();
    let hardware = task
        .into_cold(interrupts)
        .expect("an untouched task owner can be reunited")
        .release()
        .expect("an untouched Bluetooth route can be released");

    // Re-entering Wi-Fi proves that the finite HAL borrow neither moved nor
    // duplicated any protocol-neutral owner.
    let _wifi = hardware.into_wifi();
}

#[test]
fn mutable_controller_borrow_arms_fail_stop_reunion() {
    let cold = BluetoothColdOwner::from_radio_hardware(RadioHardware::for_validation());
    let (mut task, interrupts) = cold.separate_interrupt_owner();
    {
        let _controller = task.borrow_bluetooth_controller();
    }

    let failure = match task.into_cold(interrupts) {
        Ok(_) => panic!("hardware rollback is required after a mutable HAL borrow"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothTaskOwnerReuniteError::HardwareLifecycleNotRestored
    );
    let _retained_owners = failure.into_parts();
}

#[test]
fn non_pristine_interrupt_history_blocks_neutral_reunion() {
    let cold = BluetoothColdOwner::from_radio_hardware(RadioHardware::for_validation());
    let (task, mut interrupts) = cold.separate_interrupt_owner();

    // This private state mutation isolates the ownership rule without
    // issuing target MMIO from a host test. The actual prepare/release
    // methods are the only production constructors of this dirty setup.
    interrupts.reunitable = false;

    let failure = match task.into_cold(interrupts) {
        Ok(_) => panic!("interrupt MMIO history requires verified rollback"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothTaskOwnerReuniteError::InterruptLifecycleNotRestored
    );
    let _retained_owners = failure.into_parts();
}
