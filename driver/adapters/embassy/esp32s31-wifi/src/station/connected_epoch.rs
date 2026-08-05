use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_embassy_net::RawMutex as NetworkRawMutex;

use crate::connected_runner::{ConnectedRunner, ConnectedRunnerExit, ConnectedRunnerServices};

use super::{Esp32s31StationCommand, Esp32s31StationCommandReceiver};

/// How a reconnect command reached the connected runner's safe stop edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StationReconnectSource {
    /// The command future won while the runner was still connected.
    Controller,
    /// Peer loss won the same scheduling edge; the pending reconnect command
    /// was consumed after the runner had already returned link-down owners.
    CoalescedDisconnect,
}

/// Production result of one finite connected station epoch.
///
/// This keeps application control semantics out of HIL and prevents a queued
/// terminal command from leaking into a replacement epoch when peer loss wins
/// the same scheduler turn.
pub enum Esp32s31ConnectedStationExit<E> {
    Disconnected,
    ReconnectRequested {
        source: Esp32s31StationReconnectSource,
    },
    StationStopped(Esp32s31StationCommand),
    HardwareFailure(E),
}

pub(super) fn coalesce_disconnected_station_command<E, M: RawMutex>(
    control: &mut Esp32s31StationCommandReceiver<'_, M>,
) -> Esp32s31ConnectedStationExit<E> {
    match control.try_take() {
        Some(Esp32s31StationCommand::Reconnect) => {
            Esp32s31ConnectedStationExit::ReconnectRequested {
                source: Esp32s31StationReconnectSource::CoalescedDisconnect,
            }
        }
        Some(command @ (Esp32s31StationCommand::Disconnect | Esp32s31StationCommand::Stop)) => {
            control.record_terminal(command);
            Esp32s31ConnectedStationExit::StationStopped(command)
        }
        None => Esp32s31ConnectedStationExit::Disconnected,
    }
}

pub(super) fn complete_connected_station_command<E, M: RawMutex>(
    command: Esp32s31StationCommand,
    control: &mut Esp32s31StationCommandReceiver<'_, M>,
) -> Esp32s31ConnectedStationExit<E> {
    match command {
        Esp32s31StationCommand::Reconnect => Esp32s31ConnectedStationExit::ReconnectRequested {
            source: Esp32s31StationReconnectSource::Controller,
        },
        Esp32s31StationCommand::Disconnect | Esp32s31StationCommand::Stop => {
            control.record_terminal(command);
            Esp32s31ConnectedStationExit::StationStopped(command)
        }
    }
}

/// Run one connected hardware owner until peer loss or a station command.
///
/// `ConnectedRunner` observes the stop future only at a transaction-safe boundary.
/// A simultaneous peer disconnect is then coalesced with any still-pending
/// application command before ownership is handed back to the outer lifecycle.
pub async fn run_esp32s31_connected_station_epoch<
    'resources,
    'irq,
    RM,
    CM,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>(
    runner: &mut ConnectedRunner<
        'resources,
        'irq,
        RM,
        B,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >,
    control: &mut Esp32s31StationCommandReceiver<'_, CM>,
) -> Esp32s31ConnectedStationExit<B::Error>
where
    RM: NetworkRawMutex,
    CM: RawMutex,
    B: ConnectedRunnerServices<'resources, RM, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
{
    let requested_command = core::cell::Cell::new(None);
    let station_stop = async {
        requested_command.set(Some(control.wait().await));
    };
    match runner.run_until(station_stop).await {
        Ok(ConnectedRunnerExit::Disconnected) => coalesce_disconnected_station_command(control),
        Ok(ConnectedRunnerExit::Stopped) => {
            let command = requested_command
                .get()
                .expect("a stopped station runner consumed one controller command");
            complete_connected_station_command(command, control)
        }
        Err(error) => Esp32s31ConnectedStationExit::HardwareFailure(error),
    }
}
