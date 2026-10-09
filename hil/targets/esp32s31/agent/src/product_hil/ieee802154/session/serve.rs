//! The session's command loop and the PHY maintenance beside it.

use core::cell::Cell;

use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use oer_esp32s31_ieee802154_system::{Ieee802154PhyMaintenance, Ieee802154System};
use oer_esp32s31_phy::ConcurrentTrackingTick;
use oer_hil_protocol::{
    base::RejectReason, ieee802154::Ieee802154SessionConfig,
    ieee802154::Ieee802154SessionMaintenanceCounts, ieee802154::Ieee802154SessionMaintenancePolicy,
    ieee802154::Ieee802154SessionPhyMaintenance, ieee802154::Ieee802154SessionResult,
};
use oer_ieee802154::RadioCommand;

use super::super::client::Client;
use crate::console::{
    Ieee802154SessionCommand, publish_event_reliably, receive_ieee802154_session_command, respond,
};

use super::Radio;
use super::radio::Session;

/// Why the command loop returned.
#[derive(Clone, Copy)]
pub(super) enum Served {
    /// The host stopped the session with this request.
    Stop(u32),
    /// The host asked to restart the session's radio with this request.
    Restart(u32),
}

/// The foreground maintainer: the running system and what PHY maintenance
/// borrows.
type Foreground<'a> = (&'a mut Ieee802154System, &'a Radio);

/// Serve host commands until the host stops the session; returns the stop
/// request. `foreground` serves explicit maintenance requests; without it
/// background maintenance owns the schedule.
async fn serve(
    session: &mut Session,
    mut foreground: Option<Foreground<'_>>,
    restartable: bool,
) -> Served {
    loop {
        let command = match select(receive_ieee802154_session_command(), session.taken()).await {
            Either::First(command) => command,
            Either::Second(event) => {
                // An unsolicited terminal event outside a command is dropped.
                let _ = session.observe(event);
                continue;
            }
        };
        match command {
            Ieee802154SessionCommand::Transmit {
                request_id,
                request,
            } => {
                let evidence = session.transmit(&request).await;
                publish_event_reliably(
                    0,
                    request_id,
                    oer_hil_protocol::ieee802154::SessionTransmitted(evidence),
                )
                .await;
            }
            Ieee802154SessionCommand::Receive { request_id } => {
                let id = session.id();
                let channel = session.channel;
                let reply = session
                    .submit(RadioCommand::Receive { id, channel })
                    .map(|()| oer_hil_protocol::base::Accepted)
                    .map_err(|_| RejectReason::InvalidState);
                respond(0, request_id, reply).await;
            }
            Ieee802154SessionCommand::Collect { request_id } => {
                let mut evidence = session.received.take();
                if core::mem::take(&mut session.lost) {
                    evidence.result = Ieee802154SessionResult::EventsLost;
                }
                publish_event_reliably(
                    0,
                    request_id,
                    oer_hil_protocol::ieee802154::SessionReceived(evidence),
                )
                .await;
            }
            Ieee802154SessionCommand::Pending {
                request_id,
                request,
            } => {
                let reply = if session.pending(request) {
                    Ok(oer_hil_protocol::base::Accepted)
                } else {
                    Err(RejectReason::InvalidConfiguration)
                };
                respond(0, request_id, reply).await;
            }
            Ieee802154SessionCommand::MaintainPhy { request_id } => {
                let Some((system, radio)) = foreground.as_mut() else {
                    // Background maintenance owns the schedule.
                    publish_event_reliably(
                        0,
                        request_id,
                        oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
                    )
                    .await;
                    continue;
                };
                // Maintenance holds the tracking future; pin it in place.
                let maintained = core::pin::pin!(system.maintain_phy(radio));
                let outcome = match maintained.await {
                    Ok(Ieee802154PhyMaintenance::NotDue) => Ieee802154SessionPhyMaintenance::NotDue,
                    Ok(Ieee802154PhyMaintenance::Tracked) => {
                        Ieee802154SessionPhyMaintenance::Tracked
                    }
                    Ok(Ieee802154PhyMaintenance::AwaitingOtherClients) => {
                        Ieee802154SessionPhyMaintenance::AwaitingOtherClients
                    }
                    Ok(Ieee802154PhyMaintenance::Busy) => Ieee802154SessionPhyMaintenance::Busy,
                    Err(_) => Ieee802154SessionPhyMaintenance::Failed,
                };
                publish_event_reliably(
                    0,
                    request_id,
                    oer_hil_protocol::ieee802154::SessionPhyMaintained(outcome),
                )
                .await;
            }
            Ieee802154SessionCommand::Assess {
                request_id,
                request,
            } => {
                let assessment = session.assess(request).await;
                publish_event_reliably(
                    0,
                    request_id,
                    oer_hil_protocol::ieee802154::SessionAssessed(assessment),
                )
                .await;
            }
            Ieee802154SessionCommand::RecentRssi { request_id } => {
                let read = session.recent_rssi();
                publish_event_reliably(
                    0,
                    request_id,
                    oer_hil_protocol::ieee802154::SessionRecentRssi(read),
                )
                .await;
            }
            Ieee802154SessionCommand::Restart { request_id } => {
                if restartable {
                    return Served::Restart(request_id);
                }
                // Coexistence and background maintenance run beside the
                // loop and would have to leave and rejoin with the client.
                publish_event_reliably(
                    0,
                    request_id,
                    oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
                )
                .await;
            }
            Ieee802154SessionCommand::Stop { request_id } => return Served::Stop(request_id),
        }
    }
}

const fn count(
    mut counts: Ieee802154SessionMaintenanceCounts,
    outcome: Ieee802154PhyMaintenance,
) -> Ieee802154SessionMaintenanceCounts {
    match outcome {
        Ieee802154PhyMaintenance::NotDue => counts.not_due = counts.not_due.saturating_add(1),
        Ieee802154PhyMaintenance::Tracked => counts.tracked = counts.tracked.saturating_add(1),
        Ieee802154PhyMaintenance::AwaitingOtherClients => {
            counts.awaiting_other_clients = counts.awaiting_other_clients.saturating_add(1)
        }
        Ieee802154PhyMaintenance::Busy => counts.busy = counts.busy.saturating_add(1),
    }
    counts
}

/// The radio's periodic tracking (`RadioSystem::run_tracking_until`) until
/// `stop` resolves, with each tick counted. A running tick always
/// completes. Returns whether every tick succeeded; the session's client
/// keeps the domain registered and open, so an unavailable domain fails.
async fn track_until(
    radio: &Radio,
    stop: impl Future<Output = ()>,
    counts: &Cell<Ieee802154SessionMaintenanceCounts>,
) -> bool {
    let unavailable = Cell::new(false);
    let tracking = core::pin::pin!(radio.run_tracking_until(stop, |tick| {
        let mut tick_counts = counts.get();
        match tick {
            Ok(ConcurrentTrackingTick::NotDue) => {
                tick_counts.not_due = tick_counts.not_due.saturating_add(1);
            }
            Ok(ConcurrentTrackingTick::Tracked(_)) => {
                tick_counts.tracked = tick_counts.tracked.saturating_add(1);
            }
            Ok(ConcurrentTrackingTick::AwaitingQuiescence) => {
                tick_counts.awaiting_other_clients =
                    tick_counts.awaiting_other_clients.saturating_add(1);
            }
            Ok(ConcurrentTrackingTick::Unavailable(_)) | Err(_) => unavailable.set(true),
        }
        counts.set(tick_counts);
    }));
    tracking.await.is_ok() && !unavailable.get()
}

/// Serve the session's commands, with background maintenance beside them
/// when configured, until the host stops the session or restarts its radio.
pub(super) async fn serve_session(
    session: &mut Session,
    system: &mut Ieee802154System,
    client: &Client,
    config: Ieee802154SessionConfig,
    counts: &Cell<Ieee802154SessionMaintenanceCounts>,
) -> Served {
    let restartable = !config.background_maintenance && !config.wifi_coexistence;
    if config.background_maintenance {
        let stop = Signal::<CriticalSectionRawMutex, ()>::new();
        let maintenance = async {
            let maintained = match config.maintenance_policy {
                // The domain's own periodic loop, as the vendor timer runs it.
                Ieee802154SessionMaintenancePolicy::Vendor => {
                    track_until(client.radio, stop.wait(), counts).await
                }
                Ieee802154SessionMaintenancePolicy::Quiesced => system
                    .maintain_phy_until(client.radio, stop.wait(), |outcome| {
                        counts.set(count(counts.get(), outcome))
                    })
                    .await
                    .is_ok(),
            };
            if !maintained {
                let mut failed = counts.get();
                failed.failed = true;
                counts.set(failed);
            }
        };
        let commands = async {
            let request = core::pin::pin!(serve(session, None, restartable)).await;
            stop.signal(());
            request
        };
        let ((), request) = core::pin::pin!(join(maintenance, commands)).await;
        request
    } else {
        core::pin::pin!(serve(session, Some((system, client.radio)), restartable)).await
    }
}
