//! Host-driven IEEE 802.15.4 peer session.
//!
//! The session starts the composed client once, applies the host's identity
//! and filter, and then serves the host's commands until it stops: transmit
//! and report the outcome with any acknowledgement, enter receive mode,
//! report and forget the frames received so far, and change the automatic
//! frame-pending decision. Received frames accumulate between collections,
//! so the host can drive the peer while the device listens.

use core::cell::Cell;

use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer, with_timeout};
use oer_esp32s31_ieee80211_esp_hal::EspHalRadioPeripheral;
use oer_esp32s31_ieee802154_runtime::{Ieee802154OwnedFrame, Ieee802154RadioEvent};
use oer_esp32s31_ieee802154_system::{
    Ieee802154PhyMaintenance, Ieee802154System, Ieee802154SystemRuntime, MAINTENANCE_PERIOD_MICROS,
};
use oer_esp32s31_phy::ConcurrentTrackingTick;
use oer_esp32s31_radio_esp_hal::EspHalRadioClocks;
use oer_esp32s31_radio_system::RadioSystem;
use oer_hil_protocol::{
    Event as HilEvent, IEEE802154_SESSION_RECORDED_FRAMES, Ieee802154AirTxOutcome,
    Ieee802154SessionAck, Ieee802154SessionConfig, Ieee802154SessionFrame,
    Ieee802154SessionMaintenanceCounts, Ieee802154SessionMaintenancePolicy,
    Ieee802154SessionPendingMode, Ieee802154SessionPendingRequest, Ieee802154SessionPhyMaintenance,
    Ieee802154SessionReceiveEvidence, Ieee802154SessionReceivedFrame, Ieee802154SessionResult,
    Ieee802154SessionStopEvidence, Ieee802154SessionTransmitEvidence,
    Ieee802154SessionTransmitRequest, RejectReason, ieee802154_frame_crc32c,
};
use oer_ieee802154::{
    AutoPendingMode, Channel, Configuration, FrameAddress, FrameView, RadioCommand, RequestId,
    TxMode, TxRequest,
};

use super::client::{Client, tx_outcome};
use crate::console::{
    Ieee802154SessionCommand, publish_event_reliably, receive_ieee802154_session_command,
};

type Radio = RadioSystem<EspHalRadioPeripheral, EspHalRadioClocks>;

/// Bound on one transmission's terminal event.
const TRANSMIT_TIMEOUT: Duration = Duration::from_millis(500);

/// Frames received since the previous collection.
#[derive(Default)]
struct Received {
    total: u16,
    frames: heapless::Vec<Ieee802154SessionReceivedFrame, IEEE802154_SESSION_RECORDED_FRAMES>,
}

impl Received {
    fn record(&mut self, frame: &Ieee802154OwnedFrame) {
        self.total = self.total.saturating_add(1);
        let bytes = frame.frame.as_bytes();
        let _ = self.frames.push(Ieee802154SessionReceivedFrame {
            length: bytes.len() as u8,
            crc32c: ieee802154_frame_crc32c(bytes),
            rssi_dbm: frame.metadata.rssi_dbm,
            lqi: frame.metadata.link_quality,
        });
    }

    fn take(&mut self) -> Ieee802154SessionReceiveEvidence {
        let Self { total, frames } = core::mem::take(self);
        Ieee802154SessionReceiveEvidence {
            result: Ieee802154SessionResult::Done,
            total,
            frames,
        }
    }
}

struct Session {
    runtime: &'static Ieee802154SystemRuntime,
    channel: Channel,
    next_id: u32,
    received: Received,
    lost: bool,
}

impl Session {
    fn id(&mut self) -> RequestId {
        self.next_id = self.next_id.wrapping_add(1);
        RequestId::new(self.next_id)
    }

    fn submit(&mut self, command: RadioCommand<'_>) -> Result<(), Ieee802154SessionResult> {
        self.runtime
            .submit(command)
            .map(|_| ())
            .map_err(|_| Ieee802154SessionResult::CommandRejected)
    }

    /// Apply the host's identity and filter to an enabled radio.
    fn configure(
        &mut self,
        config: Ieee802154SessionConfig,
    ) -> Result<(), Ieee802154SessionResult> {
        let id = self.id();
        self.submit(RadioCommand::Enable { id })?;
        for configuration in [
            Configuration::PanId(config.pan_id),
            Configuration::ShortAddress(config.short_address),
            Configuration::ExtendedAddress(config.extended_address),
            Configuration::Promiscuous(config.promiscuous),
        ] {
            let id = self.id();
            self.submit(RadioCommand::Configure { id, configuration })?;
        }
        Ok(())
    }

    /// Take one runtime event; received frames are recorded.
    fn observe(
        &mut self,
        event: Result<Ieee802154RadioEvent, oer_esp32s31_ieee802154_runtime::Ieee802154EventsLost>,
    ) -> Option<Ieee802154RadioEvent> {
        match event {
            Ok(Ieee802154RadioEvent::Received(frame)) => {
                self.received.record(&frame);
                None
            }
            Ok(event) => Some(event),
            Err(_) => {
                self.lost = true;
                None
            }
        }
    }

    async fn transmit(
        &mut self,
        request: &Ieee802154SessionTransmitRequest,
    ) -> Ieee802154SessionTransmitEvidence {
        let mut evidence = Ieee802154SessionTransmitEvidence::default();
        let Ok(frame) = FrameView::new(&request.frame) else {
            evidence.result = Ieee802154SessionResult::CommandRejected;
            return evidence;
        };
        let id = self.id();
        let mode = if request.cca {
            TxMode::ClearChannelAssessment
        } else {
            TxMode::Direct
        };
        if let Err(result) = self.submit(RadioCommand::Transmit(TxRequest {
            id,
            frame,
            channel: self.channel,
            mode,
            transmit_power_dbm: None,
        })) {
            evidence.result = result;
            return evidence;
        }
        loop {
            let Ok(event) = with_timeout(TRANSMIT_TIMEOUT, self.runtime.next_event()).await else {
                evidence.result = Ieee802154SessionResult::EventTimeout;
                return evidence;
            };
            match self.observe(event) {
                None => continue,
                Some(Ieee802154RadioEvent::TransmitDone {
                    id: done,
                    status,
                    acknowledgement,
                }) if done == id => {
                    evidence.result = Ieee802154SessionResult::Done;
                    evidence.outcome = tx_outcome(status);
                    evidence.acknowledgement = acknowledgement.map(|ack| Ieee802154SessionAck {
                        frame: Ieee802154SessionFrame::from_slice(ack.frame.as_bytes())
                            .unwrap_or_default(),
                        rssi_dbm: ack.metadata.rssi_dbm,
                        lqi: ack.metadata.link_quality,
                    });
                    return evidence;
                }
                Some(_) => {
                    evidence.result = Ieee802154SessionResult::UnexpectedEvent;
                    evidence.outcome = Ieee802154AirTxOutcome::NotRun;
                    return evidence;
                }
            }
        }
    }

    fn pending(&mut self, request: Ieee802154SessionPendingRequest) -> bool {
        let mode = match request.mode {
            Ieee802154SessionPendingMode::Disabled => AutoPendingMode::Disable,
            Ieee802154SessionPendingMode::Enabled => AutoPendingMode::Enable,
            Ieee802154SessionPendingMode::Enhanced => AutoPendingMode::Enhanced,
            Ieee802154SessionPendingMode::Zigbee => AutoPendingMode::Zigbee,
        };
        let runtime = self.runtime;
        if runtime.set_pending_mode(mode).is_err() {
            return false;
        }
        match request.short_address {
            None => true,
            Some(short) => runtime
                .with_pending_table(|table| {
                    table.add(FrameAddress::Short(short.to_le_bytes())).is_ok()
                })
                .unwrap_or(false),
        }
    }
}

/// The foreground maintainer: the running system and what PHY maintenance
/// borrows.
type Foreground<'a> = (&'a mut Ieee802154System, &'a Radio);

/// Serve host commands until the host stops the session; returns the stop
/// request. `foreground` serves explicit maintenance requests; without it
/// background maintenance owns the schedule.
async fn serve(session: &mut Session, mut foreground: Option<Foreground<'_>>) -> u32 {
    loop {
        let command = match select(
            receive_ieee802154_session_command(),
            session.runtime.next_event(),
        )
        .await
        {
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
                    HilEvent::Ieee802154SessionTransmitted(evidence),
                )
                .await;
            }
            Ieee802154SessionCommand::Receive { request_id } => {
                let id = session.id();
                let channel = session.channel;
                let response = match session.submit(RadioCommand::Receive { id, channel }) {
                    Ok(()) => HilEvent::Accepted,
                    Err(_) => HilEvent::Rejected(RejectReason::InvalidState),
                };
                publish_event_reliably(0, request_id, response).await;
            }
            Ieee802154SessionCommand::Collect { request_id } => {
                let mut evidence = session.received.take();
                if core::mem::take(&mut session.lost) {
                    evidence.result = Ieee802154SessionResult::EventsLost;
                }
                publish_event_reliably(
                    0,
                    request_id,
                    HilEvent::Ieee802154SessionReceived(evidence),
                )
                .await;
            }
            Ieee802154SessionCommand::Pending {
                request_id,
                request,
            } => {
                let response = if session.pending(request) {
                    HilEvent::Accepted
                } else {
                    HilEvent::Rejected(RejectReason::InvalidConfiguration)
                };
                publish_event_reliably(0, request_id, response).await;
            }
            Ieee802154SessionCommand::MaintainPhy { request_id } => {
                let Some((system, radio)) = foreground.as_mut() else {
                    // Background maintenance owns the schedule.
                    publish_event_reliably(
                        0,
                        request_id,
                        HilEvent::Rejected(RejectReason::InvalidState),
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
                    HilEvent::Ieee802154SessionPhyMaintained(outcome),
                )
                .await;
            }
            Ieee802154SessionCommand::Stop { request_id } => return request_id,
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

/// The radio's periodic tracking, as `RadioSystem::run_tracking` runs it,
/// until `stop` resolves; each tick is counted. Stop is taken only while the
/// loop waits between ticks, so a running tick always completes. Returns
/// whether every tick succeeded.
async fn track_until(
    radio: &Radio,
    stop: impl Future<Output = ()>,
    counts: &Cell<Ieee802154SessionMaintenanceCounts>,
) -> bool {
    let mut stop = core::pin::pin!(stop);
    loop {
        match select(
            Timer::after_micros(MAINTENANCE_PERIOD_MICROS),
            stop.as_mut(),
        )
        .await
        {
            Either::First(()) => {}
            Either::Second(()) => return true,
        }
        let tick = core::pin::pin!(radio.track()).await;
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
            // The session's client keeps the domain registered and open.
            Ok(ConcurrentTrackingTick::Unavailable(_)) | Err(_) => return false,
        }
        counts.set(tick_counts);
    }
}

/// Run one peer session until the host stops it. The image is terminal.
pub(in crate::product_hil) async fn run_session(
    platform: EspHalRadioPeripheral,
    request_id: u32,
    config: Ieee802154SessionConfig,
) {
    let Some(channel) = Channel::new(config.channel).ok() else {
        publish_event_reliably(
            0,
            request_id,
            HilEvent::Ieee802154SessionStarted(Ieee802154SessionResult::UnsupportedSetup),
        )
        .await;
        return;
    };
    let Some((mut client, parked)) = Client::claim(platform) else {
        publish_event_reliably(
            0,
            request_id,
            HilEvent::Ieee802154SessionStarted(Ieee802154SessionResult::UnsupportedSetup),
        )
        .await;
        return;
    };
    let started = {
        let started = core::pin::pin!(client.start(parked));
        started.await
    };
    let Some(mut system) = started else {
        publish_event_reliably(
            0,
            request_id,
            HilEvent::Ieee802154SessionStarted(Ieee802154SessionResult::StartFailed),
        )
        .await;
        return;
    };
    let mut session = Session {
        runtime: system.runtime(),
        channel,
        next_id: 0,
        received: Received::default(),
        lost: false,
    };
    client
        .set_maintenance_policy(config.maintenance_policy)
        .await;
    let started = match session.configure(config) {
        Ok(()) => Ieee802154SessionResult::Done,
        Err(result) => result,
    };
    publish_event_reliably(0, request_id, HilEvent::Ieee802154SessionStarted(started)).await;

    let counts = Cell::new(Ieee802154SessionMaintenanceCounts::default());
    let stop_request = if config.background_maintenance {
        let stop = Signal::<CriticalSectionRawMutex, ()>::new();
        let maintenance = async {
            let maintained = match config.maintenance_policy {
                // The domain's own periodic loop, as the vendor timer runs it.
                Ieee802154SessionMaintenancePolicy::Vendor => {
                    track_until(&client.radio, stop.wait(), &counts).await
                }
                Ieee802154SessionMaintenancePolicy::Quiesced => system
                    .maintain_phy_until(&client.radio, stop.wait(), |outcome| {
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
            let request = core::pin::pin!(serve(&mut session, None)).await;
            stop.signal(());
            request
        };
        let ((), request) = core::pin::pin!(join(maintenance, commands)).await;
        request
    } else {
        core::pin::pin!(serve(&mut session, Some((&mut system, &client.radio)),)).await
    };

    let id = session.id();
    let _ = session.submit(RadioCommand::Sleep { id });
    let id = session.id();
    let _ = session.submit(RadioCommand::Disable { id });
    let stopped = {
        let stopped = core::pin::pin!(client.stop(system));
        stopped.await
    };
    let stopped = match stopped {
        Some(_parked) => Ieee802154SessionResult::Done,
        None => Ieee802154SessionResult::StopFailed,
    };
    publish_event_reliably(
        0,
        stop_request,
        HilEvent::Ieee802154SessionStopped(Ieee802154SessionStopEvidence {
            result: stopped,
            maintenance: counts.get(),
        }),
    )
    .await;
}
