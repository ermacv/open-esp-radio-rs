//! The radio role and its hardware behind one async lock.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::select;
use embassy_sync::{blocking_mutex::raw::RawMutex, channel::Channel, mutex::Mutex, signal::Signal};
use embassy_time::{Duration, Timer};
use oer_bluetooth_radio::{RadioInstant, RadioOutcome, RadioRequest, RadioTiming, RequestError};
use oer_esp32s31_bluetooth::{
    ControllerTimeSample,
    controller_time::{
        ControllerTimeEventError, ControllerTimeEventStep, ControllerTimeRequestError,
    },
    scheduler::{
        SchedulerFinishedListWorkerStep, SchedulerHardwareError, SchedulerNext,
        SchedulerStartError, SchedulerWait,
    },
};
use oer_esp32s31_bluetooth_radio::{
    BluetoothRadio, BluetoothRadioMemory, BluetoothRadioSink, RadioStep,
};
use oer_esp32s31_hal::{
    bluetooth::{
        BluetoothSchedulerHardwareListIndex, BluetoothSchedulerStop, BluetoothSchedulerStopStep,
    },
    shared_radio::ClientQuiescence,
};

use crate::{
    BluetoothOutcome,
    hardware::{BluetoothRadioHardware, RxChainPublicationError},
};

/// Delay before a hardware wait is observed again.
///
/// Scheduler commands, the stop sequence and the controller-time latch
/// settle within microseconds; a scheduler interrupt also ends the delay.
pub const HARDWARE_RECHECK: Duration = Duration::from_micros(20);

/// Longest time without a controller-time sample while the runtime is idle.
///
/// The radio extends the 32-bit controller clock from successive samples,
/// which needs a sample at least once per half clock range.
pub const TIME_REFRESH: Duration = Duration::from_secs(60);

/// Why the controller-time latch produced no sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTimeError {
    /// The latch refused a request.
    Request(ControllerTimeRequestError),
    /// The latch lost or refused the request in flight.
    Event(ControllerTimeEventError),
    /// The latch reported no request in flight.
    Lost,
}

/// Why a runtime operation did not run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothRuntimeError {
    /// No radio is installed.
    NotInstalled,
    /// The radio refused the request.
    Rejected(RequestError),
    /// No controller-time sample could be taken.
    Time(BluetoothTimeError),
    /// The runtime stopped on a hardware fault.
    Faulted,
}

/// Why the runtime stopped driving the radio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothRuntimeFault<E> {
    /// No radio is installed.
    NotInstalled,
    /// A list-zero step or observation failed.
    Scheduler(SchedulerHardwareError),
    /// The idle scheduler did not start.
    Start(SchedulerStartError<E>),
    /// The stop sequence could not reach the interrupt owner.
    Stop,
    /// No controller-time sample could be taken to resume.
    Time(BluetoothTimeError),
}

/// Why a radio was not installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothInstallError {
    /// A radio is already installed.
    AlreadyInstalled,
    /// The receive chains were not published.
    RxChains(RxChainPublicationError),
    /// No controller-time sample could be taken.
    Time(BluetoothTimeError),
}

/// The bounded outcome queue overflowed and dropped the outcomes it could not
/// hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothOutcomesLost;

type Radio<
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> = BluetoothRadio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>;

struct Installed<
    H,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> {
    radio: Radio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>,
    hardware: H,
    awaiting: Option<SchedulerWait>,
    faulted: bool,
}

/// What one locked pass left to do.
enum Pass {
    /// More work is ready now.
    Continue,
    /// A hardware wait is pending.
    Recheck,
    /// Nothing is ready until a wake, a request or the refresh deadline.
    Idle,
}

/// The sink of one locked entry: outcomes go to the queue.
struct QueueSink<'a, M: RawMutex, const EVENTS: usize> {
    outcomes: &'a Channel<M, BluetoothOutcome, EVENTS>,
    lost: &'a AtomicBool,
}

impl<M: RawMutex, const EVENTS: usize> BluetoothRadioSink for QueueSink<'_, M, EVENTS> {
    fn outcome(&mut self, outcome: RadioOutcome<'_>) {
        if self
            .outcomes
            .try_send(BluetoothOutcome::copy(outcome))
            .is_err()
        {
            self.lost.store(true, Ordering::Release);
        }
    }
}

/// The Bluetooth LE radio role, its hardware and the outcome queue.
///
/// [`Self::run`] drives the scheduler: it reports completions, inserts
/// admitted events and carries list transactions through their hardware
/// waits. [`Self::request`] admits one portable request with a fresh
/// controller-time sample. `EVENTS` bounds the outcomes waiting for the
/// consumer; overflow drops the newest outcome and reports
/// [`BluetoothOutcomesLost`] once.
#[allow(
    clippy::type_complexity,
    reason = "the role's pool capacities stay visible in the runtime type"
)]
pub struct BluetoothRuntime<
    M: RawMutex,
    H,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
> {
    installed: Mutex<
        M,
        Option<
            Installed<
                H,
                LEGACY,
                CONNECTABLE,
                SCANNERS,
                CONNECTIONS,
                SCAN_PACKETS,
                RX_PACKETS,
                ITEMS,
            >,
        >,
    >,
    outcomes: Channel<M, BluetoothOutcome, EVENTS>,
    lost: AtomicBool,
    work: Signal<M, ()>,
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
> Default
    for BluetoothRuntime<
        M,
        H,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
        EVENTS,
    >
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
>
    BluetoothRuntime<
        M,
        H,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
        EVENTS,
    >
{
    /// An empty runtime, suitable for a `static`.
    pub const fn new() -> Self {
        Self {
            installed: Mutex::new(None),
            outcomes: Channel::new(),
            lost: AtomicBool::new(false),
            work: Signal::new(),
        }
    }

    /// Publish the receive chains of `memory`, take the first controller-time
    /// sample and install the radio.
    ///
    /// This runs once per powered epoch, after the interrupt owner is
    /// published and before the first scheduler RUN.
    ///
    /// # Errors
    ///
    /// Returns the memory and the hardware unchanged apart from the chain
    /// publication.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "the no-alloc runtime returns the unconsumed owners by value"
    )]
    pub async fn install(
        &self,
        memory: BluetoothRadioMemory<
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
        >,
        mut hardware: H,
    ) -> Result<
        (),
        (
            BluetoothInstallError,
            BluetoothRadioMemory<
                LEGACY,
                CONNECTABLE,
                SCANNERS,
                CONNECTIONS,
                SCAN_PACKETS,
                RX_PACKETS,
            >,
            H,
        ),
    > {
        let mut installed = self.installed.lock().await;
        if installed.is_some() {
            return Err((BluetoothInstallError::AlreadyInstalled, memory, hardware));
        }
        if let Err(error) = hardware.publish_rx_chains(&memory.scanning, &memory.non_scanning) {
            return Err((BluetoothInstallError::RxChains(error), memory, hardware));
        }
        let sample = match sample_time(&mut hardware).await {
            Ok(sample) => sample,
            Err(error) => return Err((BluetoothInstallError::Time(error), memory, hardware)),
        };
        let radio = BluetoothRadio::new(
            memory,
            hardware.scheduler_config(),
            hardware.controller_time_scale(),
            &sample,
            hardware.local_sleep_clock_ppm(),
        );
        *installed = Some(Installed {
            radio,
            hardware,
            awaiting: None,
            faulted: false,
        });
        drop(installed);
        self.work.signal(());
        Ok(())
    }

    /// Admit one request against a fresh controller-time sample.
    ///
    /// # Errors
    ///
    /// No radio is installed, the runtime faulted, no time sample could be
    /// taken or the radio refused the request.
    pub async fn request(&self, request: RadioRequest<'_>) -> Result<(), BluetoothRuntimeError> {
        let mut installed = self.installed.lock().await;
        let installed = installed
            .as_mut()
            .ok_or(BluetoothRuntimeError::NotInstalled)?;
        if installed.faulted {
            return Err(BluetoothRuntimeError::Faulted);
        }
        let sample = sample_time(&mut installed.hardware)
            .await
            .map_err(BluetoothRuntimeError::Time)?;
        installed.radio.observe_time(&sample);
        let mut sink = self.sink();
        installed
            .radio
            .request(request, &mut sink)
            .map_err(BluetoothRuntimeError::Rejected)?;
        self.work.signal(());
        Ok(())
    }

    /// A fresh radio time and the radio's admission timing, for planning the
    /// next request.
    ///
    /// # Errors
    ///
    /// No radio is installed, the runtime faulted or no time sample could be
    /// taken.
    pub async fn clock(&self) -> Result<(RadioInstant, RadioTiming), BluetoothRuntimeError> {
        let mut installed = self.installed.lock().await;
        let installed = installed
            .as_mut()
            .ok_or(BluetoothRuntimeError::NotInstalled)?;
        if installed.faulted {
            return Err(BluetoothRuntimeError::Faulted);
        }
        let sample = sample_time(&mut installed.hardware)
            .await
            .map_err(BluetoothRuntimeError::Time)?;
        installed.radio.observe_time(&sample);
        Ok((installed.radio.now(), installed.radio.timing()))
    }

    /// The platform's scheduler interrupt published a wake for the worker.
    pub fn on_scheduler_wake(&self) {
        self.work.signal(());
    }

    /// Wait for the next outcome.
    ///
    /// # Errors
    ///
    /// Reports once that the queue overflowed before the outcomes after it.
    pub async fn next_outcome(&self) -> Result<BluetoothOutcome, BluetoothOutcomesLost> {
        if self.lost.swap(false, Ordering::AcqRel) {
            return Err(BluetoothOutcomesLost);
        }
        Ok(self.outcomes.receive().await)
    }

    /// Drive the radio until a hardware fault stops it.
    ///
    /// Run this on one task for the whole powered epoch. Between passes the
    /// lock is free for requests.
    pub async fn run(&self) -> BluetoothRuntimeFault<H::StartError> {
        loop {
            let pass = {
                let mut installed = self.installed.lock().await;
                let Some(installed) = installed.as_mut() else {
                    return BluetoothRuntimeFault::NotInstalled;
                };
                if !installed.faulted
                    && let Ok(sample) = sample_time(&mut installed.hardware).await
                {
                    installed.radio.observe_time(&sample);
                }
                match self.pass(installed) {
                    Ok(pass) => pass,
                    Err(fault) => {
                        installed.faulted = true;
                        return fault;
                    }
                }
            };
            match pass {
                Pass::Continue => {}
                Pass::Recheck => {
                    select(self.work.wait(), Timer::after(HARDWARE_RECHECK)).await;
                }
                Pass::Idle => {
                    select(self.work.wait(), Timer::after(TIME_REFRESH)).await;
                }
            }
        }
    }

    /// Stop the scheduler, hand the Bluetooth quiescence proof to
    /// `maintenance` and resume the scheduler afterwards.
    ///
    /// A list transaction in progress finishes first. On resume, a fresh
    /// controller-time sample decides which listed events have passed; they
    /// end as not executed, and the next pass restarts the scheduler at the
    /// first remaining event.
    ///
    /// # Errors
    ///
    /// No radio is installed, or a hardware fault stopped the runtime.
    pub async fn quiesce<T>(
        &self,
        maintenance: impl FnOnce(ClientQuiescence<'_>) -> T,
    ) -> Result<T, BluetoothRuntimeFault<H::StartError>> {
        let mut installed = self.installed.lock().await;
        let installed = installed
            .as_mut()
            .ok_or(BluetoothRuntimeFault::NotInstalled)?;
        let result = self.quiesce_installed(installed, maintenance).await;
        if result.is_err() {
            installed.faulted = true;
        }
        self.work.signal(());
        result
    }

    async fn quiesce_installed<T>(
        &self,
        installed: &mut Installed<
            H,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
        >,
        maintenance: impl FnOnce(ClientQuiescence<'_>) -> T,
    ) -> Result<T, BluetoothRuntimeFault<H::StartError>> {
        while installed.awaiting.is_some() {
            if let Pass::Recheck = self.pass(installed)? {
                Timer::after(HARDWARE_RECHECK).await;
            }
        }
        let mut stop = BluetoothSchedulerStop::default();
        let stopped = loop {
            match installed.hardware.step_stop(stop) {
                Ok(BluetoothSchedulerStopStep::Stopped(stopped)) => break stopped,
                Ok(BluetoothSchedulerStopStep::Pending(pending)) => {
                    stop = pending;
                    Timer::after(HARDWARE_RECHECK).await;
                }
                Err(_) => return Err(BluetoothRuntimeFault::Stop),
            }
        };
        let mut sink = self.sink();
        if installed
            .hardware
            .capture_stopped_finished_lists(&stopped)
            .is_ok()
        {
            drain_finished_lists(installed, &mut sink);
        }
        if installed.radio.enter_stopped(stopped).is_err() {
            return Err(BluetoothRuntimeFault::Stop);
        }
        let result = maintenance(
            installed
                .radio
                .quiescence()
                .expect("the radio holds the stopped receipt"),
        );
        // The radio stays stopped, and the runtime faulted, without a sample
        // to judge which events have passed.
        let sample = sample_time(&mut installed.hardware)
            .await
            .map_err(BluetoothRuntimeFault::Time)?;
        installed
            .radio
            .resume(&sample)
            .expect("the radio holds the stopped receipt");
        Ok(result)
    }

    fn sink(&self) -> QueueSink<'_, M, EVENTS> {
        QueueSink {
            outcomes: &self.outcomes,
            lost: &self.lost,
        }
    }

    /// One locked pass: report completions, then advance the transaction in
    /// progress or drive the next hardware work.
    fn pass(
        &self,
        installed: &mut Installed<
            H,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
        >,
    ) -> Result<Pass, BluetoothRuntimeFault<H::StartError>> {
        let mut sink = self.sink();
        if let Some(wake) = installed.hardware.take_wake()
            && installed.hardware.capture_finished_lists(wake).is_ok()
        {
            drain_finished_lists(installed, &mut sink);
        }
        if installed.faulted {
            return Ok(Pass::Idle);
        }
        let step = match installed.awaiting {
            Some(wait) => {
                let observation = installed
                    .hardware
                    .observe_wait(wait)
                    .map_err(BluetoothRuntimeFault::Scheduler)?;
                match installed.radio.advance(observation, &mut sink) {
                    Ok(step) => step,
                    Err(fault) => {
                        installed.hardware.recover(fault);
                        installed.awaiting = None;
                        return Ok(Pass::Idle);
                    }
                }
            }
            None => {
                let view = installed
                    .hardware
                    .observe()
                    .map_err(BluetoothRuntimeFault::Scheduler)?;
                installed.radio.drive(view, &mut sink)
            }
        };
        match step {
            RadioStep::Idle => Ok(Pass::Idle),
            RadioStep::Start(insertion) => {
                installed
                    .hardware
                    .start(&installed.radio.item_space(), insertion)
                    .map_err(BluetoothRuntimeFault::Start)?;
                Ok(Pass::Continue)
            }
            RadioStep::Transaction(step) => {
                installed
                    .hardware
                    .perform(&installed.radio.item_space(), &step)
                    .map_err(BluetoothRuntimeFault::Scheduler)?;
                match step.next() {
                    SchedulerNext::Finished => {
                        installed.awaiting = None;
                        Ok(Pass::Continue)
                    }
                    SchedulerNext::Await(wait) => {
                        installed.awaiting = Some(wait);
                        Ok(if step.actions().next().is_some() {
                            Pass::Continue
                        } else {
                            Pass::Recheck
                        })
                    }
                }
            }
        }
    }
}

/// Report the events of every captured finished list zero.
fn drain_finished_lists<
    H: BluetoothRadioHardware,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
>(
    installed: &mut Installed<
        H,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
    >,
    sink: &mut impl BluetoothRadioSink,
) {
    loop {
        match installed.hardware.next_finished_list() {
            SchedulerFinishedListWorkerStep::Idle | SchedulerFinishedListWorkerStep::Complete => {
                return;
            }
            SchedulerFinishedListWorkerStep::List { observed, more } => {
                if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO {
                    installed.radio.complete(sink);
                }
                if !more {
                    return;
                }
            }
        }
    }
}

/// Take one controller-time sample, draining a request a cancelled caller
/// abandoned first.
async fn sample_time(
    hardware: &mut impl BluetoothRadioHardware,
) -> Result<ControllerTimeSample, BluetoothTimeError> {
    let request = match hardware.request_time() {
        Ok(request) => request,
        Err(ControllerTimeRequestError::Busy) => {
            while let ControllerTimeEventStep::Waiting =
                hardware.drain_time().map_err(BluetoothTimeError::Event)?
            {
                Timer::after(HARDWARE_RECHECK).await;
            }
            hardware
                .request_time()
                .map_err(BluetoothTimeError::Request)?
        }
        Err(error) => return Err(BluetoothTimeError::Request(error)),
    };
    loop {
        match hardware
            .recheck_time(request)
            .map_err(BluetoothTimeError::Event)?
        {
            ControllerTimeEventStep::Sample { sample, .. } => return Ok(sample),
            ControllerTimeEventStep::Waiting => Timer::after(HARDWARE_RECHECK).await,
            ControllerTimeEventStep::Idle | ControllerTimeEventStep::OrphanDrained => {
                return Err(BluetoothTimeError::Lost);
            }
        }
    }
}
