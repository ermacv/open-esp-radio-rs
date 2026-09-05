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
    WatchdogEventEnabled,
    ClockSample(u32),
    WatchdogStarted(u32),
    WatchdogDisarmed,
}

struct FakeBackend {
    observed_policy: Ieee802154MacPolicySnapshot,
    operations: Vec<Operation>,
    clock_samples: [u32; 2],
    next_clock_sample: usize,
}

impl FakeBackend {
    fn new(observed_policy: Ieee802154MacPolicySnapshot) -> Self {
        Self {
            observed_policy,
            operations: Vec::new(),
            clock_samples: [0; 2],
            next_clock_sample: 0,
        }
    }

    fn with_clock_samples(mut self, samples: [u32; 2]) -> Self {
        self.clock_samples = samples;
        self
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

    fn enable_acknowledgement_watchdog_event(&mut self) {
        self.operations.push(Operation::WatchdogEventEnabled);
    }

    fn sample_monotonic_microseconds(&mut self) -> u32 {
        let sample = *self
            .clock_samples
            .get(self.next_clock_sample)
            .expect("the watchdog consumes exactly two clock samples");
        self.next_clock_sample += 1;
        self.operations.push(Operation::ClockSample(sample));
        sample
    }

    fn start_acknowledgement_watchdog(&mut self, threshold: u32) {
        self.operations.push(Operation::WatchdogStarted(threshold));
    }

    fn disarm_acknowledgement_watchdog(&mut self) {
        self.operations.push(Operation::WatchdogDisarmed);
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
        Ieee802154MultipanEnableState::new(true, false, true, false),
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
        base.multipan_enable_state(),
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
fn acknowledgement_watchdog_uses_two_wrapping_clock_samples_in_source_order() {
    let policy = policy();
    let started_at = u32::MAX - 99_999;
    let programmed_at = 50_000;
    let backend = FakeBackend::new(policy).with_clock_samples([started_at, programmed_at]);
    let mut core = TaskCommandExecutorCore::new(backend, policy);
    core.require_state_specific_quiescence().unwrap();
    core.refresh_static_policy().unwrap();
    core.request_command(MacCommandIntent::Transmit).unwrap();

    core.arm_acknowledgement_watchdog();
    assert_eq!(
        &core.backend.operations[core.backend.operations.len() - 4..],
        [
            Operation::WatchdogEventEnabled,
            Operation::ClockSample(started_at),
            Operation::ClockSample(programmed_at),
            Operation::WatchdogStarted(50_000),
        ]
    );
    assert_eq!(
        core.state,
        ExecutorState::Active {
            command: MacCommandIntent::Transmit,
            acknowledgement_watchdog_armed: true,
        }
    );

    core.disarm_acknowledgement_watchdog();
    assert_eq!(
        core.backend.operations.last(),
        Some(&Operation::WatchdogDisarmed)
    );
    core.complete_active_operation();
    assert_eq!(core.state, ExecutorState::Quiescent);
}

#[test]
fn acknowledgement_watchdog_threshold_fails_closed_across_half_range() {
    assert_eq!(ieee802154_ack_watchdog_threshold(0, 0), 200_000);
    assert_eq!(ieee802154_ack_watchdog_threshold(0, 199_999), 1);
    assert_eq!(ieee802154_ack_watchdog_threshold(0, 200_000), 0);
    assert_eq!(ieee802154_ack_watchdog_threshold(0, 200_001), 0);
    assert_eq!(
        ieee802154_ack_watchdog_threshold(0, 200_000_u32.wrapping_add(1_u32 << 31)),
        1_u32 << 31
    );
    assert_eq!(
        ieee802154_ack_watchdog_threshold(0, 200_000_u32.wrapping_add((1_u32 << 31) + 1),),
        i32::MAX as u32
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
        expected.multipan_enable_state(),
        expected.identity(),
    );
    let mut core = TaskCommandExecutorCore::new(FakeBackend::new(observed), expected);
    core.require_state_specific_quiescence().unwrap();

    assert_eq!(
        core.refresh_static_policy(),
        Err(Esp32s31Ieee802154CommandError::StaticPolicyReadbackMismatch { expected, observed })
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
        assert_eq!(
            core.state,
            ExecutorState::Active {
                command,
                acknowledgement_watchdog_armed: false,
            }
        );
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
