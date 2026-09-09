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
        wifi_calibration: timing(Operation::WifiCalibration),
        bluetooth_ieee802154_calibration: timing(Operation::BluetoothIeee802154Calibration),
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

pub(super) fn tx_wait_evidence(report: oer_esp32s31_phy::executor::wait::tx::Report)
    -> open_esp_radio_hil_protocol::PhyTxWaitEvidence {
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
pub(super) fn timer_evidence(report: oer_esp32s31_embassy_runtime::timer_observation::Report)
    -> open_esp_radio_hil_protocol::TimerWindowEvidence {
    let timing = |value: oer_esp32s31_embassy_runtime::timer_observation::Timing|
        open_esp_radio_hil_protocol::TimerPhaseTiming { count: value.count, total_micros: value.total_micros, maximum_micros: value.maximum_micros };
    open_esp_radio_hil_protocol::TimerWindowEvidence {
        elapsed_micros: report.elapsed_micros, invalid: report.invalid,
        registrations: report.registrations, due_at_registration: report.due_at_registration,
        due_at_program_start: report.due_at_program_start, due_at_program_return: report.due_at_program_return,
        irq_ack: timing(report.irq_ack),
        programming: timing(report.programming), alarm_to_irq: timing(report.alarm_to_irq),
        deadline_lateness: timing(report.deadline_lateness), irq_to_dispatch: timing(report.irq_to_dispatch), dispatch: timing(report.dispatch),
        interrupts: report.interrupts, replaced: report.replaced, stopped: report.stopped,
        unmatched_interrupts: report.unmatched_interrupts, unmatched_dispatches: report.unmatched_dispatches,
        coalesced_interrupts: report.coalesced_interrupts, early_interrupts: report.early_interrupts,
        armed_at_end: report.armed_at_end, irq_pending_at_end: report.irq_pending_at_end,
    }
}
