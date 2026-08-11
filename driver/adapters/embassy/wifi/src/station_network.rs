//! Chip-independent Embassy network ownership across station associations.
//!
//! A network device is consumed exactly once to construct an IP stack. The
//! resulting stack and radio-side runner then survive Wi-Fi disconnect,
//! rescan and reassociation. This module owns that one-time/running transition;
//! DHCP versus static addressing and the concrete stack task remain caller
//! policy.

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_embassy_net::{LinkState, SplitPinnedRadioRunner};

/// Network ownership before the first association or after a finite connected
/// epoch has returned its runner.
pub enum StationNetworkResources<D, R, S> {
    Unstarted { device: D, runner: R },
    Running(RunningStationNetwork<S, R>),
}

impl<D, R, S> StationNetworkResources<D, R, S> {
    pub const fn is_started(&self) -> bool {
        matches!(self, Self::Running(_))
    }

    /// Borrow the persistent radio-side network endpoint without changing
    /// the station-specific one-time start marker. Standalone AP epochs use
    /// the same queues and must not fabricate a second Embassy device.
    pub const fn radio_runner(&self) -> &R {
        match self {
            Self::Unstarted { runner, .. } => runner,
            Self::Running(network) => &network.runner,
        }
    }
}

/// Stack and radio-side network owner retained across association epochs.
pub struct RunningStationNetwork<S, R> {
    stack: S,
    runner: R,
}

impl<S, R> RunningStationNetwork<S, R> {
    pub const fn new(stack: S, runner: R) -> Self {
        Self { stack, runner }
    }

    pub fn into_parts(self) -> (S, R) {
        (self.stack, self.runner)
    }

    /// Borrow the radio-side endpoint while another role temporarily owns
    /// the shared Wi-Fi hardware. This does not restart or duplicate the
    /// network stack.
    pub const fn radio_runner(&self) -> &R {
        &self.runner
    }
}

/// Uniform network frontier entering one connected station epoch.
///
/// `initial_task` is present only when this transition consumed the original
/// network device. A reconnect receives the existing stack and runner without
/// constructing or spawning another stack task.
pub struct StartedStationNetwork<S, R, T> {
    stack: S,
    runner: R,
    initial_task: Option<T>,
}

impl<S, R, T> StartedStationNetwork<S, R, T> {
    pub fn into_parts(self) -> (S, R, Option<T>) {
        (self.stack, self.runner, self.initial_task)
    }
}

/// Narrow capability needed by the ownership transition. It deliberately does
/// not expose RX publication or TX consumption to the network initializer.
#[doc(hidden)]
pub trait StationNetworkLink {
    fn publish_link_up(&self);
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> StationNetworkLink
    for SplitPinnedRadioRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    fn publish_link_up(&self) {
        self.set_link_state(LinkState::Up);
    }
}

/// Enter a connected network epoch without rebuilding a running IP stack.
///
/// `start` owns only the one-time mapping from the network device to a stack
/// and its long-lived executor task. Address selection, random seed and stack
/// storage therefore remain explicit composition policy.
pub fn start_station_network<D, R, S, T>(
    resources: StationNetworkResources<D, R, S>,
    start: impl FnOnce(D) -> (S, T),
) -> StartedStationNetwork<S, R, T>
where
    R: StationNetworkLink,
{
    let (stack, runner, initial_task) = match resources {
        StationNetworkResources::Unstarted { device, runner } => {
            let (stack, task) = start(device);
            (stack, runner, Some(task))
        }
        StationNetworkResources::Running(network) => {
            let (stack, runner) = network.into_parts();
            (stack, runner, None)
        }
    };
    runner.publish_link_up();
    StartedStationNetwork {
        stack,
        runner,
        initial_task,
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::*;

    struct TestRunner<'a> {
        link_up: &'a Cell<u32>,
    }

    impl StationNetworkLink for TestRunner<'_> {
        fn publish_link_up(&self) {
            self.link_up.set(self.link_up.get() + 1);
        }
    }

    #[test]
    fn network_device_starts_once_and_running_owner_is_reused() {
        let link_up = Cell::new(0);
        let starts = Cell::new(0);
        let unstarted = StationNetworkResources::Unstarted {
            device: 7_u8,
            runner: TestRunner { link_up: &link_up },
        };
        let started = start_station_network(unstarted, |device| {
            starts.set(starts.get() + 1);
            (u16::from(device) + 100, 9_u32)
        });
        let (stack, runner, initial_task) = started.into_parts();
        assert_eq!(stack, 107);
        assert_eq!(initial_task, Some(9));
        assert_eq!(starts.get(), 1);
        assert_eq!(link_up.get(), 1);

        let running =
            StationNetworkResources::<u8, _, _>::Running(RunningStationNetwork::new(stack, runner));
        let started = start_station_network(running, |_device| -> (u16, u32) {
            panic!("a reconnect must not construct another network stack")
        });
        let (stack, _runner, initial_task) = started.into_parts();
        assert_eq!(stack, 107);
        assert_eq!(initial_task, None);
        assert_eq!(starts.get(), 1);
        assert_eq!(link_up.get(), 2);
    }
}
