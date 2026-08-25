//! Exact controller-register prefix for the modem low-power timer interrupt.
//!
//! The transactions in this module are the complete MMIO prefix immediately
//! before the ESP32-S31 controller installs interrupt source 127 and the
//! bounded register classifier at the start of that source's handler. They do
//! not initialize the vendor software environment, publish ISR storage,
//! install a CPU route, implement the software timer handler, or claim that
//! the timer, controller, Link Layer, or HCI is live.

#![deny(unsafe_code)]

use super::{BluetoothTaskRegisters, device_fence};

/// Affine proof that every external prerequisite of the modem low-power timer
/// register transaction has been established for the same controller owner.
///
/// The PAC cannot derive this proof: controller clocks, task software state,
/// scheduler/event-list initialization, HCI software initialization, ISR
/// storage, and CPU-route absence are owned by higher lifecycle layers.
///
/// This proof cannot be reused:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::BluetoothModemLpTimerInitializationPrerequisite;
///
/// fn consume(_: BluetoothModemLpTimerInitializationPrerequisite) {}
///
/// fn reuse(prerequisite: BluetoothModemLpTimerInitializationPrerequisite) {
///     consume(prerequisite);
///     consume(prerequisite);
/// }
/// ```
#[must_use = "the modem LP-timer prerequisite must be consumed by its exact transaction"]
pub struct BluetoothModemLpTimerInitializationPrerequisite {
    _private: (),
}

impl BluetoothModemLpTimerInitializationPrerequisite {
    /// Assume every external prerequisite for one register transaction.
    ///
    /// # Safety
    ///
    /// The caller must retain the enabled controller and low-power timer
    /// clocks, completed task/HAL/scheduler/HCI initialization, stable timer
    /// software environment, no in-flight controller-time latch, inactive
    /// source-127 CPU route, and the same unique Bluetooth task owner. No
    /// controller-time operation or other consumer may concurrently access
    /// `BTDM_RUNTIME_CONTROL`.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "construction is the explicit cross-crate modem LP-timer lifecycle boundary"
    )]
    pub unsafe fn assume_satisfied() -> Self {
        Self { _private: () }
    }
}

/// Task ownership after the exact source-127 controller-register prefix.
///
/// This state proves only that the eight reviewed MMIO operations completed
/// and were followed by a device fence. The vendor path still publishes a RAM
/// flag and installs the CPU route after this prefix.
///
/// There is deliberately no conversion back to the ordinary task or cold
/// owner because the reviewed teardown does not restore these register
/// images:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::BluetoothModemLpTimerRegistersPrepared;
///
/// fn bypass_teardown(prepared: BluetoothModemLpTimerRegistersPrepared) {
///     let _task = prepared.into_task();
/// }
/// ```
///
/// The prepared owner is affine:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::BluetoothModemLpTimerRegistersPrepared;
///
/// fn duplicate(prepared: BluetoothModemLpTimerRegistersPrepared) {
///     let _second = prepared.clone();
/// }
/// ```
#[must_use = "the prepared modem LP-timer registers must continue through route setup"]
pub struct BluetoothModemLpTimerRegistersPrepared {
    task: BluetoothTaskRegisters,
}

/// Affine proof of one entry into the source-127 hard handler.
///
/// This value is intentionally neither `Copy` nor `Clone`. One CPU interrupt
/// entry can authorize exactly one bounded register-classification step:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::BluetoothModemLpTimerInterruptEvent;
///
/// fn duplicate(event: BluetoothModemLpTimerInterruptEvent) {
///     let _first = event;
///     let _second = event;
/// }
/// ```
#[must_use = "one source-127 event must be consumed by its bounded PAC step"]
pub struct BluetoothModemLpTimerInterruptEvent {
    _private: (),
}

impl BluetoothModemLpTimerInterruptEvent {
    /// Assume execution is inside one source-127 interrupt entry.
    ///
    /// # Safety
    ///
    /// The caller must be the installed source-127 hard handler on the
    /// controller core. Its stable ISR storage must exclusively own the
    /// matching [`BluetoothModemLpTimerInterruptReady`] value, the CPU route
    /// must be active, and no task or other interrupt may concurrently access
    /// `BTDM_RUNTIME_CONTROL`. The caller must construct this proof at most
    /// once for that hardware interrupt entry.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "construction is the explicit source-127 hard-handler boundary"
    )]
    pub unsafe fn assume_pending() -> Self {
        Self { _private: () }
    }
}

/// Source-127 registers staged for exclusive placement in stable ISR storage.
///
/// The task owner remains private and cannot concurrently escape back to task
/// context. A spurious hardware entry returns this state unchanged; a real
/// timer dispatch consumes it into a terminal handler-pending state.
#[must_use = "the source-127 owner must remain in stable ISR storage"]
pub struct BluetoothModemLpTimerInterruptReady {
    task: BluetoothTaskRegisters,
}

/// Exact positional path by which source 127 reached its software handler.
///
/// These names deliberately preserve register positions rather than assign
/// undocumented RTC or timer semantics to the observed hardware values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerInterruptObservation {
    /// `STATUS_0038` was nonzero and `VALUE_006C` was zero.
    Value006cZero,
    /// `STATUS_0038` and `VALUE_006C` were nonzero; `CONTROL_0058.bit2` was
    /// clear.
    Control2Clear,
    /// `CONTROL_0058.bit2` was set and a second fresh read supplied the image
    /// in which `CONTROL_0058.bit1` was published.
    Control2SetControl1Published,
}

/// Source-127 ownership after the register prefix selected the software timer
/// handler.
///
/// The open software timer-handler body is not implemented yet. Consequently
/// this owner exposes only its positional observation and deliberately cannot
/// be rearmed, returned to task context or discarded into an apparently ready
/// interrupt owner.
#[must_use = "the required modem LP-timer software handler remains pending"]
pub struct BluetoothModemLpTimerHandlerPending {
    _ready: BluetoothModemLpTimerInterruptReady,
    observation: BluetoothModemLpTimerInterruptObservation,
}

impl BluetoothModemLpTimerHandlerPending {
    /// Return the exact register path which requires timer-handler dispatch.
    pub const fn observation(&self) -> BluetoothModemLpTimerInterruptObservation {
        self.observation
    }
}

/// Result of one finite source-127 handler prefix.
#[must_use = "retain the ready owner or complete the required software handler"]
pub enum BluetoothModemLpTimerInterruptStep {
    /// `STATUS_0038` was zero; the handler returned without further MMIO.
    Spurious(BluetoothModemLpTimerInterruptReady),
    /// A reviewed branch requires the common software timer handler.
    HandlerPending(BluetoothModemLpTimerHandlerPending),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModemLpTimerRegister {
    Command004c,
    Command0004,
    Command0008,
    Command0014,
    Command0034,
    Command0010,
}

trait ModemLpTimerTransaction {
    fn write_complete(&mut self, register: ModemLpTimerRegister, image: u32);
    fn set_control_0078_bit_25(&mut self);
    fn fence(&mut self);
}

trait ModemLpTimerInterruptTransaction {
    fn read_status_0038(&mut self) -> u32;
    fn read_value_006c(&mut self) -> u32;
    fn control_0058_bit_2_is_set(&mut self) -> bool;
    fn set_control_0058_bit_1_from_fresh_read(&mut self);
    fn fence(&mut self);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModemLpTimerInterruptDisposition {
    Spurious,
    HandlerPending(BluetoothModemLpTimerInterruptObservation),
}

fn execute_modem_lp_timer_interrupt(
    transaction: &mut impl ModemLpTimerInterruptTransaction,
) -> ModemLpTimerInterruptDisposition {
    if transaction.read_status_0038() == 0 {
        return ModemLpTimerInterruptDisposition::Spurious;
    }

    let observation = if transaction.read_value_006c() == 0 {
        BluetoothModemLpTimerInterruptObservation::Value006cZero
    } else if !transaction.control_0058_bit_2_is_set() {
        BluetoothModemLpTimerInterruptObservation::Control2Clear
    } else {
        transaction.set_control_0058_bit_1_from_fresh_read();
        BluetoothModemLpTimerInterruptObservation::Control2SetControl1Published
    };
    transaction.fence();
    ModemLpTimerInterruptDisposition::HandlerPending(observation)
}

fn execute_modem_lp_timer_prepare(transaction: &mut impl ModemLpTimerTransaction) {
    transaction.write_complete(ModemLpTimerRegister::Command004c, 0);
    transaction.set_control_0078_bit_25();
    transaction.write_complete(ModemLpTimerRegister::Command0004, 1);
    transaction.write_complete(ModemLpTimerRegister::Command0008, 1);
    transaction.write_complete(ModemLpTimerRegister::Command0014, u32::MAX);
    transaction.write_complete(ModemLpTimerRegister::Command0034, u32::MAX);
    transaction.write_complete(ModemLpTimerRegister::Command0010, 2);
    transaction.fence();
}

struct HardwareModemLpTimerTransaction<'registers> {
    registers: &'registers super::svd::BtdmRuntimeControl,
}

impl ModemLpTimerTransaction for HardwareModemLpTimerTransaction<'_> {
    #[allow(
        unsafe_code,
        reason = "the private executor supplies only the seven reviewed complete register images"
    )]
    fn write_complete(&mut self, register: ModemLpTimerRegister, image: u32) {
        macro_rules! write_image {
            ($register:expr) => {{
                // SAFETY: every call is closed inside
                // `execute_modem_lp_timer_prepare`; its exact finite image and
                // position are regression-tested below.
                unsafe {
                    $register.write_with_zero(|writer| writer.image().bits(image));
                }
            }};
        }

        match register {
            ModemLpTimerRegister::Command004c => {
                write_image!(self.registers.command_004c())
            }
            ModemLpTimerRegister::Command0004 => {
                write_image!(self.registers.command_0004())
            }
            ModemLpTimerRegister::Command0008 => {
                write_image!(self.registers.command_0008())
            }
            ModemLpTimerRegister::Command0014 => {
                write_image!(self.registers.command_0014())
            }
            ModemLpTimerRegister::Command0034 => {
                write_image!(self.registers.command_0034())
            }
            ModemLpTimerRegister::Command0010 => {
                write_image!(self.registers.command_0010())
            }
        }
    }

    fn set_control_0078_bit_25(&mut self) {
        self.registers
            .control_0078()
            .modify(|_, writer| writer.control_25().set_bit());
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl ModemLpTimerInterruptTransaction for HardwareModemLpTimerTransaction<'_> {
    fn read_status_0038(&mut self) -> u32 {
        self.registers.status_0038().read().bits()
    }

    fn read_value_006c(&mut self) -> u32 {
        self.registers.value_006c().read().bits()
    }

    fn control_0058_bit_2_is_set(&mut self) -> bool {
        self.registers
            .control_0058()
            .read()
            .control_2()
            .bit_is_set()
    }

    fn set_control_0058_bit_1_from_fresh_read(&mut self) {
        self.registers
            .control_0058()
            .modify(|_, writer| writer.control_1().set_bit());
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl BluetoothTaskRegisters {
    /// Apply the exact controller-register prefix before source 127 is routed.
    ///
    /// SOURCE: public ESP32-S31 `libbtdm_common.a` member `9.o`, complete
    /// symbol `r_sym_bt_waDX0omCE7oLuPVSoPOK`, role-mapped by the public
    /// same-chip unobfuscated `r_btdm_hal_rtc_init`. The body performs seven
    /// writes and one fresh read in the exact order encoded by this module.
    /// A single device fence follows the last write.
    ///
    /// This transition consumes the ordinary task owner before the first MMIO
    /// effect. Cancellation or panic can therefore only lose authority
    /// fail-stop; it cannot recover an apparently cold owner after a partial
    /// transaction. The returned state exposes no rollback because the
    /// reviewed source-127 teardown does not reverse these eight operations.
    #[doc(hidden)]
    pub fn prepare_modem_lp_timer_registers(
        self,
        _prerequisite: BluetoothModemLpTimerInitializationPrerequisite,
    ) -> BluetoothModemLpTimerRegistersPrepared {
        {
            let mut transaction = HardwareModemLpTimerTransaction {
                registers: &self.bluetooth.btdm_runtime_control,
            };
            execute_modem_lp_timer_prepare(&mut transaction);
        }
        BluetoothModemLpTimerRegistersPrepared { task: self }
    }
}

impl BluetoothModemLpTimerRegistersPrepared {
    /// Move the unique task-side register owner into source-127 ISR storage.
    ///
    /// This transition performs no MMIO. The platform must store the returned
    /// value before enabling the CPU route and recover it only after that route
    /// is disabled and no hard handler remains in flight.
    pub fn stage_for_interrupt(self) -> BluetoothModemLpTimerInterruptReady {
        BluetoothModemLpTimerInterruptReady { task: self.task }
    }
}

impl BluetoothModemLpTimerInterruptReady {
    /// Execute exactly one source-127 register-classification prefix.
    ///
    /// The method never waits, loops, allocates or calls software. A zero
    /// `STATUS_0038` returns the ready owner after one read. Every other branch
    /// is fenced and retains the unique owner in
    /// [`BluetoothModemLpTimerHandlerPending`] until the common timer-handler
    /// body is implemented.
    pub fn step(
        self,
        _event: BluetoothModemLpTimerInterruptEvent,
    ) -> BluetoothModemLpTimerInterruptStep {
        let disposition = {
            let mut transaction = HardwareModemLpTimerTransaction {
                registers: &self.task.bluetooth.btdm_runtime_control,
            };
            execute_modem_lp_timer_interrupt(&mut transaction)
        };
        match disposition {
            ModemLpTimerInterruptDisposition::Spurious => {
                BluetoothModemLpTimerInterruptStep::Spurious(self)
            }
            ModemLpTimerInterruptDisposition::HandlerPending(observation) => {
                BluetoothModemLpTimerInterruptStep::HandlerPending(
                    BluetoothModemLpTimerHandlerPending {
                        _ready: self,
                        observation,
                    },
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::{
        BluetoothModemLpTimerInterruptObservation, ModemLpTimerInterruptDisposition,
        ModemLpTimerInterruptTransaction, ModemLpTimerRegister, ModemLpTimerTransaction,
        execute_modem_lp_timer_interrupt, execute_modem_lp_timer_prepare,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        WriteComplete(ModemLpTimerRegister, u32),
        SetControl0078Bit25FromFreshRead,
        Fence,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum InterruptOperation {
        ReadStatus0038(u32),
        ReadValue006c(u32),
        TestControl0058Bit2(bool),
        SetControl0058Bit1FromFreshRead,
        Fence,
    }

    struct InterruptRecorder {
        status_0038: u32,
        value_006c: u32,
        control_0058_bit_2: bool,
        operations: Vec<InterruptOperation>,
    }

    impl InterruptRecorder {
        fn new(status_0038: u32, value_006c: u32, control_0058_bit_2: bool) -> Self {
            Self {
                status_0038,
                value_006c,
                control_0058_bit_2,
                operations: Vec::new(),
            }
        }
    }

    impl ModemLpTimerInterruptTransaction for InterruptRecorder {
        fn read_status_0038(&mut self) -> u32 {
            self.operations
                .push(InterruptOperation::ReadStatus0038(self.status_0038));
            self.status_0038
        }

        fn read_value_006c(&mut self) -> u32 {
            self.operations
                .push(InterruptOperation::ReadValue006c(self.value_006c));
            self.value_006c
        }

        fn control_0058_bit_2_is_set(&mut self) -> bool {
            self.operations
                .push(InterruptOperation::TestControl0058Bit2(
                    self.control_0058_bit_2,
                ));
            self.control_0058_bit_2
        }

        fn set_control_0058_bit_1_from_fresh_read(&mut self) {
            self.operations
                .push(InterruptOperation::SetControl0058Bit1FromFreshRead);
        }

        fn fence(&mut self) {
            self.operations.push(InterruptOperation::Fence);
        }
    }

    #[derive(Default)]
    struct Recorder(Vec<Operation>);

    impl Recorder {
        fn mmio_count(&self) -> usize {
            self.0
                .iter()
                .map(|operation| match operation {
                    Operation::WriteComplete(_, _) => 1,
                    Operation::SetControl0078Bit25FromFreshRead => 2,
                    Operation::Fence => 0,
                })
                .sum()
        }
    }

    impl ModemLpTimerTransaction for Recorder {
        fn write_complete(&mut self, register: ModemLpTimerRegister, image: u32) {
            self.0.push(Operation::WriteComplete(register, image));
        }

        fn set_control_0078_bit_25(&mut self) {
            self.0.push(Operation::SetControl0078Bit25FromFreshRead);
        }

        fn fence(&mut self) {
            self.0.push(Operation::Fence);
        }
    }

    #[test]
    fn source_127_prefix_is_exactly_eight_mmio_operations_then_one_fence() {
        let mut recorder = Recorder::default();

        execute_modem_lp_timer_prepare(&mut recorder);

        assert_eq!(
            recorder.0,
            [
                Operation::WriteComplete(ModemLpTimerRegister::Command004c, 0),
                Operation::SetControl0078Bit25FromFreshRead,
                Operation::WriteComplete(ModemLpTimerRegister::Command0004, 1),
                Operation::WriteComplete(ModemLpTimerRegister::Command0008, 1),
                Operation::WriteComplete(ModemLpTimerRegister::Command0014, u32::MAX),
                Operation::WriteComplete(ModemLpTimerRegister::Command0034, u32::MAX),
                Operation::WriteComplete(ModemLpTimerRegister::Command0010, 2),
                Operation::Fence,
            ]
        );
        assert_eq!(recorder.mmio_count(), 8);
        assert_eq!(
            recorder
                .0
                .iter()
                .filter(|operation| matches!(operation, Operation::Fence))
                .count(),
            1
        );
    }

    #[test]
    fn source_127_zero_status_returns_after_exactly_one_read() {
        let mut recorder = InterruptRecorder::new(0, u32::MAX, true);

        let disposition = execute_modem_lp_timer_interrupt(&mut recorder);

        assert_eq!(disposition, ModemLpTimerInterruptDisposition::Spurious);
        assert_eq!(recorder.operations, [InterruptOperation::ReadStatus0038(0)]);
    }

    #[test]
    fn source_127_zero_value_dispatches_without_control_read() {
        let mut recorder = InterruptRecorder::new(1, 0, true);

        let disposition = execute_modem_lp_timer_interrupt(&mut recorder);

        assert_eq!(
            disposition,
            ModemLpTimerInterruptDisposition::HandlerPending(
                BluetoothModemLpTimerInterruptObservation::Value006cZero
            )
        );
        assert_eq!(
            recorder.operations,
            [
                InterruptOperation::ReadStatus0038(1),
                InterruptOperation::ReadValue006c(0),
                InterruptOperation::Fence,
            ]
        );
    }

    #[test]
    fn source_127_clear_control_2_dispatches_without_write() {
        let mut recorder = InterruptRecorder::new(1, 1, false);

        let disposition = execute_modem_lp_timer_interrupt(&mut recorder);

        assert_eq!(
            disposition,
            ModemLpTimerInterruptDisposition::HandlerPending(
                BluetoothModemLpTimerInterruptObservation::Control2Clear
            )
        );
        assert_eq!(
            recorder.operations,
            [
                InterruptOperation::ReadStatus0038(1),
                InterruptOperation::ReadValue006c(1),
                InterruptOperation::TestControl0058Bit2(false),
                InterruptOperation::Fence,
            ]
        );
    }

    #[test]
    fn source_127_set_control_2_publishes_control_1_before_dispatch() {
        let mut recorder = InterruptRecorder::new(1, 1, true);

        let disposition = execute_modem_lp_timer_interrupt(&mut recorder);

        assert_eq!(
            disposition,
            ModemLpTimerInterruptDisposition::HandlerPending(
                BluetoothModemLpTimerInterruptObservation::Control2SetControl1Published
            )
        );
        assert_eq!(
            recorder.operations,
            [
                InterruptOperation::ReadStatus0038(1),
                InterruptOperation::ReadValue006c(1),
                InterruptOperation::TestControl0058Bit2(true),
                InterruptOperation::SetControl0058Bit1FromFreshRead,
                InterruptOperation::Fence,
            ]
        );
    }
}
