//! Restricted ownership for the reviewed Bluetooth interrupt transaction.

#![deny(unsafe_code)]

use super::{
    BluetoothInterruptRegisters, BluetoothInterruptSetup, device_fence,
    svd::{fixed_register_image, interrupt_snapshot},
};

/// First-bank sources enabled by the complete primary BTDM IRQ setup helper.
pub const BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK: u32 = 0x0000_8000;

/// Second-bank sources enabled by the complete primary BTDM IRQ setup helper.
pub const BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK: u32 = 0x0000_1300;

/// First-bank sources controlled by the complete dynamic scheduler helper.
///
/// The restricted PAC does not expose an enable transition yet: live shared
/// ISR storage and the scheduler-list consumer remain lifecycle prerequisites.
pub const BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK: u32 = 0x1820_0000;

/// Second-bank source controlled by the complete dynamic scheduler helper.
pub const BLUETOOTH_PRIMARY_DYNAMIC_BANK_1_MASK: u32 = 0x0000_0008;

const BLUETOOTH_PRIMARY_FAULT_BANK_0_SOURCE_15: u32 = 1 << 15;
const BLUETOOTH_PRIMARY_FAULT_BANK_1_SOURCE_8: u32 = 1 << 8;
const BLUETOOTH_PRIMARY_FAULT_BANK_1_SOURCE_9: u32 = 1 << 9;
const BLUETOOTH_PRIMARY_FAULT_BANK_1_SOURCE_12: u32 = 1 << 12;

/// Affine proof for controller-side interrupt output preparation.
///
/// The exact interrupt-bank transaction lives in the PAC, while controller
/// HAL-init and powered ordering belong to a higher lifecycle. Safe HAL code
/// consumes this value once; only that lifecycle may assume it.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_pac::BluetoothInterruptOutputPreparationPrerequisite;
///
/// fn duplicate(proof: BluetoothInterruptOutputPreparationPrerequisite) {
///     let _first = proof;
///     let _second = proof;
/// }
/// ```
#[must_use = "the interrupt-output proof must be consumed by its exact transaction"]
pub struct BluetoothInterruptOutputPreparationPrerequisite {
    _private: (),
}

impl BluetoothInterruptOutputPreparationPrerequisite {
    /// Assume every external prerequisite for controller output preparation.
    ///
    /// # Safety
    ///
    /// The caller must retain enabled clocks, completed controller HAL-init,
    /// quiescent dynamic Link-Layer sources and an inactive CPU route for the
    /// same unique Bluetooth owner.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "construction is the explicit post-controller-init IRQ proof boundary"
    )]
    pub unsafe fn assume_satisfied() -> Self {
        Self { _private: () }
    }
}

trait BluetoothInterruptControl {
    fn clear_primary_baseline_bank_0(&mut self);
    fn clear_primary_baseline_bank_1(&mut self);
    fn enable_primary_baseline_bank_0(&mut self);
    fn enable_primary_baseline_bank_1(&mut self);
    fn prepare_output(&mut self);
    fn release_output_0(&mut self);
    fn release_output_1(&mut self);
    fn mask_primary_baseline_bank_0(&mut self);
    fn mask_primary_baseline_bank_1(&mut self);
}

struct HardwareInterruptControl<'a> {
    bank: &'a super::svd::BluetoothInterruptBank,
}

impl BluetoothInterruptControl for HardwareInterruptControl<'_> {
    #[allow(
        unsafe_code,
        reason = "the complete vendor helper qualifies this exact W1C image"
    )]
    fn clear_primary_baseline_bank_0(&mut self) {
        unsafe {
            self.bank.irq_clear_0().write_with_zero(|writer| {
                writer
                    .pending_bits()
                    .bits(BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK)
            });
        }
    }

    #[allow(
        unsafe_code,
        reason = "the complete vendor helper qualifies this exact W1C image"
    )]
    fn clear_primary_baseline_bank_1(&mut self) {
        unsafe {
            self.bank.irq_clear_1().write_with_zero(|writer| {
                writer
                    .pending_bits()
                    .bits(BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK)
            });
        }
    }

    fn enable_primary_baseline_bank_0(&mut self) {
        self.bank
            .irq_enable_0()
            .modify(|_, writer| writer.source_15().set_bit());
    }

    fn enable_primary_baseline_bank_1(&mut self) {
        self.bank.irq_enable_1().modify(|_, writer| {
            writer
                .source_8()
                .set_bit()
                .source_9()
                .set_bit()
                .source_12()
                .set_bit()
        });
    }

    fn prepare_output(&mut self) {
        fixed_register_image::prepare_bluetooth_interrupt_output(self.bank);
    }

    fn release_output_0(&mut self) {
        fixed_register_image::release_bluetooth_interrupt_output_0(self.bank);
    }

    fn release_output_1(&mut self) {
        fixed_register_image::release_bluetooth_interrupt_output_1(self.bank);
    }

    fn mask_primary_baseline_bank_0(&mut self) {
        self.bank
            .irq_enable_0()
            .modify(|_, writer| writer.source_15().clear_bit());
    }

    fn mask_primary_baseline_bank_1(&mut self) {
        self.bank.irq_enable_1().modify(|_, writer| {
            writer
                .source_8()
                .clear_bit()
                .source_9()
                .clear_bit()
                .source_12()
                .clear_bit()
        });
    }
}

trait BluetoothPrimaryInterruptControl {
    type Bank0Snapshot;
    type Bank1Snapshot;

    fn sample_bank_0(&mut self) -> Self::Bank0Snapshot;
    fn sample_bank_1(&mut self) -> Self::Bank1Snapshot;
    fn bank_0_bits(&self, snapshot: &Self::Bank0Snapshot) -> u32;
    fn bank_1_bits(&self, snapshot: &Self::Bank1Snapshot) -> u32;
    fn acknowledge_bank_0(&mut self, snapshot: Self::Bank0Snapshot);
    fn acknowledge_bank_1(&mut self, snapshot: Self::Bank1Snapshot);
    fn read_diagnostic_detail_0(&mut self) -> u32;
    fn read_diagnostic_detail_1(&mut self) -> u32;
    fn read_diagnostic_state(&mut self) -> u32;
}

impl BluetoothPrimaryInterruptControl for HardwareInterruptControl<'_> {
    type Bank0Snapshot = interrupt_snapshot::BluetoothPrimaryInterruptBank0Snapshot;
    type Bank1Snapshot = interrupt_snapshot::BluetoothPrimaryInterruptBank1Snapshot;

    fn sample_bank_0(&mut self) -> Self::Bank0Snapshot {
        interrupt_snapshot::sample_bluetooth_primary_interrupt_bank_0(self.bank)
    }

    fn sample_bank_1(&mut self) -> Self::Bank1Snapshot {
        interrupt_snapshot::sample_bluetooth_primary_interrupt_bank_1(self.bank)
    }

    fn bank_0_bits(&self, snapshot: &Self::Bank0Snapshot) -> u32 {
        snapshot.bits()
    }

    fn bank_1_bits(&self, snapshot: &Self::Bank1Snapshot) -> u32 {
        snapshot.bits()
    }

    fn acknowledge_bank_0(&mut self, snapshot: Self::Bank0Snapshot) {
        interrupt_snapshot::acknowledge_bluetooth_primary_interrupt_bank_0(self.bank, snapshot);
    }

    fn acknowledge_bank_1(&mut self, snapshot: Self::Bank1Snapshot) {
        interrupt_snapshot::acknowledge_bluetooth_primary_interrupt_bank_1(self.bank, snapshot);
    }

    fn read_diagnostic_detail_0(&mut self) -> u32 {
        self.bank.irq_diagnostic_detail_0().read().bits()
    }

    fn read_diagnostic_detail_1(&mut self) -> u32 {
        self.bank.irq_diagnostic_detail_1().read().bits()
    }

    fn read_diagnostic_state(&mut self) -> u32 {
        self.bank.irq_diagnostic_state().read().bits()
    }
}

fn execute_primary_prepare(control: &mut impl BluetoothInterruptControl) {
    control.clear_primary_baseline_bank_0();
    control.clear_primary_baseline_bank_1();
    control.enable_primary_baseline_bank_0();
    control.enable_primary_baseline_bank_1();
    control.prepare_output();
}

fn execute_primary_release(control: &mut impl BluetoothInterruptControl) {
    control.release_output_0();
    control.release_output_1();
    control.mask_primary_baseline_bank_0();
    control.mask_primary_baseline_bank_1();
}

/// Controller-side interrupt output prepared before a CPU route is installed.
///
/// This state owns the exact baseline clear/enable transaction followed by
/// `IRQ_CONTROL_0 = 1`, immediately before the vendor platform allocates the
/// primary CPU interrupt. It still exposes no status capture: both CPU routes
/// must share one staged ISR owner before either route is enabled.
#[must_use = "the prepared Bluetooth interrupt output must be routed or released"]
pub struct BluetoothInterruptOutputPrepared {
    peripherals: super::svd::peripheral_ownership::BluetoothInterruptPeripherals,
}

impl BluetoothInterruptSetup {
    /// Prepare the controller-side interrupt output before installing a CPU
    /// route.
    ///
    /// SOURCE: complete ESP32-S31 `libbtdm_common.a` `btdm_hal.c` helpers. The
    /// composite setup clears bank images `0x0000_8000` and `0x0000_1300`,
    /// ORs those same baseline sources into the two enable banks, writes one
    /// to `0x2010_100c`, then the outer path calls the platform interrupt
    /// allocator for source 124. The earlier HAL-init part of that composite
    /// remains a separate lifecycle prerequisite.
    pub fn prepare_controller_output(
        self,
        _prerequisite: BluetoothInterruptOutputPreparationPrerequisite,
    ) -> BluetoothInterruptOutputPrepared {
        let mut control = HardwareInterruptControl {
            bank: &self.peripherals.bluetooth_interrupt_bank,
        };
        execute_primary_prepare(&mut control);
        device_fence();
        BluetoothInterruptOutputPrepared {
            peripherals: self.peripherals,
        }
    }
}

impl BluetoothInterruptOutputPrepared {
    pub(super) fn scheduler_busy_after_routes(&self) -> bool {
        self.peripherals
            .bluetooth_scheduler_interrupt_runtime
            .scheduler_state()
            .read()
            .busy()
            .bit_is_set()
    }

    /// Release a prepared controller output after any CPU route has been
    /// removed.
    ///
    /// The complete teardown leaf frees the CPU route before this transaction,
    /// writes image one to `0x2010_1010` and then `0x2010_1014`, and finally
    /// clears the same baseline enable groups. Dynamic Link-Layer sources must
    /// already have been quiesced by their own owners.
    pub fn release_controller_output(self) -> BluetoothInterruptSetup {
        let mut control = HardwareInterruptControl {
            bank: &self.peripherals.bluetooth_interrupt_bank,
        };
        execute_primary_release(&mut control);
        device_fence();
        BluetoothInterruptSetup {
            peripherals: self.peripherals,
        }
    }

    /// Transfer the prepared bank into stable storage shared by both hard
    /// handlers before either CPU route is enabled.
    ///
    /// This conversion performs no MMIO and does not itself prove that source
    /// 124 or source 133 has been routed. A platform adapter must retain the
    /// returned value in interrupt-safe storage, bind both routes on one core,
    /// and recover it only after both routes have been disabled.
    pub fn stage_for_cpu_routes(self) -> BluetoothInterruptRegisters {
        BluetoothInterruptRegisters {
            peripherals: self.peripherals,
        }
    }
}

#[cfg(feature = "validation-probes")]
impl BluetoothInterruptSetup {
    /// Forge the post-route state for one isolated terminal comparison image.
    ///
    /// # Safety
    ///
    /// CPU routes and shared ISR access must be absent, and the image must not
    /// perform any later radio operation or reconstruct neutral ownership.
    #[allow(
        unsafe_code,
        reason = "the isolated validation image explicitly assumes post-route ownership"
    )]
    pub(super) unsafe fn assume_output_prepared_after_routes_for_validation(
        self,
    ) -> BluetoothInterruptOutputPrepared {
        BluetoothInterruptOutputPrepared {
            peripherals: self.peripherals,
        }
    }
}

impl BluetoothInterruptRegisters {
    /// Return the interrupt partition to controller-output-only ownership.
    ///
    /// The caller must mask events and disable the CPU route first. This
    /// method performs no controller transaction; the separate output-release
    /// edge publishes the reviewed teardown strobes.
    pub fn deactivate(self) -> BluetoothInterruptOutputPrepared {
        BluetoothInterruptOutputPrepared {
            peripherals: self.peripherals,
        }
    }
}

/// Complete opaque observation captured and acknowledged by one NRT epoch.
///
/// The two words intentionally have no public inverse constructor and no
/// inferred bit semantics. They are value-only evidence for later event
/// classification and diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "an NRT observation must be retained or explicitly classified"]
pub struct BluetoothNrtInterruptObservation {
    bank_0: u32,
    bank_1: u32,
}

/// Masked primary BT MAC status captured from `IRQ_STATUS_0/1`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPrimaryInterruptObservation {
    bank_0: u32,
    bank_1: u32,
}

impl BluetoothPrimaryInterruptObservation {
    /// Complete first-bank image observed at `0x2010_105c`.
    pub const fn bank_0_bits(self) -> u32 {
        self.bank_0
    }

    /// Complete second-bank image observed at `0x2010_1068`.
    pub const fn bank_1_bits(self) -> u32 {
        self.bank_1
    }
}

impl BluetoothNrtInterruptObservation {
    /// Complete first-bank image observed at `0x2010_1340`.
    pub const fn bank_0_bits(self) -> u32 {
        self.bank_0
    }

    /// Complete second-bank image observed at `0x2010_1348`.
    pub const fn bank_1_bits(self) -> u32 {
        self.bank_1
    }
}

/// Lossless evidence for every source handled by the primary fault prefix.
///
/// Source names remain positional because the complete handler proves their
/// assertion behavior and conditional diagnostic reads, but not the silicon's
/// undocumented fault names. The optional words distinguish a pending source
/// whose captured diagnostic image is zero from a source that was not pending.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPrimaryFaultEvidence {
    bank_0_sources: u32,
    bank_1_sources: u32,
    source_9_details: Option<[u32; 2]>,
    source_12_state: Option<u32>,
}

impl BluetoothPrimaryFaultEvidence {
    const fn from_parts(
        observation: BluetoothPrimaryInterruptObservation,
        source_9_details: Option<[u32; 2]>,
        source_12_state: Option<u32>,
    ) -> Self {
        Self {
            bank_0_sources: observation.bank_0_bits() & BLUETOOTH_PRIMARY_FAULT_BANK_0_SOURCE_15,
            bank_1_sources: observation.bank_1_bits()
                & (BLUETOOTH_PRIMARY_FAULT_BANK_1_SOURCE_8
                    | BLUETOOTH_PRIMARY_FAULT_BANK_1_SOURCE_9
                    | BLUETOOTH_PRIMARY_FAULT_BANK_1_SOURCE_12),
            source_9_details,
            source_12_state,
        }
    }

    /// Whether at least one reviewed primary fault source was pending.
    pub const fn is_fault(self) -> bool {
        self.bank_0_sources != 0 || self.bank_1_sources != 0
    }

    /// Pending reviewed fault sources from masked status bank zero.
    pub const fn bank_0_source_bits(self) -> u32 {
        self.bank_0_sources
    }

    /// Pending reviewed fault sources from masked status bank one.
    pub const fn bank_1_source_bits(self) -> u32 {
        self.bank_1_sources
    }

    /// Diagnostic words captured when bank-one source 9 was pending.
    pub const fn source_9_details(self) -> Option<[u32; 2]> {
        self.source_9_details
    }

    /// Diagnostic state captured when bank-one source 12 was pending.
    pub const fn source_12_state(self) -> Option<u32> {
        self.source_12_state
    }
}

/// One complete primary source-124 sample, acknowledgement and fault capture.
///
/// Keeping the observation and fault evidence in one affine value prevents a
/// live handler from acknowledging baseline fault bits without also retaining
/// the diagnostic words that are only meaningful in that interrupt epoch.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a primary interrupt epoch must be classified before returning from the handler"]
pub struct BluetoothPrimaryInterruptEpoch {
    observation: BluetoothPrimaryInterruptObservation,
    fault_evidence: BluetoothPrimaryFaultEvidence,
}

impl BluetoothPrimaryInterruptEpoch {
    /// Complete masked status image captured before acknowledgement.
    pub const fn observation(&self) -> BluetoothPrimaryInterruptObservation {
        self.observation
    }

    /// Source and diagnostic evidence captured by the fault prefix.
    pub const fn fault_evidence(&self) -> BluetoothPrimaryFaultEvidence {
        self.fault_evidence
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn for_validation(
        bank_0: u32,
        bank_1: u32,
        diagnostic_detail_0: u32,
        diagnostic_detail_1: u32,
        diagnostic_state: u32,
    ) -> Self {
        let observation = BluetoothPrimaryInterruptObservation { bank_0, bank_1 };
        let source_9_details = if bank_1 & BLUETOOTH_PRIMARY_FAULT_BANK_1_SOURCE_9 != 0 {
            Some([diagnostic_detail_0, diagnostic_detail_1])
        } else {
            None
        };
        let source_12_state = if bank_1 & BLUETOOTH_PRIMARY_FAULT_BANK_1_SOURCE_12 != 0 {
            Some(diagnostic_state)
        } else {
            None
        };
        Self {
            observation,
            fault_evidence: BluetoothPrimaryFaultEvidence::from_parts(
                observation,
                source_9_details,
                source_12_state,
            ),
        }
    }
}

fn execute_primary_interrupt_epoch(
    control: &mut impl BluetoothPrimaryInterruptControl,
) -> BluetoothPrimaryInterruptEpoch {
    let bank_0 = control.sample_bank_0();
    let bank_1 = control.sample_bank_1();
    let observation = BluetoothPrimaryInterruptObservation {
        bank_0: control.bank_0_bits(&bank_0),
        bank_1: control.bank_1_bits(&bank_1),
    };
    control.acknowledge_bank_0(bank_0);
    control.acknowledge_bank_1(bank_1);

    let source_9_details =
        if observation.bank_1_bits() & BLUETOOTH_PRIMARY_FAULT_BANK_1_SOURCE_9 != 0 {
            Some([
                control.read_diagnostic_detail_0(),
                control.read_diagnostic_detail_1(),
            ])
        } else {
            None
        };
    let source_12_state =
        if observation.bank_1_bits() & BLUETOOTH_PRIMARY_FAULT_BANK_1_SOURCE_12 != 0 {
            Some(control.read_diagnostic_state())
        } else {
            None
        };

    BluetoothPrimaryInterruptEpoch {
        observation,
        fault_evidence: BluetoothPrimaryFaultEvidence::from_parts(
            observation,
            source_9_details,
            source_12_state,
        ),
    }
}

impl BluetoothInterruptRegisters {
    /// Capture and acknowledge one complete primary BT MAC interrupt image.
    ///
    /// This is the exact prefix of the source-124 handler: read masked status
    /// bank zero, read masked status bank one, copy the first image to clear
    /// bank zero, then copy the second image to clear bank one. The transaction
    /// then preserves the complete diagnostic prefix: source 9 captures both
    /// detail words and source 12 captures the state word. Callback dispatch
    /// and the special scheduler-event suffix remain higher-layer work.
    pub fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
        let mut control = HardwareInterruptControl {
            bank: &self.peripherals.bluetooth_interrupt_bank,
        };
        let epoch = execute_primary_interrupt_epoch(&mut control);
        device_fence();
        epoch
    }

    /// Capture and acknowledge one complete controller interrupt epoch.
    ///
    /// The order is the exact complete ESP32-S31 NRT ISR prefix:
    ///
    /// 1. read first status snapshot;
    /// 2. read second status snapshot;
    /// 3. write the first image to its write-one-to-clear bank;
    /// 4. write the second image to its write-one-to-clear bank.
    ///
    /// Separate sample or acknowledgement methods are deliberately absent:
    /// the reviewed vendor body does not authorize another ordering.
    pub fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptObservation {
        let bank_0 = interrupt_snapshot::sample_bluetooth_interrupt_bank_0(
            &self.peripherals.bluetooth_interrupt_bank,
        );
        let bank_1 = interrupt_snapshot::sample_bluetooth_interrupt_bank_1(
            &self.peripherals.bluetooth_interrupt_bank,
        );
        let observation = BluetoothNrtInterruptObservation {
            bank_0: bank_0.bits(),
            bank_1: bank_1.bits(),
        };
        interrupt_snapshot::acknowledge_bluetooth_interrupt_bank_0(
            &self.peripherals.bluetooth_interrupt_bank,
            bank_0,
        );
        interrupt_snapshot::acknowledge_bluetooth_interrupt_bank_1(
            &self.peripherals.bluetooth_interrupt_bank,
            bank_1,
        );
        device_fence();
        observation
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{
        BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK, BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK,
        BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK, BluetoothInterruptControl,
        BluetoothNrtInterruptObservation, BluetoothPrimaryInterruptControl,
        BluetoothPrimaryInterruptObservation, execute_primary_interrupt_epoch,
        execute_primary_prepare, execute_primary_release,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        ClearBank0,
        ClearBank1,
        EnableBank0,
        EnableBank1,
        PrepareOutput,
        ReleaseOutput0,
        ReleaseOutput1,
        MaskBank0,
        MaskBank1,
    }

    #[derive(Default)]
    struct SetupRecorder {
        operations: Vec<Operation>,
    }

    impl BluetoothInterruptControl for SetupRecorder {
        fn clear_primary_baseline_bank_0(&mut self) {
            self.operations.push(Operation::ClearBank0);
        }

        fn clear_primary_baseline_bank_1(&mut self) {
            self.operations.push(Operation::ClearBank1);
        }

        fn enable_primary_baseline_bank_0(&mut self) {
            self.operations.push(Operation::EnableBank0);
        }

        fn enable_primary_baseline_bank_1(&mut self) {
            self.operations.push(Operation::EnableBank1);
        }

        fn prepare_output(&mut self) {
            self.operations.push(Operation::PrepareOutput);
        }

        fn release_output_0(&mut self) {
            self.operations.push(Operation::ReleaseOutput0);
        }

        fn release_output_1(&mut self) {
            self.operations.push(Operation::ReleaseOutput1);
        }

        fn mask_primary_baseline_bank_0(&mut self) {
            self.operations.push(Operation::MaskBank0);
        }

        fn mask_primary_baseline_bank_1(&mut self) {
            self.operations.push(Operation::MaskBank1);
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum EpochOperation {
        SampleBank0,
        SampleBank1,
        AcknowledgeBank0(u32),
        AcknowledgeBank1(u32),
        ReadDiagnosticDetail0,
        ReadDiagnosticDetail1,
        ReadDiagnosticState,
    }

    struct EpochRecorder {
        bank_0: u32,
        bank_1: u32,
        operations: Vec<EpochOperation>,
    }

    impl EpochRecorder {
        fn new(bank_0: u32, bank_1: u32) -> Self {
            Self {
                bank_0,
                bank_1,
                operations: Vec::new(),
            }
        }
    }

    impl BluetoothPrimaryInterruptControl for EpochRecorder {
        type Bank0Snapshot = u32;
        type Bank1Snapshot = u32;

        fn sample_bank_0(&mut self) -> Self::Bank0Snapshot {
            self.operations.push(EpochOperation::SampleBank0);
            self.bank_0
        }

        fn sample_bank_1(&mut self) -> Self::Bank1Snapshot {
            self.operations.push(EpochOperation::SampleBank1);
            self.bank_1
        }

        fn bank_0_bits(&self, snapshot: &Self::Bank0Snapshot) -> u32 {
            *snapshot
        }

        fn bank_1_bits(&self, snapshot: &Self::Bank1Snapshot) -> u32 {
            *snapshot
        }

        fn acknowledge_bank_0(&mut self, snapshot: Self::Bank0Snapshot) {
            self.operations
                .push(EpochOperation::AcknowledgeBank0(snapshot));
        }

        fn acknowledge_bank_1(&mut self, snapshot: Self::Bank1Snapshot) {
            self.operations
                .push(EpochOperation::AcknowledgeBank1(snapshot));
        }

        fn read_diagnostic_detail_0(&mut self) -> u32 {
            self.operations.push(EpochOperation::ReadDiagnosticDetail0);
            0x1111_0000
        }

        fn read_diagnostic_detail_1(&mut self) -> u32 {
            self.operations.push(EpochOperation::ReadDiagnosticDetail1);
            0x2222_0000
        }

        fn read_diagnostic_state(&mut self) -> u32 {
            self.operations.push(EpochOperation::ReadDiagnosticState);
            0x3333_0000
        }
    }

    #[test]
    fn nrt_observation_preserves_both_opaque_banks() {
        let observation = BluetoothNrtInterruptObservation {
            bank_0: 0xa55a_00f0,
            bank_1: 0x5aa5_f00f,
        };

        assert_eq!(observation.bank_0_bits(), 0xa55a_00f0);
        assert_eq!(observation.bank_1_bits(), 0x5aa5_f00f);
    }

    #[test]
    fn primary_observation_preserves_both_masked_banks() {
        let observation = BluetoothPrimaryInterruptObservation {
            bank_0: 0x1820_8000,
            bank_1: 0x0000_1308,
        };

        assert_eq!(observation.bank_0_bits(), 0x1820_8000);
        assert_eq!(observation.bank_1_bits(), 0x0000_1308);
    }

    #[test]
    fn primary_epoch_acknowledges_before_conditional_fault_capture() {
        let mut recorder = EpochRecorder::new(
            BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK,
            BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK,
        );
        let epoch = execute_primary_interrupt_epoch(&mut recorder);

        assert_eq!(
            recorder.operations,
            [
                EpochOperation::SampleBank0,
                EpochOperation::SampleBank1,
                EpochOperation::AcknowledgeBank0(BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK),
                EpochOperation::AcknowledgeBank1(BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK),
                EpochOperation::ReadDiagnosticDetail0,
                EpochOperation::ReadDiagnosticDetail1,
                EpochOperation::ReadDiagnosticState,
            ]
        );
        assert_eq!(
            epoch.fault_evidence().bank_0_source_bits(),
            BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK
        );
        assert_eq!(
            epoch.fault_evidence().bank_1_source_bits(),
            BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK
        );
        assert_eq!(
            epoch.fault_evidence().source_9_details(),
            Some([0x1111_0000, 0x2222_0000])
        );
        assert_eq!(epoch.fault_evidence().source_12_state(), Some(0x3333_0000));
    }

    #[test]
    fn primary_epoch_skips_diagnostic_reads_without_matching_sources() {
        let dynamic_only = BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK;
        let mut recorder = EpochRecorder::new(dynamic_only, 0);
        let epoch = execute_primary_interrupt_epoch(&mut recorder);

        assert_eq!(
            recorder.operations,
            [
                EpochOperation::SampleBank0,
                EpochOperation::SampleBank1,
                EpochOperation::AcknowledgeBank0(dynamic_only),
                EpochOperation::AcknowledgeBank1(0),
            ]
        );
        assert!(!epoch.fault_evidence().is_fault());
        assert_eq!(epoch.fault_evidence().source_9_details(), None);
        assert_eq!(epoch.fault_evidence().source_12_state(), None);
    }

    #[test]
    fn primary_prepare_preserves_clear_enable_strobe_order() {
        let mut recorder = SetupRecorder::default();
        execute_primary_prepare(&mut recorder);

        assert_eq!(
            recorder.operations,
            [
                Operation::ClearBank0,
                Operation::ClearBank1,
                Operation::EnableBank0,
                Operation::EnableBank1,
                Operation::PrepareOutput,
            ]
        );
    }

    #[test]
    fn primary_release_preserves_strobe_then_mask_order() {
        let mut recorder = SetupRecorder::default();
        execute_primary_release(&mut recorder);

        assert_eq!(
            recorder.operations,
            [
                Operation::ReleaseOutput0,
                Operation::ReleaseOutput1,
                Operation::MaskBank0,
                Operation::MaskBank1,
            ]
        );
    }
}
