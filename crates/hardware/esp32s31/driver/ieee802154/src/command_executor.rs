//! ESP32-S31 HAL-backed task-side command execution.
//!
//! The executor owns the HAL task owner for the complete command epoch. The
//! only source of that owner is the HAL operational transition
//! (`Ieee802154MacPolicyConfigured::into_operational`), which also hands out
//! the inactive interrupt owner and retains the route until both return.
//!
//! The concrete leaves below are a direct typed port of the public ESP-IDF
//! common LL: policy and DMA publication precede exactly one of `TX_START`,
//! `RX_START`, `CCA_TX_START`, or `ED_START`. `STOP` is deliberately not used
//! as a synchronous idle proof; terminal IRQ reconciliation owns that edge.

use crate::{
    MacCommandCapability, MacCommandExecutor, MacOperation, MacOperationPolicyError, sealed,
};

use oer_esp32s31_ieee802154_dma::{RxDmaAddress, TxDmaAddress};

use oer_esp32s31_ieee802154_mac::{MacActivePhase, MacCommandIntent, MacTransmitAcknowledgement};

use oer_esp32s31_hal::ieee802154::{
    Ieee802154MacPolicy, Ieee802154MacPolicyCheckpoint, Ieee802154ReadbackError,
    mac::{Ieee802154Command, Ieee802154TaskOwner},
};

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
pub enum Ieee802154CommandError {
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
    /// Standalone ED or CCA was requested without a duration publication in
    /// the same preparation epoch.
    EnergyDetectionDurationMissing,
    /// The static policy read after the device fence did not match the
    /// policy retained by this executor.
    StaticPolicyReadback(Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint>),
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
    fn refresh_policy(
        &mut self,
        policy: Ieee802154MacPolicy,
    ) -> Result<(), Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint>>;
    fn publish_transmit_address(&mut self, address: u32);
    fn publish_receive_address(&mut self, address: u32);
    fn set_ed_duration(&mut self, units: u16);
    fn request_command(&mut self, command: Ieee802154Command);
    fn enable_acknowledgement_watchdog_event(&mut self);
    fn sample_monotonic_microseconds(&mut self) -> u32;
    fn start_acknowledgement_watchdog(&mut self, threshold: u32);
    fn disarm_acknowledgement_watchdog(&mut self);
    fn order_device_accesses(&mut self);
}

struct HalTaskCommandBackend {
    task: Ieee802154TaskOwner,
    clock: Ieee802154MonotonicMicrosecondClock,
}

impl TaskCommandBackend for HalTaskCommandBackend {
    fn refresh_policy(
        &mut self,
        policy: Ieee802154MacPolicy,
    ) -> Result<(), Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint>> {
        self.task.refresh_mac_policy(policy)
    }

    fn publish_transmit_address(&mut self, address: u32) {
        self.task.publish_transmit_dma_address(address);
    }

    fn publish_receive_address(&mut self, address: u32) {
        self.task.publish_receive_dma_address(address);
    }

    fn set_ed_duration(&mut self, units: u16) {
        self.task.set_ed_duration(units);
    }

    fn request_command(&mut self, command: Ieee802154Command) {
        self.task.request_command(command);
    }

    fn enable_acknowledgement_watchdog_event(&mut self) {
        self.task.enable_acknowledgement_watchdog_event();
    }

    fn sample_monotonic_microseconds(&mut self) -> u32 {
        self.clock.sample()
    }

    fn start_acknowledgement_watchdog(&mut self, threshold: u32) {
        self.task.start_acknowledgement_watchdog(threshold);
    }

    fn disarm_acknowledgement_watchdog(&mut self) {
        self.task.disarm_acknowledgement_watchdog();
    }

    fn order_device_accesses(&mut self) {
        self.task.order_device_accesses();
    }
}

/// The hardware command that starts one actor intent.
///
/// Standalone CCA is an energy-detection transaction whose sample the actor
/// interprets; the hardware has no separate CCA opcode.
const fn hardware_command(intent: MacCommandIntent) -> Ieee802154Command {
    match intent {
        MacCommandIntent::Receive => Ieee802154Command::Receive,
        MacCommandIntent::Transmit => Ieee802154Command::Transmit,
        MacCommandIntent::TransmitWithClearChannelAssessment => {
            Ieee802154Command::ClearChannelThenTransmit
        }
        MacCommandIntent::ClearChannelAssessment | MacCommandIntent::EnergyDetection => {
            Ieee802154Command::EnergyDetection
        }
    }
}

struct TaskCommandExecutorCore<Backend> {
    backend: Backend,
    expected_policy: Ieee802154MacPolicy,
    state: ExecutorState,
}

impl<Backend: TaskCommandBackend> TaskCommandExecutorCore<Backend> {
    const fn new(backend: Backend, expected_policy: Ieee802154MacPolicy) -> Self {
        Self {
            backend,
            expected_policy,
            state: ExecutorState::Quiescent,
        }
    }

    fn validate_operation_policy(
        &self,
        phase: MacActivePhase,
    ) -> Result<(), MacOperationPolicyError> {
        let control = self.expected_policy.control();
        match phase {
            MacActivePhase::Receive => {
                if control.tx_auto_ack() || control.enhanced_ack_tx() {
                    Err(
                        MacOperationPolicyError::ReceiveWouldTransmitAcknowledgement {
                            tx_auto_ack: control.tx_auto_ack(),
                            enhanced_ack_tx: control.enhanced_ack_tx(),
                        },
                    )
                } else {
                    Ok(())
                }
            }
            MacActivePhase::Transmit {
                acknowledgement, ..
            } => {
                if acknowledgement == MacTransmitAcknowledgement::Expected && !control.rx_auto_ack()
                {
                    Err(MacOperationPolicyError::AcknowledgementReceptionDisabled)
                } else {
                    Ok(())
                }
            }
            MacActivePhase::AwaitingAcknowledgement { .. } => {
                if control.rx_auto_ack() {
                    Ok(())
                } else {
                    Err(MacOperationPolicyError::AcknowledgementReceptionDisabled)
                }
            }
            MacActivePhase::ClearChannelAssessment | MacActivePhase::EnergyDetection { .. } => {
                Ok(())
            }
        }
    }

    fn require_state_specific_quiescence(&mut self) -> Result<(), Ieee802154CommandError> {
        match self.state {
            ExecutorState::Quiescent => {
                self.state = ExecutorState::Preparing {
                    policy_refreshed: false,
                    duration: None,
                };
                Ok(())
            }
            ExecutorState::Preparing { .. } => Err(Ieee802154CommandError::PreparationAlreadyOpen),
            ExecutorState::Active { command, .. } => {
                Err(Ieee802154CommandError::OperationStillActive { command })
            }
        }
    }

    fn refresh_static_policy(&mut self) -> Result<(), Ieee802154CommandError> {
        let ExecutorState::Preparing { duration, .. } = self.state else {
            return Err(Ieee802154CommandError::PreparationNotOpen);
        };

        self.backend
            .refresh_policy(self.expected_policy)
            .map_err(Ieee802154CommandError::StaticPolicyReadback)?;

        self.state = ExecutorState::Preparing {
            policy_refreshed: true,
            duration,
        };
        Ok(())
    }

    fn publish_transmit_address(
        &mut self,
        address: TxDmaAddress<'_>,
    ) -> Result<(), Ieee802154CommandError> {
        self.require_refreshed_preparation()?;
        self.backend.publish_transmit_address(address.as_u32());
        Ok(())
    }

    fn publish_receive_address(
        &mut self,
        address: RxDmaAddress<'_>,
    ) -> Result<(), Ieee802154CommandError> {
        self.require_refreshed_preparation()?;
        self.backend.publish_receive_address(address.as_u32());
        Ok(())
    }

    fn configure_energy_detection_duration(
        &mut self,
        units: u16,
    ) -> Result<(), Ieee802154CommandError> {
        self.require_refreshed_preparation()?;
        self.backend.set_ed_duration(units);
        self.backend.order_device_accesses();

        self.state = ExecutorState::Preparing {
            policy_refreshed: true,
            duration: Some(units),
        };
        Ok(())
    }

    fn request_command(&mut self, command: MacCommandIntent) -> Result<(), Ieee802154CommandError> {
        let duration = self.require_refreshed_preparation()?;
        let needs_duration = matches!(
            command,
            MacCommandIntent::ClearChannelAssessment
                | MacCommandIntent::EnergyDetection
                | MacCommandIntent::TransmitWithClearChannelAssessment
        );
        if needs_duration && duration.is_none() {
            return Err(Ieee802154CommandError::EnergyDetectionDurationMissing);
        }

        // The pre-command fence publishes policy and duration before the
        // typed open-LL command leaf. The post-command fence closes the finite
        // task-side transaction before the executor awaits its hard IRQ.
        self.backend.order_device_accesses();
        self.backend.request_command(hardware_command(command));
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

    fn require_refreshed_preparation(&self) -> Result<Option<u16>, Ieee802154CommandError> {
        match self.state {
            ExecutorState::Preparing {
                policy_refreshed: true,
                duration,
            } => Ok(duration),
            ExecutorState::Preparing {
                policy_refreshed: false,
                ..
            } => Err(Ieee802154CommandError::PolicyNotRefreshed),
            ExecutorState::Quiescent | ExecutorState::Active { .. } => {
                Err(Ieee802154CommandError::PreparationNotOpen)
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

/// Concrete task-side executor owning the unique ESP32-S31 IEEE 802.15.4 HAL
/// task capability.
///
/// The hard-IRQ capability is disjoint and cannot be recovered through this
/// value. The executor keeps task ownership across every await and returns it
/// only from a quiescent [`MacOperation`].
pub struct Ieee802154CommandExecutor {
    core: TaskCommandExecutorCore<HalTaskCommandBackend>,
}

impl Ieee802154CommandExecutor {
    const fn from_task_registers(
        task: Ieee802154TaskOwner,
        expected_policy: Ieee802154MacPolicy,
        clock: Ieee802154MonotonicMicrosecondClock,
    ) -> Self {
        Self {
            core: TaskCommandExecutorCore::new(
                HalTaskCommandBackend { task, clock },
                expected_policy,
            ),
        }
    }

    pub(crate) fn complete_active_operation(&mut self) {
        self.core.complete_active_operation()
    }

    fn into_task_registers(self) -> Ieee802154TaskOwner {
        debug_assert_eq!(self.core.state, ExecutorState::Quiescent);
        self.core.backend.task
    }
}

impl sealed::CommandExecutor for Ieee802154CommandExecutor {}

impl MacCommandExecutor for Ieee802154CommandExecutor {
    type Error = Ieee802154CommandError;

    fn validate_operation_policy(
        &self,
        phase: MacActivePhase,
    ) -> Result<(), MacOperationPolicyError> {
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

impl MacOperation<Ieee802154CommandExecutor> {
    /// Bind the operational HAL task owner and the static policy it
    /// republishes before every command, without touching MMIO.
    ///
    /// Pass the policy of the same operational route
    /// (`Ieee802154OperationalRoute::policy`). PHY/RF readiness and the CPU
    /// interrupt route remain prerequisites of the higher-level ready
    /// transition; this constructor only transfers the task capability.
    pub const fn from_esp32s31_task(
        task: Ieee802154TaskOwner,
        expected_policy: Ieee802154MacPolicy,
        clock: Ieee802154MonotonicMicrosecondClock,
    ) -> Self {
        Self::from_commands(MacCommandCapability {
            executor: Ieee802154CommandExecutor::from_task_registers(task, expected_policy, clock),
        })
    }

    /// Recover the unique task owner from an idle production command owner so
    /// it can return to its operational route.
    pub fn into_esp32s31_task(self) -> Ieee802154TaskOwner {
        self.hardware.executor.into_task_registers()
    }
}

#[cfg(test)]
mod tests;
