//! Exact controller transactions for the modem low-power timer.
//!
//! The module contains the one-command runtime-counter start performed by the
//! controller hardware-enable path, the complete MMIO prefix immediately
//! before ESP32-S31 installs interrupt source 127, and the bounded register
//! classifier and hardware-acknowledgement phase at the start of that source's
//! handler. These transactions do not initialize the vendor software
//! environment, publish ISR storage, install a CPU route, dispatch the
//! software timer queue, or claim that the controller, Link Layer, or HCI is
//! live.

#![deny(unsafe_code)]

use super::{BluetoothTaskRegisters, device_fence};

/// Evidence that the exact BTDM runtime-timer start command and trailing
/// device fence completed.
///
/// This token does not prove source-127 route setup, a working software timer
/// queue, controller readiness, or a physical time unit. A future enable
/// typestate must retain it across interrupt-output and CPU-route setup.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the started runtime timer must feed controller-enable ownership"]
pub struct BluetoothModemLpTimerCounterStarted {
    _private: (),
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

/// Source-127 registers staged for exclusive placement in stable ISR storage.
///
/// The task owner remains private and cannot concurrently escape back to task
/// context. A spurious hardware entry returns this state unchanged; a real
/// timer dispatch consumes it into the next common-handler register phase.
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
/// This owner exposes the initial positional observation and exactly one
/// bounded transition into the common handler's register-acknowledgement
/// phase. It cannot be rearmed directly, returned to task context or discarded
/// into an apparently ready interrupt owner.
#[must_use = "the common modem LP-timer register phase remains pending"]
pub struct BluetoothModemLpTimerHandlerPending {
    ready: BluetoothModemLpTimerInterruptReady,
    observation: BluetoothModemLpTimerInterruptObservation,
}

impl BluetoothModemLpTimerHandlerPending {
    /// Return the exact register path which requires timer-handler dispatch.
    pub const fn observation(&self) -> BluetoothModemLpTimerInterruptObservation {
        self.observation
    }

    /// Acknowledge the two positional state bytes at the start of the common
    /// timer handler.
    ///
    /// This second finite step never waits, loops, allocates or invokes
    /// software. If neither byte requests work, the vendor path performs its
    /// final fresh read and the source-127 owner is ready again. Otherwise the
    /// owner remains affine in [`BluetoothModemLpTimerSoftwarePending`] until
    /// the matching software state transition and final read are implemented.
    pub fn step_registers(self) -> BluetoothModemLpTimerHandlerRegisterStep {
        let disposition = {
            let mut transaction = HardwareModemLpTimerTransaction {
                registers: &self.ready.task.bluetooth.btdm_runtime_control,
            };
            execute_modem_lp_timer_handler_registers(&mut transaction)
        };

        match disposition {
            ModemLpTimerHandlerRegisterDisposition::Rearmed => {
                BluetoothModemLpTimerHandlerRegisterStep::Rearmed(self.ready)
            }
            ModemLpTimerHandlerRegisterDisposition::SoftwarePending(register_observation) => {
                BluetoothModemLpTimerHandlerRegisterStep::SoftwarePending(
                    BluetoothModemLpTimerSoftwarePending {
                        ready: self.ready,
                        interrupt_observation: self.observation,
                        register_observation,
                    },
                )
            }
        }
    }
}

/// Positional state acknowledged by the common source-127 timer handler.
///
/// The two booleans intentionally retain register positions instead of
/// assigning undocumented timer or RTC meanings. A nonzero `STATE_0024` low
/// byte requires software timer-queue dispatch. A nonzero `STATE_002C` low
/// byte requires a separate software environment update before the final
/// hardware read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothModemLpTimerHandlerRegisterObservation {
    state_0024_low_byte_nonzero: bool,
    state_002c_low_byte_nonzero: bool,
}

impl BluetoothModemLpTimerHandlerRegisterObservation {
    /// Whether the sampled low byte of `STATE_0024` was nonzero.
    pub const fn state_0024_low_byte_was_nonzero(self) -> bool {
        self.state_0024_low_byte_nonzero
    }

    /// Whether the sampled low byte of `STATE_002C` was nonzero.
    pub const fn state_002c_low_byte_was_nonzero(self) -> bool {
        self.state_002c_low_byte_nonzero
    }
}

/// Source-127 owner after hardware state acknowledgement exposed required
/// software work.
///
/// This state deliberately cannot be rearmed. The vendor handler mutates a
/// software epoch, may dispatch its timer queue, and then performs a final
/// fresh `STATE_0024` read. Those actions belong above the PAC and must be
/// represented by a later explicit completion transition.
#[must_use = "software timer work and the final hardware read remain pending"]
pub struct BluetoothModemLpTimerSoftwarePending {
    ready: BluetoothModemLpTimerInterruptReady,
    interrupt_observation: BluetoothModemLpTimerInterruptObservation,
    register_observation: BluetoothModemLpTimerHandlerRegisterObservation,
}

impl BluetoothModemLpTimerSoftwarePending {
    /// Return the initial source-127 classifier path.
    pub const fn interrupt_observation(&self) -> BluetoothModemLpTimerInterruptObservation {
        self.interrupt_observation
    }

    /// Return the state bytes whose software consequences remain pending.
    pub const fn register_observation(&self) -> BluetoothModemLpTimerHandlerRegisterObservation {
        self.register_observation
    }

    /// Sample one complete positional LP-timer instant.
    ///
    /// A newly observed rollover advances `epoch` before the fresh counter
    /// read and publishes the exact hardware acknowledgement. The method is
    /// finite and returns without polling.
    pub fn sample_counter(
        &mut self,
        epoch: &mut BluetoothModemLpTimerEpoch,
    ) -> BluetoothModemLpTimerCounterObservation {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &self.ready.task.bluetooth.btdm_runtime_control,
        };
        execute_modem_lp_timer_counter_sample(&mut transaction, epoch)
    }

    /// Disable the currently programmed positional compare.
    pub fn disable_compare(&mut self) {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &self.ready.task.bluetooth.btdm_runtime_control,
        };
        execute_modem_lp_timer_compare_disable(&mut transaction);
    }

    /// Program one positional deadline using the current software epoch.
    ///
    /// The exact finite transaction either requests immediate software work,
    /// publishes the deadline low counter image, or places a half-range
    /// checkpoint when the deadline is at least one full low-counter span
    /// away. It never waits for the compare to fire.
    pub fn program_compare(
        &mut self,
        deadline: BluetoothModemLpTimerInstant,
        epoch: BluetoothModemLpTimerEpoch,
    ) -> BluetoothModemLpTimerCompareDisposition {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &self.ready.task.bluetooth.btdm_runtime_control,
        };
        execute_modem_lp_timer_compare_program(&mut transaction, deadline, epoch)
    }

    /// Perform the common handler's final fresh state read after software work.
    ///
    /// This lower transition cannot prove that the no-RTOS timer queue was
    /// drained. The HAL/controller composition must keep the owner private and
    /// call this method only from its completed software state.
    #[doc(hidden)]
    pub fn complete_software(self) -> BluetoothModemLpTimerInterruptReady {
        {
            let mut transaction = HardwareModemLpTimerTransaction {
                registers: &self.ready.task.bluetooth.btdm_runtime_control,
            };
            transaction.sample_final_state_0024();
        }
        self.ready
    }
}

/// Source-owned high byte paired with the modem LP timer's positional counter.
///
/// The vendor environment advances this byte with wrapping arithmetic whenever
/// the reviewed rollover state is acknowledged. No physical time unit is
/// assigned here.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BluetoothModemLpTimerEpoch(u8);

impl BluetoothModemLpTimerEpoch {
    /// Begin the source-owned timer environment at epoch zero.
    pub const fn new() -> Self {
        Self(0)
    }

    /// Return the current positional high byte.
    pub const fn high_byte(self) -> u8 {
        self.0
    }

    /// Apply the rollover fact already acknowledged by the common handler's
    /// register phase.
    pub fn advance_for_handler_registers(
        &mut self,
        observation: BluetoothModemLpTimerHandlerRegisterObservation,
    ) {
        if observation.state_002c_low_byte_was_nonzero() {
            self.advance();
        }
    }

    fn advance(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }

    const fn advanced(mut self) -> Self {
        self.0 = self.0.wrapping_add(1);
        self
    }

    const fn combine(self, counter_image: u32) -> BluetoothModemLpTimerInstant {
        BluetoothModemLpTimerInstant(((self.0 as u32) << 24) | counter_image)
    }
}

/// One positional 32-bit modem LP-timer instant.
///
/// The value wraps and deliberately has no claimed physical unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothModemLpTimerInstant(u32);

impl BluetoothModemLpTimerInstant {
    /// Retain one complete positional deadline image.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Return the complete positional image.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// Result of one finite LP-counter sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothModemLpTimerCounterObservation {
    instant: BluetoothModemLpTimerInstant,
    rollover_acknowledged: bool,
}

impl BluetoothModemLpTimerCounterObservation {
    /// Return the sampled positional instant after any epoch advance.
    pub const fn instant(self) -> BluetoothModemLpTimerInstant {
        self.instant
    }

    /// Whether this sample acknowledged a newly observed rollover state.
    pub const fn rollover_was_acknowledged(self) -> bool {
        self.rollover_acknowledged
    }
}

/// Hardware branch selected while programming one positional compare.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerCompareDisposition {
    /// The deadline delta was at most two and software work was requested now.
    Immediate,
    /// The deadline lies within one low-counter span and its low image was used.
    Deadline,
    /// A half-range checkpoint was used for a more distant deadline.
    HalfRangeCheckpoint,
}

/// Result of the common handler's bounded register-acknowledgement phase.
#[must_use = "retain the ready owner or complete the required software work"]
pub enum BluetoothModemLpTimerHandlerRegisterStep {
    /// No software work was requested and the final fresh read completed.
    Rearmed(BluetoothModemLpTimerInterruptReady),
    /// At least one acknowledged state byte requires software work.
    SoftwarePending(BluetoothModemLpTimerSoftwarePending),
}

/// Result of one finite source-127 handler prefix.
#[must_use = "retain the ready owner or complete the required software handler"]
pub enum BluetoothModemLpTimerInterruptStep {
    /// `STATUS_0038` was zero; the handler returned without further MMIO.
    Spurious(BluetoothModemLpTimerInterruptReady),
    /// A reviewed branch requires the common software timer handler.
    HandlerPending(BluetoothModemLpTimerHandlerPending),
}

trait ModemLpTimerTransaction {
    fn prepare_hardware(&mut self);
    fn fence(&mut self);
}

trait ModemLpTimerStartTransaction {
    fn start_counter(&mut self);
    fn fence(&mut self);
}

trait ModemLpTimerInterruptTransaction {
    fn read_status_0038(&mut self) -> u32;
    fn read_value_006c(&mut self) -> u32;
    fn control_0058_bit_2_is_set(&mut self) -> bool;
    fn set_control_0058_bit_1_from_fresh_read(&mut self);
    fn fence(&mut self);
}

trait ModemLpTimerHandlerRegisterTransaction {
    fn sample_state_0024_low_byte_nonzero(&mut self) -> bool;
    fn clear_state_0024(&mut self);
    fn sample_state_002c_low_byte_nonzero(&mut self) -> bool;
    fn clear_state_002c(&mut self);
    fn sample_final_state_0024(&mut self);
    fn fence(&mut self);
}

trait ModemLpTimerRuntimeTransaction {
    fn read_counter(&mut self) -> u32;
    fn rollover_low_byte_nonzero(&mut self) -> bool;
    fn clear_rollover(&mut self);
    fn publish_software_pending(&mut self);
    fn disable_compare(&mut self);
    fn write_compare(&mut self, image: crate::generated::BluetoothModemLpTimerCompareImage);
    fn trigger_timer_command(&mut self);
    fn fence(&mut self);
}

const MODEM_LP_TIMER_LOW_SPAN: u32 = 0x0100_0000;
const MODEM_LP_TIMER_HALF_RANGE: u32 = 0x0080_0000;
const MODEM_LP_TIMER_IMMEDIATE_TOLERANCE: i32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModemLpTimerInterruptDisposition {
    Spurious,
    HandlerPending(BluetoothModemLpTimerInterruptObservation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModemLpTimerHandlerRegisterDisposition {
    Rearmed,
    SoftwarePending(BluetoothModemLpTimerHandlerRegisterObservation),
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

fn execute_modem_lp_timer_handler_registers(
    transaction: &mut impl ModemLpTimerHandlerRegisterTransaction,
) -> ModemLpTimerHandlerRegisterDisposition {
    let state_0024_low_byte_nonzero = transaction.sample_state_0024_low_byte_nonzero();
    if state_0024_low_byte_nonzero {
        transaction.clear_state_0024();
    }

    let state_002c_low_byte_nonzero = transaction.sample_state_002c_low_byte_nonzero();
    if state_002c_low_byte_nonzero {
        transaction.clear_state_002c();
    }

    let observation = BluetoothModemLpTimerHandlerRegisterObservation {
        state_0024_low_byte_nonzero,
        state_002c_low_byte_nonzero,
    };
    if state_0024_low_byte_nonzero || state_002c_low_byte_nonzero {
        transaction.fence();
        ModemLpTimerHandlerRegisterDisposition::SoftwarePending(observation)
    } else {
        transaction.sample_final_state_0024();
        ModemLpTimerHandlerRegisterDisposition::Rearmed
    }
}

fn execute_modem_lp_timer_counter_sample(
    transaction: &mut impl ModemLpTimerRuntimeTransaction,
    epoch: &mut BluetoothModemLpTimerEpoch,
) -> BluetoothModemLpTimerCounterObservation {
    let first_counter = transaction.read_counter();
    let rollover_acknowledged = transaction.rollover_low_byte_nonzero();
    let counter = if rollover_acknowledged {
        epoch.advance();
        let fresh_counter = transaction.read_counter();
        transaction.clear_rollover();
        transaction.publish_software_pending();
        transaction.trigger_timer_command();
        transaction.fence();
        fresh_counter
    } else {
        first_counter
    };

    BluetoothModemLpTimerCounterObservation {
        instant: epoch.combine(counter),
        rollover_acknowledged,
    }
}

fn execute_modem_lp_timer_compare_disable(transaction: &mut impl ModemLpTimerRuntimeTransaction) {
    transaction.disable_compare();
    transaction.fence();
}

fn execute_modem_lp_timer_compare_program(
    transaction: &mut impl ModemLpTimerRuntimeTransaction,
    deadline: BluetoothModemLpTimerInstant,
    epoch: BluetoothModemLpTimerEpoch,
) -> BluetoothModemLpTimerCompareDisposition {
    transaction.disable_compare();
    let first_counter = transaction.read_counter();
    let rollover = transaction.rollover_low_byte_nonzero();
    let (effective_epoch, counter) = if rollover {
        (epoch.advanced(), transaction.read_counter())
    } else {
        (epoch, first_counter)
    };
    let current = effective_epoch.combine(counter).bits();
    let delta = deadline.bits().wrapping_sub(current);

    let disposition = if (delta as i32) <= MODEM_LP_TIMER_IMMEDIATE_TOLERANCE {
        transaction.publish_software_pending();
        BluetoothModemLpTimerCompareDisposition::Immediate
    } else if delta < MODEM_LP_TIMER_LOW_SPAN {
        transaction.write_compare(crate::generated::BluetoothModemLpTimerCompareImage::new(
            deadline.bits() % MODEM_LP_TIMER_LOW_SPAN,
        ));
        BluetoothModemLpTimerCompareDisposition::Deadline
    } else {
        transaction.write_compare(crate::generated::BluetoothModemLpTimerCompareImage::new(
            counter.wrapping_add(MODEM_LP_TIMER_HALF_RANGE),
        ));
        BluetoothModemLpTimerCompareDisposition::HalfRangeCheckpoint
    };
    transaction.trigger_timer_command();
    transaction.fence();
    disposition
}

fn execute_modem_lp_timer_prepare(transaction: &mut impl ModemLpTimerTransaction) {
    transaction.prepare_hardware();
    transaction.fence();
}

fn execute_modem_lp_timer_start(
    transaction: &mut impl ModemLpTimerStartTransaction,
) -> BluetoothModemLpTimerCounterStarted {
    transaction.start_counter();
    transaction.fence();
    BluetoothModemLpTimerCounterStarted { _private: () }
}

struct HardwareModemLpTimerTransaction<'registers> {
    registers: &'registers super::svd::BtdmRuntimeControl,
}

impl ModemLpTimerStartTransaction for HardwareModemLpTimerTransaction<'_> {
    fn start_counter(&mut self) {
        super::svd::fixed_register_write::start_bluetooth_runtime_timer(self.registers);
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl ModemLpTimerTransaction for HardwareModemLpTimerTransaction<'_> {
    fn prepare_hardware(&mut self) {
        super::svd::zero_register_write::prepare_bluetooth_modem_lp_timer_command_004c(
            self.registers,
        );
        super::svd::field_or_modify::prepare_bluetooth_modem_lp_timer_control_25(self.registers);
        super::svd::fixed_register_image::prepare_bluetooth_modem_lp_timer_command_0004(
            self.registers,
        );
        super::svd::fixed_register_image::prepare_bluetooth_modem_lp_timer_command_0008(
            self.registers,
        );
        super::svd::fixed_register_image::prepare_bluetooth_modem_lp_timer_command_0014(
            self.registers,
        );
        super::svd::fixed_register_image::prepare_bluetooth_modem_lp_timer_command_0034(
            self.registers,
        );
        super::svd::fixed_register_image::prepare_bluetooth_modem_lp_timer_command_0010(
            self.registers,
        );
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl ModemLpTimerInterruptTransaction for HardwareModemLpTimerTransaction<'_> {
    fn read_status_0038(&mut self) -> u32 {
        super::svd::field_read::observe_bluetooth_modem_lp_timer_status_0038(self.registers)
    }

    fn read_value_006c(&mut self) -> u32 {
        super::svd::field_read::observe_bluetooth_modem_lp_timer_value_006c(self.registers)
    }

    fn control_0058_bit_2_is_set(&mut self) -> bool {
        super::svd::field_read::observe_bluetooth_modem_lp_timer_control_2(self.registers)
    }

    fn set_control_0058_bit_1_from_fresh_read(&mut self) {
        crate::generated::publish_bluetooth_modem_lp_timer_control_1(self.registers);
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl ModemLpTimerHandlerRegisterTransaction for HardwareModemLpTimerTransaction<'_> {
    fn sample_state_0024_low_byte_nonzero(&mut self) -> bool {
        super::svd::field_read::observe_bluetooth_modem_lp_timer_state_0024(self.registers) != 0
    }

    fn clear_state_0024(&mut self) {
        super::svd::zero_register_write::clear_bluetooth_modem_lp_timer_state_0024(self.registers);
    }

    fn sample_state_002c_low_byte_nonzero(&mut self) -> bool {
        super::svd::field_read::observe_bluetooth_modem_lp_timer_state_002c(self.registers) != 0
    }

    fn clear_state_002c(&mut self) {
        super::svd::zero_register_write::clear_bluetooth_modem_lp_timer_state_002c(self.registers);
    }

    fn sample_final_state_0024(&mut self) {
        let _ = super::svd::field_read::observe_bluetooth_modem_lp_timer_state_0024(self.registers);
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl ModemLpTimerRuntimeTransaction for HardwareModemLpTimerTransaction<'_> {
    fn read_counter(&mut self) -> u32 {
        super::svd::field_read::observe_bluetooth_modem_lp_timer_counter(self.registers)
    }

    fn rollover_low_byte_nonzero(&mut self) -> bool {
        super::svd::field_read::observe_bluetooth_modem_lp_timer_state_002c(self.registers) != 0
    }

    fn clear_rollover(&mut self) {
        super::svd::zero_register_write::clear_bluetooth_modem_lp_timer_state_002c(self.registers);
    }

    fn publish_software_pending(&mut self) {
        super::svd::fixed_register_image::publish_bluetooth_modem_lp_timer_software_pending(
            self.registers,
        );
    }

    fn disable_compare(&mut self) {
        super::svd::fixed_register_image::disable_bluetooth_modem_lp_timer_compare(self.registers);
    }

    fn write_compare(&mut self, image: crate::generated::BluetoothModemLpTimerCompareImage) {
        crate::generated::publish_bluetooth_modem_lp_timer_compare(self.registers, image);
    }

    fn trigger_timer_command(&mut self) {
        super::svd::fixed_register_image::trigger_bluetooth_modem_lp_timer_command(self.registers);
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl BluetoothTaskRegisters {
    /// Start the BTDM runtime timer during controller hardware enable.
    ///
    /// SOURCE: complete current `libbtdm_common.a` member `9.o` symbol
    /// `r_sym_bt_ymLPVGRY14FVW494j9ZD` writes the sole reviewed command and
    /// returns. The instruction-identical public same-chip predecessor names
    /// the operation `r_btdm_hal_rtc_start`; its complete caller places this
    /// edge after controller hardware/output preparation and before primary
    /// CPU-route allocation. The restricted PAC adds one device fence before
    /// returning the completion token. The token proves this invocation completed;
    /// it does not by itself enforce that the higher lifecycle invokes the command
    /// exactly once.
    ///
    /// This component is hidden because the higher lifecycle must retain the
    /// completed PHY, BTBB, BLE-engine, controller-output, software-runtime
    /// and route prerequisites in their independently proven order.
    #[doc(hidden)]
    pub fn start_modem_lp_timer_counter(&mut self) -> BluetoothModemLpTimerCounterStarted {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &self.bluetooth.btdm_runtime_control,
        };
        execute_modem_lp_timer_start(&mut transaction)
    }

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
    pub fn prepare_modem_lp_timer_registers(self) -> BluetoothModemLpTimerRegistersPrepared {
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
    /// [`BluetoothModemLpTimerHandlerPending`] for the next finite common
    /// handler register step.
    pub fn step(self) -> BluetoothModemLpTimerInterruptStep {
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
                        ready: self,
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
        BluetoothModemLpTimerCompareDisposition, BluetoothModemLpTimerEpoch,
        BluetoothModemLpTimerHandlerRegisterObservation, BluetoothModemLpTimerInstant,
        BluetoothModemLpTimerInterruptObservation, ModemLpTimerHandlerRegisterDisposition,
        ModemLpTimerHandlerRegisterTransaction, ModemLpTimerInterruptDisposition,
        ModemLpTimerInterruptTransaction, ModemLpTimerRuntimeTransaction,
        ModemLpTimerStartTransaction, ModemLpTimerTransaction,
        execute_modem_lp_timer_compare_disable, execute_modem_lp_timer_compare_program,
        execute_modem_lp_timer_counter_sample, execute_modem_lp_timer_handler_registers,
        execute_modem_lp_timer_interrupt, execute_modem_lp_timer_prepare,
        execute_modem_lp_timer_start,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum PreparationOperation {
        GeneratedTransaction,
        DeviceFence,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum StartOperation {
        StartCounter,
        DeviceFence,
    }

    struct StartRecorder {
        operations: Vec<StartOperation>,
    }

    impl ModemLpTimerStartTransaction for StartRecorder {
        fn start_counter(&mut self) {
            self.operations.push(StartOperation::StartCounter);
        }

        fn fence(&mut self) {
            self.operations.push(StartOperation::DeviceFence);
        }
    }

    #[test]
    fn runtime_counter_start_orders_the_command_before_publication() {
        let mut recorder = StartRecorder {
            operations: Vec::new(),
        };

        let _started = execute_modem_lp_timer_start(&mut recorder);

        assert_eq!(
            recorder.operations,
            [StartOperation::StartCounter, StartOperation::DeviceFence]
        );
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum InterruptOperation {
        ReadStatus0038(u32),
        ReadValue006c(u32),
        TestControl0058Bit2(bool),
        SetControl0058Bit1FromFreshRead,
        Fence,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum HandlerRegisterOperation {
        SampleState0024(bool),
        ClearState0024,
        SampleState002c(bool),
        ClearState002c,
        SampleFinalState0024,
        Fence,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RuntimeOperation {
        ReadCounter(u32),
        SampleRollover(bool),
        ClearRollover,
        PublishSoftwarePending,
        DisableCompare,
        WriteCompare(u32),
        TriggerTimerCommand,
        Fence,
    }

    struct RuntimeRecorder {
        counters: [u32; 2],
        counter_index: usize,
        rollover: bool,
        operations: Vec<RuntimeOperation>,
    }

    impl RuntimeRecorder {
        fn new(first_counter: u32, fresh_counter: u32, rollover: bool) -> Self {
            Self {
                counters: [first_counter, fresh_counter],
                counter_index: 0,
                rollover,
                operations: Vec::new(),
            }
        }
    }

    impl ModemLpTimerRuntimeTransaction for RuntimeRecorder {
        fn read_counter(&mut self) -> u32 {
            let counter = self.counters[self.counter_index.min(1)];
            self.counter_index += 1;
            self.operations.push(RuntimeOperation::ReadCounter(counter));
            counter
        }

        fn rollover_low_byte_nonzero(&mut self) -> bool {
            self.operations
                .push(RuntimeOperation::SampleRollover(self.rollover));
            self.rollover
        }

        fn clear_rollover(&mut self) {
            self.operations.push(RuntimeOperation::ClearRollover);
        }

        fn publish_software_pending(&mut self) {
            self.operations
                .push(RuntimeOperation::PublishSoftwarePending);
        }

        fn disable_compare(&mut self) {
            self.operations.push(RuntimeOperation::DisableCompare);
        }

        fn write_compare(&mut self, image: crate::generated::BluetoothModemLpTimerCompareImage) {
            self.operations
                .push(RuntimeOperation::WriteCompare(image.get()));
        }

        fn trigger_timer_command(&mut self) {
            self.operations.push(RuntimeOperation::TriggerTimerCommand);
        }

        fn fence(&mut self) {
            self.operations.push(RuntimeOperation::Fence);
        }
    }

    struct HandlerRegisterRecorder {
        state_0024_low_byte_nonzero: bool,
        state_002c_low_byte_nonzero: bool,
        operations: Vec<HandlerRegisterOperation>,
    }

    impl HandlerRegisterRecorder {
        fn new(state_0024_low_byte_nonzero: bool, state_002c_low_byte_nonzero: bool) -> Self {
            Self {
                state_0024_low_byte_nonzero,
                state_002c_low_byte_nonzero,
                operations: Vec::new(),
            }
        }
    }

    impl ModemLpTimerHandlerRegisterTransaction for HandlerRegisterRecorder {
        fn sample_state_0024_low_byte_nonzero(&mut self) -> bool {
            self.operations
                .push(HandlerRegisterOperation::SampleState0024(
                    self.state_0024_low_byte_nonzero,
                ));
            self.state_0024_low_byte_nonzero
        }

        fn clear_state_0024(&mut self) {
            self.operations
                .push(HandlerRegisterOperation::ClearState0024);
        }

        fn sample_state_002c_low_byte_nonzero(&mut self) -> bool {
            self.operations
                .push(HandlerRegisterOperation::SampleState002c(
                    self.state_002c_low_byte_nonzero,
                ));
            self.state_002c_low_byte_nonzero
        }

        fn clear_state_002c(&mut self) {
            self.operations
                .push(HandlerRegisterOperation::ClearState002c);
        }

        fn sample_final_state_0024(&mut self) {
            self.operations
                .push(HandlerRegisterOperation::SampleFinalState0024);
        }

        fn fence(&mut self) {
            self.operations.push(HandlerRegisterOperation::Fence);
        }
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
    struct Recorder(Vec<PreparationOperation>);

    impl ModemLpTimerTransaction for Recorder {
        fn prepare_hardware(&mut self) {
            self.0.push(PreparationOperation::GeneratedTransaction);
        }

        fn fence(&mut self) {
            self.0.push(PreparationOperation::DeviceFence);
        }
    }

    #[test]
    fn source_127_preparation_fences_after_the_generated_transaction() {
        let mut recorder = Recorder::default();

        execute_modem_lp_timer_prepare(&mut recorder);

        assert_eq!(
            recorder.0,
            [
                PreparationOperation::GeneratedTransaction,
                PreparationOperation::DeviceFence,
            ]
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

    #[test]
    fn source_127_common_handler_rearms_after_idle_state_samples() {
        let mut recorder = HandlerRegisterRecorder::new(false, false);

        let disposition = execute_modem_lp_timer_handler_registers(&mut recorder);

        assert_eq!(disposition, ModemLpTimerHandlerRegisterDisposition::Rearmed);
        assert_eq!(
            recorder.operations,
            [
                HandlerRegisterOperation::SampleState0024(false),
                HandlerRegisterOperation::SampleState002c(false),
                HandlerRegisterOperation::SampleFinalState0024,
            ]
        );
    }

    #[test]
    fn source_127_common_handler_acknowledges_queue_work_before_software_pending() {
        let mut recorder = HandlerRegisterRecorder::new(true, false);

        let disposition = execute_modem_lp_timer_handler_registers(&mut recorder);

        assert_eq!(
            disposition,
            ModemLpTimerHandlerRegisterDisposition::SoftwarePending(
                BluetoothModemLpTimerHandlerRegisterObservation {
                    state_0024_low_byte_nonzero: true,
                    state_002c_low_byte_nonzero: false,
                }
            )
        );
        assert_eq!(
            recorder.operations,
            [
                HandlerRegisterOperation::SampleState0024(true),
                HandlerRegisterOperation::ClearState0024,
                HandlerRegisterOperation::SampleState002c(false),
                HandlerRegisterOperation::Fence,
            ]
        );
    }

    #[test]
    fn source_127_common_handler_acknowledges_epoch_work_before_software_pending() {
        let mut recorder = HandlerRegisterRecorder::new(false, true);

        let disposition = execute_modem_lp_timer_handler_registers(&mut recorder);

        assert_eq!(
            disposition,
            ModemLpTimerHandlerRegisterDisposition::SoftwarePending(
                BluetoothModemLpTimerHandlerRegisterObservation {
                    state_0024_low_byte_nonzero: false,
                    state_002c_low_byte_nonzero: true,
                }
            )
        );
        assert_eq!(
            recorder.operations,
            [
                HandlerRegisterOperation::SampleState0024(false),
                HandlerRegisterOperation::SampleState002c(true),
                HandlerRegisterOperation::ClearState002c,
                HandlerRegisterOperation::Fence,
            ]
        );
    }

    #[test]
    fn source_127_common_handler_acknowledges_both_states_in_vendor_order() {
        let mut recorder = HandlerRegisterRecorder::new(true, true);

        let disposition = execute_modem_lp_timer_handler_registers(&mut recorder);

        assert_eq!(
            disposition,
            ModemLpTimerHandlerRegisterDisposition::SoftwarePending(
                BluetoothModemLpTimerHandlerRegisterObservation {
                    state_0024_low_byte_nonzero: true,
                    state_002c_low_byte_nonzero: true,
                }
            )
        );
        assert_eq!(
            recorder.operations,
            [
                HandlerRegisterOperation::SampleState0024(true),
                HandlerRegisterOperation::ClearState0024,
                HandlerRegisterOperation::SampleState002c(true),
                HandlerRegisterOperation::ClearState002c,
                HandlerRegisterOperation::Fence,
            ]
        );
    }

    #[test]
    fn modem_lp_counter_without_rollover_returns_one_bounded_sample() {
        let mut epoch = BluetoothModemLpTimerEpoch::new();
        epoch.advance_for_handler_registers(BluetoothModemLpTimerHandlerRegisterObservation {
            state_0024_low_byte_nonzero: false,
            state_002c_low_byte_nonzero: true,
        });
        let mut recorder = RuntimeRecorder::new(0x0000_1234, 0, false);

        let observation = execute_modem_lp_timer_counter_sample(&mut recorder, &mut epoch);

        assert_eq!(epoch.high_byte(), 1);
        assert_eq!(observation.instant().bits(), 0x0100_1234);
        assert!(!observation.rollover_was_acknowledged());
        assert_eq!(
            recorder.operations,
            [
                RuntimeOperation::ReadCounter(0x0000_1234),
                RuntimeOperation::SampleRollover(false),
            ]
        );
    }

    #[test]
    fn modem_lp_counter_rollover_advances_epoch_before_fresh_sample_and_ack() {
        let mut epoch = BluetoothModemLpTimerEpoch::new();
        let mut recorder = RuntimeRecorder::new(0x00ff_fffe, 0x0000_0003, true);

        let observation = execute_modem_lp_timer_counter_sample(&mut recorder, &mut epoch);

        assert_eq!(epoch.high_byte(), 1);
        assert_eq!(observation.instant().bits(), 0x0100_0003);
        assert!(observation.rollover_was_acknowledged());
        assert_eq!(
            recorder.operations,
            [
                RuntimeOperation::ReadCounter(0x00ff_fffe),
                RuntimeOperation::SampleRollover(true),
                RuntimeOperation::ReadCounter(0x0000_0003),
                RuntimeOperation::ClearRollover,
                RuntimeOperation::PublishSoftwarePending,
                RuntimeOperation::TriggerTimerCommand,
                RuntimeOperation::Fence,
            ]
        );
    }

    #[test]
    fn modem_lp_empty_queue_disables_compare_in_one_finite_step() {
        let mut recorder = RuntimeRecorder::new(0, 0, false);

        execute_modem_lp_timer_compare_disable(&mut recorder);

        assert_eq!(
            recorder.operations,
            [RuntimeOperation::DisableCompare, RuntimeOperation::Fence]
        );
    }

    #[test]
    fn modem_lp_compare_requests_immediate_work_for_due_deadline() {
        let mut recorder = RuntimeRecorder::new(0x100, 0, false);

        let disposition = execute_modem_lp_timer_compare_program(
            &mut recorder,
            BluetoothModemLpTimerInstant::from_bits(0x102),
            BluetoothModemLpTimerEpoch::new(),
        );

        assert_eq!(
            disposition,
            BluetoothModemLpTimerCompareDisposition::Immediate
        );
        assert_eq!(
            recorder.operations,
            [
                RuntimeOperation::DisableCompare,
                RuntimeOperation::ReadCounter(0x100),
                RuntimeOperation::SampleRollover(false),
                RuntimeOperation::PublishSoftwarePending,
                RuntimeOperation::TriggerTimerCommand,
                RuntimeOperation::Fence,
            ]
        );
    }

    #[test]
    fn modem_lp_compare_programs_near_deadline_without_callback_loop() {
        let mut recorder = RuntimeRecorder::new(0x100, 0, false);

        let disposition = execute_modem_lp_timer_compare_program(
            &mut recorder,
            BluetoothModemLpTimerInstant::from_bits(0x200),
            BluetoothModemLpTimerEpoch::new(),
        );

        assert_eq!(
            disposition,
            BluetoothModemLpTimerCompareDisposition::Deadline
        );
        assert_eq!(
            recorder.operations,
            [
                RuntimeOperation::DisableCompare,
                RuntimeOperation::ReadCounter(0x100),
                RuntimeOperation::SampleRollover(false),
                RuntimeOperation::WriteCompare(0x200),
                RuntimeOperation::TriggerTimerCommand,
                RuntimeOperation::Fence,
            ]
        );
    }

    #[test]
    fn modem_lp_compare_uses_half_range_checkpoint_for_distant_deadline() {
        let mut recorder = RuntimeRecorder::new(0x100, 0, false);

        let disposition = execute_modem_lp_timer_compare_program(
            &mut recorder,
            BluetoothModemLpTimerInstant::from_bits(0x0100_0200),
            BluetoothModemLpTimerEpoch::new(),
        );

        assert_eq!(
            disposition,
            BluetoothModemLpTimerCompareDisposition::HalfRangeCheckpoint
        );
        assert_eq!(
            recorder.operations,
            [
                RuntimeOperation::DisableCompare,
                RuntimeOperation::ReadCounter(0x100),
                RuntimeOperation::SampleRollover(false),
                RuntimeOperation::WriteCompare(0x0080_0100),
                RuntimeOperation::TriggerTimerCommand,
                RuntimeOperation::Fence,
            ]
        );
    }

    #[test]
    fn modem_lp_compare_accounts_for_unacknowledged_rollover_locally() {
        let mut recorder = RuntimeRecorder::new(0x00ff_fffe, 0x0000_0003, true);
        let epoch = BluetoothModemLpTimerEpoch::new();

        let disposition = execute_modem_lp_timer_compare_program(
            &mut recorder,
            BluetoothModemLpTimerInstant::from_bits(0x0100_0010),
            epoch,
        );

        assert_eq!(
            disposition,
            BluetoothModemLpTimerCompareDisposition::Deadline
        );
        assert_eq!(epoch.high_byte(), 0);
        assert_eq!(
            recorder.operations,
            [
                RuntimeOperation::DisableCompare,
                RuntimeOperation::ReadCounter(0x00ff_fffe),
                RuntimeOperation::SampleRollover(true),
                RuntimeOperation::ReadCounter(0x0000_0003),
                RuntimeOperation::WriteCompare(0x10),
                RuntimeOperation::TriggerTimerCommand,
                RuntimeOperation::Fence,
            ]
        );
    }
}
