//! Same-Core0 execution and owner return for the connected station datapath.
//!
//! The supervisor transfers its non-`Send` runner to one child task on its
//! recorded executor. The non-`Sync` mailbox retains the exact returned runner;
//! signals carry only stop and completion notifications. When control requests
//! a stop, the rendezvous waits for completion before the supervisor recovers
//! the runner and performs the existing IRQ/DMA quiescence and teardown.

use core::cell::{Cell, RefCell};
#[cfg(feature = "connected-datapath-cycle-telemetry")]
use core::future::{Future, poll_fn};

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use open_esp_radio_esp32s31_wifi_embassy::{
    datapath::DatapathRunnerExit,
    roles::station::connected::{Esp32s31StationCommand, Esp32s31StationCommandReceiver},
};
use open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason;
use static_cell::StaticCell;

use super::{ConnectedDatapathError, ConnectedDatapathRunner};

pub(super) struct ConnectedDatapathTaskReturn {
    pub(super) runner: ConnectedDatapathRunner,
    pub(super) result:
        Result<DatapathRunnerExit<ConnectedDisconnectReason>, ConnectedDatapathError>,
}

/// Same-executor rendezvous for the non-`Send` connected radio owner.
///
/// `StaticCell` provides stable storage, while this type deliberately does
/// not implement `Sync`: both participants are spawned by the one Core0
/// executor recorded before the physical supervisor starts.
pub(crate) struct ConnectedDatapathMailbox {
    spawner: Cell<Option<Spawner>>,
    stop: Signal<CriticalSectionRawMutex, ()>,
    completed: Signal<CriticalSectionRawMutex, ()>,
    returned: RefCell<Option<ConnectedDatapathTaskReturn>>,
    #[cfg(feature = "connected-datapath-cycle-telemetry")]
    poll_observer: Option<crate::Esp32s31ConnectedDatapathPollObserver>,
}

impl ConnectedDatapathMailbox {
    const fn new(
        #[cfg(feature = "connected-datapath-cycle-telemetry")] poll_observer: Option<
            crate::Esp32s31ConnectedDatapathPollObserver,
        >,
    ) -> Self {
        Self {
            spawner: Cell::new(None),
            stop: Signal::new(),
            completed: Signal::new(),
            returned: RefCell::new(None),
            #[cfg(feature = "connected-datapath-cycle-telemetry")]
            poll_observer,
        }
    }

    pub(in crate::supervisor) fn bind(&self, spawner: Spawner) {
        assert!(
            self.spawner.replace(Some(spawner)).is_none(),
            "connected datapath executor is bound once"
        );
    }

    pub(super) fn spawn(&'static self, runner: ConnectedDatapathRunner) {
        self.stop.reset();
        self.completed.reset();
        assert!(
            self.returned.borrow().is_none(),
            "previous connected datapath owner must be reclaimed"
        );
        self.spawner
            .get()
            .expect("radio runner binds its Core0 executor before station start")
            .spawn(
                connected_datapath_task(self, runner)
                    .expect("one connected datapath task may be active"),
            );
    }

    fn finish(&self, returned: ConnectedDatapathTaskReturn) {
        assert!(
            self.returned.borrow_mut().replace(returned).is_none(),
            "connected datapath return slot has one producer"
        );
        self.completed.signal(());
    }

    async fn wait_completed(&self) {
        self.completed.wait().await;
    }

    pub(super) fn take_return(&self) -> ConnectedDatapathTaskReturn {
        self.returned
            .borrow_mut()
            .take()
            .expect("completion publishes the connected datapath owner first")
    }
}

static CONNECTED_DATAPATH_MAILBOX: StaticCell<ConnectedDatapathMailbox> = StaticCell::new();

pub(in crate::supervisor) fn initialize_connected_datapath_mailbox(
    #[cfg(feature = "connected-datapath-cycle-telemetry")] poll_observer: Option<
        crate::Esp32s31ConnectedDatapathPollObserver,
    >,
) -> &'static ConnectedDatapathMailbox {
    CONNECTED_DATAPATH_MAILBOX.init_with(|| {
        ConnectedDatapathMailbox::new(
            #[cfg(feature = "connected-datapath-cycle-telemetry")]
            poll_observer,
        )
    })
}

#[embassy_executor::task(pool_size = 1)]
async fn connected_datapath_task(
    mailbox: &'static ConnectedDatapathMailbox,
    mut runner: ConnectedDatapathRunner,
) {
    #[cfg(feature = "connected-datapath-cycle-telemetry")]
    let result = if let Some(observer) = mailbox.poll_observer {
        const POLLS_PER_BATCH: u32 = 256;
        let cycles_per_micro = observer.cycles_per_micro();
        let mut batch = crate::Esp32s31ConnectedDatapathPollBatch::default();
        let mut run = core::pin::pin!(runner.run_until(mailbox.stop.wait()));
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
                batch = crate::Esp32s31ConnectedDatapathPollBatch::default();
            }
            result
        })
        .await;
        if batch.polls != 0 {
            observer.record(batch);
        }
        result
    } else {
        runner.run_until(mailbox.stop.wait()).await
    };
    #[cfg(not(feature = "connected-datapath-cycle-telemetry"))]
    let result = runner.run_until(mailbox.stop.wait()).await;
    mailbox.finish(ConnectedDatapathTaskReturn { runner, result });
}

pub(super) async fn wait_connected_datapath_completion(
    mailbox: &'static ConnectedDatapathMailbox,
    control: &mut Esp32s31StationCommandReceiver<'_, CriticalSectionRawMutex>,
) -> Option<Esp32s31StationCommand> {
    match select(mailbox.wait_completed(), control.wait()).await {
        Either::First(()) => None,
        Either::Second(command) => {
            mailbox.stop.signal(());
            mailbox.wait_completed().await;
            Some(command)
        }
    }
}
