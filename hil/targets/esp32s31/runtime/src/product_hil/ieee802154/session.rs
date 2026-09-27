//! Host-driven IEEE 802.15.4 peer session.
//!
//! The session starts the composed client once, applies the host's identity
//! and filter, and then serves the host's commands until it stops: transmit
//! and report the outcome with any acknowledgement, enter receive mode,
//! report and forget the frames received so far, and change the automatic
//! frame-pending decision, and measure and assess any channel. The host may
//! ask for enhanced ACKs of 2015 frames. Received frames accumulate between collections,
//! so the host can drive the peer while the device listens.

mod radio;
mod serve;

use core::cell::Cell;

use embassy_futures::join::join;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use oer_esp32s31_ieee802154_system::Ieee802154System;
use oer_esp32s31_radio_esp_hal::EspHalRadioClocks;
use oer_esp32s31_radio_esp_hal::EspHalRadioPlatform;
use oer_esp32s31_radio_runtime::RadioSystem;
use oer_hil_protocol::{
    Event as HilEvent, Ieee802154SessionCoexistence, Ieee802154SessionConfig,
    Ieee802154SessionMaintenanceCounts, Ieee802154SessionRestartEvidence, Ieee802154SessionResult,
    Ieee802154SessionStopEvidence,
};
use oer_ieee802154::{Channel, RadioCommand};

use super::client::Client;
use crate::console::publish_event_reliably;

use radio::{Received, Session};
use serve::{Served, serve_session};

type Radio = RadioSystem<EspHalRadioPlatform, EspHalRadioClocks>;

/// Stop the session's client and start it again, as the air check does
/// between cycles: the stop leaves the shared PHY and closes RF after the
/// last client, the start joins and wakes it. The session then applies its
/// configuration and receives again. A failed step leaves no client.
async fn restart(
    client: &mut Client,
    system: Ieee802154System,
    session: &mut Session,
    config: Ieee802154SessionConfig,
) -> (Ieee802154SessionRestartEvidence, Option<Ieee802154System>) {
    let mut evidence = Ieee802154SessionRestartEvidence::default();
    let id = session.id();
    let _ = session.submit(RadioCommand::Sleep { id });
    let id = session.id();
    let _ = session.submit(RadioCommand::Disable { id });
    let stopped = {
        let stopped = core::pin::pin!(client.stop(system));
        stopped.await
    };
    let Some(parked) = stopped else {
        evidence.result = Ieee802154SessionResult::StopFailed;
        return (evidence, None);
    };
    evidence.rf_closed = client.radio.lock().await.lease().attachment().rf_closed();
    let started = {
        let started = core::pin::pin!(client.start(parked));
        started.await
    };
    let Some(system) = started else {
        evidence.result = Ieee802154SessionResult::StartFailed;
        return (evidence, None);
    };
    session.runtime = system.runtime();
    evidence.result = match session.configure(config) {
        Ok(()) => {
            let id = session.id();
            let channel = session.channel;
            match session.submit(RadioCommand::Receive { id, channel }) {
                Ok(()) => Ieee802154SessionResult::Done,
                Err(result) => result,
            }
        }
        Err(result) => result,
    };
    (evidence, Some(system))
}

/// Run one peer session until the host stops it. The image is terminal.
pub(in crate::product_hil) async fn run_session(
    platform: EspHalRadioPlatform,
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
    // Coexistence with Wi-Fi, as a Thread border router enters it.
    let mut coexistence = Ieee802154SessionCoexistence::default();
    if config.wifi_coexistence && started == Ieee802154SessionResult::Done {
        system.enable_wifi_coexistence(client.radio).await;
        coexistence.enabled = true;
    }
    publish_event_reliably(0, request_id, HilEvent::Ieee802154SessionStarted(started)).await;

    // The coexistence schedule runs beside the session while it takes part.
    let schedule_stop = Signal::<CriticalSectionRawMutex, ()>::new();
    let schedule = async {
        if coexistence.enabled {
            client
                .radio
                .run_coex_schedule_until(schedule_stop.wait())
                .await;
        }
    };
    let counts = Cell::new(Ieee802154SessionMaintenanceCounts::default());
    let serving = async {
        loop {
            let served = {
                let served = core::pin::pin!(serve_session(
                    &mut session,
                    &mut system,
                    &client,
                    config,
                    &counts
                ));
                served.await
            };
            match served {
                Served::Stop(stop_request) => {
                    schedule_stop.signal(());
                    return Some((stop_request, system));
                }
                Served::Restart(request_id) => {
                    let restarted = {
                        let restarted =
                            core::pin::pin!(restart(&mut client, system, &mut session, config));
                        restarted.await
                    };
                    let (evidence, restarted) = restarted;
                    publish_event_reliably(
                        0,
                        request_id,
                        HilEvent::Ieee802154SessionRadioRestarted(evidence),
                    )
                    .await;
                    let Some(restarted) = restarted else {
                        // The client is gone: the image ends without it.
                        schedule_stop.signal(());
                        return None;
                    };
                    system = restarted;
                }
            }
        }
    };
    let ((), served) = core::pin::pin!(join(schedule, serving)).await;
    let Some((stop_request, mut system)) = served else {
        return;
    };
    if coexistence.enabled && system.disable_wifi_coexistence(client.radio).await.is_err() {
        coexistence.disable_failed = true;
    }

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
            coexistence,
        }),
    )
    .await;
}
