//! Host-runnable shape of a non-HIL Embassy station application.
//!
//! A board firmware replaces `ExampleRunner` with
//! `Esp32s31StaAttemptTargetPort` composition and replaces `ProtocolTasks`
//! with its spawned staged-RX/application tasks. The outer ownership and stop
//! contracts stay unchanged and do not depend on HIL protocol or diagnostics.

use core::future::{Future, ready};

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::Duration;
use open_esp_radio_esp32s31_wifi_embassy::station::{
    Esp32s31ConnectedTaskGroup, Esp32s31ConnectedTaskStopOutcome, Esp32s31StationAttemptRunner,
    Esp32s31StationCommandReceiver, Esp32s31StationConfig, Esp32s31StationControlResources,
    Esp32s31StationExit, Esp32s31StationStartResources, Esp32s31StationStopReason,
    prepare_esp32s31_station_task, stop_esp32s31_connected_task_group,
};
use open_esp_radio_wifi_sta::station::{StaAttemptContext, StaAttemptOutcome, StaReconnectPolicy};

#[derive(Debug, Eq, PartialEq)]
struct StationOwner {
    dma_generation: u32,
}

struct ExampleRunner;

impl Esp32s31StationAttemptRunner<NoopRawMutex> for ExampleRunner {
    type Owner = StationOwner;
    type Error = ();

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        _context: StaAttemptContext,
        _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + 'a {
        ready(StaAttemptOutcome::Stopped { owner })
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
    let (controller, runner) = prepare_esp32s31_station_task(
        Esp32s31StationConfig::new(policy),
        Esp32s31StationStartResources::new(StationOwner { dma_generation: 7 }),
        &mut control,
        ExampleRunner,
    )
    .expect("fresh station control resources must accept one task");

    assert!(controller.request_stop());
    let Esp32s31StationExit::Stopped {
        resources, reason, ..
    } = block_on(runner.run())
    else {
        panic!("finite station stop did not return its owner");
    };
    assert!(matches!(reason, Esp32s31StationStopReason::Requested(_)));
    let (owner, _runner) = resources.into_parts();
    assert_eq!(owner.dma_generation, 7);

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
