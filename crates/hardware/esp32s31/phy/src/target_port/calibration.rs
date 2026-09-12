//! Complete calibration children on the admitted physical PHY partition.
//!
//! These executors borrow the existing maintenance capability. They neither
//! acquire radio access nor publish parent calibration state. A returned child
//! completion must still be accepted by its parent transition.

#![cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    allow(unsafe_code)
)]

use super::{
    CHANNEL_READY_SAMPLE_LIMIT, PhyTargetObserver, PhyTargetPortError, RF_OPERATION_LIMIT,
};
use crate::{
    target_executor::{PhyAsyncDelay, PhyShortDelay},
    tracking::calibration::{
        PhyCalibrationChannelTransition, PhyCalibrationDcodeTransition,
        PhyCalibrationPbusClearTransition, PhyCalibrationRxGainTransition,
        PhyCalibrationTrackingCompletion,
    },
};
use oer_esp32s31_hal::owner::{SharedPhyAccess, SharedPhyContext};

/// Run the bounded PBus-clear child, including readiness and work-mode settling.
/// Once polled, keep the physical maintenance borrow until terminal completion;
/// an error does not establish a safe state for resuming radio traffic.
pub fn clear_pbus<D: PhyShortDelay, O: PhyTargetObserver>(
    mut child: PhyCalibrationPbusClearTransition,
    registers: &mut impl SharedPhyContext,
    observer: &mut O,
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    for _ in 0..RF_OPERATION_LIMIT {
        child = match child.commit() {
            Ok(completion) => return Ok(completion),
            Err(child) => child,
        };
        let binding = child
            .lower_external()
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion = complete_cold_direct::<D, _>(binding, registers, observer)?;
        child
            .advance_external(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

fn complete_cold_direct<D: PhyShortDelay, O: PhyTargetObserver>(
    binding: crate::calibration::cold::PhyColdExternalBinding,
    registers: &mut impl SharedPhyContext,
    observer: &mut O,
) -> Result<crate::analog::i2c::PhyRfInitPrefixCompletion, PhyTargetPortError> {
    use crate::{
        calibration::cold::{
            PhyColdExternalBinding, PhyColdI2cAction, PhyColdI2cError, PhyColdObservationRequest,
            PhyColdPbusObservation,
        },
        target_port::PhyRfBoundary,
    };
    match binding {
        PhyColdExternalBinding::I2cConfiguration(mut binding) => {
            let completed = crate::executor::wait::poll::bounded(|| {
                match binding.action() {
                    oer_esp32s31_hal::phy::i2c::PhyI2cConfigurationAction::StartCommand => {
                        match binding.start_target(registers) {
                            Ok(()) | Err(PhyColdI2cError::BusyAtStart) => {}
                            Err(_) => return Err(PhyTargetPortError::HardwareInvariant),
                        }
                    }
                    oer_esp32s31_hal::phy::i2c::PhyI2cConfigurationAction::AwaitCompletionEdge => {
                        binding
                            .observe_target_edge(registers)
                            .map_err(|_| PhyTargetPortError::HardwareInvariant)?;
                    }
                    oer_esp32s31_hal::phy::i2c::PhyI2cConfigurationAction::Complete => {
                        return Ok(true);
                    }
                }
                Ok(false)
            })?;
            if !completed {
                return Err(PhyTargetPortError::HardwareEdgeTimedOut);
            }
            binding
                .into_completion()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)
        }
        PhyColdExternalBinding::I2c(mut binding) => {
            let completed = crate::executor::wait::poll::bounded(|| {
                match binding.action() {
                    PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                        match binding.start_target(registers) {
                            Ok(()) | Err(PhyColdI2cError::BusyAtStart) => {}
                            Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                        }
                    }
                    PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                    | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                        binding
                            .observe_target_edge(registers)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                    }
                    PhyColdI2cAction::Complete(_) => return Ok(true),
                }
                Ok(false)
            })?;
            if !completed {
                return Err(PhyTargetPortError::HardwareEdgeTimedOut);
            }
            binding
                .into_completion()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)
        }
        PhyColdExternalBinding::Mmio(binding) => {
            let boundary = match binding.outer_action() {
                crate::analog::i2c::PhyRfInitPrefixAction::ConfigureFeBbClock => {
                    Some(PhyRfBoundary::BeforeRfInit)
                }
                crate::analog::i2c::PhyRfInitPrefixAction::ConfigureI2cClockSelection => {
                    Some(PhyRfBoundary::AfterPbusClear)
                }
                crate::analog::i2c::PhyRfInitPrefixAction::ConfigureI2cMasterRegisters => {
                    Some(PhyRfBoundary::BeforeI2cMasterRegisterInit)
                }
                crate::analog::i2c::PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters => {
                    Some(PhyRfBoundary::BeforePowerDetectorRegisterInit)
                }
                crate::analog::i2c::PhyRfInitPrefixAction::ConfigureFrontEndRegisters => {
                    Some(PhyRfBoundary::BeforeFrontEndRegisterInit)
                }
                crate::analog::i2c::PhyRfInitPrefixAction::ConfigureTemperatureSensorRead => {
                    Some(PhyRfBoundary::BeforeTemperatureSensorReadInit)
                }
                crate::analog::i2c::PhyRfInitPrefixAction::ConfigureTxPowerControlBackground => {
                    Some(PhyRfBoundary::BeforeTxPowerControlBackgroundInit)
                }
                crate::analog::i2c::PhyRfInitPrefixAction::ChannelFrequency(
                    crate::analog::frequency::PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters { .. },
                ) => Some(PhyRfBoundary::BeforeChannelFrequencyInit),
                _ => None,
            };
            if let Some(boundary) = boundary {
                observer.rf_boundary(boundary);
            }
            binding
                .execute_target(registers)
                .map_err(|error| match error {
                    crate::calibration::cold::PhyColdLoweringError::HardwareRestoreInvariant => {
                        PhyTargetPortError::HardwareInvariant
                    }
                    _ => PhyTargetPortError::UnexpectedBinding,
                })
        }
        PhyColdExternalBinding::Observation(binding) => {
            if binding.outer_action()
                == crate::analog::i2c::PhyRfInitPrefixAction::CaptureChannelFrequencyControl
            {
                observer.rf_boundary(PhyRfBoundary::BeforeChannelFrequencyInit);
            }
            match binding.request() {
                PhyColdObservationRequest::ObserveDcIqReadiness {
                    readiness_samples, ..
                }
                | PhyColdObservationRequest::ObserveSignalPowerReadiness {
                    readiness_samples,
                    ..
                } if readiness_samples >= crate::HARDWARE_EDGE_LIMIT => binding
                    .into_timeout_completion()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding),
                _ => binding
                    .execute_target(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding),
            }
        }
        PhyColdExternalBinding::Pbus(mut binding) => {
            binding
                .start_target(registers)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completed = crate::executor::wait::poll::bounded(|| {
                binding
                    .observe_target_edge(registers)
                    .map(|edge| edge == PhyColdPbusObservation::EdgeConsumed)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)
            })?;
            if !completed {
                return Err(PhyTargetPortError::HardwareEdgeTimedOut);
            }
            binding
                .into_completion()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)
        }
        PhyColdExternalBinding::Timer(binding) => {
            short_settle::<D>(binding.micros())?;
            binding
                .into_elapsed_completion()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)
        }
    }
}

/// Run the complete D-code child, retaining its nested PLL and I2C waits.
/// The caller retains the same maintenance and cancellation obligations as PBus clearing.
pub fn dcode<D: PhyShortDelay, P>(
    mut child: PhyCalibrationDcodeTransition,
    _platform: &mut P,
    registers: &mut impl SharedPhyAccess,
    mut observe: impl FnMut(bool),
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    for _ in 0..RF_OPERATION_LIMIT {
        child = match child.commit() {
            Ok(completion) => return Ok(completion),
            Err(child) => child,
        };
        let binding = child
            .lower_external()
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion = complete_dcode_direct::<D>(binding, registers)?;
        if let crate::analog::dcode::PhyDcodeCompletion::Rfpll(
            crate::analog::rfpll::RfpllFrequencyCompletion::MaskedRead {
                field: crate::analog::i2c::analog_registers::RFPLL_LOCK_STATUS,
                value,
            },
        ) = completion
        {
            observe(value != 0);
        }
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

fn complete_dcode_direct<D: PhyShortDelay>(
    binding: crate::analog::dcode::PhyDcodeExternalBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::analog::dcode::PhyDcodeCompletion, PhyTargetPortError> {
    use crate::analog::dcode::{PhyDcodeCompletion, PhyDcodeExternalBinding};
    match binding {
        PhyDcodeExternalBinding::Rfpll(binding) => Ok(PhyDcodeCompletion::Rfpll(
            complete_rfpll_hot::<D>(binding, registers)?,
        )),
        PhyDcodeExternalBinding::I2c(mut binding) => {
            use crate::calibration::cold::{
                PhyColdI2cAction, PhyColdI2cError, PhyColdI2cObservation,
            };
            let completed = crate::executor::wait::poll::bounded(|| {
                match binding.action() {
                    PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                        match binding.start_target(registers) {
                            Ok(()) | Err(PhyColdI2cError::BusyAtStart) => {}
                            Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                        }
                    }
                    PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                    | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                        match binding
                            .observe_target_edge(registers)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                        {
                            PhyColdI2cObservation::EdgeConsumed
                            | PhyColdI2cObservation::StillPending => {}
                        }
                    }
                    PhyColdI2cAction::Complete(_) => return Ok(true),
                }
                Ok(false)
            })?;
            if !completed {
                return Err(PhyTargetPortError::HardwareEdgeTimedOut);
            }
            binding
                .into_completion()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)
        }
    }
}

/// Run RX-DC calibration and publish both gain banks using the same admitted owner.
pub fn rx_gain<D: PhyAsyncDelay, P>(
    mut child: PhyCalibrationRxGainTransition,
    platform: &mut P,
    registers: &mut impl SharedPhyContext,
    observe: impl FnMut(crate::tracking::observation::Operation, crate::tracking::observation::Event),
    summarize: impl FnMut(crate::tracking::observation::RxGainExecution),
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    rx_gain_init::<D::ShortDelay, _>(
        child.transition_mut(),
        platform,
        registers,
        observe,
        summarize,
    )?;
    child
        .commit()
        .map_err(|_| PhyTargetPortError::UnexpectedBinding)
}

/// Execute an RX-gain root through its terminal edge. Its caller owns state
/// publication; success here may still contain a typed calibration failure.
#[cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    unsafe(link_section = ".hot.text.open_radio_phy_rx_gain_direct")
)]
#[inline(never)]
pub fn rx_gain_init<D: PhyShortDelay, P>(
    child: &mut crate::rx::gain::PhyRxGainInitTransition,
    _platform: &mut P,
    registers: &mut impl SharedPhyContext,
    mut observe: impl FnMut(
        crate::tracking::observation::Operation,
        crate::tracking::observation::Event,
    ),
    mut summarize: impl FnMut(crate::tracking::observation::RxGainExecution),
) -> Result<(), PhyTargetPortError> {
    use crate::tracking::observation::{Event, Operation};
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    let result = (|| {
        for _ in 0..4 {
            if child.terminal().is_some() {
                return Ok(());
            }
            if let Some(parameters) = child.direct_dc_parameters() {
                observe(Operation::RxGainControlPhase, Event::Started);
                let prepared = oer_esp32s31_hal::phy::rx_dco::prepare_control_restore(registers)
                    .map_err(|_| PhyTargetPortError::HardwareInvariant);
                observe(Operation::RxGainControlPhase, terminal_event(&prepared));
                prepared?;

                observe(Operation::RxGainDcPhase, Event::Started);
                let outcome = rx_gain_dc_direct::<D>(parameters, registers, &mut execution);
                observe(Operation::RxGainDcPhase, terminal_event(&outcome));
                let outcome = outcome?;

                observe(Operation::RxGainControlPhase, Event::Started);
                let restored = oer_esp32s31_hal::phy::rx_dco::restore_control(registers)
                    .map_err(|_| PhyTargetPortError::HardwareInvariant);
                observe(Operation::RxGainControlPhase, terminal_event(&restored));
                restored?;
                child
                    .commit_direct_dc(outcome)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                continue;
            }

            if child.phase() == Some(crate::rx::gain::Phase::Publish) {
                observe(Operation::RxGainPublishPhase, Event::Started);
                let published = child.execute_publish_target_direct::<D>(registers);
                observe(Operation::RxGainPublishPhase, terminal_event(&published));
                if !published? {
                    return Err(PhyTargetPortError::UnexpectedBinding);
                }
                continue;
            }

            if child.phase() == Some(crate::rx::gain::Phase::Control) {
                observe(Operation::RxGainControlPhase, Event::Started);
                if !child.execute_tail_target_direct(registers) {
                    observe(Operation::RxGainControlPhase, Event::Failed);
                    return Err(PhyTargetPortError::UnexpectedBinding);
                }
                observe(Operation::RxGainControlPhase, Event::Completed);
                continue;
            }
            return Err(PhyTargetPortError::UnexpectedBinding);
        }
        Err(PhyTargetPortError::RfOperationLimit)
    })();
    if result.is_ok() {
        summarize(execution);
    }
    result
}

enum DirectRxGainDcError {
    Target(PhyTargetPortError),
    Calibration(crate::rx::gain_calibration::PhyRxGainDcFailure),
}

impl From<PhyTargetPortError> for DirectRxGainDcError {
    fn from(error: PhyTargetPortError) -> Self {
        Self::Target(error)
    }
}

impl From<crate::rx::gain_calibration::PhyRxGainDcFailure> for DirectRxGainDcError {
    fn from(error: crate::rx::gain_calibration::PhyRxGainDcFailure) -> Self {
        Self::Calibration(error)
    }
}

#[cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    unsafe(link_section = ".hot.text.open_radio_phy_rx_gain_direct")
)]
fn force_rx_gain_pbus(
    registers: &mut impl SharedPhyAccess,
    bank: crate::rx::gain_calibration::PhyRxGainDcBank,
    transaction: crate::analog::pbus::PhyPbusForceTest,
) -> Result<(), crate::rx::gain_calibration::PhyRxGainDcFailure> {
    if crate::target_executor::force_pbus_direct(registers, transaction) {
        Ok(())
    } else {
        Err(crate::rx::gain_calibration::PhyRxGainDcFailure::Pbus { bank, transaction })
    }
}

#[cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    unsafe(link_section = ".hot.text.open_radio_phy_rx_gain_direct")
)]
fn rx_gain_rfpll_direct<D: PhyShortDelay>(
    crystal_selector: u8,
    registers: &mut impl SharedPhyAccess,
) -> Result<(), DirectRxGainDcError> {
    use crate::analog::rfpll::{
        RfpllFrequencyAction, RfpllFrequencyExternalBinding, RfpllFrequencyRequest,
        RfpllFrequencyTransition,
    };
    let mut transition = RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
        crystal_selector,
        frequency_code: 0x9b4,
        offset: 0,
    });
    loop {
        match transition.action() {
            RfpllFrequencyAction::Complete(_) => return Ok(()),
            RfpllFrequencyAction::Failed(failure) => {
                return Err(crate::rx::gain_calibration::PhyRxGainDcFailure::Rfpll(failure).into());
            }
            action => {
                let binding = RfpllFrequencyExternalBinding::lower(action)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                let completion = complete_rfpll_hot::<D>(binding, registers)?;
                transition
                    .advance(completion)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            }
        }
    }
}

#[cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    unsafe(link_section = ".hot.text.open_radio_phy_rx_gain_direct")
)]
fn rx_minimum_value<D: PhyShortDelay>(
    request: crate::rx::dc_offset::PhyRxDcMinimumRequest,
    registers: &mut impl SharedPhyAccess,
    execution: &mut crate::tracking::observation::RxGainExecution,
) -> Result<crate::rx::dc_offset::PhyRxDcMinimumOutcome, DirectRxGainDcError> {
    let completion = crate::rx::dc_offset::PhyRxDcMinimumTargetTransaction::new(request)
        .execute_target::<D>(u32::MAX, registers)?
        .ok_or(PhyTargetPortError::RfOperationLimit)?;
    execution.minimum_searches += 1;
    execution.minimum_operations += completion.operations();
    execution.settle_1us += 2 * u32::from(completion.estimators());
    completion
        .into_terminal()
        .map_err(|failure| crate::rx::gain_calibration::PhyRxGainDcFailure::Minimum(failure).into())
}

#[cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    unsafe(link_section = ".hot.text.open_radio_phy_rx_gain_direct")
)]
fn rx_gain_one_step_direct<D: PhyShortDelay>(
    request: crate::rx::gain_calibration::PhyRxDcCalibrationRequest,
    registers: &mut impl SharedPhyContext,
    execution: &mut crate::tracking::observation::RxGainExecution,
) -> Result<crate::rx::gain_calibration::PhyRxDcCalibrationOutcome, DirectRxGainDcError> {
    execution.outer_operations += 1;
    let mut transition = crate::rx::gain_calibration::PhyRxDcCalibrationTransition::new(request);
    let stats = transition.execute_target_direct::<D>(registers)?;
    execution.minimum_searches += stats.minimum_searches;
    execution.minimum_operations += stats.minimum_operations;
    execution.settle_1us += stats.settle_1us;
    execution.settle_10us += stats.settle_10us;
    match transition.terminal() {
        Some(Ok(outcome)) => Ok(outcome),
        Some(Err(failure)) => {
            Err(crate::rx::gain_calibration::PhyRxGainDcFailure::Calibration(failure).into())
        }
        None => Err(PhyTargetPortError::UnexpectedBinding.into()),
    }
}

#[cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    unsafe(link_section = ".hot.text.open_radio_phy_rx_gain_direct")
)]
fn rx_gain_reference_direct<D: PhyShortDelay>(
    bank: crate::rx::gain_calibration::PhyRxGainDcBank,
    measurement_base: u8,
    registers: &mut impl SharedPhyContext,
    execution: &mut crate::tracking::observation::RxGainExecution,
) -> Result<[i16; 2], DirectRxGainDcError> {
    use crate::rx::gain_calibration::reference_setup;
    for index in 0..6 {
        force_rx_gain_pbus(registers, bank, reference_setup(index))?;
    }
    if !D::settle_micros(10) {
        return Err(PhyTargetPortError::HardwareCapabilityUnavailable.into());
    }
    execution.settle_10us += 1;
    let low = rx_minimum_value::<D>(
        crate::rx::dc_offset::PhyRxDcMinimumRequest {
            measurement: measurement_base,
            control: 0x800,
            mode: 0,
            rx_saturation_detected: false,
        },
        registers,
        execution,
    )?;
    force_rx_gain_pbus(
        registers,
        bank,
        crate::analog::pbus::PhyPbusForceTest::new(1, 2, 0x20),
    )?;
    if !D::settle_micros(10) {
        return Err(PhyTargetPortError::HardwareCapabilityUnavailable.into());
    }
    execution.settle_10us += 1;
    let high = rx_minimum_value::<D>(
        crate::rx::dc_offset::PhyRxDcMinimumRequest {
            measurement: measurement_base + 1,
            control: 0x800,
            mode: 0,
            rx_saturation_detected: false,
        },
        registers,
        execution,
    )?;
    Ok([
        high.estimate.i.wrapping_sub(low.estimate.i) as i16,
        high.estimate.q.wrapping_sub(low.estimate.q) as i16,
    ])
}

#[cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    unsafe(link_section = ".hot.text.open_radio_phy_rx_gain_direct")
)]
fn rx_gain_cleanup_direct<D: PhyShortDelay>(
    registers: &mut impl SharedPhyContext,
) -> Result<(), PhyTargetPortError> {
    oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, false);
    oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, false);
    for index in 0..3 {
        let _ = crate::target_executor::force_pbus_direct(
            registers,
            crate::rx::gain_calibration::rx_off(index),
        );
    }
    if oer_esp32s31_hal::phy::pbus::configure_work_mode(registers) {
        short_settle::<D>(1)?;
        oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
        short_settle::<D>(2)?;
        oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
    }
    crate::hardware::configure_phy_rx_gain_dc_registers(registers, false);
    Ok(())
}

#[cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    unsafe(link_section = ".hot.text.open_radio_phy_rx_gain_direct")
)]
#[inline(never)]
fn rx_gain_dc_direct<D: PhyShortDelay>(
    parameters: crate::rx::gain_calibration::PhyRxGainDcParameters,
    registers: &mut impl SharedPhyContext,
    execution: &mut crate::tracking::observation::RxGainExecution,
) -> Result<
    Result<
        crate::rx::gain_calibration::PhyRxGainDcOutcome,
        crate::rx::gain_calibration::PhyRxGainDcFailure,
    >,
    PhyTargetPortError,
> {
    use crate::{
        analog::{i2c::analog_registers, pbus::PhyPbusForceTest},
        rx::gain_calibration::{
            PhyRxDcCalibrationRequest, PhyRxDcCalibrationStage, PhyRxGainDcBank,
            PhyRxGainDcOutcome, SHARED_CALIBRATION_GAIN, WIFI_CALIBRATION_GAIN, fine_code,
            fine_setup, rx_on, set_rx_gain_transaction, shared_mixer_dgain_transaction,
        },
    };

    let mut outcome = PhyRxGainDcOutcome {
        wifi_index_dc: [[0; 2]; 8],
        wifi_dc_base: [0; 2],
        shared_index_dc: [[0; 2]; 11],
        rxbb_dc_adjustments: [[0; 2]; 6],
    };
    let run = (|| -> Result<(), DirectRxGainDcError> {
        crate::hardware::configure_phy_rx_gain_dc_registers(registers, true);
        rx_gain_rfpll_direct::<D>(parameters.crystal_selector, registers)?;
        oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
        for index in 0..7 {
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                rx_on(index, parameters.pbus_rx_path_value),
            )?;
        }
        oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, true);
        oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, true);
        let shared_control = oer_esp32s31_hal::phy::pbus::read_result(registers, 1, 1)
            .ok_or(PhyTargetPortError::HardwareInvariant)?
            | 2;
        force_rx_gain_pbus(
            registers,
            PhyRxGainDcBank::Shared,
            PhyPbusForceTest::new(1, 1, shared_control),
        )?;
        crate::target_executor::write_i2c_field_direct(
            registers,
            analog_registers::SHARED_RX_GAIN_CALIBRATION_ENABLE,
            0,
        )?;
        let shared_reference =
            rx_gain_reference_direct::<D>(PhyRxGainDcBank::Shared, 0, registers, execution)?;
        for index in 0..SHARED_CALIBRATION_GAIN.len() as u8 {
            let previous = if index == 0 {
                [0x100; 2]
            } else {
                outcome.shared_index_dc[0]
            };
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                PhyPbusForceTest::new(2, 1, 0x100),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                PhyPbusForceTest::new(3, 1, 0x100),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                PhyPbusForceTest::new(2, 2, 0x100),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                PhyPbusForceTest::new(3, 2, 0x100),
            )?;
            for transaction in 0..3 {
                force_rx_gain_pbus(
                    registers,
                    PhyRxGainDcBank::Shared,
                    set_rx_gain_transaction(
                        u32::from(SHARED_CALIBRATION_GAIN[index as usize]) << 12,
                        parameters.pbus_rx_path_value,
                        transaction,
                    ),
                )?;
            }
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                shared_mixer_dgain_transaction(index, parameters.pbus_rx_path_value),
            )?;
            let calibrated = rx_gain_one_step_direct::<D>(
                PhyRxDcCalibrationRequest {
                    shared_radio: true,
                    stage: PhyRxDcCalibrationStage::Baseband,
                    control: 0x800,
                    initial: previous,
                    reference_delta: shared_reference,
                    gain_index: index,
                    rx_saturation_detected: parameters.rx_saturation_detected,
                },
                registers,
                execution,
            )?;
            outcome.shared_index_dc[index as usize] = calibrated.configuration;
        }
        crate::target_executor::write_i2c_field_direct(
            registers,
            analog_registers::SHARED_RX_GAIN_CALIBRATION_ENABLE,
            1,
        )?;
        rx_gain_cleanup_direct::<D>(registers)?;

        crate::hardware::configure_phy_rx_gain_dc_registers(registers, true);
        rx_gain_rfpll_direct::<D>(parameters.crystal_selector, registers)?;
        oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
        for index in 0..7 {
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                rx_on(index, parameters.pbus_rx_path_value),
            )?;
        }
        oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, true);
        oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, true);
        for index in 0..5 {
            force_rx_gain_pbus(registers, PhyRxGainDcBank::Wifi, fine_setup(index))?;
        }
        let mut fine_current = [0x100; 2];
        let mut fine_base = [0; 2];
        for index in 0..6_u8 {
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                PhyPbusForceTest::new(1, 2, fine_code(index)),
            )?;
            let calibrated = rx_gain_one_step_direct::<D>(
                PhyRxDcCalibrationRequest {
                    shared_radio: false,
                    stage: PhyRxDcCalibrationStage::Radio,
                    control: 0x800,
                    initial: fine_current,
                    reference_delta: [0; 2],
                    gain_index: 0,
                    rx_saturation_detected: parameters.rx_saturation_detected,
                },
                registers,
                execution,
            )?;
            fine_current = calibrated.configuration;
            if index == 0 {
                fine_base = fine_current;
                outcome.rxbb_dc_adjustments[0] = [0; 2];
            } else {
                outcome.rxbb_dc_adjustments[index as usize] = [
                    fine_current[0].wrapping_sub(fine_base[0]),
                    fine_current[1].wrapping_sub(fine_base[1]),
                ];
            }
        }
        let wifi_reference =
            rx_gain_reference_direct::<D>(PhyRxGainDcBank::Wifi, 2, registers, execution)?;
        for index in 0..WIFI_CALIBRATION_GAIN.len() as u8 {
            let previous = if index == 0 {
                [0x100; 2]
            } else {
                outcome.wifi_index_dc[0]
            };
            let baseband = if index == 0 {
                [0x100; 2]
            } else {
                outcome.wifi_dc_base
            };
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                PhyPbusForceTest::new(2, 1, 0x100),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                PhyPbusForceTest::new(3, 1, 0x100),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                PhyPbusForceTest::new(2, 2, baseband[0]),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                PhyPbusForceTest::new(3, 2, baseband[1]),
            )?;
            for transaction in 0..3 {
                force_rx_gain_pbus(
                    registers,
                    PhyRxGainDcBank::Wifi,
                    set_rx_gain_transaction(
                        u32::from(WIFI_CALIBRATION_GAIN[index as usize]) << 12,
                        parameters.pbus_rx_path_value,
                        transaction,
                    ),
                )?;
            }
            let calibrated = rx_gain_one_step_direct::<D>(
                PhyRxDcCalibrationRequest {
                    shared_radio: false,
                    stage: PhyRxDcCalibrationStage::Baseband,
                    control: 0x800,
                    initial: previous,
                    reference_delta: wifi_reference,
                    gain_index: index,
                    rx_saturation_detected: parameters.rx_saturation_detected,
                },
                registers,
                execution,
            )?;
            outcome.wifi_index_dc[index as usize] = calibrated.configuration;
            if index == 0 {
                force_rx_gain_pbus(
                    registers,
                    PhyRxGainDcBank::Wifi,
                    PhyPbusForceTest::new(1, 2, 0),
                )?;
                outcome.wifi_dc_base = rx_gain_one_step_direct::<D>(
                    PhyRxDcCalibrationRequest {
                        shared_radio: false,
                        stage: PhyRxDcCalibrationStage::Radio,
                        control: 0x800,
                        initial: [0x100; 2],
                        reference_delta: [0; 2],
                        gain_index: 0,
                        rx_saturation_detected: parameters.rx_saturation_detected,
                    },
                    registers,
                    execution,
                )?
                .configuration;
            }
        }
        Ok(())
    })();

    match run {
        Ok(()) => {
            rx_gain_cleanup_direct::<D>(registers)?;
            Ok(Ok(outcome))
        }
        Err(DirectRxGainDcError::Target(error)) => Err(error),
        Err(DirectRxGainDcError::Calibration(failure)) => {
            rx_gain_cleanup_direct::<D>(registers)?;
            Ok(Err(failure))
        }
    }
}

#[cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    unsafe(link_section = ".hot.text.open_radio_phy_rx_gain_direct")
)]
fn short_settle<D: PhyShortDelay>(micros: u32) -> Result<(), PhyTargetPortError> {
    if micros != 0 && micros <= D::MAX_MICROS && D::settle_micros(micros) {
        Ok(())
    } else {
        Err(PhyTargetPortError::HardwareCapabilityUnavailable)
    }
}

#[cfg_attr(
    all(target_arch = "riscv32", feature = "rx-gain-hot-sram"),
    unsafe(link_section = ".hot.text.open_radio_phy_rx_gain_direct")
)]
fn complete_rfpll_hot<D: PhyShortDelay>(
    binding: crate::analog::rfpll::RfpllFrequencyExternalBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, PhyTargetPortError> {
    use crate::analog::rfpll::{
        RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyExternalBinding,
    };
    match binding {
        RfpllFrequencyExternalBinding::Mmio(binding) => match binding.action() {
            RfpllFrequencyAction::ReadChannelReady { samples }
                if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
            {
                Ok(RfpllFrequencyCompletion::ChannelReadyTimedOut)
            }
            _ => Ok(binding.execute_target(registers)),
        },
        RfpllFrequencyExternalBinding::I2c(binding) => complete_rfpll_i2c_hot(binding, registers),
        RfpllFrequencyExternalBinding::Timer(binding) => {
            short_settle::<D>(binding.micros())?;
            Ok(binding.into_completion())
        }
    }
}

fn complete_rfpll_i2c_hot(
    mut binding: crate::analog::rfpll::RfpllFrequencyI2cBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, PhyTargetPortError> {
    use crate::calibration::cold::{PhyColdI2cAction, PhyColdI2cError};
    let completed = crate::executor::wait::poll::bounded(|| {
        match binding.action() {
            PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                match binding.start_target(registers) {
                    Ok(()) | Err(PhyColdI2cError::BusyAtStart) => {}
                    Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                }
            }
            PhyColdI2cAction::AwaitReadCompletionEdge { .. }
            | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            }
            PhyColdI2cAction::Complete(_) => return Ok(true),
        }
        Ok(false)
    })?;
    if !completed {
        return Err(PhyTargetPortError::HardwareEdgeTimedOut);
    }
    binding
        .into_completion()
        .map_err(|_| PhyTargetPortError::UnexpectedBinding)
}

fn terminal_event<T, E>(result: &Result<T, E>) -> crate::tracking::observation::Event {
    use crate::tracking::observation::Event;
    if result.is_ok() {
        Event::Completed
    } else {
        Event::Failed
    }
}

/// Execute the complete TX-DC/PWDET root as one vendor-shaped blocking
/// transaction, including measurement and restoration. The caller retains
/// physical access until terminal completion. Returning a target error or a
/// typed failed action does not establish a safe RF state.
#[inline(never)]
pub fn tx_dc_pwdet_init<D: PhyAsyncDelay, O: PhyTargetObserver>(
    child: &mut crate::tx::dc_power_detector::PhyTxDcPwdetTransition,
    registers: &mut impl SharedPhyContext,
    observer: &core::cell::RefCell<&mut O>,
) -> Result<(), PhyTargetPortError> {
    child.execute_target_direct(
        registers,
        |scope, micros| tx_settle::<D, O>(observer, scope, micros),
        |ready| observer.borrow_mut().tx_sar_ready(ready),
    )
}

fn tx_settle<D: PhyAsyncDelay, O: PhyTargetObserver>(
    observer: &core::cell::RefCell<&mut O>,
    scope: crate::executor::wait::tx::Scope,
    micros: u32,
) -> Result<(), PhyTargetPortError> {
    use crate::executor::wait::{Event, Kind};
    let started = O::OBSERVE_DELAYS.then(D::now_micros).flatten();
    if O::OBSERVE_DELAYS {
        observer.borrow_mut().tx_wait(
            scope,
            Kind::Settle,
            Event::Started {
                requested_micros: u64::from(micros),
            },
        );
    }
    let result = short_settle::<D::ShortDelay>(micros);
    if O::OBSERVE_DELAYS {
        let event = match (result.is_ok(), started, D::now_micros()) {
            (true, Some(started), Some(completed)) if completed >= started => {
                let elapsed_micros = completed - started;
                Event::Completed {
                    elapsed_micros,
                    lateness_micros: elapsed_micros.saturating_sub(u64::from(micros)),
                }
            }
            _ => Event::Unsupported,
        };
        observer.borrow_mut().tx_wait(scope, Kind::Settle, event);
    }
    result
}

/// Restore the operating channel without acquiring or releasing physical access.
pub fn channel<D: PhyShortDelay, P, O: PhyTargetObserver>(
    mut child: PhyCalibrationChannelTransition,
    platform: &mut P,
    registers: &mut impl SharedPhyAccess,
    observer: &mut O,
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    for _ in 0..RF_OPERATION_LIMIT {
        child = match child.commit() {
            Ok(completion) => return Ok(completion),
            Err(child) => child,
        };
        let binding = child
            .lower_external()
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion =
            complete_channel_direct::<D, _, _>(binding, platform, registers, observer)?;
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

fn complete_channel_direct<D: PhyShortDelay, P, O: PhyTargetObserver>(
    binding: crate::channel::PhyChipChannelExternalBinding,
    platform: &mut P,
    registers: &mut impl SharedPhyAccess,
    observer: &mut O,
) -> Result<crate::channel::PhyChipChannelCompletion, PhyTargetPortError> {
    use crate::channel::{
        PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelExternalBinding,
    };
    match binding {
        PhyChipChannelExternalBinding::Mmio(binding) => match binding.action() {
            PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. }
                if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
            {
                observer.channel_frequency_ready_timed_out(samples);
                Ok(PhyChipChannelCompletion::FrequencyReadyTimedOut)
            }
            _ => Ok(binding.execute_target(platform, registers)),
        },
        PhyChipChannelExternalBinding::Temperature(binding) => Ok(
            PhyChipChannelCompletion::Temperature(complete_temperature_direct(binding, registers)?),
        ),
        PhyChipChannelExternalBinding::Timer(binding) => {
            short_settle::<D>(binding.micros())?;
            Ok(binding.into_completion())
        }
        PhyChipChannelExternalBinding::I2c(binding) => {
            complete_channel_i2c_direct(binding, registers)
        }
        PhyChipChannelExternalBinding::TxGain(binding) => {
            let request = binding.request();
            let completion = binding.execute();
            if let PhyChipChannelCompletion::TxGainCalculated { image, .. } = completion {
                observer.channel_tx_gain(request, image);
            }
            Ok(completion)
        }
    }
}

fn complete_temperature_direct(
    binding: crate::analog::temperature::PhyTemperatureExternalBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::analog::temperature::PhyTemperatureCompletion, PhyTargetPortError> {
    use crate::analog::temperature::PhyTemperatureExternalBinding;
    match binding {
        PhyTemperatureExternalBinding::I2c(mut binding) => {
            use crate::calibration::cold::{PhyColdI2cAction, PhyColdI2cError};
            let completed = crate::executor::wait::poll::bounded(|| {
                match binding.action() {
                    PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                        match binding.start_target(registers) {
                            Ok(()) | Err(PhyColdI2cError::BusyAtStart) => {}
                            Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                        }
                    }
                    PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                    | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                        binding
                            .observe_target_edge(registers)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                    }
                    PhyColdI2cAction::Complete(_) => return Ok(true),
                }
                Ok(false)
            })?;
            if !completed {
                return Err(PhyTargetPortError::HardwareEdgeTimedOut);
            }
            binding
                .into_completion()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)
        }
        PhyTemperatureExternalBinding::Sample(binding) => Ok(binding.execute_target(registers)),
    }
}

fn complete_channel_i2c_direct(
    mut binding: crate::channel::PhyChipChannelI2cBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::channel::PhyChipChannelCompletion, PhyTargetPortError> {
    use crate::calibration::cold::{PhyColdI2cAction, PhyColdI2cError};
    let completed = crate::executor::wait::poll::bounded(|| {
        match binding.action() {
            PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                match binding.start_target(registers) {
                    Ok(()) | Err(PhyColdI2cError::BusyAtStart) => {}
                    Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                }
            }
            PhyColdI2cAction::AwaitReadCompletionEdge { .. }
            | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            }
            PhyColdI2cAction::Complete(_) => return Ok(true),
        }
        Ok(false)
    })?;
    if !completed {
        return Ok(binding.into_timeout_completion());
    }
    binding
        .into_completion()
        .map_err(|_| PhyTargetPortError::UnexpectedBinding)
}
