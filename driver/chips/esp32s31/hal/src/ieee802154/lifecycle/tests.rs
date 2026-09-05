use std::vec::Vec;

use open_esp_radio_esp32s31_pac::{Ieee802154FoundationSnapshot, Ieee802154Pti};

use super::{
    COEX_DISABLED_PTI, IEEE802154_MAX_CHANNEL, IEEE802154_MIN_CHANNEL, Ieee802154Channel,
    Ieee802154ChannelError, Ieee802154ClockCheckpoint, Ieee802154ClockReadback,
    Ieee802154FoundationCheckpoint, Ieee802154LifecycleBackend, Ieee802154ReadbackError,
    Ieee802154ResetCheckpoint, Ieee802154ResetReadback, establish_ieee802154_clocks,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    ConfigureClockMaps,
    ConfigureModemSource,
    EnableCoex,
    EnableWifiBb80x1,
    EnableEtm,
    EnableBtApb,
    EnableCommonBaseband,
    EnableIeee802154MacClocks,
    SetMacReset(bool),
    SetApbReset(bool),
    MaskEvents,
    MaskRxAborts,
    MaskTxAborts,
    SetEdSampleAverage,
    SetTxrxPti(u8),
    SetAckPti(u8),
    DeviceFence,
}

#[derive(Debug)]
struct FakeBackend {
    operations: Vec<Operation>,
    clock_readback: Ieee802154ClockReadback,
    reset_readback: Ieee802154ResetReadback,
    foundation_snapshot: Ieee802154FoundationSnapshot,
}

impl FakeBackend {
    fn ready() -> Self {
        Self {
            operations: Vec::new(),
            clock_readback: Ieee802154ClockReadback {
                modem_clock_maps_configured: true,
                pll_160m_clock_enabled: true,
                modem_source_clock_configured: true,
                coexistence_clock_enabled: true,
                wifi_bb_80x1_clock_enabled: true,
                etm_clock_enabled: true,
                bt_apb_clock_enabled: true,
                modem_security_apb_clock_enabled: true,
                bt_ieee802154_common_baseband_clock_enabled: true,
                ieee802154_apb_clock_enabled: true,
                ieee802154_mac_clock_enabled: true,
            },
            reset_readback: Ieee802154ResetReadback {
                mac_reset_released: true,
                apb_reset_released: true,
            },
            foundation_snapshot: Ieee802154FoundationSnapshot::new(
                true,
                true,
                true,
                true,
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
            ),
        }
    }
}

impl Ieee802154LifecycleBackend for FakeBackend {
    fn configure_modem_source_clock(&mut self) {
        self.operations.push(Operation::ConfigureModemSource);
    }
    fn configure_modem_clock_maps(&mut self) {
        self.operations.push(Operation::ConfigureClockMaps);
    }

    fn enable_wifi_bb_80x1_clock(&mut self) {
        self.operations.push(Operation::EnableWifiBb80x1);
    }

    fn enable_etm_clock(&mut self) {
        self.operations.push(Operation::EnableEtm);
    }

    fn enable_bt_apb_clocks(&mut self) {
        self.operations.push(Operation::EnableBtApb);
    }

    fn enable_bt_ieee802154_common_baseband_clock(&mut self) {
        self.operations.push(Operation::EnableCommonBaseband);
    }

    fn enable_ieee802154_mac_clocks(&mut self) {
        self.operations.push(Operation::EnableIeee802154MacClocks);
    }

    fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
        self.operations.push(Operation::SetMacReset(asserted));
    }

    fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
        self.operations.push(Operation::SetApbReset(asserted));
    }

    fn ieee802154_reset_readback(&self) -> Ieee802154ResetReadback {
        self.reset_readback
    }

    fn enable_coexistence_clock(&mut self) {
        self.operations.push(Operation::EnableCoex);
    }

    fn ieee802154_clock_readback(&self) -> Ieee802154ClockReadback {
        self.clock_readback
    }

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

    fn order_device_accesses(&mut self) {
        self.operations.push(Operation::DeviceFence);
    }

    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        self.foundation_snapshot
    }
}

#[test]
fn exact_sequence_reaches_only_foundation_configured() {
    let clocked = establish_ieee802154_clocks(FakeBackend::ready()).expect("clock readback");
    let reset = clocked.reset_mac().expect("reset readback");
    let configured = reset.configure_foundation().expect("foundation readback");
    let backend = configured.into_backend();

    assert_eq!(
        backend.operations,
        [
            Operation::ConfigureClockMaps,
            Operation::ConfigureModemSource,
            Operation::EnableCoex,
            Operation::EnableWifiBb80x1,
            Operation::EnableEtm,
            Operation::EnableBtApb,
            Operation::EnableCommonBaseband,
            Operation::EnableIeee802154MacClocks,
            Operation::SetMacReset(true),
            Operation::SetMacReset(false),
            Operation::SetApbReset(true),
            Operation::SetApbReset(false),
            Operation::MaskEvents,
            Operation::MaskRxAborts,
            Operation::MaskTxAborts,
            Operation::SetEdSampleAverage,
            Operation::SetTxrxPti(COEX_DISABLED_PTI),
            Operation::SetAckPti(COEX_DISABLED_PTI),
            Operation::DeviceFence,
        ]
    );
}

#[test]
fn clock_readback_fails_at_the_first_unproved_dependency_and_returns_owner() {
    let mut backend = FakeBackend::ready();
    backend.clock_readback.etm_clock_enabled = false;
    backend.clock_readback.bt_apb_clock_enabled = false;

    let failure = match establish_ieee802154_clocks(backend) {
        Ok(_) => panic!("unproved clocks must fail closed"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        Ieee802154ReadbackError {
            checkpoint: Ieee802154ClockCheckpoint::EtmClock,
            expected: true,
            observed: false,
        }
    );
    assert_eq!(failure.into_backend().operations.len(), 8);
}

#[test]
fn clock_readback_requires_the_shared_upstream_pll_gate() {
    let mut backend = FakeBackend::ready();
    backend.clock_readback.pll_160m_clock_enabled = false;

    let failure = match establish_ieee802154_clocks(backend) {
        Ok(_) => panic!("an unavailable upstream PLL must fail closed"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        Ieee802154ReadbackError {
            checkpoint: Ieee802154ClockCheckpoint::Pll160mClock,
            expected: true,
            observed: false,
        }
    );
    assert_eq!(failure.into_backend().operations.len(), 8);
}

#[test]
fn reset_failure_remains_clocked_and_returns_owner() {
    let mut backend = FakeBackend::ready();
    backend.reset_readback.apb_reset_released = false;
    let clocked = establish_ieee802154_clocks(backend).expect("clock readback");

    let failure = match clocked.reset_mac() {
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
    let backend = failure.into_lifecycle().into_backend();
    assert_eq!(
        &backend.operations[8..],
        [
            Operation::SetMacReset(true),
            Operation::SetMacReset(false),
            Operation::SetApbReset(true),
            Operation::SetApbReset(false),
        ]
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
    );
    let clocked = establish_ieee802154_clocks(backend).expect("clock readback");
    let reset = clocked.reset_mac().expect("reset readback");

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
    assert_eq!(backend.operations[12], Operation::MaskEvents);
    assert_eq!(backend.operations[13], Operation::MaskRxAborts);
    assert_eq!(backend.operations[14], Operation::MaskTxAborts);
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
