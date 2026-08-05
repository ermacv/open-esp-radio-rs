//! Host-runnable shape of a non-HIL Embassy station application.
//!
//! A board firmware replaces `ExampleBackend` with
//! `Esp32s31StaAttemptTargetPort` composition and replaces `ProtocolTasks`
//! with its spawned staged-RX/application tasks. The outer ownership and stop
//! contracts stay unchanged and do not depend on HIL protocol or diagnostics.

use core::future::{Future, ready};

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::Duration;
use open_esp_radio_esp32s31_wifi_embassy::station::{
    Esp32s31ConnectedTaskGroup, Esp32s31ConnectedTaskStopOutcome, Esp32s31Station,
    Esp32s31StationCommandReceiver, Esp32s31StationConfig, Esp32s31StationControlResources,
    Esp32s31StationExit, Esp32s31StationResources, Esp32s31StationStopReason,
    stop_esp32s31_connected_task_group,
};
use open_esp_radio_wifi_sta::station::{
    StaAttemptContext, StaAttemptOutcome, StaBackoffOutcome, StaBackoffReason, StaLifecycleBackend,
    StaReconnectPolicy,
};

#[derive(Debug, Eq, PartialEq)]
struct StationOwner {
    dma_generation: u32,
}

struct ExampleBackend<'control> {
    control: Esp32s31StationCommandReceiver<'control, NoopRawMutex>,
}

impl StaLifecycleBackend for ExampleBackend<'_> {
    type Owner = StationOwner;
    type Error = ();

    fn run_attempt(
        &mut self,
        owner: Self::Owner,
        _context: StaAttemptContext,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + '_ {
        let command = self
            .control
            .try_take()
            .expect("the example requests stop before running");
        self.control.record_terminal(command);
        ready(StaAttemptOutcome::Stopped { owner })
    }

    fn wait_backoff(
        &mut self,
        owner: Self::Owner,
        _delay_millis: u32,
        _reason: StaBackoffReason,
    ) -> impl Future<Output = StaBackoffOutcome<Self::Owner>> + '_ {
        ready(StaBackoffOutcome::Elapsed { owner })
    }
}

struct ProtocolTasks {
    stop_requested: bool,
    scratch: Option<[u8; 32]>,
}

impl Esp32s31ConnectedTaskGroup for ProtocolTasks {
    type Stopped = [u8; 32];

    fn request_stop(&mut self) {
        self.stop_requested = true;
    }

    fn wait_stopped(&mut self) -> impl Future<Output = Self::Stopped> + '_ {
        ready(
            self.scratch
                .take()
                .expect("connected task scratch is returned once"),
        )
    }
}

fn main() {
    let policy = StaReconnectPolicy::new(3, 100, 1_000, 100).unwrap();
    let mut control = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (controller, runner) = Esp32s31Station::new(
        Esp32s31StationConfig::new(policy),
        Esp32s31StationResources::new(StationOwner { dma_generation: 7 }),
        &mut control,
        |control| ExampleBackend { control },
    );

    assert!(controller.request_stop());
    let Esp32s31StationExit::Stopped {
        resources, reason, ..
    } = block_on(runner.run())
    else {
        panic!("finite station stop did not return its owner");
    };
    assert!(matches!(reason, Esp32s31StationStopReason::Requested(_)));
    assert_eq!(resources.into_owner().dma_generation, 7);

    // Real firmware first quiesces its MAC interrupt route, then asks every
    // spawned consumer to release connected-epoch borrows before DMA teardown.
    let mut tasks = ProtocolTasks {
        stop_requested: false,
        scratch: Some([0; 32]),
    };
    let outcome = block_on(stop_esp32s31_connected_task_group(
        &mut tasks,
        Duration::from_secs(2),
    ));
    let Esp32s31ConnectedTaskStopOutcome::Stopped(scratch) = outcome else {
        panic!("the application must reset instead of reusing a timed-out epoch");
    };
    assert!(tasks.stop_requested);
    assert_eq!(scratch.len(), 32);
}
