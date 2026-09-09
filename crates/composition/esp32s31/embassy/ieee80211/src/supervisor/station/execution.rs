//! Same-Core0 execution and owner return for the connected station datapath.
//!
//! The supervisor transfers its non-`Send` runner to one permanent child task on its
//! recorded executor. The non-`Sync` mailbox retains the exact returned runner;
//! signals carry pause, stop and completion notifications. A pause returns the
//! same runner to its parent for physical suspension and resubmission. Stop
//! stays latched across that handoff and retains ordinary terminal teardown.

use core::cell::{Cell, RefCell};

mod exchange;
#[cfg(feature = "connected-datapath-cycle-telemetry")]
use core::future::{Future, poll_fn};

use embassy_executor::Spawner;

use embassy_futures::select::{Either, Either3, select, select3};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use exchange::Exchange;

use oer_esp32s31_wifi_embassy::{
    datapath::execution::{Control, Exit},
    roles::station::connected::{StationCommand, StationCommandReceiver},
};

use oer_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason;

use static_cell::StaticCell;

use super::{ConnectedDatapathError, ConnectedDatapathRunner};

pub(super) struct ConnectedDatapathTaskReturn {
    pub(super) runner: ConnectedDatapathRunner,
    pub(super) result: Result<Exit<ConnectedDisconnectReason>, ConnectedDatapathError>,
}

/// Same-executor rendezvous for the non-`Send` connected radio owner.
///
/// `StaticCell` provides stable storage, while this type deliberately does
/// not implement `Sync`: both participants are spawned by the one Core0
/// executor recorded before the physical supervisor starts.
pub(crate) struct ConnectedDatapathMailbox {
    bound: Cell<bool>,
    pause: &'static super::pause::Storage,
    exchange: Exchange<ConnectedDatapathRunner, ConnectedDatapathTaskReturn>,
    control: RefCell<Control<CriticalSectionRawMutex>>,
    #[cfg(feature = "connected-datapath-cycle-telemetry")]
    poll_observer: Option<crate::ConnectedDatapathPollObserver>,
}

impl ConnectedDatapathMailbox {
    const fn new(
        pause: &'static super::pause::Storage,
        #[cfg(feature = "connected-datapath-cycle-telemetry")] poll_observer: Option<
            crate::ConnectedDatapathPollObserver,
        >,
    ) -> Self {
        Self {
            bound: Cell::new(false),
            pause,
            exchange: Exchange::new(),
            control: RefCell::new(Control::new()),
            #[cfg(feature = "connected-datapath-cycle-telemetry")]
            poll_observer,
        }
    }

    pub(in crate::supervisor) fn bind(&'static self, spawner: Spawner) {
        assert!(
            !self.bound.replace(true),
            "connected datapath executor is bound once"
        );
        spawner
            .spawn(connected_datapath_task(self).expect("one permanent connected datapath worker"));
    }

    #[inline(never)]
    pub(super) fn start(&self, runner: &mut Option<ConnectedDatapathRunner>) {
        assert!(
            self.bound.get(),
            "radio runner binds its Core0 executor before station start"
        );
        self.resume(runner);
        // No await: the same-core worker cannot poll before this new epoch
        // replaces control. Failed submission must never clear an old Stop.
        *self.control.borrow_mut() = Control::new();
    }

    // Materialize the by-value exchange only in this synchronous leaf. Its
    // temporary runner must not occupy the caller's poll frame while PHY
    // calibration executes through that same future.
    #[inline(never)]
    pub(super) fn resume(&self, runner: &mut Option<ConnectedDatapathRunner>) {
        let runner = runner.take().expect("live connected runner");
        if self.exchange.submit(runner).is_err() {
            panic!("previous connected datapath owner must be reclaimed");
        }
    }

    pub(super) fn request_stop(&self) {
        self.control.borrow().request_stop();
    }

    fn finish(&self, returned: ConnectedDatapathTaskReturn) {
        self.exchange.finish(returned);
    }

    async fn wait_completed(&self) {
        self.exchange.wait_completed().await;
    }

    pub(super) fn take_return(&self) -> ConnectedDatapathTaskReturn {
        self.exchange.take_return()
    }
}

static CONNECTED_PAUSE_STORAGE: StaticCell<super::pause::Storage> = StaticCell::new();

static CONNECTED_DATAPATH_MAILBOX: StaticCell<ConnectedDatapathMailbox> = StaticCell::new();

pub(in crate::supervisor) fn initialize_connected_datapath_mailbox(
    #[cfg(feature = "connected-datapath-cycle-telemetry")] poll_observer: Option<
        crate::ConnectedDatapathPollObserver,
    >,
) -> &'static ConnectedDatapathMailbox {
    CONNECTED_DATAPATH_MAILBOX.init_with(|| {
        ConnectedDatapathMailbox::new(
            CONNECTED_PAUSE_STORAGE.init_with(super::pause::Storage::new),
            #[cfg(feature = "connected-datapath-cycle-telemetry")]
            poll_observer,
        )
    })
}

#[allow(
    clippy::await_holding_refcell_ref,
    reason = "shared control borrow permits concurrent requests and prevents replacing the epoch while the worker runs"
)]
#[embassy_executor::task(pool_size = 1)]
async fn connected_datapath_task(mailbox: &'static ConnectedDatapathMailbox) {
    loop {
        let mut runner = mailbox.exchange.next().await;
        let control = mailbox.control.borrow();
        #[cfg(feature = "connected-datapath-cycle-telemetry")]
        let result = if let Some(observer) = mailbox.poll_observer {
            const POLLS_PER_BATCH: u32 = 256;
            let cycles_per_micro = observer.cycles_per_micro();
            let mut batch = crate::ConnectedDatapathPollBatch::default();
            let mut run = core::pin::pin!(runner.run_controlled(&control));
            let result = poll_fn(|context| {
                let started = riscv::register::mcycle::read() as u32;
                let result = run.as_mut().poll(context);
                let elapsed_cycles = (riscv::register::mcycle::read() as u32).wrapping_sub(started);
                let elapsed_micros = elapsed_cycles.div_ceil(cycles_per_micro);
                batch.polls = batch.polls.saturating_add(1);
                batch.poll_micros = batch.poll_micros.saturating_add(elapsed_micros);
                batch.maximum_poll_micros = batch.maximum_poll_micros.max(elapsed_micros);
                batch.over_100_micros = batch
                    .over_100_micros
                    .saturating_add(u32::from(elapsed_micros > 100));
                batch.over_500_micros = batch
                    .over_500_micros
                    .saturating_add(u32::from(elapsed_micros > 500));
                batch.over_1_000_micros = batch
                    .over_1_000_micros
                    .saturating_add(u32::from(elapsed_micros > 1_000));
                batch.over_5_000_micros = batch
                    .over_5_000_micros
                    .saturating_add(u32::from(elapsed_micros > 5_000));
                if batch.polls == POLLS_PER_BATCH {
                    observer.record(batch);
                    batch = crate::ConnectedDatapathPollBatch::default();
                }
                result
            })
            .await;
            if batch.polls != 0 {
                observer.record(batch);
            }
            result
        } else {
            runner.run_controlled(&control).await
        };
        #[cfg(not(feature = "connected-datapath-cycle-telemetry"))]
        let result = runner.run_controlled(&control).await;
        drop(control);
        mailbox.finish(ConnectedDatapathTaskReturn { runner, result });
    }
}

pub(super) async fn wait_connected_datapath_completion(
    mailbox: &'static ConnectedDatapathMailbox,
    control: &mut StationCommandReceiver<'_, CriticalSectionRawMutex>,
) -> (Option<StationCommand>, super::PauseOperation) {
    use super::PauseOperation;
    match select3(
        mailbox.wait_completed(),
        control.wait(),
        super::pause_request::REQUESTS.wait(),
    )
    .await
    {
        Either3::First(()) => (None, PauseOperation::Access),
        Either3::Second(command) => {
            mailbox.request_stop();
            mailbox.wait_completed().await;
            (Some(command), PauseOperation::Access)
        }
        Either3::Third(operation) => {
            mailbox.control.borrow().request_pause();
            match select(mailbox.wait_completed(), control.wait()).await {
                Either::First(()) => (None, operation),
                Either::Second(command) => {
                    mailbox.request_stop();
                    mailbox.wait_completed().await;
                    (Some(command), operation)
                }
            }
        }
    }
}

/// Keep active execution separate from connection assembly and terminal teardown.
/// A physical fault returns a reference to its already retained owner frontier.
#[allow(clippy::type_complexity)]
pub(super) async fn run(
    mailbox: &'static ConnectedDatapathMailbox,
    station_control: &mut StationCommandReceiver<'_, CriticalSectionRawMutex>,
    mut interrupt_epoch: crate::interrupts::MacInterruptEpoch,
    role: &mut Option<super::pause::Role>,
    runner: &mut Option<ConnectedDatapathRunner>,
) -> Result<
    (
        crate::interrupts::MacInterruptEpoch,
        Result<
            oer_esp32s31_wifi_embassy::datapath::DatapathRunnerExit<ConnectedDisconnectReason>,
            ConnectedDatapathError,
        >,
        Option<StationCommand>,
    ),
    &'static super::pause::Failure,
> {
    use super::{PauseError, pause, pause_request};
    use oer_esp32s31_wifi_embassy::datapath::DatapathRunnerExit;
    use oer_wifi_embassy::await_stack_boundary;
    let _pause_availability = pause_request::REQUESTS.open();
    let mut requested_command = None;
    mailbox.start(runner);
    loop {
        let (requested, operation) =
            await_stack_boundary!(wait_connected_datapath_completion(mailbox, station_control,));
        requested_command = requested_command
            .into_iter()
            .chain(requested)
            .max_by_key(|command| *command as u8);
        let result = take_runner(mailbox, runner);
        match result {
            Ok(Exit::Paused) => {
                if requested_command.is_none() {
                    let started = embassy_time::Instant::now();
                    match await_stack_boundary!(pause::round_trip(
                        interrupt_epoch,
                        role,
                        operation,
                        runner,
                        mailbox.pause
                    )) {
                        Ok((irq, result)) => {
                            interrupt_epoch = irq;
                            finish_pause(mailbox, result, started);
                        }
                        Err(failure) => {
                            pause_request::REQUESTS.finish(Err(failure.stage()));
                            return Err(failure);
                        }
                    }
                } else {
                    pause_request::REQUESTS.finish(Err(PauseError::Interrupted));
                }
                // Requests arriving while the parent owns the physical pause
                // must be latched before the worker can schedule another frame.
                requested_command = requested_command
                    .into_iter()
                    .chain(station_control.try_take())
                    .max_by_key(|command| *command as u8);
                if requested_command.is_some() {
                    mailbox.request_stop();
                }
                mailbox.resume(runner);
            }
            Ok(Exit::Stopped) => {
                return Ok((
                    interrupt_epoch,
                    Ok(DatapathRunnerExit::Stopped),
                    requested_command,
                ));
            }
            Ok(Exit::Role(reason)) => {
                return Ok((
                    interrupt_epoch,
                    Ok(DatapathRunnerExit::Role(reason)),
                    requested_command,
                ));
            }
            Err(error) => {
                return Ok((interrupt_epoch, Err(error), requested_command));
            }
        }
    }
}

// Materialize the diagnostic snapshot after the hardware call chain has
// unwound. Its by-value signal payload need not live on the calibration stack.
#[inline(never)]
fn finish_pause(
    mailbox: &ConnectedDatapathMailbox,
    result: Result<
        Option<oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome>,
        super::PauseError,
    >,
    started: embassy_time::Instant,
) {
    super::pause_request::REQUESTS.finish(result.map(|tracking| {
        super::PauseReport {
            timings: mailbox.pause.timings(),
            tracking,
            elapsed_micros: embassy_time::Instant::now()
                .duration_since(started)
                .as_micros(),
        }
    }));
}

/// Transfer once into caller-owned storage instead of keeping a second complete
/// return value in the execution future's poll frame.
#[inline(never)]
fn take_runner(
    mailbox: &ConnectedDatapathMailbox,
    runner: &mut Option<ConnectedDatapathRunner>,
) -> Result<Exit<ConnectedDisconnectReason>, ConnectedDatapathError> {
    let returned = mailbox.take_return();
    *runner = Some(returned.runner);
    returned.result
}
