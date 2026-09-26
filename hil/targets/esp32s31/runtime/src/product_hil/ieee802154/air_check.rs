//! Single-device on-air check of the composed IEEE 802.15.4 client.
//!
//! The check splits the radio root concurrently and runs every cycle through
//! `oer-esp32s31-ieee802154-system`: start on the shared radio arbiter, one
//! energy scan, one clear-channel assessment, one direct transmit without an
//! acknowledgement request, two transmits scheduled through the MAC timer and
//! ETM route, one promiscuous receive window, then stop. The evidence records
//! outcomes and target-monotonic times; the host judges them.

use embassy_time::{Duration, Timer, with_timeout};
use oer_esp32s31_ieee80211_esp_hal::EspHalRadioPeripheral;
use oer_esp32s31_ieee802154_runtime::Ieee802154RadioEvent;
use oer_esp32s31_ieee802154_system::Ieee802154System;
use oer_hil_protocol::{
    Ieee802154AirCcaOutcome, Ieee802154AirCheckEvidence, Ieee802154AirCheckRequest,
    Ieee802154AirCheckStop, Ieee802154AirCycle, Ieee802154AirEnergyOutcome, Ieee802154AirTransmit,
};
use oer_ieee802154::{
    Channel, Configuration, EnergyScanRequest, FrameView, RadioCommand, RadioTimestamp, RequestId,
    TxMode, TxRequest,
};

use super::client::{Client, now_micros, tx_outcome};

/// Bound on one terminal event after its command started.
const EVENT_TIMEOUT: Duration = Duration::from_millis(500);

/// A data frame with PAN ID compression and short addresses, without an
/// acknowledgement request: broadcast destination 0xffff on PAN 0x4f45 from
/// short address 0x0154, payload `OER-154`. The FCS is appended by the MAC.
const FRAME: [u8; 16] = [
    0x41, 0x88, 0x00, 0x45, 0x4f, 0xff, 0xff, 0x54, 0x01, b'O', b'E', b'R', b'-', b'1', b'5', b'4',
];

/// Why one cycle stopped early.
struct Stop(Ieee802154AirCheckStop);

/// Run the air check. The image is terminal: the radio stays split.
pub(in crate::product_hil) async fn run_air_check(
    platform: EspHalRadioPeripheral,
    request: Ieee802154AirCheckRequest,
) -> Ieee802154AirCheckEvidence {
    let mut evidence = Ieee802154AirCheckEvidence::default();
    let Ok(channel) = Channel::new(request.channel) else {
        return evidence;
    };
    let Some((mut client, mut parked)) = Client::claim(platform) else {
        return evidence;
    };
    for index in 0..usize::from(request.cycles) {
        let started = {
            let started = core::pin::pin!(client.start(parked));
            started.await
        };
        let Some(system) = started else {
            evidence.stop = Ieee802154AirCheckStop::StartFailed;
            return evidence;
        };
        let outcome = run_cycle(&system, channel, request, &mut evidence.cycles[index]).await;
        let stopped = {
            let stopped = core::pin::pin!(client.stop(system));
            stopped.await
        };
        let Some(stopped) = stopped else {
            evidence.stop = Ieee802154AirCheckStop::StopFailed;
            return evidence;
        };
        parked = stopped;
        if let Err(Stop(stop)) = outcome {
            evidence.stop = stop;
            return evidence;
        }
        evidence.completed_cycles += 1;
    }
    evidence.stop = Ieee802154AirCheckStop::Complete;
    evidence
}

async fn run_cycle(
    system: &Ieee802154System,
    channel: Channel,
    request: Ieee802154AirCheckRequest,
    cycle: &mut Ieee802154AirCycle,
) -> Result<(), Stop> {
    let runtime = system.runtime();
    let mut next_id = 0;
    let mut id = || {
        next_id += 1;
        RequestId::new(next_id)
    };
    let submit = |command| {
        runtime
            .submit(command)
            .map(|_| ())
            .map_err(|_| Stop(Ieee802154AirCheckStop::CommandRejected))
    };
    submit(RadioCommand::Enable { id: id() })?;

    submit(RadioCommand::EnergyScan(EnergyScanRequest {
        id: id(),
        channel,
        duration_us: request.energy_scan_micros,
    }))?;
    cycle.energy = match next_event(system).await? {
        Ieee802154RadioEvent::EnergyScanDone { energy_dbm, .. } => {
            Ieee802154AirEnergyOutcome::Energy(energy_dbm)
        }
        Ieee802154RadioEvent::EnergyScanFailed { .. } => Ieee802154AirEnergyOutcome::Failed,
        _ => return Err(Stop(Ieee802154AirCheckStop::UnexpectedEvent)),
    };

    submit(RadioCommand::ClearChannelAssessment { id: id(), channel })?;
    cycle.cca = match next_event(system).await? {
        Ieee802154RadioEvent::ClearChannelAssessmentDone { idle: true, .. } => {
            Ieee802154AirCcaOutcome::Clear
        }
        Ieee802154RadioEvent::ClearChannelAssessmentDone { idle: false, .. } => {
            Ieee802154AirCcaOutcome::Busy
        }
        Ieee802154RadioEvent::ClearChannelAssessmentFailed { .. } => {
            Ieee802154AirCcaOutcome::Failed
        }
        _ => return Err(Stop(Ieee802154AirCheckStop::UnexpectedEvent)),
    };

    let frame =
        FrameView::new(&FRAME).map_err(|_| Stop(Ieee802154AirCheckStop::UnsupportedSetup))?;
    let requested_at_micros = now_micros();
    submit(RadioCommand::Transmit(TxRequest {
        id: id(),
        frame,
        channel,
        mode: TxMode::Direct,
        transmit_power_dbm: None,
    }))?;
    cycle.direct = transmitted(system, requested_at_micros).await?;

    for scheduled in &mut cycle.scheduled {
        let at = now_micros() + u64::from(request.scheduled_lead_micros);
        submit(RadioCommand::Transmit(TxRequest {
            id: id(),
            frame,
            channel,
            mode: TxMode::Scheduled {
                at: RadioTimestamp::from_micros(at),
            },
            transmit_power_dbm: None,
        }))?;
        *scheduled = transmitted(system, at).await?;
    }

    submit(RadioCommand::Configure {
        id: id(),
        configuration: Configuration::Promiscuous(true),
    })?;
    submit(RadioCommand::Receive { id: id(), channel })?;
    let window = Timer::after(Duration::from_millis(u64::from(
        request.receive_window_millis,
    )));
    let mut window = core::pin::pin!(window);
    loop {
        match embassy_futures::select::select(window.as_mut(), runtime.next_event()).await {
            embassy_futures::select::Either::First(()) => break,
            embassy_futures::select::Either::Second(Ok(Ieee802154RadioEvent::Received(frame))) => {
                cycle.received_frames = cycle.received_frames.saturating_add(1);
                let rssi = frame.metadata.rssi_dbm;
                cycle.strongest_rssi_dbm = Some(match cycle.strongest_rssi_dbm {
                    Some(strongest) => strongest.max(rssi),
                    None => rssi,
                });
            }
            embassy_futures::select::Either::Second(Ok(_)) => {
                return Err(Stop(Ieee802154AirCheckStop::UnexpectedEvent));
            }
            embassy_futures::select::Either::Second(Err(_)) => {
                return Err(Stop(Ieee802154AirCheckStop::EventsLost));
            }
        }
    }
    submit(RadioCommand::Sleep { id: id() })?;
    submit(RadioCommand::Disable { id: id() })
}

async fn next_event(system: &Ieee802154System) -> Result<Ieee802154RadioEvent, Stop> {
    match with_timeout(EVENT_TIMEOUT, system.runtime().next_event()).await {
        Ok(Ok(event)) => Ok(event),
        Ok(Err(_)) => Err(Stop(Ieee802154AirCheckStop::EventsLost)),
        Err(_) => Err(Stop(Ieee802154AirCheckStop::EventTimeout)),
    }
}

async fn transmitted(
    system: &Ieee802154System,
    requested_at_micros: u64,
) -> Result<Ieee802154AirTransmit, Stop> {
    let Ieee802154RadioEvent::TransmitDone { status, .. } = next_event(system).await? else {
        return Err(Stop(Ieee802154AirCheckStop::UnexpectedEvent));
    };
    Ok(Ieee802154AirTransmit {
        outcome: tx_outcome(status),
        requested_at_micros,
        done_at_micros: now_micros(),
    })
}
