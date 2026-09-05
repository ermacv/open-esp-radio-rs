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
#[cfg(test)]
use open_esp_radio_esp32s31_pac::Ieee802154MultipanEnableState;
use open_esp_radio_esp32s31_pac::{
    Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154EdDurationUnits,
    Ieee802154FrequencyCode, Ieee802154MacCommand, Ieee802154MacControl,
    Ieee802154MacPolicySnapshot, Ieee802154PanIdentity, Ieee802154TaskRegisters,
    Ieee802154Timer0ThresholdWord,
};

use crate::{MacCommandCapability, MacCommandExecutor, MacRuntime, MacRuntimePolicyError, sealed};

/// Vendor watchdog interval started when an ACK-requesting transmit reaches
/// `TX_DONE` and enters automatic acknowledgement reception.
pub const IEEE802154_ACK_WATCHDOG_MICROSECONDS: u32 = 200_000;

/// Derive the TIMER0 threshold from the two truncated monotonic-clock samples
/// used by the public vendor driver.
///
/// A deadline behind `programmed_at` by less than half the wrapping range
/// becomes zero; otherwise the exact remaining microseconds are retained. The
/// half-range boundary itself remains live, matching the source's sign-bit
/// test exactly. This reproduces its `fire_time - current_time` rule without
/// assigning meaning to a TIMER0 counter readback.
pub const fn ieee802154_ack_watchdog_threshold(started_at: u32, programmed_at: u32) -> u32 {
    let deadline = started_at.wrapping_add(IEEE802154_ACK_WATCHDOG_MICROSECONDS);
    let remaining = deadline.wrapping_sub(programmed_at);
    if remaining > 1_u32 << 31 {
        0
    } else {
        remaining
    }
}

/// Platform-provided monotonic microsecond sampler used by the ACK watchdog.
///
/// The returned value is deliberately truncated to the vendor driver's
/// wrapping 32-bit time domain. Supplying the actual platform clock remains a
/// whole-radio integration obligation.
#[derive(Clone, Copy)]
pub struct Ieee802154MonotonicMicrosecondClock {
    sample: fn() -> u32,
}

impl Ieee802154MonotonicMicrosecondClock {
    /// Bind one no-allocation monotonic sampler.
    pub const fn new(sample: fn() -> u32) -> Self {
        Self { sample }
    }

    fn sample(&self) -> u32 {
        (self.sample)()
    }
}

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
    Active {
        command: MacCommandIntent,
        acknowledgement_watchdog_armed: bool,
    },
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
    fn enable_acknowledgement_watchdog_event(&mut self);
    fn sample_monotonic_microseconds(&mut self) -> u32;
    fn start_acknowledgement_watchdog(&mut self, threshold: u32);
    fn disarm_acknowledgement_watchdog(&mut self);
    fn order_device_accesses(&mut self);
}

struct PacTaskCommandBackend {
    task: Ieee802154TaskRegisters,
    clock: Ieee802154MonotonicMicrosecondClock,
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

    fn enable_acknowledgement_watchdog_event(&mut self) {
        self.task
            .ieee802154_register_lease()
            .timer_lease()
            .enable_acknowledgement_watchdog_event();
    }

    fn sample_monotonic_microseconds(&mut self) -> u32 {
        self.clock.sample()
    }

    fn start_acknowledgement_watchdog(&mut self, threshold: u32) {
        self.task
            .ieee802154_register_lease()
            .timer_lease()
            .start_acknowledgement_watchdog(Ieee802154Timer0ThresholdWord::new(threshold));
    }

    fn disarm_acknowledgement_watchdog(&mut self) {
        self.task
            .ieee802154_register_lease()
            .timer_lease()
            .disarm_acknowledgement_watchdog();
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
            ExecutorState::Active { command, .. } => {
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
        self.state = ExecutorState::Active {
            command,
            acknowledgement_watchdog_armed: false,
        };
        Ok(())
    }

    fn arm_acknowledgement_watchdog(&mut self) {
        let ExecutorState::Active {
            command,
            acknowledgement_watchdog_armed: false,
        } = self.state
        else {
            unreachable!("only one active ACK-requesting transmit can arm TIMER0")
        };
        assert!(
            matches!(
                command,
                MacCommandIntent::Transmit | MacCommandIntent::TransmitWithClearChannelAssessment
            ),
            "only a transmit command can enter acknowledgement reception"
        );

        self.backend.enable_acknowledgement_watchdog_event();
        let started_at = self.backend.sample_monotonic_microseconds();
        let programmed_at = self.backend.sample_monotonic_microseconds();
        self.backend
            .start_acknowledgement_watchdog(ieee802154_ack_watchdog_threshold(
                started_at,
                programmed_at,
            ));
        self.state = ExecutorState::Active {
            command,
            acknowledgement_watchdog_armed: true,
        };
    }

    fn disarm_acknowledgement_watchdog(&mut self) {
        let ExecutorState::Active {
            command,
            acknowledgement_watchdog_armed: true,
        } = self.state
        else {
            unreachable!("only an armed acknowledgement wait can disarm TIMER0")
        };
        self.backend.disarm_acknowledgement_watchdog();
        self.state = ExecutorState::Active {
            command,
            acknowledgement_watchdog_armed: false,
        };
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
            ExecutorState::Quiescent | ExecutorState::Active { .. } => {
                Err(Esp32s31Ieee802154CommandError::PreparationNotOpen)
            }
        }
    }

    fn complete_active_operation(&mut self) {
        match self.state {
            ExecutorState::Active {
                acknowledgement_watchdog_armed: false,
                ..
            } => {
                self.state = ExecutorState::Quiescent;
            }
            ExecutorState::Active {
                acknowledgement_watchdog_armed: true,
                ..
            } => unreachable!("terminal completion must disarm TIMER0 first"),
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
        clock: Ieee802154MonotonicMicrosecondClock,
    ) -> Self {
        Self {
            core: TaskCommandExecutorCore::new(
                PacTaskCommandBackend { task, clock },
                expected_policy,
            ),
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

    fn arm_acknowledgement_watchdog(&mut self) {
        self.core.arm_acknowledgement_watchdog();
    }

    fn disarm_acknowledgement_watchdog(&mut self) {
        self.core.disarm_acknowledgement_watchdog();
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
        clock: Ieee802154MonotonicMicrosecondClock,
    ) -> Self {
        Self::from_commands(MacCommandCapability {
            executor: Esp32s31Ieee802154CommandExecutor::from_task_registers(
                task,
                expected_policy,
                clock,
            ),
        })
    }

    /// Recover the unique task owner from an idle production runtime.
    pub fn into_esp32s31_task(self) -> Ieee802154TaskRegisters {
        self.hardware.executor.into_task_registers()
    }
}

#[cfg(test)]
mod tests;
