use super::*;

const CLIENTS: [PhyModemClient; 3] = [
    PhyModemClient::Wifi,
    PhyModemClient::Bluetooth,
    PhyModemClient::Ieee802154,
];

struct FixedClock(u64);

impl PhyPllTrackClock for FixedClock {
    fn now_micros(&mut self) -> u64 {
        self.0
    }
}

struct ScriptedClock<const COUNT: usize> {
    samples: [u64; COUNT],
    next: usize,
}

impl<const COUNT: usize> ScriptedClock<COUNT> {
    const fn new(samples: [u64; COUNT]) -> Self {
        Self { samples, next: 0 }
    }

    const fn samples_consumed(&self) -> usize {
        self.next
    }
}

impl<const COUNT: usize> PhyPllTrackClock for ScriptedClock<COUNT> {
    fn now_micros(&mut self) -> u64 {
        let sample = self.samples[self.next];
        self.next += 1;
        sample
    }
}

trait PhyClientStateTestExt: Sized {
    fn acquire_at(
        self,
        client: PhyModemClient,
        now_micros: u64,
    ) -> Result<PhyClientAcquireOutcome, PhyClientAcquireFailure>;

    fn evaluate_immediate_at(
        self,
        now_micros: u64,
    ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure>;

    fn evaluate_periodic_at(
        self,
        now_micros: u64,
    ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure>;
}

impl PhyClientStateTestExt for PhyClientState {
    fn acquire_at(
        self,
        client: PhyModemClient,
        now_micros: u64,
    ) -> Result<PhyClientAcquireOutcome, PhyClientAcquireFailure> {
        self.acquire(client, &mut FixedClock(now_micros))
    }

    fn evaluate_immediate_at(
        self,
        now_micros: u64,
    ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure> {
        self.evaluate_immediate_tracking(&mut FixedClock(now_micros))
    }

    fn evaluate_periodic_at(
        self,
        now_micros: u64,
    ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure> {
        self.evaluate_periodic_tracking(&mut FixedClock(now_micros))
    }
}

fn complete_acquire_for_test(outcome: PhyClientAcquireOutcome) -> PhyClientState {
    match outcome.into_owner() {
        Ok(owner) => owner,
        Err(pending) => pending.complete_for_test(),
    }
}

fn state_for_mask(mask: u8, now_micros: u64) -> PhyClientState {
    assert!(mask <= VALID_CLIENT_BITS);
    let mut state = PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS);
    for client in CLIENTS {
        if mask & client.bit() != 0 {
            state = complete_acquire_for_test(state.acquire_at(client, now_micros).unwrap());
        }
    }
    state
}

fn observed_mask(snapshot: PhyClientSnapshot) -> u8 {
    CLIENTS.into_iter().fold(0, |mask, client| {
        mask | if snapshot.contains(client) {
            client.bit()
        } else {
            0
        }
    })
}

#[test]
fn exhaustive_acquire_preserves_unrelated_bits_and_reports_first_user() {
    for mask in 0..=VALID_CLIENT_BITS {
        for client in CLIENTS {
            let state = state_for_mask(mask, 0);
            let before = state.snapshot();
            if mask & client.bit() != 0 {
                let failure = state.acquire_at(client, 0).unwrap_err();
                assert_eq!(
                    failure.error(),
                    PhyClientAcquireError::AlreadyAcquired(client)
                );
                assert_eq!(failure.owner().snapshot(), before);
                continue;
            }

            let outcome = state.acquire_at(client, 0).unwrap();
            assert_eq!(outcome.was_empty(), mask == 0);
            assert_eq!(
                outcome.ordering(),
                if mask == 0 {
                    PhyClientAcquireOrdering::ArmThenSetThenEvaluate
                } else {
                    PhyClientAcquireOrdering::SetThenEvaluate
                }
            );
            assert_eq!(
                observed_mask(outcome.owner().snapshot()),
                mask | client.bit()
            );
            assert!(outcome.owner().snapshot().tracker_model_armed());
        }
    }
}

#[test]
fn exhaustive_release_preserves_unrelated_bits_and_reports_last_user() {
    for mask in 0..=VALID_CLIENT_BITS {
        for client in CLIENTS {
            let state = state_for_mask(mask, 0);
            let before = state.snapshot();
            if mask & client.bit() == 0 {
                let failure = state.release(client).unwrap_err();
                assert_eq!(failure.error(), PhyClientReleaseError::NotAcquired(client));
                assert_eq!(failure.owner().snapshot(), before);
                continue;
            }

            let outcome = state.release(client).unwrap();
            assert_eq!(outcome.is_last(), mask == client.bit());
            assert_eq!(
                observed_mask(outcome.owner().snapshot()),
                mask & !client.bit()
            );
            assert_eq!(
                outcome.owner().snapshot().tracker_model_armed(),
                mask & !client.bit() != 0
            );
        }
    }
}

#[test]
fn strict_threshold_equality_does_not_request_but_greater_does() {
    let state = complete_acquire_for_test(
        PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .acquire_at(PhyModemClient::Ieee802154, DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .unwrap(),
    );
    let equal = state
        .evaluate_immediate_at(DEFAULT_PLL_TRACK_PERIOD_MICROS)
        .unwrap();
    assert!(equal.request().is_none());

    let greater = equal
        .into_owner()
        .unwrap()
        .evaluate_immediate_at(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
        .unwrap();
    let request = greater.request().unwrap();
    assert!(!request.wifi());
    assert!(request.bluetooth_ieee802154());
}

#[test]
fn bluetooth_and_ieee_share_timestamp_and_request_class() {
    let state = complete_acquire_for_test(
        PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .acquire_at(
                PhyModemClient::Bluetooth,
                DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
            )
            .unwrap(),
    );
    assert_eq!(
        state
            .snapshot()
            .previous_micros(PhyPllTrackClass::BluetoothIeee802154),
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 1
    );

    let outcome = state
        .acquire_at(
            PhyModemClient::Ieee802154,
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 2,
        )
        .unwrap();
    assert!(outcome.request().is_none());
    assert_eq!(
        outcome
            .owner()
            .snapshot()
            .previous_micros(PhyPllTrackClass::BluetoothIeee802154),
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 1
    );
}

#[test]
fn request_booleans_describe_every_active_class() {
    for mask in 1..=VALID_CLIENT_BITS {
        let evaluation = state_for_mask(mask, 0)
            .evaluate_immediate_at(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
            .unwrap();
        let request = evaluation.request().unwrap();
        assert_eq!(request.wifi(), mask & WIFI_BIT != 0);
        assert_eq!(
            request.bluetooth_ieee802154(),
            mask & (BLUETOOTH_BIT | IEEE802154_BIT) != 0
        );
    }
}

#[test]
fn one_due_class_refreshes_and_requests_all_active_classes() {
    let state = complete_acquire_for_test(
        PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .acquire_at(PhyModemClient::Wifi, DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
            .unwrap(),
    );
    let state = state.release(PhyModemClient::Wifi).unwrap().into_owner();
    let state = complete_acquire_for_test(
        state
            .acquire_at(
                PhyModemClient::Ieee802154,
                DEFAULT_PLL_TRACK_PERIOD_MICROS + 2,
            )
            .unwrap(),
    );
    let state = complete_acquire_for_test(
        state
            .acquire_at(PhyModemClient::Wifi, DEFAULT_PLL_TRACK_PERIOD_MICROS + 2)
            .unwrap(),
    );
    let evaluation = state
        .evaluate_immediate_at(2 * DEFAULT_PLL_TRACK_PERIOD_MICROS + 2)
        .unwrap();
    let request = evaluation.request().unwrap();
    assert!(request.wifi());
    assert!(request.bluetooth_ieee802154());
    let snapshot = evaluation.owner().snapshot();
    assert_eq!(
        snapshot.previous_micros(PhyPllTrackClass::Wifi),
        2 * DEFAULT_PLL_TRACK_PERIOD_MICROS + 2
    );
    assert_eq!(
        snapshot.previous_micros(PhyPllTrackClass::BluetoothIeee802154),
        2 * DEFAULT_PLL_TRACK_PERIOD_MICROS + 2
    );
}

#[test]
fn immediate_tracking_preserves_short_circuit_and_refresh_sample_order() {
    let state = state_for_mask(WIFI_BIT | IEEE802154_BIT, 0);
    let mut wifi_due = ScriptedClock::new([
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 2,
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 3,
    ]);
    let evaluation = state.evaluate_immediate_tracking(&mut wifi_due).unwrap();

    assert_eq!(wifi_due.samples_consumed(), 3);
    let request = evaluation.request().unwrap();
    assert!(request.wifi());
    assert!(request.bluetooth_ieee802154());
    let snapshot = evaluation.owner().snapshot();
    assert_eq!(
        snapshot.previous_micros(PhyPllTrackClass::Wifi),
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 2
    );
    assert_eq!(
        snapshot.previous_micros(PhyPllTrackClass::BluetoothIeee802154),
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 3
    );

    let state = state_for_mask(WIFI_BIT | IEEE802154_BIT, 0);
    let mut bluetooth_ieee_due = ScriptedClock::new([
        DEFAULT_PLL_TRACK_PERIOD_MICROS,
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 2,
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 3,
    ]);
    let evaluation = state
        .evaluate_immediate_tracking(&mut bluetooth_ieee_due)
        .unwrap();

    assert_eq!(bluetooth_ieee_due.samples_consumed(), 4);
    let snapshot = evaluation.owner().snapshot();
    assert_eq!(
        snapshot.previous_micros(PhyPllTrackClass::Wifi),
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 2
    );
    assert_eq!(
        snapshot.previous_micros(PhyPllTrackClass::BluetoothIeee802154),
        DEFAULT_PLL_TRACK_PERIOD_MICROS + 3
    );
}

#[test]
fn periodic_callback_requests_active_classes_without_due_check() {
    let state = state_for_mask(IEEE802154_BIT, 0);
    let evaluation = state.evaluate_periodic_at(1).unwrap();
    let request = evaluation.request().unwrap();
    assert!(!request.wifi());
    assert!(request.bluetooth_ieee802154());
    assert_eq!(
        evaluation
            .owner()
            .snapshot()
            .previous_micros(PhyPllTrackClass::BluetoothIeee802154),
        1
    );
}

#[test]
fn periodic_tracking_samples_each_active_class_once_without_due_samples() {
    let state = state_for_mask(WIFI_BIT | IEEE802154_BIT, 0);
    let mut clock = ScriptedClock::new([17, 23]);
    let evaluation = state.evaluate_periodic_tracking(&mut clock).unwrap();

    assert_eq!(clock.samples_consumed(), 2);
    let snapshot = evaluation.owner().snapshot();
    assert_eq!(snapshot.previous_micros(PhyPllTrackClass::Wifi), 17);
    assert_eq!(
        snapshot.previous_micros(PhyPllTrackClass::BluetoothIeee802154),
        23
    );
}

#[test]
fn pending_request_cannot_release_owner_without_explicit_resolution() {
    let outcome = PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
        .acquire_at(
            PhyModemClient::Ieee802154,
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
        )
        .unwrap();
    let pending = match outcome.into_owner() {
        Ok(_) => panic!("due hardware work released the owner"),
        Err(pending) => pending,
    };
    assert!(pending.request().bluetooth_ieee802154());

    let before = pending.snapshot();
    let poisoned = pending.fail();
    assert_eq!(poisoned.snapshot(), before);
    assert!(poisoned.request().bluetooth_ieee802154());
}

#[test]
fn pending_periodic_request_cannot_release_owner() {
    let evaluation = state_for_mask(IEEE802154_BIT, 0)
        .evaluate_periodic_at(1)
        .unwrap();
    let pending = match evaluation.into_owner() {
        Ok(_) => panic!("periodic hardware work released the owner"),
        Err(pending) => pending,
    };
    assert_eq!(
        observed_mask(pending.snapshot()),
        IEEE802154_BIT,
        "the pending request must retain the exact client state"
    );
}

#[test]
fn pending_request_runs_outer_tracking_before_owner_recovery() {
    let pending = match state_for_mask(IEEE802154_BIT, 0)
        .evaluate_periodic_at(2_000_000)
        .unwrap()
        .into_owner()
    {
        Ok(_) => panic!("periodic IEEE tracking must retain the owner"),
        Err(pending) => pending,
    };
    let mut tracking = pending.begin_tracking(PhyParamTrackingPolicy {
        tracking_inhibited: true,
        rfpll_cap_tracking_enabled: true,
        rfpll_cap_tracking_threshold: None,
        calibration_tracking_threshold: None,
        diagnostics: crate::tracking::parameters::PhyTrackingDiagnostics::Enabled,
        bluetooth_ieee802154_power_tracking_enabled: true,
        calibration_tracking_enabled: true,
        relaxed_power_tracking_threshold: false,
    });

    assert_eq!(tracking.action(), PhyParamTrackingAction::EnterCritical);
    assert_eq!(
        tracking.advance(PhyParamTrackingCompletion::TemperatureRead),
        Err(PhyParamTrackingTransitionError::WrongCompletion)
    );
    assert_eq!(tracking.action(), PhyParamTrackingAction::EnterCritical);
    tracking
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    assert_eq!(tracking.action(), PhyParamTrackingAction::ExitCritical);
    let mut tracking = match tracking.into_owner() {
        Ok(_) => panic!("owner escaped before the terminal action"),
        Err(tracking) => tracking,
    };
    tracking
        .advance(PhyParamTrackingCompletion::ExitedCritical)
        .unwrap();
    let owner = tracking.into_owner().unwrap();
    assert!(owner.snapshot().contains(PhyModemClient::Ieee802154));
}

#[test]
fn periodic_outer_tracking_commits_power_i2c_and_temperature_children() {
    let pending = match state_for_mask(WIFI_BIT | IEEE802154_BIT, 0)
        .evaluate_periodic_at(2_000_000)
        .unwrap()
        .into_owner()
    {
        Ok(_) => panic!("periodic shared tracking must retain the owner"),
        Err(pending) => pending,
    };
    let policy = PhyParamTrackingPolicy {
        tracking_inhibited: false,
        rfpll_cap_tracking_enabled: false,
        rfpll_cap_tracking_threshold: None,
        calibration_tracking_threshold: None,
        diagnostics: crate::tracking::parameters::PhyTrackingDiagnostics::Enabled,
        bluetooth_ieee802154_power_tracking_enabled: true,
        calibration_tracking_enabled: false,
        relaxed_power_tracking_threshold: false,
    };
    let mut tracking = pending.begin_tracking(policy);
    let mut state = crate::state::PhyState::new(crate::state::PhyConfig::production());
    state.apply_temperature_outcome(crate::analog::temperature::PhyTemperatureOutcome {
        temperature: 95,
        sensor_index: 3,
        next_dac: 4,
    });
    state.apply_channel_outcome(crate::channel::PhyChipChannelOutcome {
        channel: 11,
        frequency_mhz: 2_462,
        cbw: 1,
        init_complete: true,
        temperature: crate::analog::temperature::PhyTemperatureOutcome {
            temperature: 95,
            sensor_index: 3,
            next_dac: 4,
        },
    });

    assert_eq!(
        tracking.begin_tx_power_tracking(&mut state).unwrap_err(),
        PhyParamTrackingChildError::UnsupportedAction
    );
    assert_eq!(
        tracking.begin_rfpll_cap_tracking(&mut state).unwrap_err(),
        PhyParamTrackingChildError::UnsupportedAction
    );
    assert_eq!(
        tracking.begin_calibration_tracking(&mut state).unwrap_err(),
        PhyParamTrackingChildError::UnsupportedAction
    );
    assert_eq!(
        tracking.begin_wifi_i2c_tracking(&mut state).unwrap_err(),
        PhyParamTrackingChildError::UnsupportedAction
    );
    tracking
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    let before = state.tx_power_tracking_parameters(false);
    let bluetooth = tracking.begin_tx_power_tracking(&mut state).unwrap();
    let bluetooth = match bluetooth.commit() {
        Ok(_) => panic!("incomplete TX-power child minted a parent completion"),
        Err(bluetooth) => bluetooth,
    };
    assert_eq!(
        bluetooth.state().tx_power_tracking_parameters(false),
        before
    );
    let completion = complete_power_child(bluetooth);
    tracking.advance(completion).unwrap();
    assert_eq!(state.bluetooth_tx_gain_parameters().base, 19);
    assert_eq!(tracking.action(), PhyParamTrackingAction::WifiI2cTrack);

    let i2c = tracking.begin_wifi_i2c_tracking(&mut state).unwrap();
    let i2c = match i2c.commit() {
        Ok(_) => panic!("incomplete Wi-Fi I2C child minted a parent completion"),
        Err(i2c) => i2c,
    };
    assert_eq!(
        i2c.state().wifi_i2c_tracking_parameters().previous_band,
        crate::tracking::i2c::PhyWifiI2cTrackingBand::Nominal
    );
    let completion = complete_wifi_i2c_child(i2c);
    tracking.advance(completion).unwrap();
    assert_eq!(
        state.wifi_i2c_tracking_parameters().previous_band,
        crate::tracking::i2c::PhyWifiI2cTrackingBand::Hot
    );
    let wifi = tracking.begin_tx_power_tracking(&mut state).unwrap();
    assert_eq!(
        wifi.action(),
        crate::tracking::power::PhyTxPowerTrackingAction::SetBbpllCalibration { enabled: true }
    );
    let completion = complete_power_child(wifi);
    tracking.advance(completion).unwrap();
    assert_eq!(state.channel_parameters().tx_gain_base, 16);
    assert_eq!(tracking.action(), PhyParamTrackingAction::TemperatureRead);

    let temperature = tracking.begin_temperature_read(&mut state).unwrap();
    let completion = complete_temperature_child(temperature, 15, 50);
    tracking.advance(completion).unwrap();
    assert_eq!(
        state
            .tx_power_tracking_parameters(false)
            .current_temperature,
        1
    );
    assert_eq!(tracking.action(), PhyParamTrackingAction::ExitCritical);
}

fn complete_power_child(
    mut child: PhyParamTrackingTxPowerTransition<'_>,
) -> PhyParamTrackingCompletion {
    loop {
        if !matches!(
            child.action(),
            crate::tracking::power::PhyTxPowerTrackingAction::Complete(_)
        ) {
            let binding = child.lower_external().unwrap();
            assert_eq!(binding.action(), child.action());
        }
        let completion = match child.action() {
                crate::tracking::power::PhyTxPowerTrackingAction::SetBbpllCalibration {
                    enabled,
                } => crate::tracking::power::PhyTxPowerTrackingCompletion::BbpllCalibrationSet {
                    enabled,
                },
                crate::tracking::power::PhyTxPowerTrackingAction::RegenerateWifiGain {
                    channel,
                    gain_base,
                } => crate::tracking::power::PhyTxPowerTrackingCompletion::WifiGainRegenerated {
                    channel,
                    gain_base,
                },
                crate::tracking::power::PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain {
                    gain_base,
                } => crate::tracking::power::PhyTxPowerTrackingCompletion::BluetoothIeee802154GainRegenerated {
                    gain_base,
                },
                crate::tracking::power::PhyTxPowerTrackingAction::Complete(_) => {
                    return child.commit().unwrap();
                }
            };
        child.advance(completion).unwrap();
    }
}

fn complete_wifi_i2c_child(
    mut child: PhyParamTrackingWifiI2cTransition<'_>,
) -> PhyParamTrackingCompletion {
    loop {
        match child.action() {
            crate::tracking::i2c::PhyWifiI2cTrackingAction::MaskedWrite(action) => {
                let binding = child.lower_external().unwrap();
                let completion = match action {
                    crate::analog::i2c::MaskedI2cWriteAction::ReadByte { address } => {
                        assert!(matches!(
                            binding.action(),
                            crate::calibration::cold::PhyColdI2cAction::StartRead {
                                address: bound_address
                            } if bound_address == address
                        ));
                        crate::tracking::i2c::PhyWifiI2cTrackingCompletion::MaskedWrite(
                            crate::analog::i2c::MaskedI2cWriteCompletion::I2cReadCompleted {
                                address,
                                value: 0xa0,
                            },
                        )
                    }
                    crate::analog::i2c::MaskedI2cWriteAction::WriteByte { address, value } => {
                        assert!(matches!(
                            binding.action(),
                            crate::calibration::cold::PhyColdI2cAction::StartWrite {
                                address: bound_address,
                                value: bound_value,
                            } if bound_address == address && bound_value == value
                        ));
                        crate::tracking::i2c::PhyWifiI2cTrackingCompletion::MaskedWrite(
                            crate::analog::i2c::MaskedI2cWriteCompletion::I2cWriteCompleted {
                                address,
                            },
                        )
                    }
                    crate::analog::i2c::MaskedI2cWriteAction::Complete => unreachable!(),
                };
                child.advance(completion).unwrap();
            }
            crate::tracking::i2c::PhyWifiI2cTrackingAction::Complete(_) => {
                return child.commit().unwrap();
            }
        }
    }
}

fn complete_temperature_child(
    mut child: PhyParamTrackingTemperatureTransition<'_>,
    dac: u8,
    code: u8,
) -> PhyParamTrackingCompletion {
    assert!(matches!(
        child.lower_external(),
        Ok(crate::analog::temperature::PhyTemperatureExternalBinding::I2c(_))
    ));
    let crate::analog::temperature::PhyTemperatureAction::ReadMasked { field } = child.action()
    else {
        panic!("temperature child did not begin with its DAC read")
    };
    child
        .advance(
            crate::analog::temperature::PhyTemperatureCompletion::MaskedRead { field, value: dac },
        )
        .unwrap();
    assert!(matches!(
        child.lower_external(),
        Ok(crate::analog::temperature::PhyTemperatureExternalBinding::Sample(_))
    ));
    child
        .advance(crate::analog::temperature::PhyTemperatureCompletion::CodeSampled { value: code })
        .unwrap();
    child.commit().unwrap()
}

#[test]
fn time_reversal_rejects_acquire_and_restores_exact_owner() {
    let state = complete_acquire_for_test(
        PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .acquire_at(PhyModemClient::Wifi, DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
            .unwrap(),
    );
    let before = state.snapshot();
    let failure = state.acquire_at(PhyModemClient::Ieee802154, 0).unwrap_err();
    assert_eq!(
        failure.error(),
        PhyClientAcquireError::TrackingTime(PhyTrackTimeError::TimeReversed {
            class: PhyPllTrackClass::Wifi,
            previous_micros: DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
            now_micros: 0,
        })
    );
    assert_eq!(failure.owner().snapshot(), before);
}

#[test]
fn time_reversal_rejects_immediate_evaluation_and_restores_exact_owner() {
    let state = complete_acquire_for_test(
        PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .acquire_at(
                PhyModemClient::Ieee802154,
                DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
            )
            .unwrap(),
    );
    let before = state.snapshot();
    let failure = state.evaluate_immediate_at(0).unwrap_err();
    assert_eq!(
        failure.error(),
        PhyTrackTimeError::TimeReversed {
            class: PhyPllTrackClass::BluetoothIeee802154,
            previous_micros: DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
            now_micros: 0,
        }
    );
    assert_eq!(failure.owner().snapshot(), before);
}

#[test]
fn time_reversal_rejects_periodic_callback_and_restores_exact_owner() {
    let state = complete_acquire_for_test(
        PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .acquire_at(
                PhyModemClient::Ieee802154,
                DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
            )
            .unwrap(),
    );
    let before = state.snapshot();
    let failure = state.evaluate_periodic_at(0).unwrap_err();
    assert_eq!(
        failure.error(),
        PhyTrackTimeError::TimeReversed {
            class: PhyPllTrackClass::BluetoothIeee802154,
            previous_micros: DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
            now_micros: 0,
        }
    );
    assert_eq!(failure.owner().snapshot(), before);
}
