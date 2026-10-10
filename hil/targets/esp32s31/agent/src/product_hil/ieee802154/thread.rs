//! Host-driven Thread session: OpenThread over the composed client.
//!
//! The session starts the IEEE 802.15.4 client once, builds OpenThread over
//! its runtime through the OpenThread radio adapter, as the Thread example
//! does, joins the network of the host's active dataset as an end device
//! and binds one UDP socket. It then serves the host's commands until it
//! stops: report the role, RLOC16 and mesh-local EID, send a datagram, and
//! report and forget the datagrams received so far. The radio system's PHY
//! tracking runs beside it. The image is terminal.

use core::net::{Ipv6Addr, SocketAddrV6};

use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use esp_hal::{efuse, rng::Trng};
use oer_esp32s31_ieee802154_system::{
    IEEE802154_DEFAULT_TX_POWER_DBM, IEEE802154_RECEIVE_SENSITIVITY_DBM, Ieee802154SystemPort,
    Ieee802154SystemRuntime,
};
use oer_esp32s31_radio_esp_hal::EspHalRadioPlatform;
use oer_hil_protocol::{
    ieee802154::IEEE802154_THREAD_RECORDED_DATAGRAMS, ieee802154::Ieee802154SessionResult,
    ieee802154::Ieee802154ThreadDatagram, ieee802154::Ieee802154ThreadPayload,
    ieee802154::Ieee802154ThreadReceiveEvidence, ieee802154::Ieee802154ThreadRole,
    ieee802154::Ieee802154ThreadSendRequest, ieee802154::Ieee802154ThreadStartRequest,
    ieee802154::Ieee802154ThreadState,
};
use oer_ieee802154::{
    Ieee802154RadioPort, LifecycleCommand, LifecycleEvent, RadioCommand, RadioEvent, RadioPort,
    RequestId,
};
use oer_ieee802154_openthread::{
    MonotonicRadioClock, OPEN_THREAD_RADIO_CAPABILITIES, OpenThreadRadio, OpenThreadRadioDefaults,
    PortRssi,
};
use oer_time_embassy::EmbassyClock;
use openthread::{
    DeviceRole, OpenThread, OtResources, OtUdpResources, SimpleRamSettings, UdpSocket,
};
use static_cell::{ConstStaticCell, StaticCell};
use tinyrlibc as _;

use super::client::Client;

/// The image's own sleep after the stack stopped: the last consumer
/// identity below the backend-reserved range (`0xFFFF_FF00..`), which the
/// OpenThread radio, counting up from one, does not reach in a session.
const STOP_SLEEP: RequestId = RequestId::new(0xFFFF_FEFF);
use crate::console::{
    Ieee802154ThreadCommand, publish_event_reliably, receive_ieee802154_thread_command,
};

/// Frames OpenThread has not taken yet while it transmits or scans.
const RX_QUEUE: usize = 8;
const UDP_SOCKETS: usize = 1;
const UDP_BUFFER: usize = 1280;

type ThreadRadio<'p> = OpenThreadRadio<'p, Ieee802154SystemPort, RX_QUEUE>;

static TRNG: StaticCell<Trng> = StaticCell::new();
static OT_RESOURCES: StaticCell<OtResources> = StaticCell::new();
static OT_UDP: StaticCell<OtUdpResources<UDP_SOCKETS, UDP_BUFFER>> = StaticCell::new();
static OT_SETTINGS_BUFFER: ConstStaticCell<[u8; 1024]> = ConstStaticCell::new([0; 1024]);
static OT_SETTINGS: StaticCell<SimpleRamSettings> = StaticCell::new();
static RADIO_CLOCK: StaticCell<MonotonicRadioClock<&'static EmbassyClock>> = StaticCell::new();
static UDP_RECEIVE: ConstStaticCell<[u8; UDP_BUFFER]> = ConstStaticCell::new([0; UDP_BUFFER]);

/// The IEEE 802.15.4 EUI-64 as ESP-IDF derives it
/// (`esp_read_mac(ESP_MAC_IEEE802154)`): the base MAC's first three bytes,
/// the eFuse MAC extension, then the base MAC's last three bytes.
fn ieee_eui64() -> [u8; 8] {
    let base = efuse::base_mac_address();
    let base = base.as_bytes();
    let extension = efuse::read_field_le::<u16>(efuse::MAC_EXT).to_le_bytes();
    [
        base[0],
        base[1],
        base[2],
        extension[0],
        extension[1],
        base[3],
        base[4],
        base[5],
    ]
}

const fn role(role: DeviceRole) -> Ieee802154ThreadRole {
    match role {
        DeviceRole::Disabled => Ieee802154ThreadRole::Disabled,
        DeviceRole::Detached => Ieee802154ThreadRole::Detached,
        DeviceRole::Child => Ieee802154ThreadRole::Child,
        DeviceRole::Router => Ieee802154ThreadRole::Router,
        DeviceRole::Leader => Ieee802154ThreadRole::Leader,
        DeviceRole::Other(_) => Ieee802154ThreadRole::Other,
    }
}

/// The mesh-local EID: the mesh-local address that is not a routing
/// locator (whose interface identifier is `0000:00ff:fe00:xxxx`).
fn mesh_local_eid(ot: &OpenThread<'_>) -> Option<[u8; 16]> {
    let prefix = ot.mesh_local_prefix();
    let mut eid = None;
    let _ = ot.ipv6_addrs(|address| {
        if let Some((address, _)) = address {
            let octets = address.octets();
            if octets[..8] == prefix && octets[8..14] != [0, 0, 0, 0xff, 0xfe, 0] {
                eid = Some(octets);
            }
        }
        Ok(())
    });
    eid
}

/// Datagrams received since the previous collection.
#[derive(Default)]
struct Received {
    total: u16,
    datagrams: heapless::Vec<Ieee802154ThreadDatagram, IEEE802154_THREAD_RECORDED_DATAGRAMS>,
    truncated: bool,
}

impl Received {
    fn record(&mut self, payload: &[u8], remote: &SocketAddrV6) {
        self.total = self.total.saturating_add(1);
        let Ok(payload) = Ieee802154ThreadPayload::from_slice(payload) else {
            self.truncated = true;
            return;
        };
        let _ = self.datagrams.push(Ieee802154ThreadDatagram {
            source: remote.ip().octets(),
            port: remote.port(),
            payload,
        });
    }

    fn take(&mut self) -> Ieee802154ThreadReceiveEvidence {
        let Self {
            total,
            datagrams,
            truncated,
        } = core::mem::take(self);
        Ieee802154ThreadReceiveEvidence {
            result: if truncated {
                Ieee802154SessionResult::EventsLost
            } else {
                Ieee802154SessionResult::Done
            },
            total,
            datagrams,
        }
    }
}

/// Serve host commands and receive datagrams until the host stops the
/// session; returns the stop request.
async fn serve(ot: &OpenThread<'_>, socket: &UdpSocket<'_>) -> u32 {
    let buffer = UDP_RECEIVE.take();
    let mut received = Received::default();
    loop {
        let command = match select(receive_ieee802154_thread_command(), socket.recv(buffer)).await {
            Either::First(command) => command,
            Either::Second(Ok((length, _, remote))) => {
                received.record(&buffer[..length], &remote);
                continue;
            }
            Either::Second(Err(_)) => continue,
        };
        match command {
            Ieee802154ThreadCommand::Query { request_id } => {
                let state = Ieee802154ThreadState {
                    result: Ieee802154SessionResult::Done,
                    role: role(ot.device_role()),
                    rloc16: ot.rloc16(),
                    mesh_local_eid: mesh_local_eid(ot),
                };
                publish_event_reliably(
                    0,
                    request_id,
                    oer_hil_protocol::ieee802154::ThreadState(state),
                )
                .await;
            }
            Ieee802154ThreadCommand::Send {
                request_id,
                request:
                    Ieee802154ThreadSendRequest {
                        destination,
                        port,
                        payload,
                    },
            } => {
                let destination = SocketAddrV6::new(Ipv6Addr::from(destination), port, 0, 0);
                let result = match socket.send(&payload, None, &destination).await {
                    Ok(()) => Ieee802154SessionResult::Done,
                    Err(_) => Ieee802154SessionResult::CommandRejected,
                };
                publish_event_reliably(
                    0,
                    request_id,
                    oer_hil_protocol::ieee802154::ThreadSent(result),
                )
                .await;
            }
            Ieee802154ThreadCommand::Collect { request_id } => {
                publish_event_reliably(
                    0,
                    request_id,
                    oer_hil_protocol::ieee802154::ThreadReceived(received.take()),
                )
                .await;
            }
            Ieee802154ThreadCommand::Stop { request_id } => return request_id,
        }
    }
}

/// Build OpenThread over the runtime's clock and the port's live RSSI
/// (`rssi`), apply the dataset and link mode, start Thread and bind the
/// socket. OpenThread borrows the port for the session, which gives it back
/// to the client's stop.
fn start_openthread<'p>(
    request: &Ieee802154ThreadStartRequest,
    trng: Trng,
    runtime: &'static Ieee802154SystemRuntime,
    rssi: &'p PortRssi<'p, Ieee802154SystemPort>,
) -> Option<(OpenThread<'p>, UdpSocket<'p>)> {
    let ot_resources = OT_RESOURCES.init(OtResources::new());
    // OpenThread's `SubMac` reads the radio capabilities when the instance
    // is built: transmit security then stays with the radio.
    ot_resources.set_radio_caps(OPEN_THREAD_RADIO_CAPABILITIES);
    let ot = OpenThread::new_with_udp(
        ieee_eui64(),
        TRNG.init(trng),
        OT_SETTINGS.init(SimpleRamSettings::new(OT_SETTINGS_BUFFER.take())),
        RADIO_CLOCK.init(MonotonicRadioClock::new(runtime.clock())),
        Some(rssi),
        ot_resources,
        OT_UDP.init(OtUdpResources::new()),
    )
    .ok()?;
    ot.set_active_dataset_tlv(&request.dataset).ok()?;
    ot.set_link_mode(request.rx_on_when_idle, false, false)
        .ok()?;
    ot.enable_ipv6(true).ok()?;
    ot.enable_thread(true).ok()?;
    let socket = UdpSocket::bind(
        ot.clone(),
        &SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, request.udp_port, 0, 0),
    )
    .ok()?;
    Some((ot, socket))
}

/// Run one Thread session until the host stops it. The image is terminal.
pub(in crate::product_hil) async fn run_thread(
    spawner: embassy_executor::Spawner,
    platform: EspHalRadioPlatform,
    request_id: u32,
    request: Ieee802154ThreadStartRequest,
) {
    let started = |result| oer_hil_protocol::ieee802154::ThreadStarted(result);
    let Some((mut client, parked)) = Client::claim(spawner, platform) else {
        publish_event_reliably(
            0,
            request_id,
            started(Ieee802154SessionResult::UnsupportedSetup),
        )
        .await;
        return;
    };
    let system = {
        let started = core::pin::pin!(client.start(parked));
        started.await
    };
    let Some((system, port)) = system else {
        publish_event_reliably(0, request_id, started(Ieee802154SessionResult::StartFailed)).await;
        return;
    };
    let rssi = PortRssi::new(&port);
    let joined = Trng::try_new()
        .ok()
        .and_then(|trng| start_openthread(&request, trng, system.runtime(), &rssi));
    if joined.is_none() {
        // Nothing borrows the port any more.
        drop(joined);
        publish_event_reliably(0, request_id, started(Ieee802154SessionResult::StartFailed)).await;
        let stopped = core::pin::pin!(client.stop(system, port));
        let _ = stopped.await;
        return;
    }
    let (ot, socket) = joined.expect("the stack started");
    publish_event_reliably(0, request_id, started(Ieee802154SessionResult::Done)).await;

    let radio: ThreadRadio<'_> = OpenThreadRadio::new(
        &port,
        OpenThreadRadioDefaults::esp_idf(
            IEEE802154_DEFAULT_TX_POWER_DBM,
            IEEE802154_RECEIVE_SENSITIVITY_DBM,
        ),
    );
    let tracking_stop = Signal::<CriticalSectionRawMutex, ()>::new();
    let tracking = async {
        // The radio system's periodic PHY tracking, as ESP-IDF's
        // `phy_track_pll` timer runs it beside OpenThread.
        let _ = client
            .radio
            .run_tracking_until(tracking_stop.wait(), |_| {})
            .await;
    };
    let session = async {
        // The stack's radio, alarm and tasklet loops share one large future.
        let running = core::pin::pin!(ot.run(radio));
        // The runtime's runner progresses backoffs beside OpenThread.
        let runner = core::pin::pin!(system.run());
        let served = core::pin::pin!(serve(&ot, &socket));
        let stop_request = match select(select(running, runner), served).await {
            Either::First(Either::First(never)) => match never {},
            Either::First(Either::Second(never)) => match never {},
            Either::Second(stop_request) => stop_request,
        };
        tracking_stop.signal(());
        stop_request
    };
    let ((), stop_request) = core::pin::pin!(join(tracking, session)).await;

    let _ = ot.enable_thread(false);
    let _ = ot.enable_ipv6(false);
    // OpenThread gives the port back.
    drop(socket);
    drop(ot);
    // The stack has stopped: the image leaves receive mode and disables
    // the radio itself before the client stops, taking the events up to
    // the terminal of `Disable`, so the runtime's queue starts empty for
    // the next session.
    let _ = port.submit(RadioCommand::Sleep { id: STOP_SLEEP });
    if let Ok(Ok(())) = port.lifecycle(LifecycleCommand::Disable).await {
        loop {
            let Ok(event) = port.next_event().await;
            if let Ok(event) = event
                && let RadioEvent::Lifecycle(LifecycleEvent::Disabled) =
                    Ieee802154SystemPort::view(&event)
            {
                break;
            }
        }
    }
    let stopped = {
        let stopped = core::pin::pin!(client.stop(system, port));
        stopped.await
    };
    let result = match stopped {
        Some(_parked) => Ieee802154SessionResult::Done,
        None => Ieee802154SessionResult::StopFailed,
    };
    publish_event_reliably(
        0,
        stop_request,
        oer_hil_protocol::ieee802154::ThreadStopped(result),
    )
    .await;
}
