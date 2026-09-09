//! Physical round trip after the worker has drained active TX.
use super::*;
use oer_esp32s31_wifi_dma::rx_ring::{RxRingPaused, RxRingResumeFailure};
use oer_esp32s31_wifi_embassy::{
    datapath::{
        irq::{
            MacInterruptEpochActivateError, MacInterruptEpochQuiesceError, PausedInterruptEpoch,
        },
        maintenance::{StopError, stop_mac},
    },
    time::phy::EmbassyPhyClock,
};
use oer_esp32s31_wifi_esp_hal::mac_interrupt_epoch::{
    EspHalMacInterruptRoute, EspHalMacInterruptRouteError,
};

mod access;
mod observation;

pub(super) type Role = oer_esp32s31_wifi::runtime::WifiRoleOwner<EspHalRadioPeripheral>;
type TrackingOutcome = Option<oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome>;

type PausedRx = ConnectedRx<RxRingPaused<'static, RX_DESCRIPTOR_COUNT>>;
type FailedRx = ConnectedRx<RxRingResumeFailure<'static, RX_DESCRIPTOR_COUNT>>;
type PausedRunner = ConnectedDatapathRunner<ConnectedDriverServices<PausedRx>>;
type FailedRunner = ConnectedDatapathRunner<ConnectedDriverServices<FailedRx>>;
type PauseRejectedRunner =
    ConnectedDatapathRunner<ConnectedDriverServices<(ConnectedLiveRx, RxRingError)>>;
type PausedIrq = PausedInterruptEpoch<'static, EspHalMacInterruptRoute, CriticalSectionRawMutex>;

/// One reusable checkpoint slot, rooted beside the permanent worker mailbox.
/// It holds the same affine runner only while RX has a paused type. Keeping
/// this cold storage outside the nested station futures avoids stacking two
/// complete copies of the service graph during the physical transition.
pub(super) struct Storage {
    paused: core::cell::RefCell<Option<PausedRunner>>,
    access: &'static access::Storage,
    observations: observation::Storage,
}
impl Storage {
    pub fn new() -> Self {
        Self {
            paused: core::cell::RefCell::new(None),
            access: access::Storage::initialize(),
            observations: observation::Storage::default(),
        }
    }

    pub(super) fn timings(&self) -> Option<oer_esp32s31_phy::tracking::observation::Report> {
        self.observations.report()
    }
}

/// Every failed checkpoint retains the exact non-runnable owner frontier.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Failure {
    MacStop {
        _irq: MacInterruptEpoch,
        _runner: ConnectedDatapathRunner,
        _error: StopError,
    },
    RxPause {
        _irq: MacInterruptEpoch,
        _runner: PauseRejectedRunner,
    },
    IrqPause {
        _irq: MacInterruptEpoch,
        _runner: PausedRunner,
        _error: MacInterruptEpochQuiesceError<EspHalMacInterruptRouteError>,
    },
    RxResume {
        _irq: PausedIrq,
        _runner: FailedRunner,
    },
    IrqResume {
        _irq: PausedIrq,
        _runner: ConnectedDatapathRunner,
        _error: MacInterruptEpochActivateError<EspHalMacInterruptRouteError>,
    },
    Access {
        _failure: oer_esp32s31_wifi_embassy::datapath::irq::PausedInterruptOperationFailure<
            'static,
            EspHalMacInterruptRoute,
            CriticalSectionRawMutex,
            &'static access::Failure,
        >,
    },
}

impl Failure {
    /// Put the terminal owner in its permanent quarantine before returning
    /// through the large parent future. No owner-bearing error is copied up
    /// the async call chain.
    #[inline(never)]
    fn retain(self) -> &'static Self {
        FAULT.init(self)
    }

    pub fn stage(&self) -> PauseError {
        match self {
            Self::Access { _failure } => _failure.error().stage(),
            Self::MacStop { _error: error, .. } => {
                diagnostics_event!("open-radio: pause MAC stop failed: {error:?}");
                PauseError::MacStop
            }
            Self::RxPause {
                _runner: runner, ..
            } => {
                diagnostics_event!(
                    "open-radio: pause RX failed: {:?}",
                    runner.services().rx().dma().1
                );
                PauseError::RxPause
            }
            Self::IrqPause { _error: error, .. } => {
                diagnostics_event!("open-radio: pause IRQ failed: {error:?}");
                PauseError::IrqPause
            }
            Self::RxResume {
                _runner: runner, ..
            } => {
                diagnostics_event!(
                    "open-radio: resume RX failed: {:?}",
                    runner.services().rx().dma().resume_error()
                );
                PauseError::RxResume
            }
            Self::IrqResume { _error: error, .. } => {
                diagnostics_event!("open-radio: resume IRQ failed: {error:?}");
                PauseError::IrqResume
            }
        }
    }
}

pub(super) static FAULT: StaticCell<Failure> = StaticCell::new();

/// Preserve the connected epoch through explicit access or due PHY tracking.
/// The caller retains live-runner storage; the RX checkpoint and physical
/// operation hold every withdrawn owner until restoration or terminal failure.
pub(super) async fn round_trip(
    irq: MacInterruptEpoch,
    role: &mut Option<Role>,
    operation: PauseOperation,
    runner: &mut Option<ConnectedDatapathRunner>,
    storage: &Storage,
) -> Result<(MacInterruptEpoch, Result<TrackingOutcome, PauseError>), &'static Failure> {
    storage.observations.reset();
    if let Err(error) = stop_mac(
        runner
            .as_mut()
            .expect("paused worker returned its runner")
            .services_mut()
            .hardware_mut(),
        &mut EmbassyPhyClock,
        100_000,
    )
    .await
    {
        return Err(retain_stop(irq, runner, error));
    }
    let suspended = {
        let mut paused = storage.paused.borrow_mut();
        assert!(
            paused.is_none(),
            "previous pause checkpoint must be consumed"
        );
        let (_, platform) = role.as_mut().expect("logical PHY owner").radio_mut();
        suspend(irq, platform, runner, &mut paused)?
    };
    match suspended {
        Suspended::Ready(irq) => {
            // The complete IRQ epoch is consumed through access admission.
            // Arena borrows end before tracking awaits. A failed operation
            // cannot recover a resumable route.
            let (irq, tracking) = match irq
                .try_with_authority(async |interrupts| {
                    oer_wifi_embassy::await_stack_boundary!(access::round_trip(
                        interrupts,
                        &storage.paused,
                        storage.access,
                        &storage.observations,
                        role,
                        operation
                    ))
                })
                .await
            {
                Ok(result) => result,
                Err(failure) => return Err(retain_access(failure)),
            };
            let (_, platform) = role
                .as_mut()
                .expect("restored logical PHY owner")
                .radio_mut();
            resume(irq, platform, &mut storage.paused.borrow_mut(), runner)
                .map(|irq| (irq, Ok(tracking)))
        }
        Suspended::Busy(irq) => Ok((irq, Err(PauseError::RxBusy))),
    }
}

#[inline(never)]
fn retain_access(
    failure: oer_esp32s31_wifi_embassy::datapath::irq::PausedInterruptOperationFailure<
        'static,
        EspHalMacInterruptRoute,
        CriticalSectionRawMutex,
        &'static access::Failure,
    >,
) -> &'static Failure {
    Failure::Access { _failure: failure }.retain()
}

enum Suspended {
    Ready(PausedIrq),
    Busy(MacInterruptEpoch),
}

#[inline(never)]
fn retain_stop(
    irq: MacInterruptEpoch,
    runner: &mut Option<ConnectedDatapathRunner>,
    error: StopError,
) -> &'static Failure {
    Failure::MacStop {
        _irq: irq,
        _runner: runner.take().expect("live runner"),
        _error: error,
    }
    .retain()
}

#[inline(never)]
fn suspend(
    irq: MacInterruptEpoch,
    platform: &EspHalRadioPeripheral,
    live: &mut Option<ConnectedDatapathRunner>,
    paused: &mut Option<PausedRunner>,
) -> Result<Suspended, &'static Failure> {
    match pause_rx(live.take().expect("live runner")) {
        Ok(runner) => *paused = Some(runner),
        Err(runner) => return recover_pause_rejection(irq, runner, live),
    }
    match irq.try_pause(platform) {
        Ok(irq) => Ok(Suspended::Ready(irq)),
        Err((irq, error)) => Err(retain_irq_pause(irq, paused, error)),
    }
}

#[inline(never)]
fn recover_pause_rejection(
    irq: MacInterruptEpoch,
    runner: PauseRejectedRunner,
    live: &mut Option<ConnectedDatapathRunner>,
) -> Result<Suspended, &'static Failure> {
    if runner.services().rx().dma().1 == RxRingError::Busy {
        // Busy precedes walker mutation. Ordinary RX service must complete
        // the pending append/reload frontier before another pause request.
        *live = Some(
            runner.map_services(|services| services.map_rx(|_, rx| rx.map_dma(|(dma, _)| dma))),
        );
        live.as_mut()
            .expect("restored live runner")
            .services_mut()
            .hardware_mut()
            .resume_mac_runtime();
        Ok(Suspended::Busy(irq))
    } else {
        Err(Failure::RxPause {
            _irq: irq,
            _runner: runner,
        }
        .retain())
    }
}

#[inline(never)]
fn retain_irq_pause(
    irq: MacInterruptEpoch,
    paused: &mut Option<PausedRunner>,
    error: MacInterruptEpochQuiesceError<EspHalMacInterruptRouteError>,
) -> &'static Failure {
    Failure::IrqPause {
        _irq: irq,
        _runner: paused.take().expect("paused RX runner"),
        _error: error,
    }
    .retain()
}

#[inline(never)]
fn resume(
    irq: PausedIrq,
    platform: &EspHalRadioPeripheral,
    paused: &mut Option<PausedRunner>,
    live: &mut Option<ConnectedDatapathRunner>,
) -> Result<MacInterruptEpoch, &'static Failure> {
    match resume_rx(paused.take().expect("paused RX runner")) {
        Ok(runner) => *live = Some(runner),
        Err(runner) => {
            return Err(Failure::RxResume {
                _irq: irq,
                _runner: runner,
            }
            .retain());
        }
    }
    let irq = match irq.try_resume(platform) {
        Ok(irq) => irq,
        Err((irq, error)) => return Err(retain_irq_resume(irq, live, error)),
    };
    live.as_mut()
        .expect("restored live runner")
        .services_mut()
        .hardware_mut()
        .resume_mac_runtime();
    Ok(irq)
}

#[inline(never)]
fn retain_irq_resume(
    irq: PausedIrq,
    live: &mut Option<ConnectedDatapathRunner>,
    error: MacInterruptEpochActivateError<EspHalMacInterruptRouteError>,
) -> &'static Failure {
    Failure::IrqResume {
        _irq: irq,
        _runner: live.take().expect("restored live runner"),
        _error: error,
    }
    .retain()
}

#[inline(never)]
#[allow(clippy::result_large_err)]
fn pause_rx(runner: ConnectedDatapathRunner) -> Result<PausedRunner, PauseRejectedRunner> {
    runner.try_map_services(|services| {
        services.try_map_rx(|hardware, rx| rx.try_map_dma(|dma| dma.try_pause(hardware)))
    })
}

#[inline(never)]
#[allow(clippy::result_large_err)]
fn resume_rx(runner: PausedRunner) -> Result<ConnectedDatapathRunner, FailedRunner> {
    runner.try_map_services(|services| {
        services.try_map_rx(|hardware, rx| rx.try_map_dma(|dma| dma.try_resume(hardware)))
    })
}
