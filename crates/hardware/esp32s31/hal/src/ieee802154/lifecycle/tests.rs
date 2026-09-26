use std::vec::Vec;

use oer_esp32s31_pac::{Ieee802154FoundationSnapshot, Ieee802154Pti};

use super::{
    COEX_DISABLED_PTI, IEEE802154_MAX_CHANNEL, IEEE802154_MIN_CHANNEL, Ieee802154Channel,
    Ieee802154ChannelError, Ieee802154FoundationCheckpoint, Ieee802154Lifecycle,
    Ieee802154LifecycleBackend, Ieee802154ReadbackError, Ieee802154ResetCheckpoint,
    Ieee802154ResetPort, Ieee802154ResetReadback, state,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    MaskEvents,
    MaskRxAborts,
    MaskTxAborts,
    SetEdSampleAverage,
    SetTxrxPti(u8),
    SetAckPti(u8),
    RxOnDelay,
    DeviceFence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResetOperation {
    Mac(bool),
    Apb(bool),
}

#[derive(Debug)]
struct FakeBackend {
    operations: Vec<Operation>,
    foundation_snapshot: Ieee802154FoundationSnapshot,
}

fn valid_foundation() -> Ieee802154FoundationSnapshot {
    Ieee802154FoundationSnapshot::new(
        true,
        true,
        true,
        true,
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        true,
    )
}

impl FakeBackend {
    fn ready() -> Self {
        Self {
            operations: Vec::new(),
            foundation_snapshot: valid_foundation(),
        }
    }

    fn clocked(self) -> Ieee802154Lifecycle<Self, state::Clocked> {
        Ieee802154Lifecycle::clocked(self)
    }
}

struct FakeResetPort {
    operations: Vec<ResetOperation>,
    readback: Ieee802154ResetReadback,
}

impl FakeResetPort {
    fn released() -> Self {
        Self {
            operations: Vec::new(),
            readback: Ieee802154ResetReadback {
                mac_reset_released: true,
                apb_reset_released: true,
            },
        }
    }
}

impl Ieee802154ResetPort for FakeResetPort {
    fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
        self.operations.push(ResetOperation::Mac(asserted));
    }

    fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
        self.operations.push(ResetOperation::Apb(asserted));
    }

    fn ieee802154_reset_readback(&self) -> Ieee802154ResetReadback {
        self.readback
    }
}

impl Ieee802154LifecycleBackend for FakeBackend {
    fn mask_all_events(&mut self) {
        self.operations.push(Operation::MaskEvents);
    }

    fn mask_all_rx_aborts(&mut self) {
        self.operations.push(Operation::MaskRxAborts);
    }

    fn mask_all_tx_aborts(&mut self) {
        self.operations.push(Operation::MaskTxAborts);
    }

    fn select_average_ed_sampling(&mut self) {
        self.operations.push(Operation::SetEdSampleAverage);
    }

    fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.operations.push(Operation::SetTxrxPti(pti.value()));
    }

    fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.operations.push(Operation::SetAckPti(pti.value()));
    }

    fn apply_rx_on_delay(&mut self) {
        self.operations.push(Operation::RxOnDelay);
    }

    fn order_device_accesses(&mut self) {
        self.operations.push(Operation::DeviceFence);
    }

    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        self.foundation_snapshot
    }
}

const RESET_PULSES: [ResetOperation; 4] = [
    ResetOperation::Mac(true),
    ResetOperation::Mac(false),
    ResetOperation::Apb(true),
    ResetOperation::Apb(false),
];

#[test]
fn exact_sequence_reaches_only_foundation_configured() {
    let mut port = FakeResetPort::released();
    let reset = FakeBackend::ready()
        .clocked()
        .reset_mac(&mut port)
        .expect("reset readback");
    let configured = reset.configure_foundation().expect("foundation readback");
    let backend = configured.into_backend();

    assert_eq!(port.operations, RESET_PULSES);
    assert_eq!(
        backend.operations,
        [
            Operation::MaskEvents,
            Operation::MaskRxAborts,
            Operation::MaskTxAborts,
            Operation::SetEdSampleAverage,
            Operation::SetTxrxPti(COEX_DISABLED_PTI),
            Operation::SetAckPti(COEX_DISABLED_PTI),
            Operation::RxOnDelay,
            Operation::DeviceFence,
        ]
    );
}

#[test]
fn reset_failure_remains_clocked_and_touches_only_the_reset_port() {
    let mut port = FakeResetPort::released();
    port.readback.apb_reset_released = false;

    let failure = match FakeBackend::ready().clocked().reset_mac(&mut port) {
        Ok(_) => panic!("asserted APB reset must fail closed"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        Ieee802154ReadbackError {
            checkpoint: Ieee802154ResetCheckpoint::ApbResetReleased,
            expected: true,
            observed: false,
        }
    );
    assert_eq!(port.operations, RESET_PULSES);
    assert!(
        failure
            .into_lifecycle()
            .into_backend()
            .operations
            .is_empty()
    );
}

#[test]
fn foundation_failure_remains_reset_with_events_masked_first() {
    let mut backend = FakeBackend::ready();
    backend.foundation_snapshot = Ieee802154FoundationSnapshot::new(
        true,
        true,
        true,
        true,
        Ieee802154Pti::new(COEX_DISABLED_PTI + 1).expect("five-bit PTI"),
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        true,
    );
    let reset = backend
        .clocked()
        .reset_mac(&mut FakeResetPort::released())
        .expect("reset readback");

    let failure = match reset.configure_foundation() {
        Ok(_) => panic!("unproved coexistence disable must fail closed"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        Ieee802154ReadbackError {
            checkpoint: Ieee802154FoundationCheckpoint::TxrxPtiDisabled,
            expected: true,
            observed: false,
        }
    );
    let backend = failure.into_lifecycle().into_backend();
    assert_eq!(
        backend.operations[..3],
        [
            Operation::MaskEvents,
            Operation::MaskRxAborts,
            Operation::MaskTxAborts,
        ]
    );
}

#[test]
fn foundation_requires_the_receive_on_delay() {
    let mut backend = FakeBackend::ready();
    backend.foundation_snapshot = Ieee802154FoundationSnapshot::new(
        true,
        true,
        true,
        true,
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        false,
    );
    let reset = backend
        .clocked()
        .reset_mac(&mut FakeResetPort::released())
        .expect("reset readback");
    let failure = match reset.configure_foundation() {
        Ok(_) => panic!("a missing receive-on delay must fail closed"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error().checkpoint,
        Ieee802154FoundationCheckpoint::RxOnDelayApplied
    );
}

#[test]
fn a_returning_owner_resumes_the_foundation_without_writes() {
    let resumed =
        match Ieee802154Lifecycle::<_, state::FoundationConfigured>::resume(FakeBackend::ready()) {
            Ok(resumed) => resumed,
            Err(_) => panic!("an intact foundation resumes"),
        };
    assert!(resumed.into_backend().operations.is_empty());
}

#[test]
fn a_returning_owner_with_unmasked_events_falls_back_to_reset() {
    let mut backend = FakeBackend::ready();
    backend.foundation_snapshot = Ieee802154FoundationSnapshot::new(
        false,
        true,
        true,
        true,
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        true,
    );
    let failure = match Ieee802154Lifecycle::<_, state::FoundationConfigured>::resume(backend) {
        Ok(_) => panic!("unmasked events disprove the foundation"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error().checkpoint,
        Ieee802154FoundationCheckpoint::EventsMasked
    );
    let _reset: Ieee802154Lifecycle<FakeBackend, state::Reset> = failure.into_lifecycle();
}

#[test]
fn channel_constructor_is_exhaustive_and_fail_closed() {
    for candidate in u8::MIN..=u8::MAX {
        let result = Ieee802154Channel::new(candidate);
        if (IEEE802154_MIN_CHANNEL..=IEEE802154_MAX_CHANNEL).contains(&candidate) {
            assert_eq!(result.map(Ieee802154Channel::number), Ok(candidate));
        } else {
            assert_eq!(
                result,
                Err(Ieee802154ChannelError {
                    attempted: candidate,
                })
            );
        }
    }
}

#[test]
fn every_channel_maps_to_the_reviewed_vendor_frequency_code() {
    for number in IEEE802154_MIN_CHANNEL..=IEEE802154_MAX_CHANNEL {
        let channel = Ieee802154Channel::new(number).expect("2.4 GHz channel");
        assert_eq!(
            channel.frequency_code().value(),
            (number - IEEE802154_MIN_CHANNEL) * 5 + 3,
        );
    }

    assert_eq!(
        Ieee802154Channel::new(IEEE802154_MIN_CHANNEL)
            .expect("lower boundary")
            .frequency_code()
            .value(),
        3
    );
    assert_eq!(
        Ieee802154Channel::new(IEEE802154_MAX_CHANNEL)
            .expect("upper boundary")
            .frequency_code()
            .value(),
        78
    );
}

/// `ieee802154_freq_to_channel` inverts `ieee802154_channel_to_freq` and
/// rejects every other code.
#[test]
fn a_frequency_code_names_a_channel_only_on_the_five_code_grid() {
    for number in IEEE802154_MIN_CHANNEL..=IEEE802154_MAX_CHANNEL {
        let channel = Ieee802154Channel::new(number).unwrap();
        assert_eq!(
            Ieee802154Channel::from_frequency_code(channel.frequency_code().value()),
            Some(channel)
        );
    }
    for code in [0, 2, 4, 7, 83, 255] {
        assert_eq!(Ieee802154Channel::from_frequency_code(code), None);
    }
}
