//! An enabled-port channel setting is refused, as on S31. This backend
//! deliberately implements neither air reservation nor live retuning.

use core::{cell::Cell, future::Future};

use oer_ieee80211_lower_mac::*;

use super::*;

struct DisabledRetune {
    model: &'static Model,
    enabled: Cell<bool>,
}

type Fault = <Model as RadioPort>::Fault;

impl RadioPort for DisabledRetune {
    type Event = <Model as RadioPort>::Event;
    type Id = TxId;
    type Domain = Ieee80211Radio;
    type Fault = Fault;

    fn next_event(&self) -> impl Future<Output = PortResult<Self::Event, EventsLost, Fault>> + '_ {
        self.model.next_event()
    }

    fn now(&self) -> impl Future<Output = PortResult<Ieee80211Instant, ClockError, Fault>> + '_ {
        self.model.now()
    }

    fn cancel(&self, id: TxId) -> impl Future<Output = PortResult<(), CancelError, Fault>> + '_ {
        self.model.cancel(id)
    }

    async fn lifecycle(&self, command: LifecycleCommand) -> PortResult<(), LifecycleError, Fault> {
        let result = self.model.lifecycle(command).await;
        if matches!(&result, Ok(Ok(()))) {
            self.enabled
                .set(matches!(command, LifecycleCommand::Enable));
        }
        result
    }
}

impl Ieee80211LowerMacPort for DisabledRetune {
    type TxBuffer = <Model as Ieee80211LowerMacPort>::TxBuffer;
    type TxBody = <Model as Ieee80211LowerMacPort>::TxBody;
    type RxBuffer = <Model as Ieee80211LowerMacPort>::RxBuffer;

    fn view(event: &Self::Event) -> LowerMacEvent<'_> {
        Model::view(event)
    }

    fn into_received(event: Self::Event) -> Result<(Self::RxBuffer, RxMeta), Self::Event> {
        Model::into_received(event)
    }

    fn capabilities(&self) -> LowerMacCapabilities {
        self.model.capabilities()
    }

    fn clock_info(&self) -> ClockInfo {
        self.model.clock_info()
    }

    fn tx_buffer(&self, len: usize) -> PortResult<Option<Self::TxBuffer>, NotInstalled, Fault> {
        self.model.tx_buffer(len)
    }

    fn release_tx_buffer(&self, buffer: Self::TxBuffer) {
        self.model.release_tx_buffer(buffer);
    }

    fn submit(
        &self,
        attempt: MpduAttempt<Self::TxBuffer, Self::TxBody>,
    ) -> SubmitResult<MpduAttempt<Self::TxBuffer, Self::TxBody>, Fault> {
        self.model.submit(attempt)
    }

    fn reclaim_tx_bodies(
        &self,
        id: TxId,
        each: impl FnMut(usize, Self::TxBody),
    ) -> PortResult<(), ReclaimError, Fault> {
        self.model.reclaim_tx_bodies(id, each)
    }

    fn apply(&self, setting: LowerMacSetting) -> PortResult<(), SettingError, Fault> {
        if matches!(setting, LowerMacSetting::Channel(_)) && self.enabled.get() {
            Ok(Err(SettingError::Busy))
        } else {
            self.model.apply(setting)
        }
    }

    fn install_key(&self, key: KeyInstall<'_>) -> PortResult<KeyHandle, SettingError, Fault> {
        self.model.install_key(key)
    }

    fn clock_sample(&self) -> PortResult<Ieee80211ClockSample, ClockError, Fault> {
        self.model.clock_sample()
    }
}

impl LowerMacBeaconTiming for DisabledRetune {
    fn beacon_timing_capabilities(&self) -> BeaconTimingCapabilities {
        self.model.beacon_timing_capabilities()
    }

    fn tsf(&self, vif: VifId) -> PortResult<VifTsf, SettingError, Fault> {
        self.model.tsf(vif)
    }

    fn tsf_sample(&self, vif: VifId) -> PortResult<TsfSample, SettingError, Fault> {
        self.model.tsf_sample(vif)
    }

    fn set_tsf(&self, tsf: VifTsf) -> PortResult<(), SettingError, Fault> {
        self.model.set_tsf(tsf)
    }

    fn set_tbtt(&self, schedule: TbttSchedule) -> PortResult<(), SettingError, Fault> {
        self.model.set_tbtt(schedule)
    }

    fn stop_tbtt(&self, vif: VifId) -> PortResult<(), SettingError, Fault> {
        self.model.stop_tbtt(vif)
    }

    fn tbtt(event: &Self::Event) -> Option<TbttEvent> {
        Model::tbtt(event)
    }
}

impl LowerMacMonitor for DisabledRetune {
    fn monitor_capabilities(&self) -> MonitorCapabilities {
        self.model.monitor_capabilities()
    }

    fn set_monitor(&self, enabled: bool) -> PortResult<(), SettingError, Fault> {
        self.model.set_monitor(enabled)
    }
}

impl LowerMacAmpdu for DisabledRetune {
    type AmpduBuffer = <Model as LowerMacAmpdu>::AmpduBuffer;

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        self.model.ampdu_capabilities()
    }

    fn ampdu_buffer(&self) -> PortResult<Option<Self::AmpduBuffer>, NotInstalled, Fault> {
        self.model.ampdu_buffer()
    }

    fn release_ampdu_buffer(&self, buffer: Self::AmpduBuffer) {
        self.model.release_ampdu_buffer(buffer);
    }

    fn submit_ampdu(
        &self,
        attempt: AmpduAttempt<Self::AmpduBuffer>,
    ) -> SubmitResult<AmpduAttempt<Self::AmpduBuffer>, Fault> {
        self.model.submit_ampdu(attempt)
    }
}

type CurrentPair =
    PortStaAp<'static, Env<DisabledRetune>, Env<DisabledRetune>, &'static VirtualTimer>;

struct Setup {
    upstream: PortAccessPoint<'static, Env>,
    pair: CurrentPair,
    client: PortStation<'static, Env>,
    port: &'static DisabledRetune,
    router: &'static Router<DisabledRetune>,
}

fn setup(world: &World) -> Setup {
    let mut upstream = access_point(
        world.upstream.1,
        world.timer,
        UPSTREAM,
        UPSTREAM_SSID,
        ghz2_4(6),
        leak(TestFrames::new()),
    );
    world
        .drive(upstream.client_mut().retune(ghz2_4(6)))
        .unwrap();
    upstream.start(ghz2_4(6)).unwrap();
    let port = leak(DisabledRetune {
        model: world.pair.0,
        enabled: Cell::new(true),
    });
    let router = leak(PortRouter::new(port, 1));
    let mut pair = PortStaAp::new(
        station(
            router,
            world.timer,
            PAIR_STATION,
            UPSTREAM_SSID,
            leak(TestFrames::new()),
        ),
        access_point(
            router,
            world.timer,
            PAIR_ACCESS_POINT,
            PAIR_SSID,
            ghz2_4(1),
            world.pair_frames,
        ),
        policy(),
        world.timer,
    );
    assert!(pair.coordinator().search_policy().channels.is_empty());
    let until = world.after(3_000);
    let (_, joined) = world.drive_with_pair_router(
        join(serve_upstream(&mut upstream, until), pair.connect()),
        router,
        || {},
    );
    joined.unwrap();
    assert_eq!(
        pair.station().connection().unwrap().config().channel,
        ghz2_4(6)
    );
    let client = station(
        world.client.1,
        world.timer,
        CLIENT,
        PAIR_SSID,
        world.client_frames,
    );
    let until = world.after(4_000);
    let (_, (served, client)) = world.drive_with_pair_router(
        join(
            serve_upstream(&mut upstream, until),
            join(
                pair.run_until(until, &mut |_| {}, &mut |_| {}),
                client.connect(),
            ),
        ),
        router,
        || {},
    );
    served.unwrap();
    Setup {
        upstream,
        pair,
        client: connected(client),
        port,
        router,
    }
}

fn lose(world: &World, setup: &mut Setup) {
    world
        .drive_with_pair_router(setup.upstream.stop(), setup.router, || {})
        .unwrap();
    assert!(matches!(
        world
            .drive_with_pair_router(
                setup
                    .pair
                    .run_until(world.after(1_000), &mut |_| {}, &mut |_| {}),
                setup.router,
                || {},
            )
            .unwrap(),
        Some(PortStaApEvent::StationEnded(
            PortDisconnect::Deauthenticated { .. }
        ))
    ));
}

#[test]
fn persistent_csa_uses_the_lifecycle_retune_and_keeps_the_downstream_peer() {
    on_large_stack(|| {
        let world = World::new();
        let mut setup = setup(&world);
        let peer = setup
            .pair
            .access_point()
            .service()
            .peer_status(CLIENT)
            .unwrap();
        let lifecycle = world.pair.0.lifecycle_requests();
        setup
            .upstream
            .announce_channel_switch(ghz2_4(11), ChannelSwitchMode::Continue, 3)
            .unwrap();
        let until = world.after(2_000);
        let (_, (served, ())) = world.drive_with_pair_router(
            join(
                serve_upstream(&mut setup.upstream, until),
                join(
                    setup.pair.run_until(until, &mut |_| {}, &mut |_| {}),
                    serve_client(&mut setup.client, world.timer, until),
                ),
            ),
            setup.router,
            || {},
        );
        assert_eq!(served.unwrap(), None);
        assert!(setup.port.enabled.get());
        assert_eq!(world.pair.0.channel(), Some(ghz2_4(11)));
        assert_eq!(world.pair.0.lifecycle_requests(), lifecycle + 2);
        assert_eq!(
            setup.pair.station().connection().unwrap().config().channel,
            ghz2_4(11)
        );
        assert_eq!(
            setup.client.connection().unwrap().config().channel,
            ghz2_4(11)
        );
        let after = setup
            .pair
            .access_point()
            .service()
            .peer_status(CLIENT)
            .unwrap();
        assert_eq!(after.association_id, peer.association_id);
        assert_eq!(after.association_epoch, peer.association_epoch);

        lose(&world, &mut setup);
        setup.upstream.start(ghz2_4(11)).unwrap();
        let until = world.after(1_000);
        let (_, joined) = world.drive_with_pair_router(
            join(
                serve_upstream(&mut setup.upstream, until),
                setup.pair.reconnect(&mut |_| {}),
            ),
            setup.router,
            || {},
        );
        joined.unwrap();
        assert!(setup.pair.station().connection().is_some());
    });
}

#[test]
fn current_channel_mode_searches_and_rejoins_without_either_absence_extension() {
    on_large_stack(|| {
        let world = World::new();
        let mut setup = setup(&world);
        let peer = setup
            .pair
            .access_point()
            .service()
            .peer_status(CLIENT)
            .unwrap();
        lose(&world, &mut setup);
        let lifecycle = world.pair.0.lifecycle_requests();
        let updates = world.pair.0.channel_updates();
        let submitted = world.pair.0.submitted().len();
        assert_eq!(
            world
                .drive_with_pair_router(
                    setup
                        .pair
                        .run_until(world.after(500), &mut |_| {}, &mut |_| {}),
                    setup.router,
                    || {},
                )
                .unwrap(),
            None
        );
        setup.upstream.start(ghz2_4(6)).unwrap();
        let until = world.after(1_000);
        let (_, served) = world.drive_with_pair_router(
            join(
                serve_upstream(&mut setup.upstream, until),
                setup.pair.run_until(until, &mut |_| {}, &mut |_| {}),
            ),
            setup.router,
            || assert_eq!(world.pair.0.channel(), Some(ghz2_4(6))),
        );
        assert_eq!(served.unwrap(), None);
        assert!(setup.pair.station().connection().is_some());
        assert_eq!(world.pair.0.lifecycle_requests(), lifecycle);
        assert_eq!(world.pair.0.channel_updates(), updates);
        assert!(
            world.pair.0.submitted()[submitted..]
                .iter()
                .all(|attempt| { !matches!(attempt.frames[0][0], 0x40 | 0xc4) })
        );
        let after = setup
            .pair
            .access_point()
            .service()
            .peer_status(CLIENT)
            .unwrap();
        assert_eq!(after.association_id, peer.association_id);
        assert_eq!(after.association_epoch, peer.association_epoch);
    });
}
