//! Exact controller transactions for the modem low-power timer.
//!
//! The module contains the one-command runtime-counter start performed by the
//! controller hardware-enable path, the complete low-power configuration
//! component, the MMIO prefix immediately before ESP32-S31 installs interrupt
//! source 127, and the bounded register classifier and hardware-acknowledgement
//! phase at the start of that source's handler. These transactions do not
//! initialize the vendor software environment, publish ISR storage, install a
//! CPU route, dispatch the software timer queue, or claim that the controller,
//! Link Layer, or HCI is live.

#![deny(unsafe_code)]

use crate::{BluetoothTaskRegisters, device_fence, svd};

/// Disjoint register owner for the Bluetooth modem low-power timer.
///
/// The generated ownership partition contains only `BTDM_RUNTIME_CONTROL`.
/// Keeping it outside the controller partition lets source 127 retain its
/// hardware authority while ordinary task code continues to own scheduler,
/// baseband and Link-Layer registers.
#[must_use = "the modem LP-timer owner must remain paired with the Bluetooth lifecycle"]
pub struct BluetoothModemLpTimerRegisters {
    peripherals: svd::peripheral_ownership::BluetoothModemLpTimerPeripherals,
}

impl BluetoothModemLpTimerRegisters {
    pub(crate) const fn new(
        peripherals: svd::peripheral_ownership::BluetoothModemLpTimerPeripherals,
    ) -> Self {
        Self { peripherals }
    }

    pub(crate) fn into_peripherals(
        self,
    ) -> svd::peripheral_ownership::BluetoothModemLpTimerPeripherals {
        self.peripherals
    }
}

/// Timer-register ownership after the exact BTDM runtime-timer start command.
///
/// Consuming the preceding low-power owner makes the command one-shot for this
/// hardware epoch. This state does not prove source-127 route setup, a working
/// software timer queue, controller readiness, or a physical time unit.
#[must_use = "the started runtime timer must feed controller-enable ownership"]
pub struct BluetoothModemLpTimerCounterStarted {
    timer: BluetoothModemLpTimerRegisters,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
}

/// Why task context cannot perform a modem LP-timer transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerOwnerError {
    /// The disjoint timer partition has already moved to source-127 storage.
    OwnerSeparated,
}

/// Timer-register ownership after the exact source-127 register prefix.
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
    timer: BluetoothModemLpTimerRegisters,
}

/// Hardware branch observed while completing the BTDM low-power component.
///
/// The positional names retain only the independently reviewed register facts;
/// they do not assign an undocumented clock or sleep meaning to `CONTROL_2`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLowPowerRuntimeControlObservation {
    /// `CONTROL_2` was clear after controls zero and four were cleared.
    Control2Clear,
    /// `CONTROL_2` was set, so the fresh-read `CONTROL_1` publication ran.
    Control2SetControl1Published,
}

/// Timer ownership after the complete low-power hardware component.
///
/// The generated fixed-register sequence owns every command image, array
/// index and ordering fact. This handwritten state retains only ownership and
/// the reviewed conditional edge over generated field accessors.
#[must_use = "the initialized modem LP-timer owner must continue through route setup"]
pub struct BluetoothModemLpTimerLowPowerHardwareInitialized {
    timer: BluetoothModemLpTimerRegisters,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
}

impl BluetoothModemLpTimerLowPowerHardwareInitialized {
    /// Return the conditional runtime-control branch observed at initialization.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }
}

/// Source-127 timer registers staged for exclusive placement in stable ISR storage.
///
/// Controller task ownership is disjoint and may continue ordinary scheduler
/// work. A spurious hardware entry returns this state unchanged; a real timer
/// dispatch consumes it into the next common-handler register phase.
#[must_use = "the source-127 owner must remain in stable ISR storage"]
pub struct BluetoothModemLpTimerInterruptReady {
    timer: BluetoothModemLpTimerRegisters,
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
                registers: &self.ready.timer.peripherals.btdm_runtime_control,
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
            registers: &self.ready.timer.peripherals.btdm_runtime_control,
        };
        execute_modem_lp_timer_counter_sample(&mut transaction, epoch)
    }

    /// Disable the currently programmed positional compare.
    pub fn disable_compare(&mut self) {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &self.ready.timer.peripherals.btdm_runtime_control,
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
            registers: &self.ready.timer.peripherals.btdm_runtime_control,
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
                registers: &self.ready.timer.peripherals.btdm_runtime_control,
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

trait ModemLpTimerLowPowerInitTransaction {
    fn initialize_config(&mut self);
    fn clear_controls_0_4(&mut self);
    fn control_2_is_set(&mut self) -> bool;
    fn publish_control_1(&mut self);
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

fn execute_modem_lp_timer_low_power_init(
    transaction: &mut impl ModemLpTimerLowPowerInitTransaction,
) -> BluetoothLowPowerRuntimeControlObservation {
    transaction.initialize_config();
    transaction.clear_controls_0_4();
    let observation = if transaction.control_2_is_set() {
        transaction.publish_control_1();
        BluetoothLowPowerRuntimeControlObservation::Control2SetControl1Published
    } else {
        BluetoothLowPowerRuntimeControlObservation::Control2Clear
    };
    transaction.fence();
    observation
}

fn execute_modem_lp_timer_start(transaction: &mut impl ModemLpTimerStartTransaction) {
    transaction.start_counter();
    transaction.fence();
}

struct HardwareModemLpTimerTransaction<'registers> {
    registers: &'registers crate::svd::BtdmRuntimeControl,
}

struct HardwareModemLpTimerLowPowerInitTransaction<'registers> {
    config: &'registers crate::svd::BtdmLowPowerConfig,
    runtime_control: &'registers crate::svd::BtdmRuntimeControl,
}

impl ModemLpTimerLowPowerInitTransaction for HardwareModemLpTimerLowPowerInitTransaction<'_> {
    fn initialize_config(&mut self) {
        crate::svd::fixed_register_sequence::initialize_bluetooth_low_power_config(self.config);
    }

    fn clear_controls_0_4(&mut self) {
        crate::generated::clear_bluetooth_low_power_controls_0_4(self.runtime_control);
    }

    fn control_2_is_set(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_modem_lp_timer_control_2(self.runtime_control)
    }

    fn publish_control_1(&mut self) {
        crate::generated::publish_bluetooth_modem_lp_timer_control_1(self.runtime_control);
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl ModemLpTimerStartTransaction for HardwareModemLpTimerTransaction<'_> {
    fn start_counter(&mut self) {
        crate::svd::fixed_register_write::start_bluetooth_runtime_timer(self.registers);
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl ModemLpTimerTransaction for HardwareModemLpTimerTransaction<'_> {
    fn prepare_hardware(&mut self) {
        crate::svd::zero_register_write::prepare_bluetooth_modem_lp_timer_command_004c(
            self.registers,
        );
        crate::svd::field_or_modify::prepare_bluetooth_modem_lp_timer_control_25(self.registers);
        crate::svd::fixed_register_image::prepare_bluetooth_modem_lp_timer_command_0004(
            self.registers,
        );
        crate::svd::fixed_register_image::prepare_bluetooth_modem_lp_timer_command_0008(
            self.registers,
        );
        crate::svd::fixed_register_image::prepare_bluetooth_modem_lp_timer_command_0014(
            self.registers,
        );
        crate::svd::fixed_register_image::prepare_bluetooth_modem_lp_timer_command_0034(
            self.registers,
        );
        crate::svd::fixed_register_image::prepare_bluetooth_modem_lp_timer_command_0010(
            self.registers,
        );
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl ModemLpTimerInterruptTransaction for HardwareModemLpTimerTransaction<'_> {
    fn read_status_0038(&mut self) -> u32 {
        crate::svd::field_read::observe_bluetooth_modem_lp_timer_status_0038(self.registers)
    }

    fn read_value_006c(&mut self) -> u32 {
        crate::svd::field_read::observe_bluetooth_modem_lp_timer_value_006c(self.registers)
    }

    fn control_0058_bit_2_is_set(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_modem_lp_timer_control_2(self.registers)
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
        crate::svd::field_read::observe_bluetooth_modem_lp_timer_state_0024(self.registers) != 0
    }

    fn clear_state_0024(&mut self) {
        crate::svd::zero_register_write::clear_bluetooth_modem_lp_timer_state_0024(self.registers);
    }

    fn sample_state_002c_low_byte_nonzero(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_modem_lp_timer_state_002c(self.registers) != 0
    }

    fn clear_state_002c(&mut self) {
        crate::svd::zero_register_write::clear_bluetooth_modem_lp_timer_state_002c(self.registers);
    }

    fn sample_final_state_0024(&mut self) {
        let _ = crate::svd::field_read::observe_bluetooth_modem_lp_timer_state_0024(self.registers);
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl ModemLpTimerRuntimeTransaction for HardwareModemLpTimerTransaction<'_> {
    fn read_counter(&mut self) -> u32 {
        crate::svd::field_read::observe_bluetooth_modem_lp_timer_counter(self.registers)
    }

    fn rollover_low_byte_nonzero(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_modem_lp_timer_state_002c(self.registers) != 0
    }

    fn clear_rollover(&mut self) {
        crate::svd::zero_register_write::clear_bluetooth_modem_lp_timer_state_002c(self.registers);
    }

    fn publish_software_pending(&mut self) {
        crate::svd::fixed_register_image::publish_bluetooth_modem_lp_timer_software_pending(
            self.registers,
        );
    }

    fn disable_compare(&mut self) {
        crate::svd::fixed_register_image::disable_bluetooth_modem_lp_timer_compare(self.registers);
    }

    fn write_compare(&mut self, image: crate::generated::BluetoothModemLpTimerCompareImage) {
        crate::generated::publish_bluetooth_modem_lp_timer_compare(self.registers, image);
    }

    fn trigger_timer_command(&mut self) {
        crate::svd::fixed_register_image::trigger_bluetooth_modem_lp_timer_command(self.registers);
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
    /// This transition extracts the timer partition before the first MMIO
    /// effect while leaving the disjoint controller task owner in place.
    /// Cancellation or panic can therefore only lose timer authority
    /// fail-stop. The returned state exposes no rollback because the reviewed
    /// source-127 teardown does not reverse these eight operations.
    #[doc(hidden)]
    pub fn prepare_modem_lp_timer_registers(
        &mut self,
    ) -> Result<BluetoothModemLpTimerRegistersPrepared, BluetoothModemLpTimerOwnerError> {
        let timer = self
            .modem_lp_timer
            .take()
            .ok_or(BluetoothModemLpTimerOwnerError::OwnerSeparated)?;
        {
            let mut transaction = HardwareModemLpTimerTransaction {
                registers: &timer.peripherals.btdm_runtime_control,
            };
            execute_modem_lp_timer_prepare(&mut transaction);
        }
        Ok(BluetoothModemLpTimerRegistersPrepared { timer })
    }
}

impl BluetoothModemLpTimerRegistersPrepared {
    /// Complete the reviewed BTDM low-power hardware component.
    ///
    /// SOURCE: complete public ESP32-S31 `libbtdm_common.a` member `20.o`
    /// symbol `r_sym_bt_JHP69cMcA5vCzdaxdFgT` proves the generated twelve-write
    /// configuration sequence. Its complete caller
    /// `r_sym_bt_cgaCegmpqnbaoszyOy3c` then clears `CONTROL_0` and `CONTROL_4`,
    /// samples `CONTROL_2`, and conditionally publishes `CONTROL_1` from a
    /// fresh read. A device fence closes this restricted-PAC transaction.
    ///
    /// The disjoint task owner supplies only `BTDM_LOW_POWER_CONFIG`; this
    /// prepared owner supplies only `BTDM_RUNTIME_CONTROL`. Neither register
    /// block can alias, and task ownership remains available after return.
    #[doc(hidden)]
    pub fn initialize_low_power_hardware(
        self,
        task: &mut BluetoothTaskRegisters,
    ) -> BluetoothModemLpTimerLowPowerHardwareInitialized {
        let runtime_control = {
            let mut transaction = HardwareModemLpTimerLowPowerInitTransaction {
                config: &task.bluetooth.btdm_low_power_config,
                runtime_control: &self.timer.peripherals.btdm_runtime_control,
            };
            execute_modem_lp_timer_low_power_init(&mut transaction)
        };
        BluetoothModemLpTimerLowPowerHardwareInitialized {
            timer: self.timer,
            runtime_control,
        }
    }
}

impl BluetoothModemLpTimerLowPowerHardwareInitialized {
    /// Start the BTDM runtime timer during controller hardware enable.
    ///
    /// SOURCE: complete current `libbtdm_common.a` member `9.o` symbol
    /// `r_sym_bt_ymLPVGRY14FVW494j9ZD` writes the sole reviewed command and
    /// returns. The instruction-identical public same-chip predecessor names
    /// the operation `r_btdm_hal_rtc_start`; its complete caller places this
    /// edge after controller output preparation and before primary CPU-route
    /// allocation. Consuming this owner proves the command cannot be repeated
    /// through the same timer epoch.
    pub fn start_runtime_timer(self) -> BluetoothModemLpTimerCounterStarted {
        let mut transaction = HardwareModemLpTimerTransaction {
            registers: &self.timer.peripherals.btdm_runtime_control,
        };
        execute_modem_lp_timer_start(&mut transaction);
        BluetoothModemLpTimerCounterStarted {
            timer: self.timer,
            runtime_control: self.runtime_control,
        }
    }
}

impl BluetoothModemLpTimerCounterStarted {
    /// Return the low-power runtime-control branch retained across start.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }

    /// Move the started timer-register owner into source-127 ISR storage.
    ///
    /// This transition performs no MMIO. The platform must store the returned
    /// value before enabling the CPU route and recover it only after that route
    /// is disabled and no hard handler remains in flight.
    pub fn stage_for_interrupt(self) -> BluetoothModemLpTimerInterruptReady {
        BluetoothModemLpTimerInterruptReady { timer: self.timer }
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
                registers: &self.timer.peripherals.btdm_runtime_control,
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
mod tests;
