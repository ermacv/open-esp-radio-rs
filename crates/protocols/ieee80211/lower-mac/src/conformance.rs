//! The lower-MAC port's conformance rules beside the shared suite of
//! `oer-radio-port`, and the harness of [`LowerMacModel`].
//!
//! A backend's test implements [`LowerMacHarness`] over its port and runs
//! [`check_all`]: the shared rules of every radio port
//! ([`oer_radio_port::conformance`]) and this port's own.

use oer_radio_port::conformance::{self, EventClass, Harness, PoisoningHarness};

use crate::{
    Backoff, CoexPriority, Ieee80211LowerMacPort, KeySelector, LowerMacEvent, MpduAttempt,
    Protection, Refused, SubmitError, TxAttempt, TxBuffer, TxId, TxPayload, TxPower, TxResponse,
    VifId,
};
use oer_ieee80211_mac::{
    phy::{LegacyRate, PhyRate},
    qos::WmmAccessCategory,
};

/// A lower-MAC backend under test, with the attempts its rules submit.
pub trait LowerMacHarness: Harness<Port: Ieee80211LowerMacPort> {
    /// One attempt of identity `id` the backend admits on an enabled port:
    /// a single MPDU of the interface the harness configured.
    fn attempt(
        &self,
        id: TxId,
    ) -> MpduAttempt<
        <Self::Port as Ieee80211LowerMacPort>::TxBuffer,
        <Self::Port as Ieee80211LowerMacPort>::TxBody,
    >;
}

/// The view every lower-MAC port classifies its events by.
pub fn classify<P: Ieee80211LowerMacPort>(event: &P::Event) -> EventClass<TxId> {
    match P::view(event) {
        LowerMacEvent::TxCompleted(completion) => EventClass::Terminal(completion.id),
        LowerMacEvent::Lifecycle(event) => EventClass::Lifecycle(event),
        LowerMacEvent::Received { .. } | LowerMacEvent::Extension { .. } => EventClass::Other,
    }
}

/// A refusal is a value that hands the attempt back, and the port changes
/// nothing: an attempt on a port that is not enabled comes back with its
/// identity, and no event follows.
pub fn a_refusal_returns_the_attempt<H: LowerMacHarness>(harness: &H) {
    let refused = harness.port().submit(harness.attempt(TxId(5)));
    let Ok(Err(Refused { error, attempt })) = refused else {
        panic!("an attempt on a disabled port is refused as a value");
    };
    assert_eq!(error, SubmitError::Disabled, "refused as not enabled");
    assert_eq!(attempt.id, TxId(5), "the attempt comes back");
    assert!(conformance::idle(harness), "a refusal changes nothing");
}

/// Every rule of the lower-MAC port on fresh harnesses `new` builds. Every
/// lower-MAC backend poisons, so poisoning is among them.
pub fn check_all<H: LowerMacHarness + PoisoningHarness>(mut new: impl FnMut() -> H) {
    conformance::check_all_poisoning(&mut new);
    a_refusal_returns_the_attempt(&new());
}

/// An MPDU attempt of `vif` at a non-HT rate with no key, its header in
/// `frame`.
pub fn attempt<B: TxBuffer, O>(id: TxId, vif: VifId, frame: B) -> MpduAttempt<B, O> {
    TxAttempt {
        id,
        vif,
        access_category: WmmAccessCategory::BestEffort,
        payload: TxPayload {
            frame,
            body: None,
            response: TxResponse::Ack,
        },
        rate: PhyRate::Legacy(LegacyRate::Ofdm24M),
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
        backoff: Backoff::Slots(3),
        coex: CoexPriority::Normal,
    }
}

/// The harness of [`LowerMacModel`](crate::model::LowerMacModel): one
/// station interface on channel 6 with monitor reception, so every frame it
/// is given is received.
#[cfg(any(test, feature = "model"))]
pub struct ModelHarness {
    model: crate::model::LowerMacModel<crate::model::ModelBody>,
}

#[cfg(any(test, feature = "model"))]
impl ModelHarness {
    const VIF: VifId = VifId(0);
    const ADDRESS: crate::MacAddress = [0x02, 0, 0, 0, 0, 1];
    const PEER: crate::MacAddress = [0x02, 0, 0, 0, 0, 2];

    /// A disabled model with its interface, channel and monitor set.
    pub fn new() -> Self {
        use crate::{LowerMacMonitor as _, LowerMacSetting, ReceiveFilter, VifConfig, VifRole};
        let model = crate::model::LowerMacModel::new();
        let channel = crate::Channel::ghz2_4(6, crate::ChannelWidth::Mhz20)
            .expect("channel 6 is a 2.4 GHz channel");
        for setting in [
            LowerMacSetting::Channel(channel),
            LowerMacSetting::Vif {
                vif: Self::VIF,
                config: Some(VifConfig {
                    address: Self::ADDRESS,
                    role: VifRole::Station,
                    bssid: Some(Self::PEER),
                    receive: ReceiveFilter::BSS_MEMBER,
                }),
            },
        ] {
            assert_eq!(
                model.apply(setting),
                Ok(Ok(())),
                "the model takes {setting:?}"
            );
        }
        assert_eq!(model.set_monitor(true), Ok(Ok(())), "monitor reception");
        Self { model }
    }

    /// The queue the harness's attempts occupy.
    fn queue() -> u8 {
        crate::model::MODEL_CAPABILITIES.tx_queue(WmmAccessCategory::BestEffort)
    }
}

#[cfg(any(test, feature = "model"))]
impl Default for ModelHarness {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "model"))]
impl Harness for ModelHarness {
    type Port = crate::model::LowerMacModel<crate::model::ModelBody>;

    fn port(&self) -> &Self::Port {
        &self.model
    }

    fn drive<F: core::future::Future>(&self, future: F) -> F::Output {
        conformance::drive_alone(future, 1)
    }

    fn admit(&self) -> TxId {
        let id = TxId(1);
        assert_eq!(
            self.model.submit(self.attempt(id)),
            Ok(Ok(())),
            "the enabled model admits the harness's attempt"
        );
        id
    }

    fn finish(&self, _: TxId) {
        self.model.complete(Self::queue(), crate::TxStatus::Success);
    }

    fn produce_undelivered(&self) -> Option<TxId> {
        let mut beacon = [0_u8; 24];
        beacon[0] = 0x80;
        beacon[4..10].copy_from_slice(&[0xff; 6]);
        beacon[10..16].copy_from_slice(&Self::PEER);
        beacon[16..22].copy_from_slice(&Self::PEER);
        let channel = crate::Channel::ghz2_4(6, crate::ChannelWidth::Mhz20)
            .expect("channel 6 is a 2.4 GHz channel");
        self.model
            .receive(&beacon, crate::RxMeta::unavailable(channel));
        None
    }

    fn undelivered_capacity(&self) -> usize {
        crate::model::MODEL_EVENT_CAPACITY
    }

    fn classify(event: &<Self::Port as oer_radio_port::RadioPort>::Event) -> EventClass<TxId> {
        classify::<Self::Port>(event)
    }
}

#[cfg(any(test, feature = "model"))]
impl LowerMacHarness for ModelHarness {
    fn attempt(&self, id: TxId) -> MpduAttempt<crate::model::ModelBuffer, crate::model::ModelBody> {
        let mut header = [0_u8; 24];
        header[0] = 0x08;
        header[1] = 0x01;
        header[4..10].copy_from_slice(&Self::PEER);
        header[10..16].copy_from_slice(&Self::ADDRESS);
        header[16..22].copy_from_slice(&Self::PEER);
        let mut frame = self
            .model
            .tx_buffer(header.len())
            .ok()
            .flatten()
            .expect("the model lends a buffer");
        frame.frame_mut().copy_from_slice(&header);
        attempt(id, Self::VIF, frame)
    }
}

#[cfg(any(test, feature = "model"))]
impl PoisoningHarness for ModelHarness {
    fn poison(&self) {
        self.model.poison();
    }
}
