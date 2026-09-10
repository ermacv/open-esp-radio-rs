//! Exact-arena register withdrawal and checked maintenance admission.
//!
//! Access-only and explicit due-tracking operations share the same physical
//! handoff. Tracking consumes the logical PHY/platform owner, reestablishes
//! MAC stop and checks station receive policy before publishing registers.
//! RX restoration separately verifies the retained descriptor cursor.

use super::*;
use oer_esp32s31_hal::{
    ieee80211::arena::{
        RadioOwnerArenaError, RadioOwnerRepublish, RadioOwnerRepublishFailure, ReclaimedRadioOwner,
    },
    owner::{MacInterruptCheckpoint, maintenance},
};
use oer_esp32s31_wifi_embassy::datapath::services::SingleRoleServices;
use oer_esp32s31_wifi_embassy::time::phy::EmbassyPhyDelay;

type ParkedRunner = ConnectedDatapathRunner<ConnectedDriverServices<PausedRx, ()>>;
/// Cold datapath state never participates in the nested PHY future. On failure
/// or cancellation it stays here without register authority; only successful
/// republication consumes it. No borrow of this slot crosses an await.
pub(crate) struct Storage {
    parked: core::cell::RefCell<Option<ParkedRunner>>,
}
impl Storage {
    pub(super) fn initialize() -> &'static Self {
        static STORAGE: StaticCell<Storage> = StaticCell::new();
        STORAGE.init_with(|| Self {
            parked: core::cell::RefCell::new(None),
        })
    }
}

type ReclaimFailureRunner = ConnectedDatapathRunner<
    ConnectedDriverServices<PausedRx, (ConnectedHardware, RadioOwnerArenaError)>,
>;

#[allow(clippy::large_enum_variant)]
pub(crate) enum Failure {
    Tracking {
        _runner: &'static Storage,
        _republish: RadioOwnerRepublish<'static>,
        _failure: oer_esp32s31_wifi::runtime::WifiRoleMaintenanceFailure<
            EspHalRadioPeripheral,
            MacInterruptCheckpoint,
        >,
    },
    Restoration {
        _runner: &'static Storage,
        _republish: RadioOwnerRepublish<'static>,
        _access: maintenance::WifiAccess<MacInterruptCheckpoint>,
        _error: RestorationError,
    },
    Reclaim {
        _runner: ReclaimFailureRunner,
        _interrupts: MacInterruptCheckpoint,
    },
    Admission {
        _runner: &'static Storage,
        _republish: RadioOwnerRepublish<'static>,
        _failure: maintenance::AdmissionFailure<MacInterruptCheckpoint>,
    },
    Release {
        _runner: &'static Storage,
        _republish: RadioOwnerRepublish<'static>,
        _failure: maintenance::ReleaseFailure<MacInterruptCheckpoint>,
    },
    Republish {
        _runner: &'static Storage,
        _interrupts: MacInterruptCheckpoint,
        _failure: RadioOwnerRepublishFailure<'static>,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RestorationError {
    Mac(StopError),
    ReceivePolicyChanged,
}

impl Failure {
    pub(super) fn stage(&self) -> PauseError {
        match self {
            Self::Tracking { _failure, .. } => {
                diagnostics_event!(
                    "open-radio: paused PHY tracking failed: {:?}",
                    _failure.error()
                );
                PauseError::PhyTracking
            }
            Self::Restoration { _error, .. } => match _error {
                RestorationError::Mac(error) => {
                    diagnostics_event!("open-radio: paused PHY MAC restoration failed: {error:?}");
                    PauseError::MacRestoration
                }
                RestorationError::ReceivePolicyChanged => PauseError::ReceivePolicyChanged,
            },
            Self::Reclaim { _runner, .. } => {
                diagnostics_event!(
                    "open-radio: pause register reclaim failed: {:?}",
                    _runner.services().hardware().1
                );
                PauseError::RegisterReclaim
            }
            Self::Admission { _failure, .. } => {
                diagnostics_event!(
                    "open-radio: pause PHY admission failed: {:?}",
                    _failure.error
                );
                PauseError::PhyAdmission
            }
            Self::Release { _failure, .. } => {
                diagnostics_event!("open-radio: pause PHY release failed: {:?}", _failure.error);
                PauseError::PhyRelease
            }
            Self::Republish { _failure, .. } => {
                diagnostics_event!(
                    "open-radio: pause register republish failed: {:?}",
                    _failure.error
                );
                PauseError::RegisterRepublish
            }
        }
    }
}

static FAULT: StaticCell<Failure> = StaticCell::new();

#[inline(never)]
pub(super) async fn round_trip(
    interrupts: MacInterruptCheckpoint,
    paused: &core::cell::RefCell<Option<PausedRunner>>,
    storage: &'static Storage,
    observations: &super::observation::Storage,
    role: &mut Option<Role>,
    operation: PauseOperation,
) -> Result<(MacInterruptCheckpoint, TrackingOutcome), &'static Failure> {
    let (interrupts, reclaimed) = reclaim(interrupts, &mut paused.borrow_mut(), storage)?;
    let (registers, republish) = reclaimed.into_parts();
    let mut access = match registers.try_into_phy_maintenance(interrupts) {
        Ok(access) => access,
        Err(failure) => {
            return Err(retain_admission(storage, republish, failure));
        }
    };
    let mut clock = EmbassyPhyClock;
    let tracking = if operation != PauseOperation::Access {
        let request = match operation {
            PauseOperation::ObservedOperation {
                operation,
                maximum_age_micros,
            } => oer_esp32s31_phy::WifiPhyMaintenanceRequest::ObservedOperation {
                operation,
                maximum_age_micros,
            },
            PauseOperation::Automatic(operation) => {
                oer_esp32s31_phy::WifiPhyMaintenanceRequest::ObservedOperation {
                    operation,
                    maximum_age_micros: super::super::pause_request::REQUESTS
                        .automatic
                        .snapshot()
                        .0
                        .map_or(0, |config| config.sample_period_micros()),
                }
            }
            PauseOperation::Operation(operation) => {
                oer_esp32s31_phy::WifiPhyMaintenanceRequest::Operation(operation)
            }
            PauseOperation::CommonCalibration => {
                oer_esp32s31_phy::WifiPhyMaintenanceRequest::CalibrateCommon
            }
            PauseOperation::TxCalibration => {
                oer_esp32s31_phy::WifiPhyMaintenanceRequest::CalibrateTransmit
            }
            PauseOperation::Rfpll => oer_esp32s31_phy::WifiPhyMaintenanceRequest::MeasureRfpll,
            PauseOperation::Calibration => oer_esp32s31_phy::WifiPhyMaintenanceRequest::Calibrate,
            _ => oer_esp32s31_phy::WifiPhyMaintenanceRequest::Track,
        };
        let before = access.wifi_mac_hal().station_receive_policy_snapshot();
        let owner = role.take().expect("paused logical PHY owner");
        let (owner, returned, outcome) = match oer_wifi_embassy::await_stack_boundary!(
            owner.maintain_phy::<EmbassyPhyDelay, _, _>(
                access,
                request,
                &mut clock,
                observations.observer()
            )
        ) {
            Ok(result) => result,
            Err(failure) => return Err(retain_tracking(storage, republish, failure)),
        };
        *role = Some(owner);
        access = returned;
        if outcome.is_some() {
            let stopped = {
                let mut mac = access.wifi_mac_hal();
                stop_mac(&mut mac, &mut clock, 100_000).await
            };
            if let Err(error) = stopped {
                return Err(retain_restoration(
                    storage,
                    republish,
                    access,
                    RestorationError::Mac(error),
                ));
            }
            if before != access.wifi_mac_hal().station_receive_policy_snapshot() {
                return Err(retain_restoration(
                    storage,
                    republish,
                    access,
                    RestorationError::ReceivePolicyChanged,
                ));
            }
        }
        outcome
    } else {
        None
    };
    let (registers, interrupts) = match access.try_release() {
        Ok(result) => result,
        Err(failure) => {
            return Err(retain_release(storage, republish, failure));
        }
    };
    let published = match republish.try_publish(registers) {
        Ok(published) => published,
        Err(failure) => {
            return Err(retain_republish(storage, interrupts, failure));
        }
    };
    restore(
        storage,
        ConnectedHardware::new(published),
        &mut paused.borrow_mut(),
    );
    Ok((interrupts, tracking))
}

#[inline(never)]
fn retain_tracking(
    runner: &'static Storage,
    republish: RadioOwnerRepublish<'static>,
    failure: oer_esp32s31_wifi::runtime::WifiRoleMaintenanceFailure<
        EspHalRadioPeripheral,
        MacInterruptCheckpoint,
    >,
) -> &'static Failure {
    FAULT.init(Failure::Tracking {
        _runner: runner,
        _republish: republish,
        _failure: failure,
    })
}

#[inline(never)]
fn retain_restoration(
    runner: &'static Storage,
    republish: RadioOwnerRepublish<'static>,
    access: maintenance::WifiAccess<MacInterruptCheckpoint>,
    error: RestorationError,
) -> &'static Failure {
    FAULT.init(Failure::Restoration {
        _runner: runner,
        _republish: republish,
        _access: access,
        _error: error,
    })
}

// Retain each large owner directly at the failed transition. Passing a small
// reference through the async IRQ operation avoids stacking copies of the
// complete runner in nested Result values.
#[inline(never)]
fn retain_reclaim(
    runner: ReclaimFailureRunner,
    interrupts: MacInterruptCheckpoint,
) -> &'static Failure {
    FAULT.init(Failure::Reclaim {
        _runner: runner,
        _interrupts: interrupts,
    })
}

#[inline(never)]
fn retain_admission(
    runner: &'static Storage,
    republish: RadioOwnerRepublish<'static>,
    failure: maintenance::AdmissionFailure<MacInterruptCheckpoint>,
) -> &'static Failure {
    FAULT.init(Failure::Admission {
        _runner: runner,
        _republish: republish,
        _failure: failure,
    })
}

#[inline(never)]
fn retain_release(
    runner: &'static Storage,
    republish: RadioOwnerRepublish<'static>,
    failure: maintenance::ReleaseFailure<MacInterruptCheckpoint>,
) -> &'static Failure {
    FAULT.init(Failure::Release {
        _runner: runner,
        _republish: republish,
        _failure: failure,
    })
}

#[inline(never)]
fn retain_republish(
    runner: &'static Storage,
    interrupts: MacInterruptCheckpoint,
    failure: RadioOwnerRepublishFailure<'static>,
) -> &'static Failure {
    FAULT.init(Failure::Republish {
        _runner: runner,
        _interrupts: interrupts,
        _failure: failure,
    })
}

#[inline(never)]
fn reclaim(
    interrupts: MacInterruptCheckpoint,
    paused: &mut Option<PausedRunner>,
    storage: &'static Storage,
) -> Result<(MacInterruptCheckpoint, ReclaimedRadioOwner<'static>), &'static Failure> {
    let mut slot = storage.parked.borrow_mut();
    assert!(slot.is_none(), "previous parked owner must be consumed");
    let mut reclaimed = None;
    let parked = paused
        .take()
        .expect("paused RX runner")
        .try_map_services(|services| {
            let (hardware, rx, tx, control) = services.into_parts();
            match hardware.try_into_reclaimed_registers() {
                Ok(owner) => {
                    reclaimed = Some(owner);
                    Ok(SingleRoleServices::with_control((), rx, tx, control))
                }
                Err(failure) => Err(SingleRoleServices::with_control(failure, rx, tx, control)),
            }
        });
    match parked {
        Ok(parked) => *slot = Some(parked),
        Err(runner) => return Err(retain_reclaim(runner, interrupts)),
    }
    Ok((
        interrupts,
        reclaimed.expect("successful reclaim returned register owner"),
    ))
}

#[inline(never)]
fn restore(
    storage: &'static Storage,
    hardware: ConnectedHardware,
    paused: &mut Option<PausedRunner>,
) {
    let parked = storage.parked.borrow_mut().take().expect("parked runner");
    *paused = Some(parked.map_services(|services| {
        let ((), rx, tx, control) = services.into_parts();
        SingleRoleServices::with_control(hardware, rx, tx, control)
    }));
}
