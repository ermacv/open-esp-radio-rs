//! The radio role and its hardware behind one async lock.

use core::{
    cell::Cell,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_futures::select::select;
use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::RawMutex},
    channel::Channel,
    mutex::Mutex,
    signal::Signal,
};
use oer_bluetooth_radio::{
    EventsLost, FailureClass, LeRadioCapabilities, PortError, RadioActivity, RadioInstant,
    RadioOutcome, RadioRequest, RadioTiming, RequestError,
};
use oer_esp32s31_bluetooth::{
    ControllerTimeSample,
    controller_time::{
        ControllerTimeEventError, ControllerTimeEventStep, ControllerTimeRequestError,
    },
    scheduler::{
        SchedulerFinishedListWorkerStep, SchedulerHardwareError, SchedulerNext,
        SchedulerStartError, SchedulerStopError, SchedulerWait,
    },
};
use oer_esp32s31_bluetooth_radio::{
    BluetoothRadio, BluetoothRadioMemory, BluetoothRadioSink, CoexistenceProfile, RadioStep,
};
use oer_esp32s31_hal::{
    bluetooth::{
        BluetoothSchedulerHardwareListIndex, BluetoothSchedulerStop, BluetoothSchedulerStopStep,
    },
    shared_radio::ClientQuiescence,
};
use oer_time::{Duration, Timer};

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

impl PortError for BluetoothRuntimeError {
    fn class(&self) -> FailureClass {
        match self {
            // Not installed is a state an install ends.
            Self::NotInstalled | Self::Rejected(_) => FailureClass::Rejected,
            // The latch may take the next sample.
            Self::Time(_) => FailureClass::Recoverable,
            Self::Faulted => FailureClass::Poisoned,
        }
    }
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
    /// The stop sequence ended before the scheduler stopped.
    StopSequence(SchedulerStopError),
    /// The radio refused the stopped receipt.
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
    H: BluetoothRadioHardware,
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
    /// The BLE PHY ETM route a test session disabled; restored when the
    /// session ends, when the runtime faults and when the radio leaves.
    phy_route: Option<H::DisabledPhyRoute>,
    faulted: bool,
}

/// What one locked pass left to do.
impl<
    H: BluetoothRadioHardware,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> Installed<H, LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>
{
    /// Restore the BLE PHY ETM route if a test session still holds it.
    fn restore_phy_route(&mut self) {
        if let Some(route) = self.phy_route.take() {
            self.hardware.restore_phy_etm_route(route);
        }
    }

    /// Stop driving the radio after a hardware fault.
    fn fault(&mut self) {
        self.faulted = true;
        self.restore_phy_route();
    }
}

enum Pass {
    /// The radio asked for a scheduler stop.
    Stop,
    /// More work is ready now.
    Continue,
    /// A hardware wait is pending.
    Recheck,
    /// Nothing is ready until a wake, a request or the refresh deadline.
    Idle,
}

/// The bounded outcome queue: overflow is reported in its order, a loss
/// marker taking the place of the first dropped outcome.
struct OutcomeQueue<M: RawMutex, const EVENTS: usize> {
    entries: Channel<M, Result<BluetoothOutcome, EventsLost>, EVENTS>,
    /// An outcome was dropped and its marker is not queued yet.
    lost: BlockingMutex<M, Cell<bool>>,
}

impl<M: RawMutex, const EVENTS: usize> OutcomeQueue<M, EVENTS> {
    const fn new() -> Self {
        Self {
            entries: Channel::new(),
            lost: BlockingMutex::new(Cell::new(false)),
        }
    }

    fn push(&self, outcome: BluetoothOutcome) {
        self.lost.lock(|lost| {
            if lost.get() {
                if self.entries.try_send(Err(EventsLost)).is_err() {
                    return;
                }
                lost.set(false);
            }
            if self.entries.try_send(Ok(outcome)).is_err() {
                lost.set(true);
            }
        });
    }

    fn take(&self) -> Option<Result<BluetoothOutcome, EventsLost>> {
        self.lost.lock(|lost| match self.entries.try_receive() {
            Ok(entry) => Some(entry),
            Err(_) if lost.replace(false) => Some(Err(EventsLost)),
            Err(_) => None,
        })
    }
}

/// The sink of one locked entry: outcomes go to the queue, and a fault
/// poisons the runtime.
struct QueueSink<'a, M: RawMutex, const EVENTS: usize> {
    outcomes: &'a OutcomeQueue<M, EVENTS>,
    poisoned: &'a AtomicBool,
    changed: &'a Signal<M, ()>,
}

impl<M: RawMutex, const EVENTS: usize> BluetoothRadioSink for QueueSink<'_, M, EVENTS> {
    fn outcome(&mut self, outcome: RadioOutcome<'_>) {
        let copied = BluetoothOutcome::copy(outcome);
        // A copy that could not hold its PDU is a fault as well.
        if let BluetoothOutcome::Fault(_) = copied {
            self.poisoned.store(true, Ordering::Release);
            self.changed.signal(());
        }
        self.outcomes.push(copied);
    }
}

/// The Bluetooth LE radio role, its hardware and the outcome queue.
///
/// [`Self::run`] drives the scheduler: it reports completions, inserts
/// admitted events and carries list transactions through their hardware
/// waits; it is the runner the composition polls beside the consumer of
/// [`Self::next_outcome`], which only takes outcomes. [`Self::request`]
/// admits one portable request with a fresh controller-time sample.
/// `EVENTS` bounds the outcomes waiting for the consumer, a loss marker
/// included; overflow drops the newest outcome and reports [`EventsLost`]
/// once in its place.
///
/// A hardware fault of the radio or the runner poisons the runtime: the
/// queued outcomes, the fault's cause among them, are still reported, then
/// [`BluetoothOutcome::Poisoned`] at every call, and requests fail as
/// [`BluetoothRuntimeError::Faulted`] until a new install.
#[allow(
    clippy::type_complexity,
    reason = "the role's pool capacities stay visible in the runtime type"
)]
pub struct BluetoothRuntime<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T,
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
    outcomes: OutcomeQueue<M, EVENTS>,
    /// The radio or the runner faulted; a new install clears it.
    poisoned: AtomicBool,
    /// Raised when the consumer must look again without a new outcome.
    changed: Signal<M, ()>,
    work: Signal<M, ()>,
    activity: Signal<M, RadioActivity>,
    /// Whether another radio shares the antenna.
    shared: AtomicBool,
    /// The image's monotonic time the runner's rechecks wait on.
    timer: T,
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T: Timer + Default,
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
        T,
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
        Self::new(T::default())
    }
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T: Timer,
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
        T,
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
    /// An empty runtime whose waits are on `timer`, suitable for a
    /// `static`.
    pub const fn new(timer: T) -> Self {
        Self {
            timer,
            installed: Mutex::new(None),
            outcomes: OutcomeQueue::new(),
            poisoned: AtomicBool::new(false),
            changed: Signal::new(),
            work: Signal::new(),
            activity: Signal::new(),
            shared: AtomicBool::new(false),
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
        hardware.publish_device_table(memory.device_table.publication());
        let sample = match sample_time(&self.timer, &mut hardware).await {
            Ok(sample) => sample,
            Err(error) => return Err((BluetoothInstallError::Time(error), memory, hardware)),
        };
        let mut radio = BluetoothRadio::new(
            memory,
            hardware.scheduler_config(),
            hardware.controller_time_scale(),
            &sample,
            hardware.local_sleep_clock_ppm(),
        );
        radio.set_coexistence(self.coexistence());
        *installed = Some(Installed {
            radio,
            hardware,
            awaiting: None,
            phy_route: None,
            faulted: false,
        });
        drop(installed);
        self.poisoned.store(false, Ordering::Release);
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
        // A faulted radio is poisoned, not a refusal of this request.
        if installed.faulted || installed.radio.is_faulted() {
            return Err(BluetoothRuntimeError::Faulted);
        }
        let sample = sample_time(&self.timer, &mut installed.hardware)
            .await
            .map_err(BluetoothRuntimeError::Time)?;
        installed.radio.observe_time(&sample);
        let starts_scanner = matches!(request, RadioRequest::ConfigureScanner(_));
        let changes_list = matches!(request, RadioRequest::FilterAcceptList(_));
        let mut sink = self.sink();
        installed
            .radio
            .request(request, &mut sink)
            .map_err(BluetoothRuntimeError::Rejected)?;
        if starts_scanner {
            installed.hardware.publish_scan_start();
        }
        if changes_list {
            let publication = installed.radio.device_table_publication();
            installed.hardware.publish_device_table(publication);
        }
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
        if installed.faulted || installed.radio.is_faulted() {
            return Err(BluetoothRuntimeError::Faulted);
        }
        let sample = sample_time(&self.timer, &mut installed.hardware)
            .await
            .map_err(BluetoothRuntimeError::Time)?;
        installed.radio.observe_time(&sample);
        Ok((installed.radio.now(), installed.radio.timing()))
    }

    /// What the radio serves: its roles on LE 1M, Direct Test Mode and
    /// connections whose acknowledgement the hardware runs.
    pub const fn capabilities(&self) -> LeRadioCapabilities {
        Radio::<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>::CAPABILITIES
    }

    /// The platform's scheduler interrupt published a wake for the worker.
    pub fn on_scheduler_wake(&self) {
        self.work.signal(());
    }

    /// Take the next outcome: the queued ones in their order, then the
    /// terminal [`BluetoothOutcome::Poisoned`] of a poisoned runtime.
    ///
    /// # Errors
    ///
    /// Reports once, in place of the first dropped outcome, that the queue
    /// overflowed.
    pub async fn next_outcome(&self) -> Result<BluetoothOutcome, EventsLost> {
        loop {
            if let Some(outcome) = self.outcomes.take() {
                return outcome;
            }
            if self.poisoned.load(Ordering::Acquire) {
                return Ok(BluetoothOutcome::Poisoned);
            }
            select(
                self.outcomes.entries.ready_to_receive(),
                self.changed.wait(),
            )
            .await;
        }
    }

    /// Poison the runtime after a fault of the runner.
    fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
        self.changed.signal(());
    }

    /// Follow whether another radio shares the antenna. The installed radio
    /// and every later installation use the matching coexistence profile.
    pub async fn set_coexistence(&self, shared: bool) {
        self.shared.store(shared, Ordering::Release);
        let mut installed = self.installed.lock().await;
        if let Some(installed) = installed.as_mut() {
            installed.radio.set_coexistence(self.coexistence());
        }
    }

    fn coexistence(&self) -> CoexistenceProfile {
        if self.shared.load(Ordering::Acquire) {
            CoexistenceProfile::Shared
        } else {
            CoexistenceProfile::Standalone
        }
    }

    /// Publish the roles the Controller has active. Only the latest summary
    /// is kept for [`Self::next_activity`].
    pub fn publish_activity(&self, activity: RadioActivity) {
        self.activity.signal(activity);
    }

    /// Forget an unread summary; the next epoch starts with no role active.
    pub fn clear_activity(&self) {
        self.activity.reset();
    }

    /// Wait for the next change of the active roles; a burst of changes
    /// yields only the latest.
    pub async fn next_activity(&self) -> RadioActivity {
        self.activity.wait().await
    }

    /// Drive the radio until a hardware fault stops it.
    ///
    /// Run this on one task for the whole powered epoch. Between passes the
    /// lock is free for requests.
    pub async fn run(&self) -> BluetoothRuntimeFault<H::StartError> {
        let mut budget = ContinueBudget::new();
        loop {
            let pass = {
                let mut installed = self.installed.lock().await;
                let Some(installed) = installed.as_mut() else {
                    return BluetoothRuntimeFault::NotInstalled;
                };
                if !installed.faulted
                    && let Ok(sample) = sample_time(&self.timer, &mut installed.hardware).await
                {
                    installed.radio.observe_time(&sample);
                }
                match self.pass(installed) {
                    // A test event is published only on an idle scheduler, and
                    // a listed one leaves its list only after a stop; resuming
                    // restarts at the remaining events.
                    Ok(Pass::Stop) => match self.quiesce_installed(installed, |_| ()).await {
                        Ok(()) => Pass::Continue,
                        Err(fault) => {
                            installed.fault();
                            self.poison();
                            return fault;
                        }
                    },
                    Ok(pass) => pass,
                    Err(fault) => {
                        installed.fault();
                        self.poison();
                        return fault;
                    }
                }
            };
            match pass {
                Pass::Stop | Pass::Continue => budget.spend().await,
                Pass::Recheck => {
                    budget.refill();
                    select(self.work.wait(), wait_for(&self.timer, HARDWARE_RECHECK)).await;
                }
                Pass::Idle => {
                    budget.refill();
                    select(self.work.wait(), wait_for(&self.timer, TIME_REFRESH)).await;
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
    pub async fn quiesce<R>(
        &self,
        maintenance: impl FnOnce(ClientQuiescence<'_>) -> R,
    ) -> Result<R, BluetoothRuntimeFault<H::StartError>> {
        let mut installed = self.installed.lock().await;
        let installed = installed
            .as_mut()
            .ok_or(BluetoothRuntimeFault::NotInstalled)?;
        let result = self.quiesce_installed(installed, maintenance).await;
        if result.is_err() {
            installed.fault();
            self.poison();
        }
        self.work.signal(());
        result
    }

    /// Stop the scheduler, take the radio and its hardware out of the
    /// runtime, and stop the runner.
    ///
    /// A list transaction in progress finishes first, and a controller-time
    /// request that a cancelled [`Self::run`] left in flight is drained.
    /// Events still listed stay with the stopped radio, which returns its
    /// memory only after the Controller reset
    /// ([`BluetoothRadio::into_memory`]); outcomes already queued stay
    /// readable. [`Self::run`] then returns
    /// [`BluetoothRuntimeFault::NotInstalled`].
    ///
    /// # Errors
    ///
    /// No radio is installed, or the scheduler could not be stopped; the
    /// radio then stays installed and faulted.
    #[allow(
        clippy::type_complexity,
        reason = "the role's pool capacities stay visible in the returned radio"
    )]
    pub async fn uninstall(
        &self,
    ) -> Result<
        (
            Radio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>,
            H,
        ),
        BluetoothRuntimeFault<H::StartError>,
    > {
        let mut slot = self.installed.lock().await;
        let installed = slot.as_mut().ok_or(BluetoothRuntimeFault::NotInstalled)?;
        if let Err(fault) = self.stop_scheduler(installed).await {
            installed.fault();
            self.poison();
            return Err(fault);
        }
        // The hardware leaves with its PHY routed as initialization left it.
        installed.restore_phy_route();
        // A cancelled runner may have left a controller-time request in
        // flight; the task owner retires only once it completed.
        loop {
            match installed.hardware.drain_time() {
                Ok(ControllerTimeEventStep::Waiting) => {
                    wait_for(&self.timer, HARDWARE_RECHECK).await
                }
                Ok(_) => break,
                Err(error) => {
                    installed.fault();
                    self.poison();
                    return Err(BluetoothRuntimeFault::Time(BluetoothTimeError::Event(
                        error,
                    )));
                }
            }
        }
        let Installed {
            radio, hardware, ..
        } = slot.take().expect("the radio is installed");
        drop(slot);
        self.work.signal(());
        Ok((radio, hardware))
    }

    async fn quiesce_installed<R>(
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
        maintenance: impl FnOnce(ClientQuiescence<'_>) -> R,
    ) -> Result<R, BluetoothRuntimeFault<H::StartError>> {
        self.stop_scheduler(installed).await?;
        let result = maintenance(
            installed
                .radio
                .quiescence()
                .expect("the radio holds the stopped receipt"),
        );
        // The radio stays stopped, and the runtime faulted, without a sample
        // to judge which events have passed.
        let sample = sample_time(&self.timer, &mut installed.hardware)
            .await
            .map_err(BluetoothRuntimeFault::Time)?;
        installed
            .radio
            .resume(&sample)
            .expect("the radio holds the stopped receipt");
        Ok(result)
    }

    /// Finish the list transaction in progress, stop the scheduler, report
    /// what it finished and hand the stopped receipt to the radio.
    async fn stop_scheduler(
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
    ) -> Result<(), BluetoothRuntimeFault<H::StartError>> {
        while installed.awaiting.is_some() {
            if let Pass::Recheck = self.pass(installed)? {
                wait_for(&self.timer, HARDWARE_RECHECK).await;
            }
        }
        let mut stop = BluetoothSchedulerStop::default();
        let stopped = loop {
            match installed.hardware.step_stop(stop) {
                Ok(BluetoothSchedulerStopStep::Stopped(stopped)) => break stopped,
                Ok(BluetoothSchedulerStopStep::Pending(pending)) => {
                    stop = pending;
                    wait_for(&self.timer, HARDWARE_RECHECK).await;
                }
                Err(error) => return Err(BluetoothRuntimeFault::StopSequence(error)),
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
        installed
            .radio
            .enter_stopped(stopped)
            .map_err(|_| BluetoothRuntimeFault::Stop)
    }

    fn sink(&self) -> QueueSink<'_, M, EVENTS> {
        QueueSink {
            outcomes: &self.outcomes,
            poisoned: &self.poisoned,
            changed: &self.changed,
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
            RadioStep::StopScheduler => Ok(Pass::Stop),
            RadioStep::EnterTest(insertion) => {
                let route = installed.hardware.disable_phy_etm_route();
                installed.phy_route = Some(route);
                installed
                    .hardware
                    .start(&installed.radio.item_space(), insertion)
                    .map_err(BluetoothRuntimeFault::Start)?;
                Ok(Pass::Continue)
            }
            RadioStep::LeaveTest => {
                installed.restore_phy_route();
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
/// Consecutive passes the runner may continue before it yields.
const CONTINUE_BUDGET: u8 = 16;

/// Bounds how long the runner keeps the executor without awaiting.
///
/// A pass that continues does not await, and its time sample awaits only
/// while the latch is pending. After a fixed number of such passes the
/// runner yields once, so sibling tasks on the same executor always make
/// progress.
struct ContinueBudget(u8);

impl ContinueBudget {
    const fn new() -> Self {
        Self(CONTINUE_BUDGET)
    }

    /// Account one continued pass and yield when the budget is spent.
    async fn spend(&mut self) {
        self.0 -= 1;
        if self.0 == 0 {
            self.refill();
            embassy_futures::yield_now().await;
        }
    }

    /// Start a new budget after the runner awaited.
    fn refill(&mut self) {
        self.0 = CONTINUE_BUDGET;
    }
}

/// Wait for `duration` on `timer`; a deadline past the timer's range never
/// ends.
pub(crate) async fn wait_for(timer: &impl Timer, duration: Duration) {
    if timer.wait_for(duration).await.is_err() {
        core::future::pending::<()>().await;
    }
}

async fn sample_time(
    timer: &impl Timer,
    hardware: &mut impl BluetoothRadioHardware,
) -> Result<ControllerTimeSample, BluetoothTimeError> {
    let request = match hardware.request_time() {
        Ok(request) => request,
        Err(ControllerTimeRequestError::Busy) => {
            while let ControllerTimeEventStep::Waiting =
                hardware.drain_time().map_err(BluetoothTimeError::Event)?
            {
                wait_for(timer, HARDWARE_RECHECK).await;
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
            ControllerTimeEventStep::Waiting => wait_for(timer, HARDWARE_RECHECK).await,
            ControllerTimeEventStep::Idle | ControllerTimeEventStep::OrphanDrained => {
                return Err(BluetoothTimeError::Lost);
            }
        }
    }
}

#[cfg(test)]
mod tests;
