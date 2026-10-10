//! The radio role and its hardware behind one async lock.

use core::{
    cell::{Cell, RefCell},
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::RawMutex},
    channel::Channel,
    mutex::Mutex,
    signal::Signal,
};
use oer_bluetooth_radio::{
    CancelError, ClockError, EventId, EventsLost, LeInstant, LeRadioCapabilities, LifecycleCommand,
    LifecycleError, LifecycleEvent, Poisoned, PortResult, RadioActivity, RadioOutcome,
    RadioRequest, RequestError,
};
use oer_esp32s31_bluetooth::{
    ControllerTimeSample,
    controller_time::{
        ControllerTimeEventError, ControllerTimeEventStep, ControllerTimeRequestError,
    },
    scheduler::SchedulerSoftwareConfig,
    scheduler::{
        SchedulerFinishedListWorkerStep, SchedulerHardwareError, SchedulerNext,
        SchedulerStartError, SchedulerStopError, SchedulerWait,
    },
};
use oer_esp32s31_bluetooth_radio::{
    BluetoothRadio, BluetoothRadioMemory, BluetoothRadioSink, CoexistenceProfile, RadioFault,
    RadioStep,
};
use oer_esp32s31_hal::{
    bluetooth::{
        BluetoothSchedulerHardwareListIndex, BluetoothSchedulerStop, BluetoothSchedulerStopStep,
    },
    shared_radio::ClientQuiescence,
};
use oer_time::{Duration, Timer};

use crate::port::{BluetoothControl, BluetoothPort};
use crate::{
    BluetoothOutcome,
    hardware::{BluetoothRadioHardware, RxChainPublicationError},
};

/// Delay before a hardware wait is observed again.
///
/// Scheduler commands, the stop sequence and the controller-time latch
/// settle within microseconds; a scheduler interrupt also ends the delay.
pub const HARDWARE_RECHECK: Duration = Duration::from_micros(20);

/// The invariant of every port call: the port exists only while its radio
/// is installed.
const INSTALLED: &str = "the port exists only while its radio is installed";

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
    /// The runtime's timer cannot express a wait's deadline.
    Deadline,
}

/// Why the runtime is poisoned: the cause its port reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothFault<E> {
    /// The radio faulted.
    Radio(RadioFault),
    /// The runner stopped on a hardware fault.
    Runner(BluetoothRuntimeFault<E>),
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
    /// A finished-list observation could not be captured losslessly.
    FinishedLists(oer_esp32s31_bluetooth::scheduler::SchedulerFinishedListCaptureError),
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
    /// The hardware's scheduler policy or sleep-clock accuracy is not the
    /// one the runtime states its timing for.
    TimingMismatch,
}

pub(crate) type Radio<
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

/// The port's admission of events: its lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Admission {
    Disabled,
    Enabled,
    /// Every admitted event was cancelled; `Disabled` follows their ends.
    Disabling,
    /// Admission is closed; `Quiesced` follows the admitted events' ends.
    Quiescing,
    Quiesced,
}

/// What the outcome queue owes the consumer; `CONNECTIONS` bounds the
/// connection events admitted at once.
struct Owed<const CONNECTIONS: usize> {
    /// A received PDU was dropped: the next outcome reports the loss first.
    lost: bool,
    /// An outcome taken after the loss it reported.
    held: Option<BluetoothOutcome>,
    /// Admitted events whose end is not queued yet.
    events: usize,
    /// Slots reserved for outcomes that are never dropped: two for each
    /// admitted event (its end, and the acknowledgement or test report
    /// before it), the received PDUs an admitted connection event may still
    /// report, and one for a deferred lifecycle terminal.
    reserved: usize,
    /// Each admitted connection event and the slots it still holds for the
    /// data PDUs the hardware acknowledges.
    data: [Option<(EventId, usize)>; CONNECTIONS],
    /// The acknowledgement or test report of the event about to end took
    /// one of its reserved slots.
    companion: bool,
    admission: Admission,
    /// Counts the lifecycle commands that started: a closing admission and
    /// the terminal it reaches share one generation. It survives a reset.
    generation: u32,
}

/// One queued outcome, with whether a loss precedes it.
struct Entry {
    lost_before: bool,
    outcome: BluetoothOutcome,
}

/// The bounded outcome queue. An event's admission reserves the slots of
/// its terminal outcomes and, for a connection event, of the data PDUs the
/// hardware acknowledges, so none of them is lost: the Link Layer promises
/// its peer reliable delivery. A full queue drops only the received PDUs
/// nothing acknowledges (advertising and scan reports) and reports the loss
/// in place of the first dropped one.
struct OutcomeQueue<M: RawMutex, const EVENTS: usize, const CONNECTIONS: usize> {
    entries: Channel<M, Entry, EVENTS>,
    owed: BlockingMutex<M, RefCell<Owed<CONNECTIONS>>>,
}

impl<M: RawMutex, const EVENTS: usize, const CONNECTIONS: usize>
    OutcomeQueue<M, EVENTS, CONNECTIONS>
{
    const fn new() -> Self {
        Self {
            entries: Channel::new(),
            owed: BlockingMutex::new(RefCell::new(Owed {
                lost: false,
                held: None,
                events: 0,
                reserved: 0,
                data: [None; CONNECTIONS],
                companion: false,
                admission: Admission::Disabled,
                generation: 0,
            })),
        }
    }

    fn owed<R>(&self, entry: impl FnOnce(&mut Owed<CONNECTIONS>) -> R) -> R {
        self.owed.lock(|owed| entry(&mut owed.borrow_mut()))
    }

    fn admission(&self) -> Admission {
        self.owed(|owed| owed.admission)
    }

    /// The admission and the generation of the command that set it.
    fn lifecycle_state(&self) -> (Admission, u32) {
        self.owed(|owed| (owed.admission, owed.generation))
    }

    /// Whether `slots` more terminal outcomes fit beside the queued and
    /// reserved ones.
    fn has_room(&self, owed: &Owed<CONNECTIONS>, slots: usize) -> bool {
        self.entries.len() + owed.reserved + slots <= EVENTS
    }

    /// Reserve the terminal slots of event `id` before its admission, and
    /// `data` more for the data PDUs a connection event may receive;
    /// `false` when they do not fit.
    fn admit_event(&self, id: EventId, data: usize) -> bool {
        self.owed(|owed| {
            let free = owed.data.iter().position(Option::is_none);
            if !self.has_room(owed, 2 + data) || (data > 0 && free.is_none()) {
                return false;
            }
            if data > 0 {
                owed.data[free.expect("checked above")] = Some((id, data));
            }
            owed.reserved += 2 + data;
            owed.events += 1;
            true
        })
    }

    /// Return the slots of event `id`, which the radio refused.
    fn withdraw_event(&self, id: EventId) {
        self.owed(|owed| {
            owed.reserved -= 2 + Self::release_data(owed, id);
            owed.events -= 1;
        });
    }

    /// Forget the data slots event `id` still holds; how many.
    fn release_data(owed: &mut Owed<CONNECTIONS>, id: EventId) -> usize {
        owed.data
            .iter_mut()
            .find(|slot| slot.is_some_and(|(event, _)| event == id))
            .and_then(Option::take)
            .map_or(0, |(_, left)| left)
    }

    /// Queue a lifecycle terminal now, in a free slot; `false` when none is
    /// free.
    fn lifecycle_now(&self, terminal: LifecycleEvent, admission: Admission) -> bool {
        self.owed(|owed| {
            if !self.has_room(owed, 1) {
                return false;
            }
            owed.admission = admission;
            owed.generation = owed.generation.wrapping_add(1);
            self.send(owed, BluetoothOutcome::Lifecycle(terminal));
            true
        })
    }

    /// Close admission towards `admission` (`Disabling` or `Quiescing`),
    /// reserving the slot of its terminal; `false` when none is free.
    fn begin(&self, admission: Admission) -> bool {
        self.owed(|owed| {
            if !self.has_room(owed, 1) {
                return false;
            }
            owed.reserved += 1;
            owed.admission = admission;
            owed.generation = owed.generation.wrapping_add(1);
            true
        })
    }

    /// Report the terminal of a closing admission once no admitted event is
    /// left; whether it was reported.
    fn finish_if_idle(&self) -> bool {
        self.owed(|owed| self.finish(owed))
    }

    fn finish(&self, owed: &mut Owed<CONNECTIONS>) -> bool {
        if owed.events != 0 {
            return false;
        }
        let (terminal, admission) = match owed.admission {
            Admission::Disabling => (LifecycleEvent::Disabled, Admission::Disabled),
            Admission::Quiescing => (LifecycleEvent::Quiesced, Admission::Quiesced),
            _ => return false,
        };
        owed.reserved -= 1;
        owed.admission = admission;
        self.send(owed, BluetoothOutcome::Lifecycle(terminal));
        true
    }

    /// Queue `outcome`: a terminal one in the slot reserved for it, a
    /// received PDU when a slot is free. Whether a closing admission ended.
    fn push(&self, outcome: BluetoothOutcome) -> bool {
        self.owed(|owed| {
            match &outcome {
                BluetoothOutcome::Received { id, .. } => {
                    let data = owed
                        .data
                        .iter_mut()
                        .flatten()
                        .find(|(event, left)| *event == *id && *left > 0);
                    if let Some((_, left)) = data {
                        // Acknowledged connection data: its reserved slot.
                        *left -= 1;
                        owed.reserved -= 1;
                    } else if !self.has_room(owed, 1) {
                        owed.lost = true;
                        return false;
                    }
                }
                BluetoothOutcome::TransmitAcknowledged(_) | BluetoothOutcome::TestReport { .. } => {
                    owed.reserved -= 1;
                    owed.companion = true;
                }
                BluetoothOutcome::EventEnded { id, .. } => {
                    let unused_data = Self::release_data(owed, *id);
                    owed.reserved -= unused_data
                        + if core::mem::take(&mut owed.companion) {
                            1
                        } else {
                            2
                        };
                    owed.events -= 1;
                }
                // The command found or reserved its slot.
                BluetoothOutcome::Lifecycle(_) => {}
            }
            let ended = matches!(outcome, BluetoothOutcome::EventEnded { .. });
            self.send(owed, outcome);
            ended && self.finish(owed)
        })
    }

    /// Queue `outcome` in a slot known to be free.
    fn send(&self, owed: &mut Owed<CONNECTIONS>, outcome: BluetoothOutcome) {
        let lost_before = core::mem::take(&mut owed.lost);
        let sent = self.entries.try_send(Entry {
            lost_before,
            outcome,
        });
        assert!(sent.is_ok(), "a terminal outcome holds a reserved slot");
    }

    fn take(&self) -> Option<Result<BluetoothOutcome, EventsLost>> {
        self.owed(|owed| {
            if let Some(outcome) = owed.held.take() {
                return Some(Ok(outcome));
            }
            match self.entries.try_receive() {
                Ok(Entry {
                    lost_before: true,
                    outcome,
                }) => {
                    owed.held = Some(outcome);
                    Some(Err(EventsLost))
                }
                Ok(Entry { outcome, .. }) => Some(Ok(outcome)),
                Err(_) if core::mem::take(&mut owed.lost) => Some(Err(EventsLost)),
                Err(_) => None,
            }
        })
    }

    /// Forget the events of a radio that left: their ends never come, and
    /// the next radio starts disabled.
    fn reset(&self) {
        self.owed(|owed| {
            owed.events = 0;
            owed.reserved = 0;
            owed.data = [None; CONNECTIONS];
            owed.companion = false;
            owed.admission = Admission::Disabled;
            owed.generation = owed.generation.wrapping_add(1);
        });
    }
}

/// The sink of one locked entry: outcomes go to the queue, and a fault
/// poisons the runtime.
struct QueueSink<'a, M: RawMutex, E, const EVENTS: usize, const CONNECTIONS: usize> {
    outcomes: &'a OutcomeQueue<M, EVENTS, CONNECTIONS>,
    fault: &'a BlockingMutex<M, Cell<Option<BluetoothFault<E>>>>,
    changed: &'a Signal<M, ()>,
    admission: &'a Signal<M, ()>,
}

impl<M: RawMutex, E: Copy, const EVENTS: usize, const CONNECTIONS: usize> BluetoothRadioSink
    for QueueSink<'_, M, E, EVENTS, CONNECTIONS>
{
    fn outcome(&mut self, outcome: RadioOutcome<'_>) {
        match BluetoothOutcome::copy(outcome) {
            Some(copied) => {
                if self.outcomes.push(copied) {
                    self.admission.signal(());
                }
            }
            // A copy that could not hold its PDU is a fault as well.
            None => self.fault(RadioFault::MemoryInconsistency),
        }
    }

    fn fault(&mut self, fault: RadioFault) {
        self.fault.lock(|cause| {
            if cause.get().is_none() {
                cause.set(Some(BluetoothFault::Radio(fault)));
            }
        });
        self.changed.signal(());
        self.admission.signal(());
    }
}

/// The Bluetooth LE radio role, its hardware and the outcome queue.
///
/// [`Self::run`] drives the scheduler: it reports completions, inserts
/// admitted events and carries list transactions through their hardware
/// waits; it is the runner the composition polls beside the consumer of the
/// port's events, which only takes them. The port
/// ([`oer_bluetooth_radio::LeRadioPort`]) admits one portable request with a
/// fresh controller-time sample, while its lifecycle enables it. `EVENTS`
/// bounds the outcomes waiting for the consumer: each admitted event
/// reserves the slots of its end and of the acknowledgement or test report
/// before it, a connection event also `RX_PACKETS` slots for the data PDUs
/// the hardware acknowledges, a lifecycle command the slot of its terminal,
/// and an event that finds its slots taken is refused as `Busy`. A full
/// queue drops the newest advertising or scan report and reports
/// [`EventsLost`] once in its place.
///
/// The timing the port states ([`LeRadioCapabilities::timing`]) is the one
/// of the scheduler policy and sleep-clock accuracy the runtime is built
/// with; an install refuses hardware of another.
///
/// A hardware fault of the radio or the runner poisons the runtime: the
/// queued outcomes are still reported, then [`Poisoned`] with its
/// [`BluetoothFault`] at every call, until a new install.
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
    outcomes: OutcomeQueue<M, EVENTS, CONNECTIONS>,
    /// Why the radio or the runner faulted; a new install clears it.
    fault: BlockingMutex<M, Cell<Option<BluetoothFault<H::StartError>>>>,
    /// Raised when the consumer must look again without a new outcome.
    changed: Signal<M, ()>,
    /// Raised when a closing admission ended.
    admission: Signal<M, ()>,
    /// The scheduler policy and sleep-clock accuracy the timing is for.
    config: SchedulerSoftwareConfig,
    local_sleep_clock_ppm: u16,
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
    /// An empty runtime whose waits are on `timer`, for hardware of the
    /// scheduler policy `config` and a sleep clock of
    /// `local_sleep_clock_ppm`, suitable for a `static`.
    pub const fn new(
        timer: T,
        config: SchedulerSoftwareConfig,
        local_sleep_clock_ppm: u16,
    ) -> Self {
        Self {
            timer,
            installed: Mutex::new(None),
            outcomes: OutcomeQueue::new(),
            fault: BlockingMutex::new(Cell::new(None)),
            changed: Signal::new(),
            admission: Signal::new(),
            config,
            local_sleep_clock_ppm,
            work: Signal::new(),
            activity: Signal::new(),
            shared: AtomicBool::new(false),
        }
    }

    /// Publish the receive chains of `memory`, take the first controller-time
    /// sample and install the radio; its port and its control are returned,
    /// and [`BluetoothControl::uninstall`] takes the radio out again.
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
        (BluetoothPort<'_, Self>, BluetoothControl<'_, Self>),
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
        if hardware.scheduler_config() != self.config
            || hardware.local_sleep_clock_ppm() != self.local_sleep_clock_ppm
        {
            return Err((BluetoothInstallError::TimingMismatch, memory, hardware));
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
        self.outcomes.reset();
        self.admission.signal(());
        self.fault.lock(|fault| fault.set(None));
        self.work.signal(());
        Ok((BluetoothPort::new(self), BluetoothControl::new(self)))
    }

    /// The port's [`Poisoned`] once the radio or the runner faulted.
    fn poisoned(&self) -> Option<Poisoned<BluetoothFault<H::StartError>>> {
        self.fault.lock(Cell::get).map(|cause| Poisoned { cause })
    }

    /// Admit one request against a fresh controller-time sample: the
    /// port's submission. An event reserves the slots of its terminal
    /// outcomes first.
    pub(crate) async fn submit_request(
        &self,
        request: RadioRequest<'_>,
    ) -> PortResult<(), RequestError, BluetoothFault<H::StartError>> {
        let mut installed = self.installed.lock().await;
        if let Some(poisoned) = self.poisoned() {
            return Err(poisoned);
        }
        let installed = installed.as_mut().expect(INSTALLED);
        if self.outcomes.admission() != Admission::Enabled {
            return Ok(Err(RequestError::Disabled));
        }
        let Ok(sample) = sample_time(&self.timer, &mut installed.hardware).await else {
            return Ok(Err(RequestError::ClockUnavailable));
        };
        installed.radio.observe_time(&sample);
        // An event reserves its terminal outcomes; a connection event also
        // every data PDU its receive chain holds, which the hardware
        // acknowledges to the peer.
        let event = match request {
            RadioRequest::Advertise(event) => Some((event.id, 0)),
            RadioRequest::Scan(window) => Some((window.id, 0)),
            RadioRequest::ConnectionEvent(event) => Some((event.id, RX_PACKETS)),
            RadioRequest::TestTransmit(test) => Some((test.id, 0)),
            RadioRequest::TestReceive(test) => Some((test.id, 0)),
            _ => None,
        };
        if let Some((id, data)) = event
            && !self.outcomes.admit_event(id, data)
        {
            return Ok(Err(RequestError::Busy));
        }
        let starts_scanner = matches!(request, RadioRequest::ConfigureScanner(_));
        let changes_list = matches!(request, RadioRequest::FilterAcceptList(_));
        if let Err(error) = installed.radio.request(request) {
            if let Some((id, _)) = event {
                self.outcomes.withdraw_event(id);
            }
            return Ok(Err(error));
        }
        if starts_scanner {
            installed.hardware.publish_scan_start();
        }
        if changes_list {
            let publication = installed.radio.device_table_publication();
            installed.hardware.publish_device_table(publication);
        }
        self.work.signal(());
        Ok(Ok(()))
    }

    /// A fresh radio time: the port's clock.
    pub(crate) async fn read_now(
        &self,
    ) -> PortResult<LeInstant, ClockError, BluetoothFault<H::StartError>> {
        let mut installed = self.installed.lock().await;
        if let Some(poisoned) = self.poisoned() {
            return Err(poisoned);
        }
        let installed = installed.as_mut().expect(INSTALLED);
        let Ok(sample) = sample_time(&self.timer, &mut installed.hardware).await else {
            return Ok(Err(ClockError::Unavailable));
        };
        installed.radio.observe_time(&sample);
        Ok(Ok(installed.radio.now()))
    }

    /// Withdraw one scheduled event: the port's cancellation.
    pub(crate) async fn cancel_event(
        &self,
        id: EventId,
    ) -> PortResult<(), CancelError, BluetoothFault<H::StartError>> {
        let mut installed = self.installed.lock().await;
        if let Some(poisoned) = self.poisoned() {
            return Err(poisoned);
        }
        let installed = installed.as_mut().expect(INSTALLED);
        let cancelled = installed.radio.cancel(id, &mut self.sink());
        self.work.signal(());
        Ok(cancelled)
    }

    /// Run one lifecycle command: the port's lifecycle.
    ///
    /// - `Enable` opens admission of a disabled or quiesced port.
    /// - `Disable` cancels every admitted event and reports `Disabled`
    ///   after their ends.
    /// - `Quiesce` closes admission and reports `Quiesced` after the ends
    ///   of the admitted events.
    ///
    /// A command while `Disable` or `Quiesce` has not ended, or whose
    /// terminal finds no free slot, is refused as `Busy`.
    pub(crate) async fn run_lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> PortResult<(), LifecycleError, BluetoothFault<H::StartError>> {
        Ok(self.start_lifecycle(command).await?.map(drop))
    }

    /// [`Self::run_lifecycle`], returning the generation of the command
    /// that started, taken under the same lock as its transition.
    pub(crate) async fn start_lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> PortResult<u32, LifecycleError, BluetoothFault<H::StartError>> {
        let mut installed = self.installed.lock().await;
        if let Some(poisoned) = self.poisoned() {
            return Err(poisoned);
        }
        let installed = installed.as_mut().expect(INSTALLED);
        let queue = &self.outcomes;
        let started = match (command, queue.admission()) {
            (_, Admission::Disabling | Admission::Quiescing) => Err(LifecycleError::Busy),
            (LifecycleCommand::Enable, Admission::Enabled)
            | (LifecycleCommand::Disable, Admission::Disabled)
            | (LifecycleCommand::Quiesce, Admission::Quiesced) => {
                Err(LifecycleError::AlreadyInState)
            }
            (LifecycleCommand::Quiesce, Admission::Disabled) => Err(LifecycleError::InvalidState),
            (LifecycleCommand::Enable, _) => queue
                .lifecycle_now(LifecycleEvent::Enabled, Admission::Enabled)
                .then_some(())
                .ok_or(LifecycleError::Busy),
            (LifecycleCommand::Disable, Admission::Quiesced) => queue
                .lifecycle_now(LifecycleEvent::Disabled, Admission::Disabled)
                .then_some(())
                .ok_or(LifecycleError::Busy),
            (LifecycleCommand::Disable, _) => {
                if queue.begin(Admission::Disabling) {
                    installed.radio.cancel_all(&mut self.sink());
                    queue.finish_if_idle();
                    Ok(())
                } else {
                    Err(LifecycleError::Busy)
                }
            }
            (LifecycleCommand::Quiesce, _) => {
                if queue.begin(Admission::Quiescing) {
                    queue.finish_if_idle();
                    Ok(())
                } else {
                    Err(LifecycleError::Busy)
                }
            }
        };
        // Only a command that started moves admission; a refused one must
        // not wake the waiters it refused, or they would retry at once.
        if started.is_ok() {
            self.admission.signal(());
        }
        self.changed.signal(());
        self.work.signal(());
        Ok(started.map(|()| queue.lifecycle_state().1))
    }

    /// What the radio serves: its roles on LE 1M, Direct Test Mode,
    /// connections whose acknowledgement the hardware runs, and the timing
    /// of the runtime's scheduler policy and sleep clock.
    pub const fn capabilities(&self) -> LeRadioCapabilities {
        Radio::<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>::capabilities(
            Radio::<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>::radio_timing(
                self.config,
                self.local_sleep_clock_ppm,
            ),
        )
    }

    /// The platform's scheduler interrupt published a wake for the worker.
    pub fn on_scheduler_wake(&self) {
        self.work.signal(());
    }

    /// Take the next outcome: the queued ones in their order, then the
    /// [`Poisoned`] of a poisoned runtime.
    pub(crate) async fn wait_event(
        &self,
    ) -> PortResult<BluetoothOutcome, EventsLost, BluetoothFault<H::StartError>> {
        loop {
            if let Some(outcome) = self.outcomes.take() {
                // A freed slot may admit a lifecycle command refused as Busy.
                self.admission.signal(());
                return Ok(outcome);
            }
            if let Some(poisoned) = self.poisoned() {
                return Err(poisoned);
            }
            select(
                self.outcomes.entries.ready_to_receive(),
                self.changed.wait(),
            )
            .await;
        }
    }

    /// Poison the runtime after a fault of the runner, unless the radio
    /// faulted first.
    fn poison(&self, fault: BluetoothRuntimeFault<H::StartError>) {
        self.fault.lock(|cause| {
            if cause.get().is_none() {
                cause.set(Some(BluetoothFault::Runner(fault)));
            }
        });
        self.changed.signal(());
        // A lifecycle waiter must observe the poison: no end follows.
        self.admission.signal(());
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
                if !installed.faulted {
                    match sample_time(&self.timer, &mut installed.hardware).await {
                        Ok(sample) => installed.radio.observe_time(&sample),
                        Err(BluetoothTimeError::Deadline) => {
                            let fault = BluetoothRuntimeFault::Time(BluetoothTimeError::Deadline);
                            installed.fault();
                            self.poison(fault);
                            return fault;
                        }
                        Err(_) => {}
                    }
                }
                match self.pass(installed, &mut self.sink()) {
                    // A test event is published only on an idle scheduler, and
                    // a listed one leaves its list only after a stop; resuming
                    // restarts at the remaining events.
                    Ok(Pass::Stop) => match self.quiesce_installed(installed, |_| ()).await {
                        Ok(()) => Pass::Continue,
                        Err(fault) => {
                            installed.fault();
                            self.poison(fault);
                            return fault;
                        }
                    },
                    Ok(pass) => pass,
                    Err(fault) => {
                        installed.fault();
                        self.poison(fault);
                        return fault;
                    }
                }
            };
            let wait = match pass {
                Pass::Stop | Pass::Continue => {
                    budget.spend().await;
                    continue;
                }
                Pass::Recheck => HARDWARE_RECHECK,
                Pass::Idle => TIME_REFRESH,
            };
            budget.refill();
            if let Either::Second(Err(error)) =
                select(self.work.wait(), wait_for(&self.timer, wait)).await
            {
                if let Some(installed) = self.installed.lock().await.as_mut() {
                    installed.fault();
                }
                let fault = BluetoothRuntimeFault::Time(error);
                self.poison(fault);
                return fault;
            }
        }
    }

    /// Quiesce the port, stop the scheduler, hand the Bluetooth quiescence
    /// proof to `maintenance`, resume the scheduler and enable the port
    /// again: shared-PHY maintenance as a layer over the port's lifecycle.
    ///
    /// An enabled port runs `Quiesce` and waits for `Quiesced`, so every
    /// admitted event ends first; a disabled or quiesced port admits
    /// nothing already and stays as it is. The lifecycle terminals go to the
    /// port's event consumer, which must keep taking outcomes: a full
    /// queue refuses `Quiesce` and the final `Enable` as busy until it frees
    /// a slot. A list transaction in progress finishes before the stop.
    ///
    /// # Errors
    ///
    /// [`Poisoned`] when a hardware fault stopped the runtime.
    pub(crate) async fn quiesce<R>(
        &self,
        maintenance: impl FnOnce(ClientQuiescence<'_>) -> R,
    ) -> Result<R, Poisoned<BluetoothFault<H::StartError>>> {
        let resume = loop {
            match self.start_lifecycle(LifecycleCommand::Quiesce).await? {
                Ok(generation) => break Some(generation),
                Err(LifecycleError::AlreadyInState | LifecycleError::InvalidState) => break None,
                // Another command closes admission: wait for its end.
                Err(LifecycleError::Busy) => self.admission.wait().await,
            }
        };
        // The last end or a poison moves admission on.
        while self.outcomes.admission() == Admission::Quiescing {
            if let Some(poisoned) = self.poisoned() {
                return Err(poisoned);
            }
            self.admission.wait().await;
        }
        let result = {
            let mut installed = self.installed.lock().await;
            let installed = installed.as_mut().expect(INSTALLED);
            let result = self.quiesce_installed(installed, maintenance).await;
            if let Err(fault) = result {
                installed.fault();
                self.poison(fault);
            }
            self.work.signal(());
            result
        };
        let Ok(result) = result else {
            return Err(self.poisoned().expect("the runtime is poisoned"));
        };
        if let Some(generation) = resume {
            self.reopen(generation).await?;
        }
        Ok(result)
    }

    /// Enable the port that the quiesce of `generation` left `Quiesced`.
    ///
    /// The decision and the transition happen under one lock: the port is
    /// enabled only while it is still in that quiesce's `Quiesced`. Another
    /// owner's later command, even one that ended in `Quiesced` again,
    /// decides the state instead. A full queue defers `Enabled`
    /// until the consumer takes an outcome.
    pub(crate) async fn reopen(
        &self,
        generation: u32,
    ) -> Result<(), Poisoned<BluetoothFault<H::StartError>>> {
        loop {
            {
                // The lock orders the decision with every other lifecycle
                // command.
                let _installed = self.installed.lock().await;
                if let Some(poisoned) = self.poisoned() {
                    return Err(poisoned);
                }
                if self.outcomes.lifecycle_state() != (Admission::Quiesced, generation) {
                    return Ok(());
                }
                if self
                    .outcomes
                    .lifecycle_now(LifecycleEvent::Enabled, Admission::Enabled)
                {
                    self.admission.signal(());
                    self.changed.signal(());
                    self.work.signal(());
                    return Ok(());
                }
            }
            self.admission.wait().await;
        }
    }

    /// Stop the scheduler, take the radio and its hardware out of the
    /// runtime, and stop the runner.
    ///
    /// A list transaction in progress finishes first, and a controller-time
    /// request that a cancelled [`Self::run`] left in flight is drained.
    /// Events still listed stay with the stopped radio, which returns its
    /// memory only after the Controller reset
    /// ([`BluetoothRadio::into_memory`]): their ends never come, so the
    /// slots they reserved are released, and the next radio starts
    /// disabled. Outcomes already queued stay readable. [`Self::run`] then
    /// returns [`BluetoothRuntimeFault::NotInstalled`].
    ///
    /// # Errors
    ///
    /// The scheduler could not be stopped; the radio then stays installed
    /// and faulted.
    #[allow(
        clippy::type_complexity,
        reason = "the role's pool capacities stay visible in the returned radio"
    )]
    pub(crate) async fn uninstall(
        &self,
    ) -> Result<
        (
            Radio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>,
            H,
        ),
        BluetoothRuntimeFault<H::StartError>,
    > {
        let mut slot = self.installed.lock().await;
        let installed = slot.as_mut().expect(INSTALLED);
        if let Err(fault) = self.stop_scheduler(installed, &mut self.sink()).await {
            installed.fault();
            self.poison(fault);
            return Err(fault);
        }
        // The hardware leaves with its PHY routed as initialization left it.
        installed.restore_phy_route();
        // A cancelled runner may have left a controller-time request in
        // flight; the task owner retires only once it completed.
        loop {
            match installed.hardware.drain_time() {
                Ok(ControllerTimeEventStep::Waiting) => {
                    if let Err(error) = wait_for(&self.timer, HARDWARE_RECHECK).await {
                        let fault = BluetoothRuntimeFault::Time(error);
                        installed.fault();
                        self.poison(fault);
                        return Err(fault);
                    }
                }
                Ok(_) => break,
                Err(error) => {
                    let fault = BluetoothRuntimeFault::Time(BluetoothTimeError::Event(error));
                    installed.fault();
                    self.poison(fault);
                    return Err(fault);
                }
            }
        }
        let Installed {
            radio, hardware, ..
        } = slot.take().expect("the radio is installed");
        drop(slot);
        self.outcomes.reset();
        self.admission.signal(());
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
        self.stop_scheduler(installed, &mut self.sink()).await?;
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
        sink: &mut impl BluetoothRadioSink,
    ) -> Result<(), BluetoothRuntimeFault<H::StartError>> {
        while installed.awaiting.is_some() {
            if let Pass::Recheck = self.pass(installed, sink)? {
                wait_for(&self.timer, HARDWARE_RECHECK)
                    .await
                    .map_err(BluetoothRuntimeFault::Time)?;
            }
        }
        let mut stop = BluetoothSchedulerStop::default();
        let stopped = loop {
            match installed.hardware.step_stop(stop) {
                Ok(BluetoothSchedulerStopStep::Stopped(stopped)) => break stopped,
                Ok(BluetoothSchedulerStopStep::Pending(pending)) => {
                    stop = pending;
                    wait_for(&self.timer, HARDWARE_RECHECK)
                        .await
                        .map_err(BluetoothRuntimeFault::Time)?;
                }
                Err(error) => return Err(BluetoothRuntimeFault::StopSequence(error)),
            }
        };
        // Drain a prior captured observation before transferring the final
        // stopped snapshot. No finished-list bit may be silently ignored.
        drain_finished_lists(installed, sink);
        installed
            .hardware
            .capture_stopped_finished_lists(&stopped)
            .map_err(BluetoothRuntimeFault::FinishedLists)?;
        drain_finished_lists(installed, sink);
        installed
            .radio
            .enter_stopped(stopped)
            .map_err(|_| BluetoothRuntimeFault::Stop)
    }

    fn sink(&self) -> QueueSink<'_, M, H::StartError, EVENTS, CONNECTIONS> {
        QueueSink {
            outcomes: &self.outcomes,
            fault: &self.fault,
            changed: &self.changed,
            admission: &self.admission,
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
        sink: &mut impl BluetoothRadioSink,
    ) -> Result<Pass, BluetoothRuntimeFault<H::StartError>> {
        drain_finished_lists(installed, sink);
        if let Some(wake) = installed.hardware.take_wake() {
            installed
                .hardware
                .capture_finished_lists(wake)
                .map_err(BluetoothRuntimeFault::FinishedLists)?;
            drain_finished_lists(installed, sink);
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
                match installed.radio.advance(observation, sink) {
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
                installed.radio.drive(view, sink)
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
/// Wait `duration` on `timer`.
///
/// # Errors
///
/// [`BluetoothTimeError::Deadline`] when the timer cannot express the
/// deadline; the caller faults instead of waiting forever.
pub(crate) async fn wait_for(
    timer: &impl Timer,
    duration: Duration,
) -> Result<(), BluetoothTimeError> {
    timer
        .wait_for(duration)
        .await
        .map_err(|_| BluetoothTimeError::Deadline)
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
                wait_for(timer, HARDWARE_RECHECK).await?;
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
            ControllerTimeEventStep::Waiting => wait_for(timer, HARDWARE_RECHECK).await?,
            ControllerTimeEventStep::Idle | ControllerTimeEventStep::OrphanDrained => {
                return Err(BluetoothTimeError::Lost);
            }
        }
    }
}

#[cfg(test)]
mod tests;
