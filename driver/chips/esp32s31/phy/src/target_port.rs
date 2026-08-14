//! Complete ESP32-S31 target port for the recovered PHY transitions.
//!
//! This module owns the target-side composition of the individual PHY state
//! machines. Applications inject only an asynchronous delay and an optional
//! observer; they must not reconstruct the recovered hardware contract.

use core::marker::PhantomData;

use open_esp_radio_esp32s31_hal::PhyHal;
use open_esp_radio_esp32s31_hal::{
    Radio, analog_i2c::PhyPmuControl, phy_i2c::PhyI2cMasterControl,
    phy_prelude::PhyPreludePlatformControl, phy_temperature::PhyTemperatureSystemControl,
    power_detector_platform::PhyPowerDetectorPlatformControl, state::Powered,
    wifi_bb::PhyWifiBbControl,
};

use crate::{
    HARDWARE_EDGE_LIMIT, PhyRegisterPort,
    phy_bb::{PhyBbExternalBinding, PhyBbInitCompletion},
    phy_bluetooth::{
        PhyBluetoothTxGainInitCompletion, PhyBluetoothTxGainInitExternalBinding,
        PhyBluetoothTxPowerCompletion, PhyBluetoothTxPowerExternalBinding,
    },
    phy_channel::{
        PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelExternalBinding,
        PhyChipChannelFailure, PhyChipChannelOutcome, PhyChipChannelRequest,
        PhyChipChannelTransition, PhyWifiTxGainImage, PhyWifiTxGainRequest,
    },
    phy_cold::{
        PhyColdExternalBinding, PhyColdI2cAction, PhyColdI2cError, PhyColdI2cObservation,
        PhyColdObservationRequest, PhyColdPbusObservation,
    },
    phy_dc_iq::{PhyDcIqCompletion, PhyDcIqExternalBinding},
    phy_dcode::{PhyDcodeCompletion, PhyDcodeExternalBinding},
    phy_i2c::{PhyRfInitPrefixAction, PhyRfInitPrefixCompletion},
    phy_pbus::PhyPbusHardwareObservation,
    phy_pwdet::{PhyPwdetCompletion, PhyPwdetExternalBinding, PhyPwdetPbusObservation},
    phy_register::{PhyRegisterCompletion, PhyRegisterExternalBinding},
    phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyExternalBinding},
    phy_rx_dco::{
        PhyRxDcMinimumCompletion, PhyRxDcMinimumExternalBinding, PhyRxDcoCompletion,
        PhyRxDcoExternalBinding,
    },
    phy_rx_gain::{
        PhyRxGainInitCompletion, PhyRxGainInitExternalBinding, PhyRxGainPublishCompletion,
        PhyRxGainPublishExternalBinding,
    },
    phy_rx_gain_cal::{
        PhyRxDcCalibrationCompletion, PhyRxDcCalibrationExternalBinding, PhyRxGainDcCompletion,
        PhyRxGainDcExternalBinding,
    },
    phy_rx_saturation::{PhyRxSaturationCompletion, PhyRxSaturationExternalBinding},
    phy_rxiq::{
        PhyRxIqCoverCompletion, PhyRxIqCoverExternalBinding, PhyRxIqDataCompletion,
        PhyRxIqDataExternalBinding, PhyRxIqEstimatorCompletion, PhyRxIqEstimatorExternalBinding,
        PhyRxIqGainCompletion, PhyRxIqGainExternalBinding, PhyRxIqInitCompletion,
        PhyRxIqInitExternalBinding, PhyRxIqRfCalibrationCompletion,
        PhyRxIqRfCalibrationExternalBinding,
    },
    phy_state::PhyState,
    phy_temperature::{PhyTemperatureCompletion, PhyTemperatureExternalBinding},
    phy_tx_cal::{
        PhyPowerAttenuationCompletion, PhyPowerAttenuationExternalBinding, PhyToneSarCompletion,
        PhyToneSarExternalBinding, PhyTxCalibrationEnvironmentCompletion,
        PhyTxCalibrationEnvironmentExternalBinding, PhyTxCapCompletion, PhyTxCapExternalBinding,
        PhyTxCapSearchCompletion, PhyTxCapSearchExternalBinding,
    },
    phy_tx_power::{
        PhyPowerControlPointCompletion, PhyPowerControlPointExternalBinding, PhyTxPowerCompletion,
        PhyTxPowerExternalBinding,
    },
    phy_txdc::{PhyTxDcAction, PhyTxDcCompletion, PhyTxDcExternalBinding},
    phy_txdc_pwdet::{
        PhyTxDcPwdetCompletion, PhyTxDcPwdetExternalBinding, PhyTxDcPwdetSearchCompletion,
        PhyTxDcPwdetSearchExternalBinding,
    },
    phy_txiq::{
        PhyTxIqCalibrationCompletion, PhyTxIqCalibrationExternalBinding, PhyTxIqCoverCompletion,
        PhyTxIqCoverExternalBinding, PhyTxIqInitCompletion, PhyTxIqInitExternalBinding,
        PhyTxIqLinearPowerCompletion, PhyTxIqLinearPowerExternalBinding, PhyTxIqLoopbackCompletion,
        PhyTxIqLoopbackExternalBinding, PhyTxIqMisPowerCompletion, PhyTxIqMisPowerExternalBinding,
    },
    target_executor::{
        PhyAsyncDelay, PhyTargetPortError, complete_bluetooth_i2c, complete_bluetooth_pbus,
        complete_channel_i2c, complete_dcode_i2c, complete_final_i2c, complete_masked_i2c,
        complete_rfpll_i2c, complete_rx_dc_calibration_pbus, complete_rx_dco_pbus,
        complete_rx_gain_dc_pbus, complete_rx_gain_publish_pbus, complete_rx_saturation_pbus,
        complete_rxiq_adjusted_tx_i2c, complete_rxiq_gain_i2c, complete_rxiq_gain_pbus,
        complete_rxiq_init_i2c, complete_rxiq_init_pbus, complete_temperature_i2c,
        complete_tx_calibration_environment_pbus, complete_tx_dc_pwdet_pbus,
        complete_tx_dc_pwdet_search_pbus, complete_tx_power_i2c, complete_txiq_init_i2c,
        complete_txiq_pbus,
    },
};

const CHANNEL_READY_SAMPLE_LIMIT: u32 = 10_000;
const RF_OPERATION_LIMIT: u32 = 100_000;
const MAC_CHANNEL_SETTLE_US: u64 = 20;
const MAC_CHANNEL_IDLE_SETTLE_US: u64 = 5;

/// A semantic checkpoint exposed to target diagnostics without exporting raw
/// MMIO or application logging into the driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfBoundary {
    BeforeRfInit,
    AfterPbusClear,
    BeforeI2cMasterRegisterInit,
    BeforePowerDetectorRegisterInit,
    BeforeFrontEndRegisterInit,
    BeforeTemperatureSensorReadInit,
    BeforeTxPowerControlBackgroundInit,
    BeforeChannelFrequencyInit,
}

/// Optional, synchronous target observations which cannot affect PHY state.
///
/// Production integrations normally use [`NoopPhyTargetObserver`]. HIL code
/// can implement this trait to compare a completed Rust result with ROM or to
/// capture diagnostic MMIO without placing either dependency in this crate.
pub trait PhyTargetObserver {
    fn operation_started(&mut self) {}
    fn operation_completed(&mut self) {}
    fn channel_frequency_ready_timed_out(&mut self, _samples: u32) {}
    fn channel_tx_gain(&mut self, _request: PhyWifiTxGainRequest, _image: PhyWifiTxGainImage) {}
    fn channel_completed(&mut self, _outcome: PhyChipChannelOutcome, _operations: u32) {}
    fn channel_failed(&mut self, _failure: PhyChipChannelFailure, _operations: u32) {}
    fn mac_channel_restarted(&mut self, _channel_or_frequency: u16, _cbw: u8, _link: u8) {}
    fn tx_dc_entry(&mut self) {}
    fn tx_dc_comparator(&mut self, _gain_index: u8, _iteration: u8, _comparator_high: [bool; 2]) {}
    fn power_detector_sample(
        &mut self,
        _measurement_index: u8,
        _sample_index: u8,
        _sample_value: u16,
    ) {
    }
    fn rf_boundary(&mut self, _boundary: PhyRfBoundary) {}
}

/// Observer used by production integrations which need no diagnostic side
/// channel.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopPhyTargetObserver;

impl PhyTargetObserver for NoopPhyTargetObserver {}

/// Operation counts produced by one PHY registration run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PhyTargetPortCounters {
    pub mmio: u16,
    pub delays: u16,
    pub reset_samples: u16,
    pub rf_operations: u32,
    pub baseband_operations: u32,
}

/// Complete target-side implementation of [`PhyRegisterPort`].
pub struct TargetPhyRegisterPort<'a, P, D, O = NoopPhyTargetObserver> {
    radio: &'a mut Radio<P, Powered>,
    observer: O,
    counters: PhyTargetPortCounters,
    delay: PhantomData<D>,
}

struct TargetCompleter<D>(PhantomData<D>);

impl<D: PhyAsyncDelay> TargetCompleter<D> {
    async fn complete_rfpll<P: PhyI2cMasterControl>(
        binding: RfpllFrequencyExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<RfpllFrequencyCompletion, PhyTargetPortError> {
        match binding {
            RfpllFrequencyExternalBinding::Mmio(binding) => match binding.action() {
                RfpllFrequencyAction::ReadChannelReady { samples }
                    if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
                {
                    Ok(RfpllFrequencyCompletion::ChannelReadyTimedOut)
                }
                _ => Ok(binding.execute_target(registers)),
            },
            RfpllFrequencyExternalBinding::I2c(binding) => {
                complete_rfpll_i2c::<D>(binding, platform).await
            }
            RfpllFrequencyExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_tx_calibration_environment<P: PhyPowerDetectorPlatformControl>(
        binding: PhyTxCalibrationEnvironmentExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyTxCalibrationEnvironmentCompletion, PhyTargetPortError> {
        match binding {
            PhyTxCalibrationEnvironmentExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(platform, registers))
            }
            PhyTxCalibrationEnvironmentExternalBinding::Pbus(binding) => {
                complete_tx_calibration_environment_pbus::<D>(binding, registers).await
            }
            PhyTxCalibrationEnvironmentExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_tone_sar(
        binding: PhyToneSarExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyToneSarCompletion, PhyTargetPortError> {
        match binding {
            PhyToneSarExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyToneSarExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_temperature<P: PhyTemperatureSystemControl + PhyI2cMasterControl>(
        binding: PhyTemperatureExternalBinding,
        platform: &mut P,
    ) -> Result<PhyTemperatureCompletion, PhyTargetPortError> {
        match binding {
            PhyTemperatureExternalBinding::I2c(binding) => {
                complete_temperature_i2c::<D>(binding, platform).await
            }
            PhyTemperatureExternalBinding::Sample(binding) => Ok(binding.execute_target(platform)),
        }
    }

    async fn complete_power_control_point(
        binding: PhyPowerControlPointExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyPowerControlPointCompletion, PhyTargetPortError> {
        match binding {
            PhyPowerControlPointExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyPowerControlPointExternalBinding::ToneSar(binding) => {
                let completion = Self::complete_tone_sar(binding, registers).await?;
                Ok(PhyPowerControlPointCompletion::ToneSar(completion))
            }
        }
    }

    async fn complete_tx_power<P: PhyPowerDetectorPlatformControl + PhyI2cMasterControl>(
        binding: PhyTxPowerExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyTxPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyTxPowerExternalBinding::Environment(binding) => {
                Ok(PhyTxPowerCompletion::Environment(
                    Self::complete_tx_calibration_environment(binding, platform, registers).await?,
                ))
            }
            PhyTxPowerExternalBinding::Rfpll(binding) => Ok(PhyTxPowerCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyTxPowerExternalBinding::I2c(binding) => {
                complete_tx_power_i2c::<D>(binding, platform).await
            }
            PhyTxPowerExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxPowerExternalBinding::ToneSar(binding) => {
                let completion = Self::complete_tone_sar(binding, registers).await?;
                Ok(PhyTxPowerCompletion::ToneSar(completion))
            }
            PhyTxPowerExternalBinding::Point(binding) => Ok(PhyTxPowerCompletion::Point(
                Self::complete_power_control_point(binding, registers).await?,
            )),
        }
    }

    async fn complete_bluetooth_tx_power<
        P: PhyPowerDetectorPlatformControl + PhyI2cMasterControl,
    >(
        binding: PhyBluetoothTxPowerExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyBluetoothTxPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyBluetoothTxPowerExternalBinding::I2c(binding) => {
                complete_bluetooth_i2c::<D>(binding, platform).await
            }
            PhyBluetoothTxPowerExternalBinding::Prepare(binding) => {
                Ok(PhyBluetoothTxPowerCompletion::Prepare(
                    Self::complete_tx_calibration_environment(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxPowerExternalBinding::Cleanup(binding) => {
                Ok(PhyBluetoothTxPowerCompletion::Cleanup(
                    Self::complete_tx_calibration_environment(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxPowerExternalBinding::Pbus(binding) => {
                complete_bluetooth_pbus::<D>(binding, registers).await
            }
            PhyBluetoothTxPowerExternalBinding::ReadPbus(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyBluetoothTxPowerExternalBinding::Calibration(binding) => {
                Ok(PhyBluetoothTxPowerCompletion::Calibration(
                    Self::complete_tx_power(binding, platform, registers).await?,
                ))
            }
        }
    }

    async fn complete_bluetooth_tx_gain<
        P: PhyPowerDetectorPlatformControl + PhyI2cMasterControl,
        O: PhyTargetObserver,
    >(
        binding: PhyBluetoothTxGainInitExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
        observer: &mut O,
    ) -> Result<PhyBluetoothTxGainInitCompletion, PhyTargetPortError> {
        match binding {
            PhyBluetoothTxGainInitExternalBinding::Rfpll(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::Rfpll(
                    Self::complete_rfpll(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxCap(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxCap(
                    Self::complete_tx_power(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxDc(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxDc(
                    Self::complete_tx_dc(binding, registers, observer).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxPower(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxPower(
                    Self::complete_bluetooth_tx_power(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxDcPwdet(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxDcPwdet(
                    Self::complete_tx_dc_pwdet(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::Publish(binding) => {
                Ok(binding.execute_target(registers))
            }
        }
    }

    async fn complete_power_attenuation(
        binding: PhyPowerAttenuationExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyPowerAttenuationCompletion, PhyTargetPortError> {
        match binding {
            PhyPowerAttenuationExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyPowerAttenuationExternalBinding::ToneSar(binding) => {
                let completion = Self::complete_tone_sar(binding, registers).await?;
                Ok(PhyPowerAttenuationCompletion::ToneSar(completion))
            }
        }
    }

    async fn complete_tx_cap_search<P: PhyI2cMasterControl>(
        binding: PhyTxCapSearchExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyTxCapSearchCompletion, PhyTargetPortError> {
        match binding {
            PhyTxCapSearchExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxCapSearchExternalBinding::I2c(binding) => Ok(PhyTxCapSearchCompletion::I2c(
                complete_masked_i2c::<D>(binding, platform).await?,
            )),
            PhyTxCapSearchExternalBinding::ToneSar(binding) => {
                let completion = Self::complete_tone_sar(binding, registers).await?;
                Ok(PhyTxCapSearchCompletion::ToneSar(completion))
            }
        }
    }

    async fn complete_tx_cap<P: PhyPowerDetectorPlatformControl + PhyI2cMasterControl>(
        binding: PhyTxCapExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyTxCapCompletion, PhyTargetPortError> {
        match binding {
            PhyTxCapExternalBinding::Environment(binding) => Ok(PhyTxCapCompletion::Environment(
                Self::complete_tx_calibration_environment(binding, platform, registers).await?,
            )),
            PhyTxCapExternalBinding::Rfpll(binding) => Ok(PhyTxCapCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyTxCapExternalBinding::I2c(binding) => Ok(PhyTxCapCompletion::I2c(
                complete_masked_i2c::<D>(binding, platform).await?,
            )),
            PhyTxCapExternalBinding::Attenuation(binding) => Ok(PhyTxCapCompletion::Attenuation(
                Self::complete_power_attenuation(binding, registers).await?,
            )),
            PhyTxCapExternalBinding::Search(binding) => Ok(PhyTxCapCompletion::Search(
                Self::complete_tx_cap_search(binding, platform, registers).await?,
            )),
        }
    }

    async fn complete_tx_dc_pwdet_search(
        binding: PhyTxDcPwdetSearchExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyTxDcPwdetSearchCompletion, PhyTargetPortError> {
        match binding {
            PhyTxDcPwdetSearchExternalBinding::Pbus(binding) => {
                complete_tx_dc_pwdet_search_pbus::<D>(binding, registers).await
            }
            PhyTxDcPwdetSearchExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxDcPwdetSearchExternalBinding::ToneSar(binding) => {
                Ok(PhyTxDcPwdetSearchCompletion::ToneSar(
                    Self::complete_tone_sar(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_tx_dc_pwdet<P: PhyPowerDetectorPlatformControl>(
        binding: PhyTxDcPwdetExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyTxDcPwdetCompletion, PhyTargetPortError> {
        match binding {
            PhyTxDcPwdetExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(platform, registers))
            }
            PhyTxDcPwdetExternalBinding::Pbus(binding) => {
                complete_tx_dc_pwdet_pbus::<D>(binding, registers).await
            }
            PhyTxDcPwdetExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxDcPwdetExternalBinding::Search(binding) => Ok(PhyTxDcPwdetCompletion::Search(
                Self::complete_tx_dc_pwdet_search(binding, registers).await?,
            )),
        }
    }

    async fn complete_dcode<P: PhyI2cMasterControl>(
        binding: PhyDcodeExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyDcodeCompletion, PhyTargetPortError> {
        match binding {
            PhyDcodeExternalBinding::Rfpll(binding) => Ok(PhyDcodeCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyDcodeExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyDcodeExternalBinding::I2c(binding) => {
                complete_dcode_i2c::<D>(binding, platform).await
            }
        }
    }

    async fn complete_txiq_linear_power(
        binding: PhyTxIqLinearPowerExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyTxIqLinearPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqLinearPowerExternalBinding::ToneSar(binding) => {
                Ok(PhyTxIqLinearPowerCompletion::ToneSar(
                    Self::complete_tone_sar(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_txiq_mis_power(
        binding: PhyTxIqMisPowerExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyTxIqMisPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqMisPowerExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxIqMisPowerExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxIqMisPowerExternalBinding::LinearPower(binding) => {
                Ok(PhyTxIqMisPowerCompletion::LinearPower(
                    Self::complete_txiq_linear_power(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_txiq_cover(
        binding: PhyTxIqCoverExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyTxIqCoverCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqCoverExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxIqCoverExternalBinding::MisPower(binding) => Ok(PhyTxIqCoverCompletion::MisPower(
                Self::complete_txiq_mis_power(binding, registers).await?,
            )),
        }
    }

    async fn complete_txiq_loopback<P: PhyI2cMasterControl>(
        binding: PhyTxIqLoopbackExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyTxIqLoopbackCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqLoopbackExternalBinding::I2c(binding) => Ok(PhyTxIqLoopbackCompletion::I2c(
                complete_masked_i2c::<D>(binding, platform).await?,
            )),
            PhyTxIqLoopbackExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
        }
    }

    async fn complete_txiq_calibration<P: PhyPowerDetectorPlatformControl + PhyI2cMasterControl>(
        binding: PhyTxIqCalibrationExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyTxIqCalibrationCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqCalibrationExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyTxIqCalibrationExternalBinding::Loopback(binding) => {
                Ok(PhyTxIqCalibrationCompletion::Loopback(
                    Self::complete_txiq_loopback(binding, platform, registers).await?,
                ))
            }
            PhyTxIqCalibrationExternalBinding::Pbus(binding) => {
                complete_txiq_pbus::<D>(binding, registers).await
            }
            PhyTxIqCalibrationExternalBinding::Environment(binding) => {
                Ok(PhyTxIqCalibrationCompletion::Environment(
                    Self::complete_tx_calibration_environment(binding, platform, registers).await?,
                ))
            }
            PhyTxIqCalibrationExternalBinding::PowerAttenuation(binding) => {
                Ok(PhyTxIqCalibrationCompletion::PowerAttenuation(
                    Self::complete_power_attenuation(binding, registers).await?,
                ))
            }
            PhyTxIqCalibrationExternalBinding::Cover(binding) => {
                Ok(PhyTxIqCalibrationCompletion::Cover(
                    Self::complete_txiq_cover(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_txiq<
        P: PhyPowerDetectorPlatformControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
    >(
        binding: PhyTxIqInitExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyTxIqInitCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqInitExternalBinding::Rfpll(binding) => Ok(PhyTxIqInitCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyTxIqInitExternalBinding::I2c(binding) => {
                complete_txiq_init_i2c::<D>(binding, platform).await
            }
            PhyTxIqInitExternalBinding::Calibration(binding) => {
                Ok(PhyTxIqInitCompletion::Calibration(
                    Self::complete_txiq_calibration(binding, platform, registers).await?,
                ))
            }
            PhyTxIqInitExternalBinding::Temperature(binding) => {
                Ok(PhyTxIqInitCompletion::Temperature(
                    Self::complete_temperature(binding, platform).await?,
                ))
            }
        }
    }

    async fn complete_dc_iq(
        binding: PhyDcIqExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyDcIqCompletion, PhyTargetPortError> {
        match binding {
            PhyDcIqExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyDcIqExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyDcIqExternalBinding::Readiness(binding) => {
                if binding.samples() >= HARDWARE_EDGE_LIMIT {
                    Ok(binding.into_timeout_completion())
                } else {
                    D::after_micros(1).await;
                    Ok(binding.execute_target(registers))
                }
            }
        }
    }

    async fn complete_rx_dco(
        binding: PhyRxDcoExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyRxDcoCompletion, PhyTargetPortError> {
        match binding {
            PhyRxDcoExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxDcoExternalBinding::Pbus(binding) => {
                complete_rx_dco_pbus::<D>(binding, registers).await
            }
            PhyRxDcoExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxDcoExternalBinding::DcIq(binding) => Ok(PhyRxDcoCompletion::DcIq(
                Self::complete_dc_iq(binding, registers).await?,
            )),
        }
    }

    async fn complete_rxiq_estimator(
        binding: PhyRxIqEstimatorExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyRxIqEstimatorCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqEstimatorExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqEstimatorExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxIqEstimatorExternalBinding::Readiness(binding) => {
                if binding.samples() >= HARDWARE_EDGE_LIMIT {
                    Ok(binding.into_timeout_completion())
                } else {
                    D::after_micros(1).await;
                    Ok(binding.execute_target(registers))
                }
            }
        }
    }

    async fn complete_rxiq_cover(
        binding: PhyRxIqCoverExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyRxIqCoverCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqCoverExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqCoverExternalBinding::Estimator(binding) => {
                Ok(PhyRxIqCoverCompletion::Estimator(
                    Self::complete_rxiq_estimator(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rxiq_rf_calibration(
        binding: PhyRxIqRfCalibrationExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyRxIqRfCalibrationCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqRfCalibrationExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyRxIqRfCalibrationExternalBinding::Cover(binding) => {
                Ok(PhyRxIqRfCalibrationCompletion::Cover(
                    Self::complete_rxiq_cover(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rxiq_data(
        binding: PhyRxIqDataExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyRxIqDataCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqDataExternalBinding::Calibration(binding) => {
                Ok(PhyRxIqDataCompletion::Calibration(
                    Self::complete_rxiq_rf_calibration(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rxiq_gain<P: PhyI2cMasterControl>(
        binding: PhyRxIqGainExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyRxIqGainCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqGainExternalBinding::Pbus(binding) => {
                complete_rxiq_gain_pbus::<D>(binding, registers).await
            }
            PhyRxIqGainExternalBinding::I2c(binding) => {
                complete_rxiq_gain_i2c::<D>(binding, platform).await
            }
            PhyRxIqGainExternalBinding::AdjustTx(binding) => Ok(PhyRxIqGainCompletion::AdjustTx(
                complete_rxiq_adjusted_tx_i2c::<D>(binding, platform).await?,
            )),
            PhyRxIqGainExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqGainExternalBinding::Dco(binding) => Ok(PhyRxIqGainCompletion::Dco(
                Self::complete_rx_dco(binding, registers).await?,
            )),
            PhyRxIqGainExternalBinding::Estimator(binding) => Ok(PhyRxIqGainCompletion::Estimator(
                Self::complete_rxiq_estimator(binding, registers).await?,
            )),
            PhyRxIqGainExternalBinding::Data(binding) => Ok(PhyRxIqGainCompletion::Data(
                Self::complete_rxiq_data(binding, registers).await?,
            )),
        }
    }

    async fn complete_rxiq<P: PhyI2cMasterControl>(
        binding: PhyRxIqInitExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyRxIqInitCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqInitExternalBinding::Rfpll(binding) => Ok(PhyRxIqInitCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyRxIqInitExternalBinding::I2c(binding) => {
                complete_rxiq_init_i2c::<D>(binding, platform).await
            }
            PhyRxIqInitExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqInitExternalBinding::Pbus(binding) => {
                complete_rxiq_init_pbus::<D>(binding, registers).await
            }
            PhyRxIqInitExternalBinding::Loopback(binding) => Ok(PhyRxIqInitCompletion::Loopback(
                Self::complete_txiq_loopback(binding, platform, registers).await?,
            )),
            PhyRxIqInitExternalBinding::Gain(binding) => Ok(PhyRxIqInitCompletion::Gain(
                Self::complete_rxiq_gain(binding, platform, registers).await?,
            )),
            PhyRxIqInitExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_rx_saturation(
        binding: PhyRxSaturationExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyRxSaturationCompletion, PhyTargetPortError> {
        match binding {
            PhyRxSaturationExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxSaturationExternalBinding::Pbus(binding) => {
                complete_rx_saturation_pbus::<D>(binding, registers).await
            }
            PhyRxSaturationExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxSaturationExternalBinding::Sample(binding) => {
                Ok(binding.execute_target(registers))
            }
        }
    }

    async fn complete_rx_dc_minimum(
        binding: PhyRxDcMinimumExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyRxDcMinimumCompletion, PhyTargetPortError> {
        match binding {
            PhyRxDcMinimumExternalBinding::DcIq(binding) => Ok(PhyRxDcMinimumCompletion::DcIq(
                Self::complete_dc_iq(binding, registers).await?,
            )),
        }
    }

    async fn complete_rx_dc_calibration(
        binding: PhyRxDcCalibrationExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyRxDcCalibrationCompletion, PhyTargetPortError> {
        match binding {
            PhyRxDcCalibrationExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyRxDcCalibrationExternalBinding::Pbus(binding) => {
                complete_rx_dc_calibration_pbus::<D>(binding, registers).await
            }
            PhyRxDcCalibrationExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxDcCalibrationExternalBinding::Minimum(binding) => {
                Ok(PhyRxDcCalibrationCompletion::Minimum(
                    Self::complete_rx_dc_minimum(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rx_gain_dc<P: PhyI2cMasterControl>(
        binding: PhyRxGainDcExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyRxGainDcCompletion, PhyTargetPortError> {
        match binding {
            PhyRxGainDcExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxGainDcExternalBinding::Rfpll(binding) => Ok(PhyRxGainDcCompletion::Rfpll(
                Self::complete_rfpll(binding, platform, registers).await?,
            )),
            PhyRxGainDcExternalBinding::Pbus(binding) => {
                complete_rx_gain_dc_pbus::<D>(binding, registers).await
            }
            PhyRxGainDcExternalBinding::I2c(binding) => Ok(PhyRxGainDcCompletion::I2c(
                complete_masked_i2c::<D>(binding, platform).await?,
            )),
            PhyRxGainDcExternalBinding::Calibration(binding) => {
                Ok(PhyRxGainDcCompletion::Calibration(
                    Self::complete_rx_dc_calibration(binding, registers).await?,
                ))
            }
            PhyRxGainDcExternalBinding::Minimum(binding) => Ok(PhyRxGainDcCompletion::Minimum(
                Self::complete_rx_dc_minimum(binding, registers).await?,
            )),
            PhyRxGainDcExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_rx_gain_publish(
        binding: PhyRxGainPublishExternalBinding,
        registers: &mut PhyHal,
    ) -> Result<PhyRxGainPublishCompletion, PhyTargetPortError> {
        match binding {
            PhyRxGainPublishExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxGainPublishExternalBinding::Pbus(binding) => {
                complete_rx_gain_publish_pbus::<D>(binding, registers).await
            }
            PhyRxGainPublishExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_rx_gain<P: PhyI2cMasterControl>(
        binding: PhyRxGainInitExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
    ) -> Result<PhyRxGainInitCompletion, PhyTargetPortError> {
        match binding {
            PhyRxGainInitExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxGainInitExternalBinding::Dc(binding) => Ok(PhyRxGainInitCompletion::Dc(
                Self::complete_rx_gain_dc(binding, platform, registers).await?,
            )),
            PhyRxGainInitExternalBinding::Publish(binding) => Ok(PhyRxGainInitCompletion::Publish(
                Self::complete_rx_gain_publish(binding, registers).await?,
            )),
        }
    }

    async fn complete_channel<
        P: PhyWifiBbControl + PhyTemperatureSystemControl + PhyI2cMasterControl,
        O: PhyTargetObserver,
    >(
        binding: PhyChipChannelExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
        observer: &mut O,
    ) -> Result<PhyChipChannelCompletion, PhyTargetPortError> {
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
            PhyChipChannelExternalBinding::Temperature(binding) => {
                Ok(PhyChipChannelCompletion::Temperature(
                    Self::complete_temperature(binding, platform).await?,
                ))
            }
            PhyChipChannelExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyChipChannelExternalBinding::I2c(binding) => {
                complete_channel_i2c::<D>(binding, platform).await
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

    async fn complete_channel_hal<
        P: open_esp_radio_esp32s31_hal::channel::RadioChannelPlatform,
        O: PhyTargetObserver,
    >(
        binding: PhyChipChannelExternalBinding,
        channel: &mut open_esp_radio_esp32s31_hal::channel::RadioChannelHal<'_, P>,
        observer: &mut O,
    ) -> Result<PhyChipChannelCompletion, PhyTargetPortError> {
        match binding {
            PhyChipChannelExternalBinding::Mmio(binding) => match binding.action() {
                PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. }
                    if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
                {
                    observer.channel_frequency_ready_timed_out(samples);
                    Ok(PhyChipChannelCompletion::FrequencyReadyTimedOut)
                }
                _ => Ok(binding.execute_channel_hal(channel)),
            },
            PhyChipChannelExternalBinding::Temperature(binding) => {
                Ok(PhyChipChannelCompletion::Temperature(
                    Self::complete_temperature(binding, channel.platform_mut()).await?,
                ))
            }
            PhyChipChannelExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyChipChannelExternalBinding::I2c(binding) => {
                complete_channel_i2c::<D>(binding, channel.platform_mut()).await
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

    async fn select_channel_hal<
        P: open_esp_radio_esp32s31_hal::channel::RadioChannelPlatform,
        O: PhyTargetObserver,
    >(
        state: &mut PhyState,
        channel_or_frequency: u16,
        cbw: u8,
        channel: &mut open_esp_radio_esp32s31_hal::channel::RadioChannelHal<'_, P>,
        observer: &mut O,
    ) -> Result<(), PhyTargetPortError> {
        let mut transition = PhyChipChannelTransition::new(PhyChipChannelRequest {
            channel_or_frequency,
            cbw,
            parameters: state.channel_parameters(),
        });

        for operation in 0..RF_OPERATION_LIMIT {
            match transition.action() {
                PhyChipChannelAction::Complete(outcome) => {
                    state.apply_channel_outcome(outcome);
                    observer.channel_completed(outcome, operation);
                    return Ok(());
                }
                PhyChipChannelAction::Failed(failure) => {
                    observer.channel_failed(failure, operation);
                    return Err(PhyTargetPortError::UnexpectedBinding);
                }
                action => {
                    let binding = PhyChipChannelExternalBinding::lower(action)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                    let completion = Self::complete_channel_hal(binding, channel, observer).await?;
                    transition
                        .advance(completion)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                }
            }
        }

        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn switch_channel_hal_with_mac_restart<
        P: open_esp_radio_esp32s31_hal::channel::RadioChannelPlatform,
        O: PhyTargetObserver,
    >(
        state: &mut PhyState,
        channel_or_frequency: u16,
        cbw: u8,
        channel: &mut open_esp_radio_esp32s31_hal::channel::RadioChannelHal<'_, P>,
        observer: &mut O,
    ) -> Result<(), PhyTargetPortError> {
        channel.request_mac_stop();
        D::after_micros(MAC_CHANNEL_SETTLE_US).await;
        for _ in 0..RF_OPERATION_LIMIT {
            if channel.mac_active_state() == 0 {
                D::after_micros(MAC_CHANNEL_IDLE_SETTLE_US).await;
                Self::select_channel_hal(state, channel_or_frequency, cbw, channel, observer)
                    .await?;
                let regdma_link = channel.restart_mac();
                observer.mac_channel_restarted(channel_or_frequency, cbw, regdma_link);
                return Ok(());
            }
            D::after_micros(1).await;
        }
        Err(PhyTargetPortError::HardwareEdgeTimedOut)
    }

    async fn complete_tx_dc<O: PhyTargetObserver>(
        binding: PhyTxDcExternalBinding,
        registers: &mut PhyHal,
        observer: &mut O,
    ) -> Result<PhyTxDcCompletion, PhyTargetPortError> {
        match binding {
            PhyTxDcExternalBinding::Mmio(binding) => {
                if binding.action() == PhyTxDcAction::ConfigurePbusDebugMode {
                    observer.tx_dc_entry();
                }
                let completion = binding.execute_target(registers);
                if let PhyTxDcCompletion::ComparatorsRead {
                    gain_index,
                    iteration,
                    comparator_high,
                } = completion
                {
                    observer.tx_dc_comparator(gain_index, iteration, comparator_high);
                }
                Ok(completion)
            }
            PhyTxDcExternalBinding::Ready(binding) => Ok(binding.execute_target(registers)),
            PhyTxDcExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxDcExternalBinding::Pbus(mut binding) => {
                let mut started = false;
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    if binding.start_target(registers).is_ok() {
                        started = true;
                        break;
                    }
                    D::after_micros(1).await;
                }
                if !started {
                    return Err(PhyTargetPortError::HardwareEdgeTimedOut);
                }
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    D::after_micros(1).await;
                    match binding
                        .observe_target_edge(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                    {
                        PhyPbusHardwareObservation::EdgeConsumed => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                        PhyPbusHardwareObservation::StillPending => {}
                    }
                }
                Err(PhyTargetPortError::HardwareEdgeTimedOut)
            }
        }
    }

    async fn complete_pwdet<P: PhyPowerDetectorPlatformControl, O: PhyTargetObserver>(
        binding: PhyPwdetExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
        observer: &mut O,
    ) -> Result<PhyPwdetCompletion, PhyTargetPortError> {
        match binding {
            PhyPwdetExternalBinding::Mmio(binding) => {
                let completion = binding.execute_target(platform, registers);
                if let PhyPwdetCompletion::SarSampled {
                    measurement_index,
                    sample_index,
                    value,
                    ..
                } = completion
                {
                    observer.power_detector_sample(measurement_index, sample_index, value);
                }
                Ok(completion)
            }
            PhyPwdetExternalBinding::Ready(binding) => Ok(binding.execute_target(registers)),
            PhyPwdetExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyPwdetExternalBinding::Pbus(mut binding) => {
                let mut started = false;
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    if binding.start_target(registers).is_ok() {
                        started = true;
                        break;
                    }
                    D::after_micros(1).await;
                }
                if !started {
                    return Err(PhyTargetPortError::HardwareEdgeTimedOut);
                }
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    D::after_micros(1).await;
                    match binding
                        .sample_target_once(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                    {
                        PhyPwdetPbusObservation::Completed => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                        PhyPwdetPbusObservation::StillPending => {}
                    }
                }
                Err(PhyTargetPortError::HardwareEdgeTimedOut)
            }
        }
    }

    async fn complete_baseband<
        P: PhyWifiBbControl
            + PhyPowerDetectorPlatformControl
            + PhyTemperatureSystemControl
            + PhyI2cMasterControl,
        O: PhyTargetObserver,
    >(
        binding: PhyBbExternalBinding,
        platform: &mut P,
        registers: &mut PhyHal,
        observer: &mut O,
    ) -> Result<PhyBbInitCompletion, PhyTargetPortError> {
        match binding {
            PhyBbExternalBinding::Mmio(binding) => Ok(PhyBbInitCompletion::Mmio(
                binding.execute_target(platform, registers),
            )),
            PhyBbExternalBinding::TxDc(binding) => Ok(PhyBbInitCompletion::TxDc(
                Self::complete_tx_dc(binding, registers, observer).await?,
            )),
            PhyBbExternalBinding::Pwdet(binding) => Ok(PhyBbInitCompletion::Pwdet(
                Self::complete_pwdet(binding, platform, registers, observer).await?,
            )),
            PhyBbExternalBinding::TxCap(binding) => Ok(PhyBbInitCompletion::TxCap(
                Self::complete_tx_cap(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::Temperature(binding) => Ok(PhyBbInitCompletion::Temperature(
                Self::complete_temperature(binding, platform).await?,
            )),
            PhyBbExternalBinding::TxPower(binding) => Ok(PhyBbInitCompletion::TxPower(
                Self::complete_tx_power(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::TxDcPwdet(binding) => Ok(PhyBbInitCompletion::TxDcPwdet(
                Self::complete_tx_dc_pwdet(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::Dcode(binding) => Ok(PhyBbInitCompletion::Dcode(
                Self::complete_dcode(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::TxIq(binding) => Ok(PhyBbInitCompletion::TxIq(
                Self::complete_txiq(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::TxCfr(binding) => Ok(PhyBbInitCompletion::TxCfr(
                binding.execute_target(registers),
            )),
            PhyBbExternalBinding::BluetoothTxGain(binding) => {
                Ok(PhyBbInitCompletion::BluetoothTxGain(
                    Self::complete_bluetooth_tx_gain(binding, platform, registers, observer)
                        .await?,
                ))
            }
            PhyBbExternalBinding::PbusMemory(binding) => Ok(PhyBbInitCompletion::PbusMemory(
                binding
                    .execute_target(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
            )),
            PhyBbExternalBinding::RxIq(binding) => Ok(PhyBbInitCompletion::RxIq(
                Self::complete_rxiq(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::RxSaturation(binding) => Ok(PhyBbInitCompletion::RxSaturation(
                Self::complete_rx_saturation(binding, registers).await?,
            )),
            PhyBbExternalBinding::RxGain(binding) => Ok(PhyBbInitCompletion::RxGain(
                Self::complete_rx_gain(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::Channel(binding) => Ok(PhyBbInitCompletion::Channel(
                Self::complete_channel(binding, platform, registers, observer).await?,
            )),
        }
    }

    async fn complete_rf<
        P: PhyPmuControl
            + PhyPowerDetectorPlatformControl
            + PhyTemperatureSystemControl
            + PhyI2cMasterControl,
        O: PhyTargetObserver,
    >(
        binding: PhyColdExternalBinding,
        radio: &mut Radio<P, Powered>,
        observer: &mut O,
    ) -> Result<PhyRfInitPrefixCompletion, PhyTargetPortError> {
        match binding {
            PhyColdExternalBinding::I2c(mut binding) => {
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    match binding.action() {
                        PhyColdI2cAction::StartRead { .. }
                        | PhyColdI2cAction::StartWrite { .. } => {
                            match binding.start_target(radio.phy_hal_parts().0) {
                                Ok(()) => {}
                                Err(PhyColdI2cError::BusyAtStart) => D::after_micros(1).await,
                                Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                            }
                        }
                        PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                        | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                            D::after_micros(1).await;
                            match binding
                                .observe_target_edge(radio.phy_hal_parts().0)
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                            {
                                PhyColdI2cObservation::EdgeConsumed
                                | PhyColdI2cObservation::StillPending => {}
                            }
                        }
                        PhyColdI2cAction::Complete(_) => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                    }
                }
                Err(PhyTargetPortError::HardwareEdgeTimedOut)
            }
            PhyColdExternalBinding::Mmio(binding) => {
                let boundary = match binding.outer_action() {
                    PhyRfInitPrefixAction::ConfigureFeBbClock => Some(PhyRfBoundary::BeforeRfInit),
                    PhyRfInitPrefixAction::ConfigureI2cClockSelection { .. } => {
                        Some(PhyRfBoundary::AfterPbusClear)
                    }
                    PhyRfInitPrefixAction::ConfigureI2cMasterRegisters => {
                        Some(PhyRfBoundary::BeforeI2cMasterRegisterInit)
                    }
                    PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters => {
                        Some(PhyRfBoundary::BeforePowerDetectorRegisterInit)
                    }
                    PhyRfInitPrefixAction::ConfigureFrontEndRegisters => {
                        Some(PhyRfBoundary::BeforeFrontEndRegisterInit)
                    }
                    PhyRfInitPrefixAction::ConfigureTemperatureSensorRead => {
                        Some(PhyRfBoundary::BeforeTemperatureSensorReadInit)
                    }
                    PhyRfInitPrefixAction::ConfigureTxPowerControlBackground => {
                        Some(PhyRfBoundary::BeforeTxPowerControlBackgroundInit)
                    }
                    PhyRfInitPrefixAction::ChannelFrequency(
                        crate::phy_frequency::PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
                            ..
                        },
                    ) => Some(PhyRfBoundary::BeforeChannelFrequencyInit),
                    _ => None,
                };
                if let Some(boundary) = boundary {
                    observer.rf_boundary(boundary);
                }
                binding
                    .execute_target(radio)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)
            }
            PhyColdExternalBinding::Observation(binding) => {
                if binding.outer_action() == PhyRfInitPrefixAction::CaptureChannelFrequencyControl {
                    observer.rf_boundary(PhyRfBoundary::BeforeChannelFrequencyInit);
                }
                match binding.request() {
                    PhyColdObservationRequest::ObserveDcIqReadiness {
                        readiness_samples, ..
                    }
                    | PhyColdObservationRequest::ObserveSignalPowerReadiness {
                        readiness_samples,
                        ..
                    } if readiness_samples >= HARDWARE_EDGE_LIMIT => binding
                        .into_timeout_completion()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding),
                    PhyColdObservationRequest::ObserveDcIqReadiness { .. }
                    | PhyColdObservationRequest::ObserveSignalPowerReadiness { .. } => {
                        D::after_micros(1).await;
                        binding
                            .execute_target(radio)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)
                    }
                    _ => binding
                        .execute_target(radio)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding),
                }
            }
            PhyColdExternalBinding::Pbus(mut binding) => {
                let registers = radio.phy_hal_mut();
                binding
                    .start_target(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    D::after_micros(1).await;
                    match binding
                        .observe_target_edge(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                    {
                        PhyColdPbusObservation::EdgeConsumed => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                        PhyColdPbusObservation::StillPending => {}
                    }
                }
                Err(PhyTargetPortError::HardwareEdgeTimedOut)
            }
            PhyColdExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                binding
                    .into_elapsed_completion()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)
            }
        }
    }
}

impl<'a, P, D, O> TargetPhyRegisterPort<'a, P, D, O> {
    /// Bind the complete target port to the uniquely owned powered radio.
    pub fn new(radio: &'a mut Radio<P, Powered>, observer: O) -> Self {
        Self {
            radio,
            observer,
            counters: PhyTargetPortCounters::default(),
            delay: PhantomData,
        }
    }

    /// Snapshot operation counts without releasing the radio borrow.
    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }
}

impl<
    P: PhyPreludePlatformControl
        + PhyPmuControl
        + PhyWifiBbControl
        + PhyPowerDetectorPlatformControl
        + PhyTemperatureSystemControl
        + PhyI2cMasterControl,
    D: PhyAsyncDelay,
    O: PhyTargetObserver,
> PhyRegisterPort for TargetPhyRegisterPort<'_, P, D, O>
{
    type Error = PhyTargetPortError;

    async fn complete(
        &mut self,
        binding: PhyRegisterExternalBinding,
    ) -> Result<PhyRegisterCompletion, Self::Error> {
        self.observer.operation_started();
        let result = match binding {
            PhyRegisterExternalBinding::Mmio(binding) => {
                self.counters.mmio += 1;
                Ok(PhyRegisterCompletion::Mmio(
                    binding.execute_target(self.radio),
                ))
            }
            PhyRegisterExternalBinding::Timer(binding) => {
                D::after_micros(u64::from(binding.micros())).await;
                self.counters.delays += 1;
                Ok(binding.into_completion())
            }
            PhyRegisterExternalBinding::ResetSample(binding) => {
                self.counters.reset_samples += 1;
                Ok(binding.execute_target(self.radio))
            }
            PhyRegisterExternalBinding::Rf(binding) => {
                if self.counters.rf_operations >= RF_OPERATION_LIMIT {
                    Err(PhyTargetPortError::RfOperationLimit)
                } else {
                    let completion =
                        TargetCompleter::<D>::complete_rf(binding, self.radio, &mut self.observer)
                            .await?;
                    self.counters.rf_operations += 1;
                    Ok(PhyRegisterCompletion::Rf(completion))
                }
            }
            PhyRegisterExternalBinding::Baseband(binding) => {
                let (platform, registers) = self.radio.phy_hal_parts();
                let completion = TargetCompleter::<D>::complete_baseband(
                    binding,
                    platform,
                    registers,
                    &mut self.observer,
                )
                .await?;
                self.counters.baseband_operations += 1;
                Ok(PhyRegisterCompletion::Baseband(completion))
            }
            PhyRegisterExternalBinding::Temperature(binding) => {
                let platform = self.radio.phy_hal_parts().0;
                Ok(PhyRegisterCompletion::Temperature(
                    TargetCompleter::<D>::complete_temperature(binding, platform).await?,
                ))
            }
            PhyRegisterExternalBinding::FinalI2c(binding) => {
                complete_final_i2c::<D>(binding, self.radio.phy_hal_parts().0).await
            }
        };
        if result.is_ok() {
            self.observer.operation_completed();
        }
        result
    }
}

/// Select a PHY channel with the same finite target contract used by cold
/// registration.
/// Select a PHY channel through a temporary borrow of the complete HAL owner.
pub async fn select_phy_channel_with_hal<
    D: PhyAsyncDelay,
    P: open_esp_radio_esp32s31_hal::channel::RadioChannelPlatform,
    O: PhyTargetObserver,
>(
    state: &mut PhyState,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut open_esp_radio_esp32s31_hal::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), PhyTargetPortError> {
    TargetCompleter::<D>::select_channel_hal(state, channel_or_frequency, cbw, channel, observer)
        .await
}

/// Stop, retune and restart through a temporary borrow of the HAL owner.
pub async fn switch_phy_channel_with_hal_and_mac_restart<
    D: PhyAsyncDelay,
    P: open_esp_radio_esp32s31_hal::channel::RadioChannelPlatform,
    O: PhyTargetObserver,
>(
    state: &mut PhyState,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut open_esp_radio_esp32s31_hal::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), PhyTargetPortError> {
    TargetCompleter::<D>::switch_channel_hal_with_mac_restart(
        state,
        channel_or_frequency,
        cbw,
        channel,
        observer,
    )
    .await
}
