//! Host-runnable shape of a non-HIL Embassy station application.
//!
//! A board firmware replaces `ExampleRunner` with
//! `Esp32s31StaAttemptTargetPort` composition and replaces `ProtocolTasks`
//! with its spawned staged-RX/application tasks. The outer ownership and stop
//! contracts stay unchanged and do not depend on HIL protocol or diagnostics.

use core::future::{Future, ready};

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_esp32s31_wifi_embassy::station::{
    Esp32s31StationAttemptRunner, Esp32s31StationCommandReceiver, Esp32s31StationConfig,
    Esp32s31StationControlResources, Esp32s31StationExit, Esp32s31StationStartResources,
    Esp32s31StationStopReason, prepare_esp32s31_station_task,
};
use open_esp_radio_wifi_embassy::connected_tasks::{ConnectedTaskGroup, stop_connected_task_group};
use open_esp_radio_wifi_sta::station::{StaAttemptContext, StaAttemptOutcome, StaReconnectPolicy};

#[derive(Debug, Eq, PartialEq)]
struct StationOwner {
    dma_generation: u32,
}

struct ExampleRunner;

impl Esp32s31StationAttemptRunner<NoopRawMutex> for ExampleRunner {
    type Owner = StationOwner;
    type Error = ();
    type Fault = core::convert::Infallible;

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

impl ConnectedTaskGroup for ProtocolTasks {
    type Stopped = [u8; 32];

    fn request_stop(&mut self) {
        self.stop_requested = true;
    }

    async fn wait_stopped(&mut self) -> Self::Stopped {
        ready(
            self.scratch
                .take()
                .expect("connected task scratch is returned once"),
        )
        .await
    }
}

fn main() {
    let policy = StaReconnectPolicy::new(3, 100, 1_000, 100).unwrap();
    let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
    let (controller, runner) = prepare_esp32s31_station_task(
        Esp32s31StationConfig::new(policy),
        Esp32s31StationStartResources::new(StationOwner { dma_generation: 7 }),
        &control,
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
    let scratch = block_on(stop_connected_task_group(&mut tasks));
    assert!(tasks.stop_requested);
    assert_eq!(scratch.len(), 32);
}
