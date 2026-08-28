#![expect(
    clippy::too_many_arguments,
    reason = "connected-epoch assembly makes each independently borrowed hardware and service owner explicit"
)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_embassy_net::RawMutex as NetworkRawMutex;
use open_esp_radio_esp32s31_hal::{ConnectedStaInterruptPrepared, MacInterruptSetup};
use open_esp_radio_esp32s31_wifi_mac::{init::MAC_COLD_RX_INTERRUPT_MASK, irq::MacInterruptRoute};
use open_esp_radio_esp32s31_wifi_sta::{
    attempt::{Esp32s31StaAttemptSecurity, Esp32s31StaInstalledSecurity},
    connected_control::ConnectedDisconnectReason,
    peer::Esp32s31ConnectedStaPeer,
};
use open_esp_radio_wifi_softmac::interface::BoundVirtualInterface;

use crate::roles::station::connected::port::Esp32s31ConnectedStaConfig;
use crate::{
    datapath::irq::{Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochActivateError},
    datapath::{DatapathRunner, DatapathRunnerExit, DatapathServices},
};

use super::{Esp32s31StationCommand, Esp32s31StationCommandReceiver};

/// How a reconnect command reached the DATAPATH runner's safe stop edge.
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
    Disconnected(ConnectedDisconnectReason),
    ReconnectRequested {
        source: Esp32s31StationReconnectSource,
    },
    StationStopped(Esp32s31StationCommand),
    HardwareFailure(E),
}

/// Hardware/RX frontier accepted by one connected station epoch.
///
/// The initial variant consumes the runtime register owner and the halted
/// pre-connected ring. A later epoch can only be constructed from the exact
/// disconnected owners returned by the preceding teardown.
pub enum Esp32s31ConnectedEpochResources<H, R, E> {
    Initial { hardware: H, receive: R },
    Reconnected(E),
}

impl<H, R, E> Esp32s31ConnectedEpochResources<H, R, E> {
    pub const fn is_reconnected(&self) -> bool {
        matches!(self, Self::Reconnected(_))
    }
}

/// Complete owner handoff from a successful join into one connected service.
///
/// `R` is the role-wide runtime resource graph, `E` is the initial or
/// reconnected hardware frontier, and `N` is the one-time/running network
/// owner. Security is moved as one value so PMK, nonce and sequence spaces
/// cannot be split across competing composition roots.
pub struct Esp32s31ConnectedServiceResources<'security, R, E, N> {
    runtime: R,
    epoch: E,
    network: N,
    interface: BoundVirtualInterface,
    config: Esp32s31ConnectedStaConfig,
    peer: Esp32s31ConnectedStaPeer,
    installed_security: Esp32s31StaInstalledSecurity,
    security: Esp32s31StaAttemptSecurity<'security>,
}

impl<'security, R, E, N> Esp32s31ConnectedServiceResources<'security, R, E, N> {
    pub const fn new(
        runtime: R,
        epoch: E,
        network: N,
        interface: BoundVirtualInterface,
        config: Esp32s31ConnectedStaConfig,
        peer: Esp32s31ConnectedStaPeer,
        installed_security: Esp32s31StaInstalledSecurity,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            runtime,
            epoch,
            network,
            interface,
            config,
            peer,
            installed_security,
            security,
        }
    }

    pub fn into_parts(self) -> Esp32s31ConnectedServiceParts<'security, R, E, N> {
        Esp32s31ConnectedServiceParts {
            runtime: self.runtime,
            epoch: self.epoch,
            network: self.network,
            interface: self.interface,
            config: self.config,
            peer: self.peer,
            installed_security: self.installed_security,
            security: self.security,
        }
    }
}

/// Named decomposition visible only after the connected service consumes the
/// complete join handoff.
pub struct Esp32s31ConnectedServiceParts<'security, R, E, N> {
    pub runtime: R,
    pub epoch: E,
    pub network: N,
    pub interface: BoundVirtualInterface,
    pub config: Esp32s31ConnectedStaConfig,
    pub peer: Esp32s31ConnectedStaPeer,
    pub installed_security: Esp32s31StaInstalledSecurity,
    pub security: Esp32s31StaAttemptSecurity<'security>,
}

/// Open the production connected interrupt route and publish one durable RX
/// handoff probe.
///
/// Scan and join intentionally run with the route masked. A descriptor may
/// complete between their last polling observation and route activation; the
/// coalesced probe guarantees the DATAPATH runner checks that frontier even
/// when hardware produces no later edge.
pub fn activate_esp32s31_connected_epoch<'runtime, R, M>(
    interrupt: &mut Esp32s31MacInterruptEpoch<'runtime, R, M>,
    platform: &R::Platform,
    _prepared: ConnectedStaInterruptPrepared,
) -> Result<(), Esp32s31MacInterruptEpochActivateError<R::Error>>
where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    M: RawMutex,
{
    interrupt.mac_runtime().begin_rx_moderation();
    if let Err(error) = interrupt.activate(platform, MAC_COLD_RX_INTERRUPT_MASK) {
        interrupt.mac_runtime().end_rx_moderation();
        return Err(error);
    }
    interrupt.mac_runtime().notify_rx_handoff();
    Ok(())
}

pub(in crate::roles::station) fn coalesce_disconnected_station_command<E, M: RawMutex>(
    control: &mut Esp32s31StationCommandReceiver<'_, M>,
    reason: ConnectedDisconnectReason,
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
        None => Esp32s31ConnectedStationExit::Disconnected(reason),
    }
}

pub(in crate::roles::station) fn complete_connected_station_command<E, M: RawMutex>(
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
/// `DatapathRunner` observes the stop future only at a transaction-safe boundary.
/// A simultaneous peer disconnect is then coalesced with any still-pending
/// application command before ownership is handed back to the outer lifecycle.
pub async fn run_esp32s31_connected_station_epoch<
    'resources,
    'irq,
    RM,
    CM,
    N,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>(
    runner: &mut DatapathRunner<
        'resources,
        'irq,
        RM,
        N,
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
    N: crate::datapath::network::DatapathNetwork<
            'resources,
            RM,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    B: DatapathServices<
            'resources,
            RM,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
            Exit = ConnectedDisconnectReason,
        >,
{
    let requested_command = core::cell::Cell::new(None);
    let station_stop = async {
        requested_command.set(Some(control.wait().await));
    };
    match runner.run_until(station_stop).await {
        Ok(DatapathRunnerExit::Role(reason)) => {
            coalesce_disconnected_station_command(control, reason)
        }
        Ok(DatapathRunnerExit::Stopped) => {
            let command = requested_command
                .get()
                .expect("a stopped station runner consumed one controller command");
            complete_connected_station_command(command, control)
        }
        Err(error) => Esp32s31ConnectedStationExit::HardwareFailure(error),
    }
}
