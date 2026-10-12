//! The service on the model, the test playing the access point: the
//! station's frames go at the rates its selection picks, and the control
//! record switches once the station associated, the handshake's EAPOL on
//! the association's record (owner decision on #563, a deliberate
//! departure from the old ESP32-S31 path, which switched once connected).

use core::cell::RefCell;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Poll, Waker};

use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::Channel as CommandChannel;
use oer_espressif_ieee80211_policy::{
    rate_control::EspressifRateControl, rate_selection::EspressifRateSelection,
};
use oer_ieee80211_arbiter::ArbiterPolicy;
use oer_ieee80211_lower_mac::{
    Channel, ChannelWidth, IndividualAddress, LowerMacSetup, SignalMetadata,
};
use oer_ieee80211_mac::{
    phy::LegacyRate,
    phy::PhyRate,
    security::rsn::Akm,
    station::{AssociationCapabilities, association::PhyMode},
};
use oer_ieee80211_port::{Ended, PortBody, run_port};
use oer_ieee80211_rsn::{Pmk, PtkContext, frames::RsnTxFrame};
use oer_ieee80211_sta::machine::{StaConfig, StaSecurity};
use oer_ieee80211_upper_mac::rate_control::RateControl;
use oer_time::Duration;
use oer_time_virtual::VirtualClock;
use std::boxed::Box;
use std::rc::Rc;
use std::vec::Vec;

use super::*;
use crate::lower_mac_owner::model::{
    AirFrame, MODEL_RX_EVENTS, ModelAir, ModelAnyBuffer, ModelBackend, ModelCaps24, ModelLarge,
    ModelPhase, ModelResources, ModelRxMeta, ModelSetup, ModelSmall, ModelTypes,
};

const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const ACCESS_POINT_ADDRESS: [u8; 6] = [0x02, 0, 0, 0, 0, 0x99];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];
const SSID: &[u8] = b"open-net";
const SNONCE: [u8; 32] = [3; 32];
const ANONCE: [u8; 32] = [4; 32];
const PMK: [u8; 32] = [7; 32];
/// CCMP-128 with PSK.
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

impl<'id> StationPort<'id> for ModelBackend<ModelCaps24> {
    type Types = ModelTypes;
    type Small = ModelSmall;
    type Large = ModelLarge;

    fn lend<'a, 'r>(
        phase: &'r mut ModelPhase<'a, 'id, ModelCaps24>,
    ) -> &'r mut impl LowerMacPhase<'a, 'id, Caps = ModelCaps24, Types = ModelTypes>
    where
        'id: 'a,
    {
        phase
    }

    fn tune(setup: &mut ModelSetup<'_, ModelCaps24>, channel: SupportedChannel<ModelCaps24>) {
        setup.set_channel(channel);
    }

    const RX_BUFFERS: usize = MODEL_RX_EVENTS;

    fn event(event: LowerMacEvent<'id, ModelTypes>) -> LowerMacEvent<'id, ModelTypes> {
        event
    }

    fn sort(buffer: ModelAnyBuffer<'id>) -> Sorted<'id, Self> {
        match buffer {
            ModelAnyBuffer::Small(buffer) => Sorted::Small(buffer),
            ModelAnyBuffer::SmallNumbered(buffer) => Sorted::SmallNumbered(buffer),
            ModelAnyBuffer::Large(buffer) => Sorted::Large(buffer),
            ModelAnyBuffer::LargeNumbered(buffer) => Sorted::LargeNumbered(buffer),
        }
    }
}

static CAPABILITIES: AssociationCapabilities = AssociationCapabilities {
    ht20: [0; 28],
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: [0; 14],
    wmm: [0; 9],
};

fn nonce() -> [u8; 32] {
    SNONCE
}

fn channel6() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}

fn config(security: StaSecurity) -> StaConfig<ModelCaps24> {
    let mut ssid = [0; 32];
    ssid[..SSID.len()].copy_from_slice(SSID);
    let mut channels = [None; 16];
    channels[0] = SupportedChannel::new(channel6());
    StaConfig {
        address: LOCAL,
        ssid,
        ssid_len: SSID.len() as u8,
        channels,
        dwell: Duration::from_millis(10),
        skipped_absences: 3,
        access_point_margin_db: 6,
        supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
        association: &CAPABILITIES,
        phy: PhyMode::Legacy,
        security,
        nonce,
        he_packet_padding: |_| oer_ieee80211_upper_mac::HePacketPadding::None,
    }
}

/// A beacon of the network on channel 6, protected or not.
fn beacon(protected: bool) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 55];
    frame[0..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&[0xff; 6]);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    let capability: u16 = if protected { 0x0011 } else { 0x0001 };
    frame[34..36].copy_from_slice(&capability.to_le_bytes());
    frame[37] = SSID.len() as u8;
    frame[38..46].copy_from_slice(SSID);
    frame[46] = 1;
    frame[47] = 4;
    frame[48..52].copy_from_slice(&[0x82, 0x84, 0x8b, 0x96]);
    frame[52] = 3;
    frame[53] = 1;
    frame[54] = 6;
    if protected {
        frame.extend_from_slice(&RSN);
    }
    frame
}

fn authentication_response() -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
    frame
}

fn association_response(protected: bool) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x0010_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    let capability: u16 = if protected { 0x0431 } else { 0x0421 };
    frame[24..26].copy_from_slice(&capability.to_le_bytes());
    frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
    frame
}

/// An EAPOL frame from the access point, in a data frame from the DS.
fn eapol_frame(eapol: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x0208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    frame.extend_from_slice(eapol);
    frame
}

/// Group Message 1 of key `key_id` with replay counter `replay`.
fn group_message1(ptk: &oer_ieee80211_rsn::Ptk, replay: u64, key_id: u8) -> Vec<u8> {
    let mut plain = Vec::new();
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, key_id, 0x00]);
    plain.extend_from_slice(&[0x77; 16]);
    plain.extend_from_slice(&[0xdd, 0, 0, 0, 0, 0, 0, 0]);
    let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
    let message =
        RsnTxFrame::<512>::group_message1(Akm::Psk, LOCAL, replay, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(ptk);
    eapol_frame(message.as_bytes())
}

/// What the port reported of a frame the access point sent: its signal
/// over the noise floor, 55 dB.
fn meta() -> ModelRxMeta {
    ModelRxMeta::new(channel6(), -40, -95)
}

/// The Frame Control subtype bits of a frame.
fn kind(frame: &AirFrame) -> u16 {
    u16::from_le_bytes([frame.frame[0], frame.frame[1]]) & 0x00fc
}

const PROBE_REQUEST: u16 = 0x0040;
const AUTHENTICATION: u16 = 0x00b0;
const ASSOCIATION_REQUEST: u16 = 0x0000;
const DEAUTHENTICATION: u16 = 0x00c0;
const DATA: u16 = 0x0008;

/// Let the port's other futures run once.
async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

/// How long a service test may take before its watchdog fails it.
const WATCHDOG: std::time::Duration = std::time::Duration::from_secs(30);

/// Poll `future` with a waker that does nothing until it is ready, under a
/// watchdog: a loop inside one poll never returns to the poll count, so a
/// watchdog thread aborts the test process once `WATCHDOG` passed, and a
/// loop is a failed test, not a hang. (A panic in the watchdog's own
/// thread would not fail the test.)
fn block_on<F: Future>(future: F) -> F::Output {
    let (done, watched) = std::sync::mpsc::channel::<()>();
    let test = std::string::String::from(std::thread::current().name().unwrap_or("a test"));
    let watchdog = std::thread::spawn(move || {
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = watched.recv_timeout(WATCHDOG) {
            std::eprintln!("watchdog: {test} did not end within {WATCHDOG:?}");
            std::process::abort();
        }
    });
    let mut future = core::pin::pin!(future);
    let mut context = core::task::Context::from_waker(Waker::noop());
    let mut value = None;
    for _ in 0..100_000 {
        if let Poll::Ready(ready) = future.as_mut().poll(&mut context) {
            value = Some(ready);
            break;
        }
    }
    let _ = done.send(());
    let _ = watchdog.join();
    value.expect("the test never ended")
}

/// The world the test plays: the air, the clock, the application's ends.
struct World<'b> {
    air: Rc<ModelAir>,
    clock: VirtualClock,
    commands: CommandChannel<NoopRawMutex, ServiceCommand<ModelCaps24>, 4>,
    outputs: RefCell<Vec<ServiceOutput<ModelCaps24>>>,
    /// The network stack's end of the transmit queue.
    stack: RefCell<zerocopy_channel::Sender<'b, NoopRawMutex, TxMsdu>>,
    /// The network stack's end of the receive queue.
    delivered: RefCell<zerocopy_channel::Receiver<'b, NoopRawMutex, RxMsdu>>,
}

impl World<'_> {
    /// The next frame of `subtype` the station sent after the first
    /// `seen`, advancing the clock to the machines' deadlines while it
    /// waits.
    async fn sent(&self, seen: &mut usize, subtype: u16) -> AirFrame {
        for _ in 0..1000 {
            for _ in 0..20 {
                yield_now().await;
            }
            let sent = self.air.sent();
            while *seen < sent.len() {
                let frame = sent[*seen].clone();
                *seen += 1;
                if kind(&frame) == subtype {
                    return frame;
                }
            }
            self.clock.advance_to_next_deadline();
        }
        panic!("the station never sent {subtype:#06x}");
    }

    /// Wait until the service told the application `notice`.
    async fn notice(&self, notice: StaNotice) {
        for _ in 0..1000 {
            yield_now().await;
            if self
                .outputs
                .borrow()
                .iter()
                .any(|output| matches!(output, ServiceOutput::Notice(seen) if *seen == notice))
            {
                return;
            }
        }
        panic!("no {notice:?}");
    }

    /// The Ethernet frames the network stack received, once the service
    /// had the chance to deliver what the air holds.
    async fn delivered(&self) -> Vec<Vec<u8>> {
        for _ in 0..50 {
            yield_now().await;
        }
        let mut delivered = self.delivered.borrow_mut();
        let mut frames = Vec::new();
        while let Some(msdu) = delivered.try_receive() {
            frames.push(msdu.frame[..msdu.len].to_vec());
            delivered.receive_done();
        }
        frames
    }

    /// Run the 4-way handshake as the access point after the association:
    /// the pairwise key it derived, once the station is connected.
    async fn handshake(&self, seen: &mut usize) -> oer_ieee80211_rsn::Ptk {
        let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
        self.air.receive(&eapol_frame(message1.as_bytes()), meta());
        let message2 = self.sent(seen, DATA).await;
        assert_eq!(message2.rate, ofdm6());
        let ptk = Pmk::from_bytes(PMK).derive_ptk(
            Akm::Psk,
            PtkContext {
                authenticator_address: BSSID,
                supplicant_address: LOCAL,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            },
        );
        let mut plain = Vec::new();
        plain.extend_from_slice(&RSN);
        plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
        plain.extend_from_slice(&[0x99; 16]);
        plain.extend_from_slice(&[0xdd, 0]);
        let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
        let message3 =
            RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
                .unwrap()
                .authenticate(&ptk);
        self.air.receive(&eapol_frame(message3.as_bytes()), meta());
        // Message 4: the association's record, and unprotected.
        let message4 = self.sent(seen, DATA).await;
        assert_eq!(message4.rate, ofdm6());
        assert_eq!(message4.key, None);
        assert_eq!(message4.frame[1] & 0x40, 0);
        self.notice(StaNotice::Connected {
            bssid: BSSID,
            channel: 6,
        })
        .await;
        ptk
    }

    async fn command(&self, command: ServiceCommand<ModelCaps24>) {
        self.commands.send(command).await;
    }

    /// The network stack sends one Ethernet frame to `destination`.
    fn send_msdu(&self, destination: [u8; 6]) {
        assert!(self.try_send_msdu(destination), "the queue has room");
    }

    /// The network stack sends one Ethernet frame to `destination` if its
    /// queue has room.
    fn try_send_msdu(&self, destination: [u8; 6]) -> bool {
        let mut stack = self.stack.borrow_mut();
        let Some(slot) = stack.try_send() else {
            return false;
        };
        slot.priority = WmmUserPriority::new(0).unwrap();
        slot.frame[..6].copy_from_slice(&destination);
        slot.frame[6..12].copy_from_slice(&LOCAL);
        slot.frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        slot.frame[14..114].fill(0xab);
        slot.len = 114;
        stack.send_done();
        true
    }

    /// Scan, authenticate and associate: the probe, the Authentication and
    /// the Association Request as they went.
    async fn associate(&self, seen: &mut usize, protected: bool) -> [AirFrame; 3] {
        self.command(ServiceCommand::Station(StaCommand::Connect))
            .await;
        let probe = self.sent(seen, PROBE_REQUEST).await;
        self.air.receive(&beacon(protected), meta());
        let authentication = self.sent(seen, AUTHENTICATION).await;
        self.air.receive(&authentication_response(), meta());
        let association = self.sent(seen, ASSOCIATION_REQUEST).await;
        self.air.receive(&association_response(protected), meta());
        [probe, authentication, association]
    }
}

/// The body that runs the service beside the test's script.
struct Body<'w, 'b, S> {
    world: &'w World<'b>,
    transmit: zerocopy_channel::Receiver<'b, NoopRawMutex, TxMsdu>,
    receive: zerocopy_channel::Sender<'b, NoopRawMutex, RxMsdu>,
    config: StaConfig<ModelCaps24>,
    script: S,
}

/// A test's script: the access point and the application, on the world.
type Script<'w> = Pin<Box<dyn Future<Output = ()> + 'w>>;

impl<'w, 'b, S> PortBody<ModelBackend<ModelCaps24>> for Body<'w, 'b, S>
where
    S: FnOnce(&'w World<'b>) -> Script<'w>,
{
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend<ModelCaps24>, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend<ModelCaps24>> {
        {
            let ModelResources {
                tickets,
                small,
                large,
                station_pairwise,
                station_group,
                gate,
                ..
            } = resources;
            let mut tickets = tickets.into_iter();
            let station_tickets: [_; 4] = core::array::from_fn(|_| tickets.next().unwrap());
            let data_tickets: [_; 4] = core::array::from_fn(|_| tickets.next().unwrap());
            let small: [_; 4] = small.try_into().ok().unwrap();
            let large: [_; 4] = large.try_into().ok().unwrap();
            let [pairwise, _] = station_pairwise;
            let channel = SupportedChannel::new(channel6()).unwrap();
            let (arbiter, place, absence) = PortArbiter::<ModelBackend<ModelCaps24>>::new(
                ArbiterPolicy::DEFAULT,
                channel,
                gate,
            );
            let station = Station::new(
                self.config,
                station_tickets,
                small,
                place,
                absence,
                pairwise,
                station_group,
            );
            let mut service = StationService::<_, _, 4>::new(
                station,
                StaData::new(data_tickets, large),
                arbiter,
                EspressifRateSelection::<ModelCaps24>::new(),
            );
            let world = self.world;
            let mut output = |output| world.outputs.borrow_mut().push(output);
            let mut io = ServiceIo {
                commands: world.commands.dyn_receiver(),
                transmit: self.transmit,
                receive: self.receive,
                output: &mut output,
            };
            let script = self.script;
            let (end, ()) = join(service.run(owner, &mut io, &world.clock), async move {
                script(world).await;
                world.command(ServiceCommand::Stop).await;
            })
            .await;
            match end {
                ServiceEnd::Stopped(owner) | ServiceEnd::Failed(owner) => owner.uninstall(()),
                ServiceEnd::Poisoned(poisoned) => poisoned.uninstall(()),
            }
        }
    }
}

/// Run the service on a 2.4 GHz model with `security`, the test playing
/// the access point by `script`.
fn run<S>(security: StaSecurity, script: S)
where
    S: for<'w, 'b> FnOnce(&'w World<'b>) -> Script<'w>,
{
    run_with(config(security), script);
}

/// Run the service on a 2.4 GHz model with `config`.
fn run_with<S>(config: StaConfig<ModelCaps24>, script: S)
where
    S: for<'w, 'b> FnOnce(&'w World<'b>) -> Script<'w>,
{
    let (backend, air) = ModelBackend::<ModelCaps24>::new(
        channel6(),
        [
            IndividualAddress::new(LOCAL).unwrap(),
            IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap(),
        ],
    );
    let mut transmit: [TxMsdu; 2] = core::array::from_fn(|_| TxMsdu {
        priority: WmmUserPriority::new(0).unwrap(),
        len: 0,
        frame: [0; ETHERNET_FRAME],
    });
    let mut receive: [RxMsdu; 2] = core::array::from_fn(|_| RxMsdu {
        len: 0,
        frame: [0; ETHERNET_FRAME],
    });
    let mut transmit = zerocopy_channel::Channel::<NoopRawMutex, TxMsdu>::new(&mut transmit);
    let mut receive = zerocopy_channel::Channel::<NoopRawMutex, RxMsdu>::new(&mut receive);
    let (stack, transmit) = transmit.split();
    let (receive, delivered) = receive.split();
    let world = World {
        air,
        clock: VirtualClock::new(),
        commands: CommandChannel::new(),
        outputs: RefCell::new(Vec::new()),
        stack: RefCell::new(stack),
        delivered: RefCell::new(delivered),
    };
    let uninstalled = block_on(run_port(
        backend,
        Body {
            world: &world,
            transmit,
            receive,
            config,
            script,
        },
    ));
    assert!(uninstalled.is_some(), "the port ends through its uninstall");
}

fn dsss1() -> PhyRate {
    PhyRate::Legacy(LegacyRate::Dsss1M)
}

fn ofdm6() -> PhyRate {
    PhyRate::Legacy(LegacyRate::Ofdm6M)
}

#[test]
fn on_an_open_network_the_join_goes_on_the_default_record_and_then_the_associations() {
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            let mut seen = 0;
            let [probe, authentication, association] = world.associate(&mut seen, false).await;
            // Before the connection: the probe once at 1 Mb/s, the join on the
            // default control record, 1 Mb/s DSSS.
            assert_eq!(probe.rate, dsss1());
            assert_eq!(authentication.rate, dsss1());
            assert_eq!(association.rate, dsss1());
            world
                .notice(StaNotice::Connected {
                    bssid: BSSID,
                    channel: 6,
                })
                .await;
            // Connected at once on an open network: data at the rate control's
            // rate, the next control frame on the association's record.
            world.send_msdu([0x02, 0, 0, 0, 0, 0x07]);
            let data = world.sent(&mut seen, DATA).await;
            let peer = oer_ieee80211_upper_mac::rate_control::RatePeer {
                phy: PhyMode::Legacy,
                ht_capabilities: None,
                he_capabilities: None,
                he_peer_state: None,
            };
            assert_eq!(
                data.rate,
                EspressifRateControl::for_peer((), &peer, meta().link_metric()).mpdu_rate()
            );
            world
                .command(ServiceCommand::Station(StaCommand::Disconnect))
                .await;
            let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
            assert_eq!(deauthentication.rate, ofdm6());
        })
    });
}

#[test]
fn on_wpa2_the_join_goes_on_the_default_record_and_the_handshake_on_the_associations() {
    run(
        StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                let [_, authentication, association] = world.associate(&mut seen, true).await;
                assert_eq!(authentication.rate, dsss1());
                assert_eq!(association.rate, dsss1());
                // Associated: the rates start from the association, so the
                // handshake's Message 2 goes on its record, 6 Mb/s OFDM first
                // (owner decision on #563).
                let ptk = world.handshake(&mut seen).await;
                // Connected: Group Message 2 goes under the pairwise key the
                // link lends, with the Protected bit, on the association's
                // record.
                world.air.receive(&group_message1(&ptk, 3, 2), meta());
                let group_message2 = world.sent(&mut seen, DATA).await;
                assert!(
                    matches!(group_message2.key, Some((_, true))),
                    "{group_message2:?}"
                );
                assert_ne!(group_message2.frame[1] & 0x40, 0);
                assert!(group_message2.packet_number.is_some());
                assert_eq!(group_message2.rate, ofdm6());
                world
                    .command(ServiceCommand::Station(StaCommand::Disconnect))
                    .await;
                let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                assert_eq!(deauthentication.rate, ofdm6());
            })
        },
    );
}

/// A protected data MPDU from the DS to the station, as the hardware
/// hands it up once it decrypted and verified it: the CCMP header of
/// `packet_number` stays, the MIC is gone.
fn protected_data(packet_number: u8, source: [u8; 6], payload: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x4208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&source);
    frame[22..24].copy_from_slice(&(u16::from(packet_number) << 4).to_le_bytes());
    frame.extend_from_slice(&[packet_number, 0, 0, 0x20, 0, 0, 0, 0]);
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    frame.extend_from_slice(payload);
    frame
}

fn decrypted() -> ModelRxMeta {
    meta().with_crypto(oer_ieee80211_lower_mac::RxEvidence::HardwareObserved(
        oer_ieee80211_lower_mac::RxCryptoStatus::DecryptedAndIntegrityVerified,
    ))
}

/// The whole path on the model, the test playing the access point and the
/// network stack: scan, join, the 4-way handshake, data both ways under
/// the pairwise key, and the disconnection, after which no data goes.
#[test]
fn a_station_scans_joins_handshakes_carries_data_both_ways_and_disconnects() {
    run(
        StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;

                // TX: the stack's MSDU goes as one protected MPDU under the
                // pairwise key, to the access point, its payload after the
                // CCMP header's room and the LLC/SNAP header.
                let destination = [0x02, 0, 0, 0, 0, 0x07];
                world.send_msdu(destination);
                let data = world.sent(&mut seen, DATA).await;
                assert!(matches!(data.key, Some((_, true))), "{data:?}");
                assert!(data.packet_number.is_some());
                assert_ne!(data.frame[1] & 0x40, 0);
                assert_eq!(data.frame[1] & 0x03, 0x01, "to the DS");
                assert_eq!(data.frame[4..10], BSSID);
                assert_eq!(data.frame[10..16], LOCAL);
                assert_eq!(data.frame[16..22], destination);
                assert_eq!(data.frame[32..40], [0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
                assert_eq!(data.frame[40..], [0xab; 100]);

                // RX: a decrypted MPDU reaches the stack as its Ethernet
                // frame; its replay, an unverified one and an unprotected
                // one do not.
                let source = [0x02, 0, 0, 0, 0, 0x08];
                world
                    .air
                    .receive(&protected_data(1, source, &[0xcd; 40]), decrypted());
                let mut expected = Vec::new();
                expected.extend_from_slice(&LOCAL);
                expected.extend_from_slice(&source);
                expected.extend_from_slice(&[0x08, 0x00]);
                expected.extend_from_slice(&[0xcd; 40]);
                assert_eq!(world.delivered().await, [expected]);
                world
                    .air
                    .receive(&protected_data(1, source, &[0xcd; 40]), decrypted());
                world
                    .air
                    .receive(&protected_data(2, source, &[0xcd; 40]), meta());
                let mut plain = protected_data(3, source, &[0xcd; 40]);
                plain[1] &= !0x40;
                plain.drain(24..32);
                world.air.receive(&plain, meta());
                assert!(world.delivered().await.is_empty());
                assert_eq!(world.air.pending_rx(), 0, "every buffer recycled");

                // The disconnection: Deauthentication, the notice, and no
                // data after it.
                world
                    .command(ServiceCommand::Station(StaCommand::Disconnect))
                    .await;
                let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                assert_eq!(deauthentication.frame[4..10], BSSID);
                world
                    .notice(StaNotice::Disconnected(
                        oer_ieee80211_sta::machine::StaDisconnectReason::Requested,
                    ))
                    .await;
                world.send_msdu(destination);
                for _ in 0..5 {
                    for _ in 0..20 {
                        yield_now().await;
                    }
                    world.clock.advance_to_next_deadline();
                }
                assert!(
                    world.air.sent()[seen..]
                        .iter()
                        .all(|frame| kind(frame) != DATA),
                    "no data after the disconnection"
                );
            })
        },
    );
}

const NULL: u16 = 0x0048;

/// Data takes every token of its own and the station's frames still go:
/// with the best-effort queue held, the stack fills the data machine's
/// tickets and buffers and its own queue, and the group handshake's EAPOL,
/// the Null before a background scan's absence and the Deauthentication
/// still reach the air (owner decision on #563, comment 6131495971).
#[test]
fn data_takes_all_its_tokens_and_eapol_null_and_deauthentication_still_go() {
    // A second channel, so the background scan leaves the home channel
    // and announces its absence with a Null.
    let mut config = config(StaSecurity::Wpa2Personal {
        pmk: Pmk::from_bytes(PMK),
    });
    config.channels[1] = SupportedChannel::new(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap());
    run_with(config, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.associate(&mut seen, true).await;
            let ptk = world.handshake(&mut seen).await;
            world.air.hold(WmmAccessCategory::BestEffort, true);
            // The data machine takes four MSDUs, its four tickets and
            // buffers; two more wait in the stack's queue, which then
            // refuses.
            let mut sent = 0;
            while world.try_send_msdu([0x02, 0, 0, 0, 0, 0x07]) {
                sent += 1;
                for _ in 0..20 {
                    yield_now().await;
                }
            }
            assert_eq!(sent, 4 + 2);
            assert!(
                world.air.sent()[seen..]
                    .iter()
                    .all(|frame| kind(frame) != DATA)
            );

            world.air.receive(&group_message1(&ptk, 3, 2), meta());
            let group_message2 = world.sent(&mut seen, DATA).await;
            // Protected after the handshake: the EtherType follows the
            // CCMP header's room.
            assert_ne!(group_message2.frame[1] & 0x40, 0);
            assert_eq!(group_message2.frame[38..40], [0x88, 0x8e], "EAPOL");

            world
                .command(ServiceCommand::Station(StaCommand::BackgroundScan))
                .await;
            let null = world.sent(&mut seen, NULL).await;
            assert_ne!(null.frame[1] & 0x10, 0, "PM=1 before the absence");

            world
                .command(ServiceCommand::Station(StaCommand::Disconnect))
                .await;
            world.sent(&mut seen, DEAUTHENTICATION).await;
        })
    });
}

/// A station deadline reaches the station: an Authentication without a
/// response goes again once its wait passed, and the service does not spin
/// on the passed deadline meanwhile.
#[test]
fn a_passed_station_deadline_ticks_the_station() {
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world
                .command(ServiceCommand::Station(StaCommand::Connect))
                .await;
            world.sent(&mut seen, PROBE_REQUEST).await;
            world.air.receive(&beacon(false), meta());
            world.sent(&mut seen, AUTHENTICATION).await;
            // No response: the wait passes and the station tries again.
            world.sent(&mut seen, AUTHENTICATION).await;
        })
    });
}

/// The data queue belongs to the link (owner decision on #563, comment
/// 6133717216): a disconnection during a background scan's absence, while
/// the port holds the link's attempts, leaves none of them to go after the
/// reconnection, and every data token comes back.
#[test]
fn a_disconnection_during_an_absence_leaves_no_old_frame_to_go_after_the_reconnection() {
    let mut config = config(StaSecurity::Open);
    config.channels[1] = SupportedChannel::new(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap());
    run_with(config, |world| {
        Box::pin(async move {
            const OLD: [u8; 6] = [0x02, 0, 0, 0, 0, 0x0a];
            const NEW: [u8; 6] = [0x02, 0, 0, 0, 0, 0x0b];
            let to = |destination: [u8; 6]| {
                move |frame: &AirFrame| kind(frame) == DATA && frame.frame[16..22] == destination
            };
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: BSSID,
                    channel: 6,
                })
                .await;
            // The link's four MPDUs wait at the port.
            world.air.hold(WmmAccessCategory::BestEffort, true);
            for _ in 0..4 {
                world.send_msdu(OLD);
                for _ in 0..20 {
                    yield_now().await;
                }
            }
            // The absence starts; the station leaves during it.
            world
                .command(ServiceCommand::Station(StaCommand::BackgroundScan))
                .await;
            world.sent(&mut seen, NULL).await;
            world
                .command(ServiceCommand::Station(StaCommand::Disconnect))
                .await;
            world
                .notice(StaNotice::Disconnected(
                    oer_ieee80211_sta::machine::StaDisconnectReason::Requested,
                ))
                .await;
            // Reconnected: every data token is back, so the machine takes
            // four MSDUs again and two more wait in the stack's queue.
            world.associate(&mut seen, false).await;
            let mut sent = 0;
            while world.try_send_msdu(NEW) {
                sent += 1;
                for _ in 0..20 {
                    yield_now().await;
                }
            }
            assert_eq!(sent, 4 + 2);
            // The queue opens: only the new link's frames go.
            world.air.hold(WmmAccessCategory::BestEffort, false);
            for _ in 0..6 {
                world.sent(&mut seen, DATA).await;
            }
            for _ in 0..5 {
                for _ in 0..20 {
                    yield_now().await;
                }
                world.clock.advance_to_next_deadline();
            }
            let air = world.air.sent();
            assert!(!air.iter().any(to(OLD)), "no frame of the old link went");
            assert_eq!(air.iter().filter(|frame| to(NEW)(frame)).count(), 6);
        })
    });
}

/// Beside an access point on the upstream's channel, a background scan's
/// absence starts after one of its TBTTs; MSDUs the stack sends during the
/// absence do not go while the station is away and all go once it is back,
/// none lost (owner decision on #563, comment 6131495971).
#[test]
fn beside_an_access_point_msdus_survive_an_absence() {
    use oer_time::Clock as _;
    let mut config = config(StaSecurity::Open);
    config.channels[1] = SupportedChannel::new(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap());
    run_with(config, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: BSSID,
                    channel: 6,
                })
                .await;
            let tbtt = world.clock.now().saturating_add(Duration::from_millis(5));
            world
                .command(ServiceCommand::AccessPoint(AccessPointFact::Running {
                    channel: SupportedChannel::new(channel6()).unwrap(),
                    next_tbtt: tbtt,
                    beacon_interval: Duration::from_micros(102_400),
                }))
                .await;
            world
                .command(ServiceCommand::Station(StaCommand::BackgroundScan))
                .await;
            let away = world.sent(&mut seen, NULL).await;
            assert_ne!(away.frame[1] & 0x10, 0, "PM=1 before the absence");
            assert!(world.clock.now() >= tbtt, "the absence follows a TBTT");
            let left = seen;
            let destinations = [[0x02, 0, 0, 0, 0, 0x0c], [0x02, 0, 0, 0, 0, 0x0d]];
            for destination in destinations {
                world.send_msdu(destination);
            }
            let back = world.sent(&mut seen, NULL).await;
            assert_eq!(back.frame[1] & 0x10, 0, "PM=0 once back");
            let returned = seen;
            assert!(
                world.air.sent()[left..returned]
                    .iter()
                    .all(|frame| kind(frame) != DATA),
                "no data while away"
            );
            for _ in destinations {
                world.sent(&mut seen, DATA).await;
            }
            let air = world.air.sent();
            for destination in destinations {
                assert_eq!(
                    air[returned..]
                        .iter()
                        .filter(|frame| kind(frame) == DATA && frame.frame[16..22] == destination)
                        .count(),
                    1,
                    "{destination:02x?} went once"
                );
            }
        })
    });
}
