pub(super) fn phy_timing_evidence(
    report: oer_esp32s31_phy::tracking::observation::Report,
) -> open_esp_radio_hil_protocol::PhyTimingEvidence {
    use oer_esp32s31_phy::tracking::observation::Operation;
    let timing = |operation| {
        let value = report.timing(operation);
        open_esp_radio_hil_protocol::PhyOperationTiming {
            started: value.started,
            completed: value.completed,
            failed: value.failed,
            elapsed_micros: value.elapsed_micros,
            maximum_micros: value.maximum_micros,
        }
    };
    let polls = |operation| {
        let value = report.poll_timing(operation).unwrap_or_default();
        open_esp_radio_hil_protocol::PhyPollTiming {
            pending: value.pending,
            suspended_micros: value.suspended_micros,
            maximum_suspension_micros: value.maximum_suspension_micros,
            polls: value.polls,
            elapsed_micros: value.elapsed_micros,
            maximum_micros: value.maximum_micros,
        }
    };
    open_esp_radio_hil_protocol::PhyTimingEvidence {
        dcode_waits: open_esp_radio_hil_protocol::PhyDcodeWaitEvidence {
            i2c: bus_wait_evidence(report.dcode_waits.i2c),
            rfpll_i2c: bus_wait_evidence(report.dcode_waits.rfpll_i2c),
            rfpll_settle: wait_timing_evidence(report.dcode_waits.rfpll_settle),
            pll_locked: report.dcode_waits.pll_locked,
            pll_unlocked: report.dcode_waits.pll_unlocked,
        },
        invalid: report.invalid,
        dcode_polls: polls(Operation::Dcode),
        rx_gain_polls: polls(Operation::RxGain),
        tx_dc_pwdet_polls: polls(Operation::TxDcPwdet),
        rfpll: timing(Operation::Rfpll),
        wifi_power: timing(Operation::WifiPower),
        bluetooth_ieee802154_power: timing(Operation::BluetoothIeee802154Power),
        wifi_i2c: timing(Operation::WifiI2c),
        calibration: timing(Operation::Calibration),
        temperature: timing(Operation::Temperature),
        pbus_clear: timing(Operation::PbusClear),
        dcode: timing(Operation::Dcode),
        rx_gain: timing(Operation::RxGain),
        channel_restore: timing(Operation::ChannelRestore),
        force_tx_rx: timing(Operation::ForceTxRx),
        tx_dc_pwdet: timing(Operation::TxDcPwdet),
        tx_gain_publication: timing(Operation::TxGainPublication),
        frequency_settle: timing(Operation::FrequencySettle),
    }
}

fn wait_timing_evidence(
    value: oer_esp32s31_phy::executor::wait::Timing,
) -> open_esp_radio_hil_protocol::PhyWaitTiming {
    open_esp_radio_hil_protocol::PhyWaitTiming {
        count: value.count,
        requested_micros: value.requested_micros,
        elapsed_micros: value.elapsed_micros,
        maximum_lateness_micros: value.maximum_lateness_micros,
    }
}

fn bus_wait_evidence(
    value: oer_esp32s31_phy::executor::wait::Bus,
) -> open_esp_radio_hil_protocol::PhyBusWaitEvidence {
    open_esp_radio_hil_protocol::PhyBusWaitEvidence {
        bus_busy: value.bus_busy,
        timing: wait_timing_evidence(value.timing),
    }
}

pub(super) fn tx_wait_evidence(
    report: oer_esp32s31_phy::executor::wait::tx::Report,
) -> open_esp_radio_hil_protocol::PhyTxWaitEvidence {
    open_esp_radio_hil_protocol::PhyTxWaitEvidence {
        pbus: bus_wait_evidence(report.pbus),
        search: wait_timing_evidence(report.search),
        tone: wait_timing_evidence(report.tone),
        sar: wait_timing_evidence(report.sar),
        root: wait_timing_evidence(report.root),
        sar_ready: report.sar_ready,
        sar_not_ready: report.sar_not_ready,
    }
}

#[cfg(feature = "driver-observation")]
pub(super) fn timer_evidence(
    report: oer_esp32s31_embassy_runtime::timer_observation::Report,
) -> open_esp_radio_hil_protocol::TimerWindowEvidence {
    let timing = |value: oer_esp32s31_embassy_runtime::timer_observation::Timing| {
        open_esp_radio_hil_protocol::TimerPhaseTiming {
            count: value.count,
            total_micros: value.total_micros,
            maximum_micros: value.maximum_micros,
        }
    };
    open_esp_radio_hil_protocol::TimerWindowEvidence {
        elapsed_micros: report.elapsed_micros,
        invalid: report.invalid,
        registrations: report.registrations,
        due_at_registration: report.due_at_registration,
        due_at_program_start: report.due_at_program_start,
        due_at_program_return: report.due_at_program_return,
        irq_ack: timing(report.irq_ack),
        programming: timing(report.programming),
        alarm_to_irq: timing(report.alarm_to_irq),
        deadline_lateness: timing(report.deadline_lateness),
        irq_to_dispatch: timing(report.irq_to_dispatch),
        dispatch: timing(report.dispatch),
        interrupts: report.interrupts,
        replaced: report.replaced,
        stopped: report.stopped,
        overlapping_interrupts: report.overlapping_interrupts,
        unmatched_interrupts: report.unmatched_interrupts,
        unmatched_dispatches: report.unmatched_dispatches,
        coalesced_interrupts: report.coalesced_interrupts,
        early_interrupts: report.early_interrupts,
        armed_at_end: report.armed_at_end,
        irq_pending_at_end: report.irq_pending_at_end,
    }
}

/// Observe a service window, then disable it and wait for physical restoration.
/// The timer defines the measured workload interval, not a readiness guess.
pub(super) async fn run_service_window() -> Result<
    (
        oer_esp32s31_embassy_wifi::PauseReport,
        open_esp_radio_hil_protocol::StationTrackingServiceEvidence,
    ),
    oer_esp32s31_embassy_wifi::PauseError,
> {
    use oer_esp32s31_embassy_wifi as wifi;
    struct Disable;
    impl Drop for Disable {
        fn drop(&mut self) {
            let _ = wifi::configure_station_tracking(None);
        }
    }
    let started = embassy_time::Instant::now();
    wifi::configure_station_tracking(Some(wifi::TrackingConfig::new(
        core::num::NonZeroU64::new(1_000_000).unwrap(),
    )))?;
    let disable = Disable;
    embassy_time::Timer::after_micros(
        open_esp_radio_hil_protocol::STATION_TRACKING_SERVICE_WINDOW_MICROS,
    )
    .await;
    drop(disable);
    // Serialized explicit access waits behind any operation already admitted.
    let mut pause = wifi::station_pause_round_trip(wifi::PauseOperation::Access).await?;
    let report = wifi::station_tracking_report();
    let evidence = open_esp_radio_hil_protocol::StationTrackingServiceEvidence {
        operations: report.operations,
        deferred: report.deferred,
        common_calibrated: report.common_calibrated,
        wifi_calibrated: report.wifi_calibrated,
        elapsed_micros: started.elapsed().as_micros(),
        pause_micros: report.pause_micros,
        maximum_pause_micros: report.maximum_pause_micros,
        failed: report.failed,
        suspended: report.suspended,
        invalid: report.invalid,
    };
    // The final access-only handoff is not the aggregate PHY observation.
    pause.timings = None;
    pause.elapsed_micros = evidence.elapsed_micros;
    Ok((pause, evidence))
}

pub(super) fn rfpll_evidence(
    value: oer_esp32s31_phy::tracking::rfpll::Observation,
) -> open_esp_radio_hil_protocol::RfpllEvidence {
    open_esp_radio_hil_protocol::RfpllEvidence {
        temperature: value.request.current_temperature,
        sample_age_micros: value.sample_age_micros,
        reference_before: value.request.reference_temperature,
        reference_after: value.outcome.reference_temperature,
        threshold: value.request.threshold(),
        channel: value.request.current_channel,
        correction: value.outcome.correction.map(|correction| {
            open_esp_radio_hil_protocol::RfpllCorrectionEvidence {
                initial_cap: correction.search.initial_cap,
                selected_cap: correction.search.selected_cap,
                accepted_samples: correction.search.accepted_samples,
                entries_updated: correction.memory.map_or(0, |memory| memory.entries_updated),
                restored_frequency_index: correction
                    .memory
                    .map(|memory| memory.restored_frequency_index),
            }
        }),
    }
}

/// Keep maintenance reports and their serialization outside the role-task poll frame.
pub(super) async fn run_station_pause(
    request_id: u32,
    operation: open_esp_radio_hil_protocol::StationPauseOperation,
) {
    use oer_esp32s31_embassy_wifi::{PauseError, PauseOperation};
    use oer_wifi_embassy::await_stack_boundary;
    use open_esp_radio_hil_protocol::{StationPauseEvidence, StationPauseResult};
    #[cfg(feature = "driver-observation")]
    let timer_window = oer_esp32s31_embassy_runtime::timer_observation::Window::begin();
    let mut tx_waits = None;
    let mut rfpll = None;
    let mut service = None;
    let result = if operation == open_esp_radio_hil_protocol::StationPauseOperation::TrackingService
    {
        match await_stack_boundary!(self::run_service_window()) {
            Ok((report, observed)) => {
                service = Some(observed);
                Ok(report)
            }
            Err(error) => Err(error),
        }
    } else {
        use oer_esp32s31_phy::tracking::maintenance::Operation as PhyOperation;
        use open_esp_radio_hil_protocol::StationPauseOperation as Wire;
        let operation = match operation {
            Wire::Access => PauseOperation::Access,
            Wire::Tracking => PauseOperation::Tracking,
            Wire::Calibration => PauseOperation::Calibration,
            Wire::Temperature => PauseOperation::Operation(PhyOperation::Temperature),
            Wire::WifiPower => PauseOperation::Operation(PhyOperation::WifiPower),
            Wire::WifiI2c => PauseOperation::Operation(PhyOperation::WifiI2c),
            Wire::CommonCalibration => PauseOperation::CommonCalibration,
            Wire::TxCalibration => PauseOperation::TxCalibration,
            Wire::Rfpll => PauseOperation::Rfpll,
            Wire::RfpllCheck => PauseOperation::Operation(PhyOperation::Rfpll),
            Wire::RfpllObserved => PauseOperation::ObservedOperation {
                operation: PhyOperation::Rfpll,
                maximum_age_micros:
                    open_esp_radio_hil_protocol::STATION_RFPLL_SAMPLE_MAX_AGE_MICROS,
            },
            Wire::TrackingService => unreachable!(),
        };
        await_stack_boundary!(oer_esp32s31_embassy_wifi::station_pause_round_trip(
            operation
        ))
    };
    let evidence = match result {
        Ok(report) => {
            tx_waits = report
                .timings
                .map(|value| self::tx_wait_evidence(value.tx_waits));
            rfpll = report
                .timings
                .and_then(|value| value.rfpll)
                .map(self::rfpll_evidence);
            StationPauseEvidence {
                timings: report.timings.map(phy_timing_evidence),
                tracking: report.tracking.map(|outcome| {
                    open_esp_radio_hil_protocol::StationPhyTrackingEvidence {
                        inhibited: outcome.tracking_inhibited,
                        common_calibrated: outcome.calibration.common,
                        wifi_calibrated: outcome.calibration.wifi,
                        bluetooth_ieee802154_calibrated: outcome.calibration.bluetooth_ieee802154,
                    }
                }),
                result: StationPauseResult::Resumed,
                elapsed_micros: report.elapsed_micros,
            }
        }
        Err(error) => StationPauseEvidence {
            timings: None,
            tracking: None,
            result: match error {
                PauseError::Unavailable => StationPauseResult::Unavailable,
                PauseError::Busy => StationPauseResult::Busy,
                PauseError::Interrupted => StationPauseResult::Interrupted,
                PauseError::MacStop => StationPauseResult::MacStop,
                PauseError::RxBusy => StationPauseResult::RxBusy,
                PauseError::RxPause => StationPauseResult::RxPause,
                PauseError::IrqPause => StationPauseResult::IrqPause,
                PauseError::RxResume => StationPauseResult::RxResume,
                PauseError::IrqResume => StationPauseResult::IrqResume,
                PauseError::RegisterReclaim => StationPauseResult::RegisterReclaim,
                PauseError::PhyAdmission => StationPauseResult::PhyAdmission,
                PauseError::PhyRelease => StationPauseResult::PhyRelease,
                PauseError::RegisterRepublish => StationPauseResult::RegisterRepublish,
                PauseError::PhyTracking => StationPauseResult::PhyTracking,
                PauseError::MacRestoration => StationPauseResult::MacRestoration,
                PauseError::ReceivePolicyChanged => StationPauseResult::ReceivePolicyChanged,
            },
            elapsed_micros: 0,
        },
    };
    #[cfg(feature = "driver-observation")]
    let timer = timer_window.map(|window| self::timer_evidence(window.finish()));
    #[cfg(not(feature = "driver-observation"))]
    let timer = None;
    crate::console::complete_station_pause(request_id, evidence, tx_waits, timer, service, rfpll)
        .await;
}
