//! The service on the model, the test playing the access point: the
//! station's frames go at the rates its selection picks, and the control
//! record switches once the station associated, the handshake's EAPOL on
//! the association's record (owner decision on #563, a deliberate
//! departure from the old ESP32-S31 path, which switched once connected).

use core::cell::{Cell, RefCell};
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Poll, Waker};

use embassy_futures::join::join;
use oer_espressif_ieee80211_policy::{
    rate_control::EspressifRateControl, rate_selection::EspressifRateSelection,
};
use oer_ieee80211_arbiter::ArbiterPolicy;
use oer_ieee80211_lower_mac::{Channel, ChannelWidth, LowerMacSetup, SignalMetadata};
use oer_ieee80211_mac::address::IndividualAddress;
use oer_ieee80211_mac::{
    phy::LegacyRate,
    phy::PhyRate,
    security::rsn::Akm,
    station::{AssociationCapabilities, association::PhyMode},
};
use oer_ieee80211_port::{Ended, PortBody, run_port};
use oer_ieee80211_rsn::{Pmk, PtkContext, frames::RsnTxFrame};
use oer_ieee80211_sta::machine::{StaConfig, StaSecurity, StationResources};
use oer_ieee80211_upper_mac::rate_control::RateControl;
use oer_time::Duration;
use oer_time_virtual::VirtualClock;
use std::boxed::Box;
use std::rc::Rc;
use std::vec::Vec;

use super::*;
use crate::lower_mac_owner::model::{
    AirFrame, MODEL_RX_EVENTS, MODEL_TX_QUEUE_DEPTH, ModelAir, ModelAnyBuffer, ModelBackend,
    ModelCaps24, ModelLarge, ModelPhase, ModelResources, ModelRxMeta, ModelSetup, ModelSmall,
    ModelTypes,
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

struct ServiceContexts;

impl crate::lower_mac_owner::model::ModelContext for ServiceContexts {
    type Context<'id> =
        TxContext<'id, ModelCaps24, ModelLarge, crate::lower_mac_owner::model::ModelPool<4>>;
}

impl<'id> StationPort<'id> for ModelBackend<ModelCaps24, ServiceContexts> {
    type Types = ModelTypes<ServiceContexts>;
    type Small = ModelSmall;
    type Large = ModelLarge;

    fn lend<'a, 'r>(
        phase: &'r mut ModelPhase<'a, 'id, ModelCaps24, ServiceContexts>,
    ) -> &'r mut impl LowerMacPhase<'a, 'id, Caps = ModelCaps24, Types = ModelTypes<ServiceContexts>>
    where
        'id: 'a,
    {
        phase
    }

    fn tune(setup: &mut ModelSetup<'_, ModelCaps24>, channel: SupportedChannel<ModelCaps24>) {
        setup.set_channel(channel);
    }

    const RX_BUFFERS: usize = MODEL_RX_EVENTS;

    fn event(
        event: LowerMacEvent<'id, ModelTypes<ServiceContexts>>,
    ) -> LowerMacEvent<'id, ModelTypes<ServiceContexts>> {
        event
    }

    fn tx_buffer(buffer: ModelAnyBuffer<'id>) -> StationTxBuffer<'id, Self> {
        match buffer {
            ModelAnyBuffer::Small(buffer) => StationTxBuffer::Small(ReturnedMpdu::Fresh(buffer)),
            ModelAnyBuffer::SmallSent(buffer) => StationTxBuffer::Small(ReturnedMpdu::Sent(buffer)),
            ModelAnyBuffer::Large(buffer) => StationTxBuffer::Large(ReturnedMpdu::Fresh(buffer)),
            ModelAnyBuffer::LargeSent(buffer) => StationTxBuffer::Large(ReturnedMpdu::Sent(buffer)),
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
    let channels =
        heapless::Vec::from_slice(&[SupportedChannel::new(channel6()).unwrap()]).unwrap();
    StaConfig {
        address: oer_ieee80211_mac::address::OwnAddress::new(
            oer_ieee80211_mac::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        ssid: heapless::Vec::from_slice(SSID).unwrap(),
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

fn authentication_response(request: &[u8]) -> Option<[u8; 30]> {
    let (request, _) = request.split_first_chunk::<30>()?;
    if u16::from_le_bytes([request[0], request[1]]) & !0x0800 != 0x00b0
        || request[4..10] != BSSID
        || request[10..16] != LOCAL
        || request[16..22] != BSSID
        || request[24..30] != [0, 0, 1, 0, 0, 0]
    {
        return None;
    }
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
    Some(frame)
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

fn protected_group_message1(ptk: &oer_ieee80211_rsn::Ptk, replay: u64, key_id: u8) -> Vec<u8> {
    use oer_ieee80211_mac::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber};
    let mut frame = group_message1(ptk, replay, key_id);
    frame[1] |= 0x40;
    let header = CcmpHeader::new(
        CcmpPacketNumber::new(replay).unwrap(),
        CcmpKeyId::new(0).unwrap(),
    );
    frame.splice(24..24, header.encode());
    frame
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
    commands: Cell<Option<MsduSender<'b, ServiceCommand<ModelCaps24>, 4>>>,
    outputs: RefCell<Vec<ServiceOutput<ModelCaps24>>>,
    completions: RefCell<Vec<FrameRates>>,
    refuse_rate: Cell<bool>,
    counters: Cell<Option<ServiceCounters>>,
    expect_queued_command_on_poison: Cell<bool>,
    /// The network stack's end of the transmit queue.
    stack: RefCell<MsduSender<'b, TxMsdu, 2>>,
    /// The network stack's end of the receive queue.
    delivered: RefCell<MsduReceiver<'b, EthernetFrame, 2>>,
}

/// Observe the operation class supplied by the service, independently of
/// the transmitted bytes and the order in which the model ends queues.
struct RecordingRates<'a> {
    inner: EspressifRateSelection<ModelCaps24>,
    completions: &'a RefCell<Vec<FrameRates>>,
    refuse_rate: &'a Cell<bool>,
}

impl RateSelection<ModelRxMeta> for RecordingRates<'_> {
    fn plan<R>(
        &self,
        frame: FrameRates,
        carried: impl Fn(PhyRate) -> Option<R>,
    ) -> Option<oer_ieee80211_lower_mac::RetryPlan<R>> {
        if self.refuse_rate.replace(false) {
            return None;
        }
        <EspressifRateSelection<ModelCaps24> as RateSelection<ModelRxMeta>>::plan(
            &self.inner,
            frame,
            carried,
        )
    }

    fn associated(
        &mut self,
        peer: &oer_ieee80211_upper_mac::rate_control::RatePeer,
        response: &ModelRxMeta,
    ) {
        self.inner.associated(peer, response);
    }

    fn disconnected(&mut self) {
        <EspressifRateSelection<ModelCaps24> as RateSelection<ModelRxMeta>>::disconnected(
            &mut self.inner,
        );
    }

    fn completed(&mut self, now: Instant, frame: FrameRates, completion: &TxOutcome) {
        self.completions.borrow_mut().push(frame);
        <EspressifRateSelection<ModelCaps24> as RateSelection<ModelRxMeta>>::completed(
            &mut self.inner,
            now,
            frame,
            completion,
        );
    }
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
        let delivered = self.delivered.borrow();
        let mut frames = Vec::new();
        while let Some(msdu) = delivered.try_receive() {
            frames.push(msdu.bytes().collect());
        }
        frames
    }

    /// Begin the 4-way handshake after association and return the derived
    /// key once the station has answered Message 1 with Message 2.
    async fn start_handshake(&self, seen: &mut usize) -> oer_ieee80211_rsn::Ptk {
        let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
        self.air.receive(&eapol_frame(message1.as_bytes()), meta());
        let message2 = self.sent(seen, DATA).await;
        assert_eq!(message2.rate, ofdm6());
        Pmk::from_bytes(PMK).derive_ptk(
            Akm::Psk,
            PtkContext {
                authenticator_address: BSSID,
                supplicant_address: LOCAL,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            },
        )
    }

    async fn handshake(&self, seen: &mut usize) -> oer_ieee80211_rsn::Ptk {
        let ptk = self.start_handshake(seen).await;
        self.air.receive(&message3(&ptk), meta());
        // Message 4: the association's record, and unprotected.
        let message4 = self.sent(seen, DATA).await;
        assert_eq!(message4.rate, ofdm6());
        assert_eq!(message4.key, None);
        assert_eq!(message4.frame[1] & 0x40, 0);
        self.notice(StaNotice::Connected {
            bssid: oer_ieee80211_mac::address::Bssid::new(
                oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
            ),
            channel: 6,
        })
        .await;
        ptk
    }

    async fn command(&self, command: ServiceCommand<ModelCaps24>) {
        let mut sender = self
            .commands
            .take()
            .expect("one application script owns the sender");
        sender.ready().await.send(command);
        self.commands.set(Some(sender));
    }

    /// The network stack sends one Ethernet frame to `destination`.
    fn send_msdu(&self, destination: [u8; 6]) {
        assert!(self.try_send_msdu(destination), "the queue has room");
    }

    /// The network stack sends one Ethernet frame to `destination` if its
    /// queue has room.
    fn try_send_msdu(&self, destination: [u8; 6]) -> bool {
        self.try_send_msdu_at(destination, 0)
    }

    /// The network stack sends one Ethernet frame of user `priority` to
    /// `destination` if its queue has room.
    fn try_send_msdu_at(&self, destination: [u8; 6], priority: u8) -> bool {
        let packet = TxMsdu {
            priority: WmmUserPriority::new(priority).unwrap(),
            frame: EthernetFrame::from_parts(oer_ieee80211_mac::data::EthernetFrameParts {
                destination,
                source: LOCAL,
                ether_type: 0x0800,
                payload: &[0xab; 100],
            })
            .unwrap(),
        };
        self.stack.borrow().try_send(packet).is_ok()
    }

    /// Scan, authenticate and associate: the probe, the Authentication and
    /// the Association Request as they went.
    async fn associate(&self, seen: &mut usize, protected: bool) -> [AirFrame; 3] {
        self.associate_with(seen, beacon(protected), protected)
            .await
    }

    /// [`Self::associate`] after `beacon`.
    async fn associate_with(
        &self,
        seen: &mut usize,
        beacon: Vec<u8>,
        protected: bool,
    ) -> [AirFrame; 3] {
        self.command(ServiceCommand::Station(StaCommand::Connect))
            .await;
        let probe = self.sent(seen, PROBE_REQUEST).await;
        self.air.receive(&beacon, meta());
        let authentication = self.sent(seen, AUTHENTICATION).await;
        let response = authentication_response(&authentication.frame)
            .expect("the test AP requires a valid wire Authentication Request");
        self.air.receive(&response, meta());
        let association = self.sent(seen, ASSOCIATION_REQUEST).await;
        self.air.receive(&association_response(protected), meta());
        [probe, authentication, association]
    }
}

/// The body that runs the service beside the test's script.
struct Body<'w, 'b, S, const GROUPS: usize> {
    world: &'w World<'b>,
    commands: MsduReceiver<'b, ServiceCommand<ModelCaps24>, 4>,
    transmit: MsduReceiver<'b, TxMsdu, 2>,
    receive: MsduSender<'b, EthernetFrame, 2>,
    config: StaConfig<ModelCaps24>,
    script: S,
}

/// A test's script: the access point and the application, on the world.
type Script<'w> = Pin<Box<dyn Future<Output = ()> + 'w>>;

impl<'w, 'b, S, const GROUPS: usize> PortBody<ModelBackend<ModelCaps24, ServiceContexts>>
    for Body<'w, 'b, S, GROUPS>
where
    S: FnOnce(&'w World<'b>) -> Script<'w>,
{
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend<ModelCaps24, ServiceContexts>, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend<ModelCaps24, ServiceContexts>> {
        {
            let ModelResources {
                connection_epoch,
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
            let mut group_slots = station_group.into_iter();
            let groups: [_; GROUPS] = core::array::from_fn(|_| group_slots.next().unwrap());
            let channel = SupportedChannel::new(channel6()).unwrap();
            let (arbiter, place, absence) =
                PortArbiter::<ModelBackend<ModelCaps24, ServiceContexts>>::new(
                    ArbiterPolicy::DEFAULT,
                    channel,
                    gate,
                    <ModelBackend<ModelCaps24, ServiceContexts> as LowerMacBackend>::PUBLICATION_BOUND,
                );
            let station = Station::new(
                self.config,
                StationResources {
                    epochs: oer_ieee80211_sta::epoch::LinkEpochSource::new(connection_epoch),
                    tickets: station_tickets,
                    buffers: small,
                    place,
                    absence_place: absence,
                    pairwise,
                    groups,
                },
            )
            .unwrap_or_else(|_| panic!("protected fixture has group slots"));
            let mut service = StationService::<_, _, 4>::new(
                station,
                StaData::new(data_tickets, large, MODEL_TX_QUEUE_DEPTH),
                arbiter,
                RecordingRates {
                    inner: EspressifRateSelection::<ModelCaps24>::new(),
                    completions: &self.world.completions,
                    refuse_rate: &self.world.refuse_rate,
                },
            );
            let world = self.world;
            let mut output = |output| world.outputs.borrow_mut().push(output);
            let mut io = ServiceIo {
                commands: self.commands,
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
            if world.expect_queued_command_on_poison.get() {
                assert!(matches!(&end, ServiceEnd::Poisoned(_)));
                assert!(
                    matches!(
                        io.commands.try_receive(),
                        Some(ServiceCommand::WantAccessPoint)
                    ),
                    "a simultaneously ready poisoned event must not consume the command"
                );
                assert!(matches!(
                    io.commands.try_receive(),
                    Some(ServiceCommand::Stop)
                ));
            }
            world.counters.set(Some(service.counters));
            assert_eq!(service.station.lost_tokens(), 0);
            assert_eq!(service.data.counters().lost_tokens, 0);
            if matches!(&end, ServiceEnd::Stopped(_)) {
                for category in WmmAccessCategory::ALL {
                    assert_eq!(
                        service.data.available(category),
                        u32::from(MODEL_TX_QUEUE_DEPTH.get()),
                        "disable returns every attempt"
                    );
                }
            }
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
fn run_with<S>(config: StaConfig<ModelCaps24>, script: S) -> ServiceCounters
where
    S: for<'w, 'b> FnOnce(&'w World<'b>) -> Script<'w>,
{
    run_with_groups::<S, 2>(config, script)
}

fn run_with_groups<S, const GROUPS: usize>(
    config: StaConfig<ModelCaps24>,
    script: S,
) -> ServiceCounters
where
    S: for<'w, 'b> FnOnce(&'w World<'b>) -> Script<'w>,
{
    let (backend, air) = ModelBackend::<ModelCaps24, ServiceContexts>::new(
        channel6(),
        [
            IndividualAddress::new(LOCAL).unwrap(),
            IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap(),
        ],
    );
    let mut commands = MsduQueue::<ServiceCommand<ModelCaps24>, 4>::new();
    let (application, commands) = commands.split();
    let mut transmit = MsduQueue::<TxMsdu, 2>::new();
    let mut receive = MsduQueue::<EthernetFrame, 2>::new();
    let (stack, transmit) = transmit.split();
    let (receive, delivered) = receive.split();
    let world = World {
        air,
        clock: VirtualClock::new(),
        commands: Cell::new(Some(application)),
        outputs: RefCell::new(Vec::new()),
        completions: RefCell::new(Vec::new()),
        refuse_rate: Cell::new(false),
        counters: Cell::new(None),
        expect_queued_command_on_poison: Cell::new(false),
        stack: RefCell::new(stack),
        delivered: RefCell::new(delivered),
    };
    let uninstalled = block_on(run_port(
        backend,
        Body::<S, GROUPS> {
            world: &world,
            commands,
            transmit,
            receive,
            config,
            script,
        },
    ));
    assert!(uninstalled.is_some(), "the port ends through its uninstall");
    world.counters.get().expect("service returned its counters")
}

fn dsss1() -> PhyRate {
    PhyRate::Legacy(LegacyRate::Dsss1M)
}

#[test]
fn a_command_ready_with_a_poisoned_event_stays_owned_by_its_queue() {
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            // Let the initial enable finish, leaving serve waiting for its inputs.
            for _ in 0..4 {
                yield_now().await;
            }
            world.expect_queued_command_on_poison.set(true);
            world.command(ServiceCommand::WantAccessPoint).await;
            world.air.poison();
        })
    });
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
                    bssid: oer_ieee80211_mac::address::Bssid::new(
                        oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
                    ),
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
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
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

#[test]
fn a_one_slot_station_completes_group_replacement_through_the_service_event_stream() {
    use oer_ieee80211_sta::machine::StaDisconnectReason;
    run_with_groups::<_, 1>(
        config(StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(PMK),
        }),
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.handshake(&mut seen).await;
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let response = world.sent(&mut seen, DATA).await;
                assert!(matches!(response.key, Some((_, true))));
                world
                    .command(ServiceCommand::Station(StaCommand::Disconnect))
                    .await;
                world.sent(&mut seen, DEAUTHENTICATION).await;
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::Requested))
                    .await;
            })
        },
    );
}

fn message3(ptk: &oer_ieee80211_rsn::Ptk) -> Vec<u8> {
    let mut plain = Vec::new();
    plain.extend_from_slice(&RSN);
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
    plain.extend_from_slice(&[0x99; 16]);
    plain.extend_from_slice(&[0xdd, 0]);
    let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
    let message3 =
        RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(ptk);
    eapol_frame(message3.as_bytes())
}

#[test]
fn refusing_m4_releases_handshake_resources_without_connecting_and_service_can_reconnect() {
    run(
        StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.start_handshake(&mut seen).await;
                let sent_before = world.air.sent().len();
                for _ in 0..8 {
                    yield_now().await;
                }
                let statistics_before = world.completions.borrow().len();
                world.refuse_rate.set(true);
                world.air.receive(&message3(&ptk), meta());
                world
                    .notice(StaNotice::Disconnected(
                        oer_ieee80211_sta::machine::StaDisconnectReason::HandshakeFailed,
                    ))
                    .await;
                assert!(world.outputs.borrow().iter().any(|output| matches!(
                    output,
                    ServiceOutput::TxRefused {
                        operation: TxOperation::Station,
                        reason: TxRefusal::NoRate
                    }
                )));
                assert!(!world.outputs.borrow().iter().any(|output| matches!(
                    output,
                    ServiceOutput::Notice(StaNotice::Connected { .. })
                )));
                let sent = world.air.sent();
                assert!(
                    sent[sent_before..]
                        .iter()
                        .all(|frame| kind(frame) == DEAUTHENTICATION)
                );
                // Only accepted frames (the leave's deauthentication) update rates.
                assert_eq!(
                    world.completions.borrow().len() - statistics_before,
                    sent.len() - sent_before
                );
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;
            })
        },
    );
}

#[test]
fn a_data_preparation_refusal_releases_admission_and_the_next_msdu_can_go() {
    run(
        StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;
                let sent_before = world.air.sent().len();
                for _ in 0..8 {
                    yield_now().await;
                }
                let statistics_before = world.completions.borrow().len();
                world.refuse_rate.set(true);
                world.send_msdu(ACCESS_POINT_ADDRESS);
                for _ in 0..50 {
                    yield_now().await;
                }
                assert!(world.outputs.borrow().iter().any(|output| matches!(
                    output,
                    ServiceOutput::TxRefused {
                        operation: TxOperation::Data,
                        reason: TxRefusal::NoRate
                    }
                )));
                assert_eq!(world.air.sent().len(), sent_before);
                assert_eq!(world.completions.borrow().len(), statistics_before);
                world.send_msdu(ACCESS_POINT_ADDRESS);
                let frame = world.sent(&mut seen, DATA).await;
                assert!(frame.packet_number.is_some());
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
    config
        .channels
        .push(SupportedChannel::new(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap()).unwrap())
        .unwrap();
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

            world
                .air
                .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
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
    config
        .channels
        .push(SupportedChannel::new(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap()).unwrap())
        .unwrap();
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
                    bssid: oer_ieee80211_mac::address::Bssid::new(
                        oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
                    ),
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
            // Leaving waits for the absence to end before sending the
            // Deauthentication. `sent` advances the virtual clock to that
            // deadline; waiting only for a notice leaves time stopped.
            world.sent(&mut seen, DEAUTHENTICATION).await;
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
    config
        .channels
        .push(SupportedChannel::new(Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap()).unwrap())
        .unwrap();
    run_with(config, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: oer_ieee80211_mac::address::Bssid::new(
                        oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
                    ),
                    channel: 6,
                })
                .await;
            let tbtt = world.clock.now().saturating_add(Duration::from_millis(5));
            world
                .command(ServiceCommand::AccessPoint(AccessPointFact::Running {
                    channel: SupportedChannel::new(channel6()).unwrap(),
                    next_tbtt: tbtt,
                    beacon_interval: oer_ieee80211_arbiter::coordinator::BeaconInterval::new(
                        Duration::from_micros(102_400),
                    )
                    .unwrap(),
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

const QOS_DATA: u16 = 0x0088;

/// A WMM Parameter element: the network is a QoS BSS.
const WMM: [u8; 26] = [
    0xdd, 24, 0x00, 0x50, 0xf2, 0x02, 0x01, 0x01, 0x00, 0x00, 0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4,
    0x00, 0x00, 0x42, 0x43, 0x5e, 0x00, 0x62, 0x32, 0x2f, 0x00,
];

/// Whether `frame` is a protected QoS Data frame carrying EAPOL.
fn carries_eapol(frame: &AirFrame) -> bool {
    kind(frame) == QOS_DATA && frame.frame.get(40..42) == Some(&[0x88, 0x8e][..])
}

/// The data attempts the air saw from `from` before the first frame
/// `until` takes.
fn data_before(air: &[AirFrame], from: usize, until: impl Fn(&AirFrame) -> bool) -> usize {
    air[from..]
        .iter()
        .take_while(|frame| !until(frame))
        .filter(|frame| kind(frame) == QOS_DATA && !carries_eapol(frame))
        .count()
}

#[test]
fn interleaved_station_and_data_terminals_keep_their_owner_and_voice_order() {
    run(
        StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let mut beacon = beacon(true);
                beacon.extend_from_slice(&WMM);
                let mut seen = 0;
                world.associate_with(&mut seen, beacon, true).await;
                let ptk = world.handshake(&mut seen).await;
                let baseline = world.completions.borrow().len();
                for category in WmmAccessCategory::ALL {
                    world.air.hold(category, true);
                }
                // Two TIDs share Voice, while Video and BE can complete first.
                // Distinct destinations let the script observe the original
                // order; the service never reads them to route a terminal.
                for (index, priority) in [6, 0, 7, 4].into_iter().enumerate() {
                    assert!(world.try_send_msdu_at([2, 0, 0, 0, 0, index as u8], priority));
                    for _ in 0..20 {
                        yield_now().await;
                    }
                }
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                for _ in 0..20 {
                    yield_now().await;
                }
                assert_eq!(world.completions.borrow().len(), baseline);

                for (category, destination) in [
                    (WmmAccessCategory::Video, 3),
                    (WmmAccessCategory::BestEffort, 1),
                ] {
                    world.air.hold(category, false);
                    let frame = world.sent(&mut seen, QOS_DATA).await;
                    assert_eq!(frame.frame[21], destination);
                    assert!(!carries_eapol(&frame));
                }
                world.air.hold(WmmAccessCategory::Voice, false);
                for (destination, tid) in [(0, 6), (2, 7)] {
                    let frame = world.sent(&mut seen, QOS_DATA).await;
                    assert_eq!(frame.frame[21], destination);
                    assert_eq!(frame.frame[24] & 0x0f, tid);
                    assert!(!carries_eapol(&frame));
                }
                assert!(carries_eapol(&world.sent(&mut seen, QOS_DATA).await));
                assert_eq!(
                    &world.completions.borrow()[baseline..],
                    &[
                        FrameRates::Data,
                        FrameRates::Data,
                        FrameRates::Data,
                        FrameRates::Data,
                        FrameRates::Control,
                    ]
                );

                // All four data owners and the station owner are reusable.
                for priority in [0, 4, 6, 7] {
                    assert!(world.try_send_msdu_at([2, 0, 0, 0, 1, priority], priority));
                    let frame = world.sent(&mut seen, QOS_DATA).await;
                    assert_eq!(frame.frame[21], priority);
                }
                world
                    .command(ServiceCommand::Station(StaCommand::Disconnect))
                    .await;
                world.sent(&mut seen, DEAUTHENTICATION).await;
            })
        },
    );
}

/// The station's frames share the Voice queue with data of priority 6–7
/// and get no queue of their own; the service keeps at most the backend's
/// queue depth of data attempts at the port, so once a held Voice queue is
/// released EAPOL and Deauthentication go after at most depth + 1 data
/// attempts (owner decision on #563, comment 6141039497).
#[test]
fn eapol_and_deauthentication_wait_for_at_most_the_queue_depth_and_one_data_attempt() {
    run(
        StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(PMK),
        },
        |world| {
            Box::pin(async move {
                let bound = usize::from(MODEL_TX_QUEUE_DEPTH.get()) + 1;
                let destination = [0x02, 0, 0, 0, 0, 0x07];
                let mut beacon = beacon(true);
                beacon.extend_from_slice(&WMM);
                let mut seen = 0;
                world.associate_with(&mut seen, beacon, true).await;
                let ptk = world.handshake(&mut seen).await;
                // Voice held, data of priority 6 fills it to the limit:
                // the data machine's tickets, then the stack's queue.
                let fill = async || {
                    world.air.hold(WmmAccessCategory::Voice, true);
                    let mut sent = 0;
                    while world.try_send_msdu_at(destination, 6) {
                        sent += 1;
                        for _ in 0..20 {
                            yield_now().await;
                        }
                    }
                    assert_eq!(sent, 4 + 2);
                };
                fill().await;
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                for _ in 0..20 {
                    yield_now().await;
                }
                let released = world.air.sent().len();
                world.air.hold(WmmAccessCategory::Voice, false);
                while !world.air.sent()[released..].iter().any(carries_eapol) {
                    world.sent(&mut seen, QOS_DATA).await;
                }
                let before = data_before(&world.air.sent(), released, carries_eapol);
                assert!(before <= bound, "EAPOL after {before} data attempts");

                // The same for the Deauthentication, the queue full again.
                for _ in 0..6 {
                    world.sent(&mut seen, QOS_DATA).await;
                }
                fill().await;
                world
                    .command(ServiceCommand::Station(StaCommand::Disconnect))
                    .await;
                for _ in 0..20 {
                    yield_now().await;
                }
                let released = world.air.sent().len();
                world.air.hold(WmmAccessCategory::Voice, false);
                world.sent(&mut seen, DEAUTHENTICATION).await;
                let before = data_before(&world.air.sent(), released, |frame| {
                    kind(frame) == DEAUTHENTICATION
                });
                assert!(
                    before <= bound,
                    "Deauthentication after {before} data attempts"
                );
            })
        },
    );
}

#[test]
fn continuous_rx_cannot_starve_data_tx_or_application_commands() {
    run(StaSecurity::Open, |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: oer_ieee80211_mac::address::Bssid::new(
                        oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
                    ),
                    channel: 6,
                })
                .await;
            let first = world.air.sent().len();
            world.air.receive_continuously(&beacon(false), meta());
            world.send_msdu(ACCESS_POINT_ADDRESS);
            world.command(ServiceCommand::WantAccessPoint).await;
            let mut progressed = false;
            for _ in 0..64 {
                yield_now().await;
                let tx = world
                    .air
                    .sent()
                    .iter()
                    .skip(first)
                    .any(|frame| kind(frame) == DATA);
                let command = world.outputs.borrow().iter().any(|output| {
                    matches!(
                        output,
                        ServiceOutput::AccessPoint(AccessPointCommand::Start(_))
                    )
                });
                if tx && command {
                    progressed = true;
                    break;
                }
            }
            assert!(
                progressed,
                "ready RX cannot consume the TX/command share of a batch"
            );
            world.air.stop_continuous_receive();
        })
    });
}

#[test]
fn lost_port_events_are_counted_and_the_service_keeps_running() {
    let counters = run_with(config(StaSecurity::Open), |world| {
        Box::pin(async move {
            let mut seen = 0;
            world.associate(&mut seen, false).await;
            world
                .notice(StaNotice::Connected {
                    bssid: oer_ieee80211_mac::address::Bssid::new(
                        oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
                    ),
                    channel: 6,
                })
                .await;
            for _ in 0..MODEL_RX_EVENTS + 5 {
                world.air.receive(&beacon(false), meta());
            }
            for _ in 0..64 {
                yield_now().await;
            }
            world.send_msdu(ACCESS_POINT_ADDRESS);
            let data = world.sent(&mut seen, DATA).await;
            assert_eq!(data.frame[4..10], BSSID);
        })
    });
    assert_eq!(counters.events_lost.get(), 1);
}

#[test]
fn protected_qos_ht_group_message_rekeys_once_after_integrity_and_replay_admission() {
    use oer_ieee80211_mac::{
        ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber},
        receive::RxDropReason,
    };
    let counters = run_with(
        config(StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(PMK),
        }),
        |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.handshake(&mut seen).await;
                let plain = group_message1(&ptk, 3, 2);
                let mut protected = plain[..24].to_vec();
                protected[..2].copy_from_slice(&0xc288_u16.to_le_bytes());
                protected.extend_from_slice(&[7, 0, 0, 0, 0, 0]);
                protected.extend_from_slice(
                    &CcmpHeader::new(
                        CcmpPacketNumber::new(1).unwrap(),
                        CcmpKeyId::new(0).unwrap(),
                    )
                    .encode(),
                );
                protected.extend_from_slice(&plain[24..]);
                world.air.receive(&protected, meta());
                for _ in 0..20 {
                    yield_now().await;
                }
                assert!(world.outputs.borrow().iter().any(|output| matches!(
                    output,
                    ServiceOutput::RxFrameRefused(RxDropReason::Integrity)
                )));
                world.air.receive(&protected, decrypted());
                let reply = world.sent(&mut seen, DATA).await;
                assert!(reply.key.is_some());
                let header = oer_ieee80211_mac::frame::DecodedHeader::decode(&reply.frame).unwrap();
                assert_eq!(header.body()[14..16], [0x88, 0x8e]);
                let after = world.air.sent().len();
                world.air.receive(&protected, decrypted());
                for _ in 0..20 {
                    yield_now().await;
                }
                assert_eq!(world.air.sent().len(), after);
                assert!(world.outputs.borrow().iter().any(|output| matches!(
                    output,
                    ServiceOutput::RxFrameRefused(RxDropReason::Replay)
                )));
            })
        },
    );
    assert_eq!(counters.rx_frames_rejected.get(), 2);
}
