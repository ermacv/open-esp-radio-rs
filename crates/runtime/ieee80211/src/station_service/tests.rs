//! The service on the model, the test playing the access point: the
//! station's frames go at the rates its selection picks, and the control
//! record switches where the old ESP32-S31 path switched it, once the
//! station is connected ([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6132066470)).

use core::cell::RefCell;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Poll, Waker};

use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::Channel as CommandChannel;
use oer_espressif_ieee80211_policy::{rate_control::EspressifRateControl, rate_selection::EspressifRateSelection};
use oer_ieee80211_arbiter::ArbiterPolicy;
use oer_ieee80211_lower_mac::{Channel, ChannelWidth, IndividualAddress, LowerMacSetup, RxEvidence, RxMeta};
use oer_ieee80211_mac::{
    phy::LegacyRate,
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
    AirFrame, ModelAir, ModelBackend, ModelCaps24, ModelLarge, ModelPhase, ModelResources,
    ModelSetup, ModelSmall, ModelTypes,
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

    fn event(event: LowerMacEvent<'id, ModelTypes>) -> LowerMacEvent<'id, ModelTypes> {
        event
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
fn meta() -> RxMeta {
    let mut meta = RxMeta::unavailable(channel6());
    meta.rssi_dbm = RxEvidence::HardwareObserved(-40);
    meta.noise_floor_dbm = RxEvidence::HardwareObserved(-95);
    meta
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

/// Poll `future` with a waker that does nothing until it is ready.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut context = core::task::Context::from_waker(Waker::noop());
    for _ in 0..100_000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
    }
    panic!("the test never ended");
}

/// The world the test plays: the air, the clock, the application's ends.
struct World<'b> {
    air: Rc<ModelAir>,
    clock: VirtualClock,
    commands: CommandChannel<NoopRawMutex, ServiceCommand<ModelCaps24>, 4>,
    outputs: RefCell<Vec<ServiceOutput<ModelCaps24>>>,
    /// The network stack's end of the transmit queue.
    stack: RefCell<zerocopy_channel::Sender<'b, NoopRawMutex, TxMsdu>>,
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

    async fn command(&self, command: ServiceCommand<ModelCaps24>) {
        self.commands.send(command).await;
    }

    /// The network stack sends one Ethernet frame to `destination`.
    fn send_msdu(&self, destination: [u8; 6]) {
        let mut stack = self.stack.borrow_mut();
        let slot = stack.try_send().expect("the queue has room");
        slot.priority = WmmUserPriority::new(0).unwrap();
        slot.frame[..6].copy_from_slice(&destination);
        slot.frame[6..12].copy_from_slice(&LOCAL);
        slot.frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        slot.frame[14..114].fill(0xab);
        slot.len = 114;
        stack.send_done();
    }

    /// Scan, authenticate and associate: the probe, the Authentication and
    /// the Association Request as they went.
    async fn associate(&self, seen: &mut usize, protected: bool) -> [AirFrame; 3] {
        self.command(ServiceCommand::Station(StaCommand::Connect)).await;
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
    security: StaSecurity,
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
            let (arbiter, place, absence) =
                PortArbiter::<ModelBackend<ModelCaps24>>::new(ArbiterPolicy::DEFAULT, channel, gate);
            let station = Station::new(
                config(self.security),
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
    let (receive, _stack) = receive.split();
    let world = World {
        air,
        clock: VirtualClock::new(),
        commands: CommandChannel::new(),
        outputs: RefCell::new(Vec::new()),
        stack: RefCell::new(stack),
    };
    let uninstalled = block_on(run_port(
        backend,
        Body {
            world: &world,
            transmit,
            receive,
            security,
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
fn on_an_open_network_control_frames_switch_to_the_association_record_once_associated() {
    run(StaSecurity::Open, |world| Box::pin(async move {
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
            EspressifRateControl::for_peer((), &peer, Some(55)).mpdu_rate()
        );
        world
            .command(ServiceCommand::Station(StaCommand::Disconnect))
            .await;
        let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
        assert_eq!(deauthentication.rate, ofdm6());
    }));
}

#[test]
fn on_wpa2_the_handshake_goes_unprotected_on_the_default_record_and_group_message_2_under_the_key() {
    run(
        StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(PMK),
        },
        |world| Box::pin(async move {
            let mut seen = 0;
            let [_, authentication, association] = world.associate(&mut seen, true).await;
            assert_eq!(authentication.rate, dsss1());
            assert_eq!(association.rate, dsss1());
            // Associated, not connected: Message 2 on the default record.
            let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
            world.air.receive(&eapol_frame(message1.as_bytes()), meta());
            let message2 = world.sent(&mut seen, DATA).await;
            assert_eq!(message2.rate, dsss1());
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
            let wrapped =
                oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
            let message3 = RsnTxFrame::<512>::message3(
                Akm::Psk,
                LOCAL,
                2,
                ANONCE,
                [0; 8],
                wrapped.as_bytes(),
            )
            .unwrap()
            .authenticate(&ptk);
            world.air.receive(&eapol_frame(message3.as_bytes()), meta());
            // Message 4 is the handshake's: still the default record, and
            // unprotected.
            let message4 = world.sent(&mut seen, DATA).await;
            assert_eq!(message4.rate, dsss1());
            assert_eq!(message4.key, None);
            assert_eq!(message4.frame[1] & 0x40, 0);
            world
                .notice(StaNotice::Connected {
                    bssid: BSSID,
                    channel: 6,
                })
                .await;
            // Connected: Group Message 2 goes under the pairwise key the
            // link lends, with the Protected bit, on the association's
            // record.
            world.air.receive(&group_message1(&ptk, 3, 2), meta());
            let group_message2 = world.sent(&mut seen, DATA).await;
            assert!(matches!(group_message2.key, Some((_, true))), "{group_message2:?}");
            assert_ne!(group_message2.frame[1] & 0x40, 0);
            assert!(group_message2.packet_number.is_some());
            assert_eq!(group_message2.rate, ofdm6());
            // Connected: the association's record, 6 Mb/s OFDM first.
            world
                .command(ServiceCommand::Station(StaCommand::Disconnect))
                .await;
            let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
            assert_eq!(deauthentication.rate, ofdm6());
        }),
    );
}
