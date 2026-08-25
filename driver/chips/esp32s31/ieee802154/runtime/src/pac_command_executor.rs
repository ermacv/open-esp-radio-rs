//! ESP32-S31 PAC-backed task-side command execution.
//!
//! The executor owns one narrow IEEE 802.15.4 register lease for the complete
//! command epoch. Its constructor is crate-private until the whole-radio HAL
//! can transfer a proved stopped owner together with IRQ-route ownership.
//! Consequently this module adds no public escape hatch from a raw PAC owner.
//!
//! The concrete leaves below are a direct typed port of the public ESP-IDF
//! common LL: policy and DMA publication precede exactly one of `TX_START`,
//! `RX_START`, `CCA_TX_START`, or `ED_START`. `STOP` is deliberately not used
//! as a synchronous idle proof; terminal IRQ reconciliation owns that edge.

use open_esp_radio_esp32s31_ieee802154_dma::{RxDmaAddress, TxDmaAddress};
use open_esp_radio_esp32s31_ieee802154_mac::{
    MacActivePhase, MacCommandIntent, MacTransmitAcknowledgement,
};
use open_esp_radio_esp32s31_pac::{
    Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154EdDurationUnits,
    Ieee802154FrequencyCode, Ieee802154MacCommand, Ieee802154MacControl,
    Ieee802154MacPolicySnapshot, Ieee802154PanIdentity, Ieee802154TaskRegisters,
};

use crate::{MacCommandCapability, MacCommandExecutor, MacRuntime, MacRuntimePolicyError, sealed};

/// Failure of one finite ESP32-S31 task-side command step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31Ieee802154CommandError {
    /// A second start preparation was requested before the first command was
    /// either issued or abandoned.
    PreparationAlreadyOpen,
    /// A prior command remains active and has not crossed the reviewed
    /// completion/reconciliation boundary.
    OperationStillActive {
        /// Command whose completion still owns the executor.
        command: MacCommandIntent,
    },
    /// A plan step was invoked without first opening its quiescent start gate.
    PreparationNotOpen,
    /// A command-dependent step was reached before static policy passed its
    /// exact post-write readback.
    PolicyNotRefreshed,
    /// The requested ED duration could not be represented by the generated
    /// PAC field type without truncation.
    EnergyDetectionDurationOutOfRange {
        /// Complete rejected source-level duration.
        units: u16,
    },
    /// Standalone ED or CCA was requested without a duration publication in
    /// the same preparation epoch.
    EnergyDetectionDurationMissing,
    /// The static-policy image read after the device fence did not match the
    /// exact retained image.
    StaticPolicyReadbackMismatch {
        /// Policy image retained by this command epoch.
        expected: Ieee802154MacPolicySnapshot,
        /// Complete policy image sampled after refresh.
        observed: Ieee802154MacPolicySnapshot,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExecutorState {
    Quiescent,
    Preparing {
        policy_refreshed: bool,
        duration: Option<u16>,
    },
    Active(MacCommandIntent),
}

trait TaskCommandBackend {
    fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode);
    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode);
    fn set_cca_threshold_code(&mut self, threshold: i8);
    fn set_mac_control(&mut self, control: Ieee802154MacControl);
    fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeoutUnits);
    fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity);
    fn mac_policy_snapshot(&mut self) -> Ieee802154MacPolicySnapshot;
    fn publish_transmit_address(&mut self, address: u32);
    fn publish_receive_address(&mut self, address: u32);
    fn set_ed_duration(&mut self, duration: Ieee802154EdDurationUnits);
    fn request_command(&mut self, command: MacCommandIntent);
    fn order_device_accesses(&mut self);
}

struct PacTaskCommandBackend {
    task: Ieee802154TaskRegisters,
}

impl TaskCommandBackend for PacTaskCommandBackend {
    fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
        self.task
            .ieee802154_register_lease()
            .set_frequency_code(code);
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.task.ieee802154_register_lease().set_cca_mode(mode);
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.task
            .ieee802154_register_lease()
            .set_cca_threshold_code(threshold);
    }

    fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.task
            .ieee802154_register_lease()
            .set_mac_control(control);
    }

    fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeoutUnits) {
        self.task
            .ieee802154_register_lease()
            .set_ack_timeout(timeout);
    }

    fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.task
            .ieee802154_register_lease()
            .set_primary_pan_identity(identity);
    }

    fn mac_policy_snapshot(&mut self) -> Ieee802154MacPolicySnapshot {
        self.task.ieee802154_register_lease().mac_policy_snapshot()
    }

    fn publish_transmit_address(&mut self, address: u32) {
        self.task
            .ieee802154_register_lease()
            .publish_transmit_dma_address(address);
    }

    fn publish_receive_address(&mut self, address: u32) {
        self.task
            .ieee802154_register_lease()
            .publish_receive_dma_address(address);
    }

    fn set_ed_duration(&mut self, duration: Ieee802154EdDurationUnits) {
        self.task
            .ieee802154_register_lease()
            .set_ed_duration(duration);
    }

    fn request_command(&mut self, command: MacCommandIntent) {
        let command = match command {
            MacCommandIntent::Receive => Ieee802154MacCommand::Receive,
            MacCommandIntent::Transmit => Ieee802154MacCommand::Transmit,
            MacCommandIntent::TransmitWithClearChannelAssessment => {
                Ieee802154MacCommand::ClearChannelThenTransmit
            }
            MacCommandIntent::ClearChannelAssessment => Ieee802154MacCommand::EnergyDetection,
            MacCommandIntent::EnergyDetection => Ieee802154MacCommand::EnergyDetection,
        };
        self.task
            .ieee802154_register_lease()
            .request_mac_command(command);
    }

    fn order_device_accesses(&mut self) {
        self.task.order_device_accesses();
    }
}

struct TaskCommandExecutorCore<Backend> {
    backend: Backend,
    expected_policy: Ieee802154MacPolicySnapshot,
    state: ExecutorState,
}

impl<Backend: TaskCommandBackend> TaskCommandExecutorCore<Backend> {
    const fn new(backend: Backend, expected_policy: Ieee802154MacPolicySnapshot) -> Self {
        Self {
            backend,
            expected_policy,
            state: ExecutorState::Quiescent,
        }
    }

    fn validate_operation_policy(
        &self,
        phase: MacActivePhase,
    ) -> Result<(), MacRuntimePolicyError> {
        let control = self.expected_policy.control();
        match phase {
            MacActivePhase::Receive => {
                if control.tx_auto_ack() || control.enhanced_ack_tx() {
                    Err(MacRuntimePolicyError::ReceiveWouldTransmitAcknowledgement {
                        tx_auto_ack: control.tx_auto_ack(),
                        enhanced_ack_tx: control.enhanced_ack_tx(),
                    })
                } else {
                    Ok(())
                }
            }
            MacActivePhase::Transmit {
                acknowledgement, ..
            } => {
                if acknowledgement == MacTransmitAcknowledgement::Expected && !control.rx_auto_ack()
                {
                    Err(MacRuntimePolicyError::AcknowledgementReceptionDisabled)
                } else {
                    Ok(())
                }
            }
            MacActivePhase::AwaitingAcknowledgement { .. } => {
                if control.rx_auto_ack() {
                    Ok(())
                } else {
                    Err(MacRuntimePolicyError::AcknowledgementReceptionDisabled)
                }
            }
            MacActivePhase::ClearChannelAssessment | MacActivePhase::EnergyDetection { .. } => {
                Ok(())
            }
        }
    }

    fn require_state_specific_quiescence(&mut self) -> Result<(), Esp32s31Ieee802154CommandError> {
        match self.state {
            ExecutorState::Quiescent => {
                self.state = ExecutorState::Preparing {
                    policy_refreshed: false,
                    duration: None,
                };
                Ok(())
            }
            ExecutorState::Preparing { .. } => {
                Err(Esp32s31Ieee802154CommandError::PreparationAlreadyOpen)
            }
            ExecutorState::Active(command) => {
                Err(Esp32s31Ieee802154CommandError::OperationStillActive { command })
            }
        }
    }

    fn refresh_static_policy(&mut self) -> Result<(), Esp32s31Ieee802154CommandError> {
        let ExecutorState::Preparing { duration, .. } = self.state else {
            return Err(Esp32s31Ieee802154CommandError::PreparationNotOpen);
        };

        let expected = self.expected_policy;
        self.backend.set_frequency_code(expected.frequency_code());
        self.backend.set_cca_mode(expected.cca_mode());
        self.backend
            .set_cca_threshold_code(expected.cca_threshold_code());
        self.backend.set_mac_control(expected.control());
        self.backend.set_ack_timeout(expected.ack_timeout());
        self.backend.set_primary_pan_identity(expected.identity());
        self.backend.order_device_accesses();

        let observed = self.backend.mac_policy_snapshot();
        if observed != expected {
            return Err(
                Esp32s31Ieee802154CommandError::StaticPolicyReadbackMismatch { expected, observed },
            );
        }

        self.state = ExecutorState::Preparing {
            policy_refreshed: true,
            duration,
        };
        Ok(())
    }

    fn publish_transmit_address(
        &mut self,
        address: TxDmaAddress<'_>,
    ) -> Result<(), Esp32s31Ieee802154CommandError> {
        self.require_refreshed_preparation()?;
        self.backend.publish_transmit_address(address.as_u32());
        Ok(())
    }

    fn publish_receive_address(
        &mut self,
        address: RxDmaAddress<'_>,
    ) -> Result<(), Esp32s31Ieee802154CommandError> {
        self.require_refreshed_preparation()?;
        self.backend.publish_receive_address(address.as_u32());
        Ok(())
    }

    fn configure_energy_detection_duration(
        &mut self,
        units: u16,
    ) -> Result<(), Esp32s31Ieee802154CommandError> {
        self.require_refreshed_preparation()?;
        let duration = Ieee802154EdDurationUnits::new(u32::from(units))
            .ok_or(Esp32s31Ieee802154CommandError::EnergyDetectionDurationOutOfRange { units })?;
        self.backend.set_ed_duration(duration);
        self.backend.order_device_accesses();

        self.state = ExecutorState::Preparing {
            policy_refreshed: true,
            duration: Some(units),
        };
        Ok(())
    }

    fn request_command(
        &mut self,
        command: MacCommandIntent,
    ) -> Result<(), Esp32s31Ieee802154CommandError> {
        let duration = self.require_refreshed_preparation()?;
        let needs_duration = matches!(
            command,
            MacCommandIntent::ClearChannelAssessment
                | MacCommandIntent::EnergyDetection
                | MacCommandIntent::TransmitWithClearChannelAssessment
        );
        if needs_duration && duration.is_none() {
            return Err(Esp32s31Ieee802154CommandError::EnergyDetectionDurationMissing);
        }

        // The pre-command fence publishes policy and duration before the
        // typed open-LL command leaf. The post-command fence closes the finite
        // task-side transaction before the executor awaits its hard IRQ.
        self.backend.order_device_accesses();
        self.backend.request_command(command);
        self.backend.order_device_accesses();
        self.state = ExecutorState::Active(command);
        Ok(())
    }

    fn require_refreshed_preparation(&self) -> Result<Option<u16>, Esp32s31Ieee802154CommandError> {
        match self.state {
            ExecutorState::Preparing {
                policy_refreshed: true,
                duration,
            } => Ok(duration),
            ExecutorState::Preparing {
                policy_refreshed: false,
                ..
            } => Err(Esp32s31Ieee802154CommandError::PolicyNotRefreshed),
            ExecutorState::Quiescent | ExecutorState::Active(_) => {
                Err(Esp32s31Ieee802154CommandError::PreparationNotOpen)
            }
        }
    }

    fn complete_active_operation(&mut self) {
        match self.state {
            ExecutorState::Active(_) => {
                self.state = ExecutorState::Quiescent;
            }
            ExecutorState::Quiescent | ExecutorState::Preparing { .. } => {
                unreachable!("only an active command can accept a terminal IRQ")
            }
        }
    }
}

/// Concrete task-side executor owning the unique ESP32-S31 IEEE 802.15.4 PAC
/// task capability.
///
/// The hard-IRQ capability is disjoint and cannot be recovered through this
/// value. The executor keeps task ownership across every await and returns it
/// only from a quiescent [`MacRuntime`].
pub struct Esp32s31Ieee802154CommandExecutor {
    core: TaskCommandExecutorCore<PacTaskCommandBackend>,
}

impl Esp32s31Ieee802154CommandExecutor {
    const fn from_task_registers(
        task: Ieee802154TaskRegisters,
        expected_policy: Ieee802154MacPolicySnapshot,
    ) -> Self {
        Self {
            core: TaskCommandExecutorCore::new(PacTaskCommandBackend { task }, expected_policy),
        }
    }

    pub(crate) fn complete_active_operation(&mut self) {
        self.core.complete_active_operation()
    }

    fn into_task_registers(self) -> Ieee802154TaskRegisters {
        debug_assert_eq!(self.core.state, ExecutorState::Quiescent);
        self.core.backend.task
    }
}

impl sealed::CommandExecutor for Esp32s31Ieee802154CommandExecutor {}

impl MacCommandExecutor for Esp32s31Ieee802154CommandExecutor {
    type Error = Esp32s31Ieee802154CommandError;

    fn validate_operation_policy(
        &self,
        phase: MacActivePhase,
    ) -> Result<(), MacRuntimePolicyError> {
        self.core.validate_operation_policy(phase)
    }

    fn require_state_specific_quiescence(&mut self) -> Result<(), Self::Error> {
        self.core.require_state_specific_quiescence()
    }

    fn refresh_static_policy(&mut self) -> Result<(), Self::Error> {
        self.core.refresh_static_policy()
    }

    fn publish_transmit_address(&mut self, address: TxDmaAddress<'_>) -> Result<(), Self::Error> {
        self.core.publish_transmit_address(address)
    }

    fn publish_receive_address(&mut self, address: RxDmaAddress<'_>) -> Result<(), Self::Error> {
        self.core.publish_receive_address(address)
    }

    fn configure_energy_detection_duration(&mut self, units: u16) -> Result<(), Self::Error> {
        self.core.configure_energy_detection_duration(units)
    }

    fn request_command(&mut self, command: MacCommandIntent) -> Result<(), Self::Error> {
        self.core.request_command(command)
    }

    fn finish_terminal_operation(&mut self) {
        self.complete_active_operation();
    }
}

impl MacRuntime<Esp32s31Ieee802154CommandExecutor> {
    /// Bind one dedicated PAC task owner and its reviewed static-policy image
    /// to the production command runtime without touching MMIO.
    ///
    /// PHY, BTBB, coexistence, event masks, and the CPU interrupt route remain
    /// prerequisites of the higher-level ready transition. This constructor
    /// only transfers the already-exclusive task capability.
    pub const fn from_esp32s31_task(
        task: Ieee802154TaskRegisters,
        expected_policy: Ieee802154MacPolicySnapshot,
    ) -> Self {
        Self::from_commands(MacCommandCapability {
            executor: Esp32s31Ieee802154CommandExecutor::from_task_registers(task, expected_policy),
        })
    }

    /// Recover the unique task owner from an idle production runtime.
    pub fn into_esp32s31_task(self) -> Ieee802154TaskRegisters {
        self.hardware.executor.into_task_registers()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_ieee802154_dma::{
        DMA_LOW, DmaFrameAddress, PreparedTx, RxArm, RxPoolStorage, TxStorage,
    };
    use std::boxed::Box;
    use std::vec::Vec;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Frequency(Ieee802154FrequencyCode),
        CcaMode(Ieee802154CcaMode),
        CcaThreshold(i8),
        MacControl(Ieee802154MacControl),
        AckTimeout(Ieee802154AckTimeoutUnits),
        PanIdentity(Ieee802154PanIdentity),
        Fence,
        PolicySnapshot,
        TxAddress(u32),
        RxAddress(u32),
        EdDuration(u32),
        Command(MacCommandIntent),
    }

    struct FakeBackend {
        observed_policy: Ieee802154MacPolicySnapshot,
        operations: Vec<Operation>,
    }

    impl FakeBackend {
        fn new(observed_policy: Ieee802154MacPolicySnapshot) -> Self {
            Self {
                observed_policy,
                operations: Vec::new(),
            }
        }
    }

    impl TaskCommandBackend for FakeBackend {
        fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
            self.operations.push(Operation::Frequency(code));
        }

        fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
            self.operations.push(Operation::CcaMode(mode));
        }

        fn set_cca_threshold_code(&mut self, threshold: i8) {
            self.operations.push(Operation::CcaThreshold(threshold));
        }

        fn set_mac_control(&mut self, control: Ieee802154MacControl) {
            self.operations.push(Operation::MacControl(control));
        }

        fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeoutUnits) {
            self.operations.push(Operation::AckTimeout(timeout));
        }

        fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
            self.operations.push(Operation::PanIdentity(identity));
        }

        fn mac_policy_snapshot(&mut self) -> Ieee802154MacPolicySnapshot {
            self.operations.push(Operation::PolicySnapshot);
            self.observed_policy
        }

        fn publish_transmit_address(&mut self, address: u32) {
            self.operations.push(Operation::TxAddress(address));
        }

        fn publish_receive_address(&mut self, address: u32) {
            self.operations.push(Operation::RxAddress(address));
        }

        fn set_ed_duration(&mut self, duration: Ieee802154EdDurationUnits) {
            self.operations.push(Operation::EdDuration(duration.get()));
        }

        fn request_command(&mut self, command: MacCommandIntent) {
            self.operations.push(Operation::Command(command));
        }

        fn order_device_accesses(&mut self) {
            self.operations.push(Operation::Fence);
        }
    }

    fn policy() -> Ieee802154MacPolicySnapshot {
        Ieee802154MacPolicySnapshot::new(
            Ieee802154FrequencyCode::new(15),
            Ieee802154CcaMode::CarrierOrEnergyDetection,
            -72,
            Ieee802154AckTimeoutUnits::new(864),
            Ieee802154MacControl::new(true, true, false, false, false, true),
            0b0101,
            Ieee802154PanIdentity::new(
                0x1234,
                0x5678,
                [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe],
            ),
        )
    }

    fn refreshed_core() -> TaskCommandExecutorCore<FakeBackend> {
        let policy = policy();
        let mut core = TaskCommandExecutorCore::new(FakeBackend::new(policy), policy);
        core.require_state_specific_quiescence().unwrap();
        core.refresh_static_policy().unwrap();
        core
    }

    fn policy_with_control(control: Ieee802154MacControl) -> Ieee802154MacPolicySnapshot {
        let base = policy();
        Ieee802154MacPolicySnapshot::new(
            base.frequency_code(),
            base.cca_mode(),
            base.cca_threshold_code(),
            base.ack_timeout(),
            control,
            base.multipan_enable_mask(),
            base.identity(),
        )
    }

    #[test]
    fn automatic_ack_policy_must_match_the_operation_terminal_model() {
        let receive = MacActivePhase::Receive;
        for control in [
            Ieee802154MacControl::new(true, false, false, false, false, false),
            Ieee802154MacControl::new(false, false, true, false, false, false),
        ] {
            let policy = policy_with_control(control);
            let core = TaskCommandExecutorCore::new(FakeBackend::new(policy), policy);
            assert_eq!(
                core.validate_operation_policy(receive),
                Err(MacRuntimePolicyError::ReceiveWouldTransmitAcknowledgement {
                    tx_auto_ack: control.tx_auto_ack(),
                    enhanced_ack_tx: control.enhanced_ack_tx(),
                })
            );
        }

        let no_ack = MacActivePhase::Transmit {
            access: open_esp_radio_esp32s31_ieee802154_mac::MacTransmitAccess::Direct,
            acknowledgement: MacTransmitAcknowledgement::None,
        };
        let expects_ack = MacActivePhase::Transmit {
            access: open_esp_radio_esp32s31_ieee802154_mac::MacTransmitAccess::Direct,
            acknowledgement: MacTransmitAcknowledgement::Expected,
        };
        for (phase, rx_auto_ack, expected) in [
            (no_ack, false, Ok(())),
            (no_ack, true, Ok(())),
            (expects_ack, true, Ok(())),
            (
                expects_ack,
                false,
                Err(MacRuntimePolicyError::AcknowledgementReceptionDisabled),
            ),
        ] {
            let control = Ieee802154MacControl::new(false, rx_auto_ack, false, false, false, false);
            let policy = policy_with_control(control);
            let core = TaskCommandExecutorCore::new(FakeBackend::new(policy), policy);
            assert_eq!(core.validate_operation_policy(phase), expected);
        }
    }

    #[test]
    fn policy_refresh_uses_exact_order_fence_and_readback() {
        let expected = policy();
        let mut core = TaskCommandExecutorCore::new(FakeBackend::new(expected), expected);
        core.require_state_specific_quiescence().unwrap();
        core.refresh_static_policy().unwrap();

        assert_eq!(
            core.backend.operations,
            [
                Operation::Frequency(expected.frequency_code()),
                Operation::CcaMode(expected.cca_mode()),
                Operation::CcaThreshold(expected.cca_threshold_code()),
                Operation::MacControl(expected.control()),
                Operation::AckTimeout(expected.ack_timeout()),
                Operation::PanIdentity(expected.identity()),
                Operation::Fence,
                Operation::PolicySnapshot,
            ]
        );
    }

    #[test]
    fn policy_mismatch_fails_closed_before_any_command() {
        let expected = policy();
        let observed = Ieee802154MacPolicySnapshot::new(
            expected.frequency_code(),
            Ieee802154CcaMode::Carrier,
            expected.cca_threshold_code(),
            expected.ack_timeout(),
            expected.control(),
            expected.multipan_enable_mask(),
            expected.identity(),
        );
        let mut core = TaskCommandExecutorCore::new(FakeBackend::new(observed), expected);
        core.require_state_specific_quiescence().unwrap();

        assert_eq!(
            core.refresh_static_policy(),
            Err(
                Esp32s31Ieee802154CommandError::StaticPolicyReadbackMismatch { expected, observed }
            )
        );
        assert!(
            !core
                .backend
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::Command(_)))
        );
    }

    #[test]
    fn ed_and_cca_publish_bounded_duration_then_the_same_open_ll_start() {
        for command in [
            MacCommandIntent::EnergyDetection,
            MacCommandIntent::ClearChannelAssessment,
        ] {
            let mut core = refreshed_core();
            let prefix_len = core.backend.operations.len();
            core.configure_energy_detection_duration(u16::MAX).unwrap();
            core.request_command(command).unwrap();

            assert_eq!(
                &core.backend.operations[prefix_len..],
                [
                    Operation::EdDuration(u32::from(u16::MAX)),
                    Operation::Fence,
                    Operation::Fence,
                    Operation::Command(command),
                    Operation::Fence,
                ]
            );
            assert_eq!(core.state, ExecutorState::Active(command));
        }
    }

    #[test]
    fn supported_command_requires_policy_and_duration_in_the_same_epoch() {
        let expected = policy();
        let mut unrefreshed = TaskCommandExecutorCore::new(FakeBackend::new(expected), expected);
        unrefreshed.require_state_specific_quiescence().unwrap();
        assert_eq!(
            unrefreshed.configure_energy_detection_duration(8),
            Err(Esp32s31Ieee802154CommandError::PolicyNotRefreshed)
        );

        let mut missing_duration = refreshed_core();
        assert_eq!(
            missing_duration.request_command(MacCommandIntent::EnergyDetection),
            Err(Esp32s31Ieee802154CommandError::EnergyDetectionDurationMissing)
        );
        assert!(
            !missing_duration
                .backend
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::Command(_)))
        );
    }

    #[test]
    fn dma_addresses_and_all_open_ll_commands_are_published_in_order() {
        let mut core = refreshed_core();
        let prefix_len = core.backend.operations.len();
        let tx_storage = Box::leak(Box::new(TxStorage::new()));
        let mut tx =
            TxStorage::pin_static_model(tx_storage, DmaFrameAddress::try_new(DMA_LOW).unwrap());
        let PreparedTx::AckNotRequested(tx) = tx.prepare(&[0x01]).unwrap() else {
            panic!("fixture must not request an ACK");
        };
        let tx = tx.arm();
        let rx_storage = Box::leak(Box::new(RxPoolStorage::<1>::new()));
        let rx = RxPoolStorage::pin_static_model(
            rx_storage,
            DmaFrameAddress::try_new(DMA_LOW + 0x100).unwrap(),
        )
        .unwrap();
        let rx = rx.arm_next().unwrap();

        core.publish_transmit_address(tx.dma_address()).unwrap();
        let rx_address = match &rx {
            RxArm::Buffer(armed) => armed.dma_address(),
            RxArm::Stub(armed) => armed.dma_address(),
        };
        core.publish_receive_address(rx_address).unwrap();
        core.configure_energy_detection_duration(8).unwrap();
        core.request_command(MacCommandIntent::TransmitWithClearChannelAssessment)
            .unwrap();

        assert_eq!(
            &core.backend.operations[prefix_len..],
            [
                Operation::TxAddress(DMA_LOW),
                Operation::RxAddress(DMA_LOW + 0x100),
                Operation::EdDuration(8),
                Operation::Fence,
                Operation::Fence,
                Operation::Command(MacCommandIntent::TransmitWithClearChannelAssessment),
                Operation::Fence,
            ]
        );

        for command in [MacCommandIntent::Receive, MacCommandIntent::Transmit] {
            let mut core = refreshed_core();
            let prefix_len = core.backend.operations.len();
            core.request_command(command).unwrap();
            assert_eq!(
                &core.backend.operations[prefix_len..],
                [
                    Operation::Fence,
                    Operation::Command(command),
                    Operation::Fence
                ]
            );
        }
    }

    #[test]
    fn affine_state_blocks_reentry_until_exact_completion_handoff() {
        let mut core = refreshed_core();
        core.configure_energy_detection_duration(8).unwrap();
        core.request_command(MacCommandIntent::ClearChannelAssessment)
            .unwrap();
        assert_eq!(
            core.require_state_specific_quiescence(),
            Err(Esp32s31Ieee802154CommandError::OperationStillActive {
                command: MacCommandIntent::ClearChannelAssessment,
            })
        );

        core.complete_active_operation();
        assert_eq!(core.require_state_specific_quiescence(), Ok(()));
        assert_eq!(
            core.require_state_specific_quiescence(),
            Err(Esp32s31Ieee802154CommandError::PreparationAlreadyOpen)
        );
    }
}
