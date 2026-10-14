//! Exercise the library compiled with `test-support`, without `cfg(test)`.
#![cfg(feature = "test-support")]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
    )
)]

use core::{future::Future, num::NonZeroU8, task::Poll};
use oer_ieee80211_mac::frame::FrameControl;
use std::{rc::Rc, task::Waker};

use oer_ieee80211_lower_mac::{
    AckHeader, Attempt, Channel, ChannelWidth, ClassFrame, CoexPriority, HeaderSpec,
    InterfacePhase, LowerMacEvent, LowerMacPhase, MpduLength, Protection, RetryPlan, RetryStage,
    RxBuffer, SizedFrame, Station, SupportedCoex, SupportedPower, TxBufferPool, TxCompleted,
    TxFrame, TxPower, TxProtection, TxSource,
};
use oer_ieee80211_mac::{
    address::IndividualAddress,
    phy::{LegacyRate, PhyRate},
    qos::WmmAccessCategory,
};
use oer_ieee80211_port::{Disabled, Ended, Owner, PortBody, PortPhase, run_port};
use oer_ieee80211_runtime::lower_mac_owner::model::{
    MODEL_RX_EVENTS, ModelAir, ModelAnyBuffer, ModelBackend, ModelLarge, ModelResources,
    ModelRxMeta, ModelSmall, ModelTypes,
};

const STATION: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];

fn backend() -> (ModelBackend, Rc<ModelAir>) {
    ModelBackend::new(
        Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
        [
            IndividualAddress::new(STATION).unwrap(),
            IndividualAddress::new(PEER).unwrap(),
        ],
    )
}

fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
    core::pin::pin!(future).poll(&mut core::task::Context::from_waker(Waker::noop()))
}

fn run<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut context = core::task::Context::from_waker(Waker::noop());
    for _ in 0..1_000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
    }
    panic!("the model installation did not complete");
}

struct Receive(Rc<ModelAir>);

impl PortBody<ModelBackend> for Receive {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        _: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("enable")
        };
        {
            let mut phase = enabled.phase();
            self.0
                .receive_continuously(&[1, 2, 3], ModelRxMeta::new(phase.channel(), -40, -90));
            let mut held = Vec::new();
            for _ in 0..MODEL_RX_EVENTS {
                let Poll::Ready(Ok(Ok(LowerMacEvent::Received { frame, .. }))) =
                    poll_once(phase.next_event())
                else {
                    panic!("continuous RX must be ready within its capacity");
                };
                assert_eq!(frame.bytes(), &[1, 2, 3]);
                held.push(frame);
            }
            assert!(poll_once(phase.next_event()).is_pending());
            phase.recycle(held.pop().unwrap());
            // Recycling the continuous source restores capacity immediately,
            // without giving the driver another poll.
            let Poll::Ready(Ok(Ok(LowerMacEvent::Received { frame, .. }))) =
                poll_once(phase.next_event())
            else {
                panic!("the returned RX slot must be reusable");
            };
            held.push(frame);
            self.0.stop_continuous_receive();
            for frame in held {
                phase.recycle(frame);
            }
            assert!(poll_once(phase.next_event()).is_pending());
        }
        let Ok(disabled) = enabled
            .disable(&mut |_| panic!("all RX slots were recycled"))
            .await
        else {
            panic!("recycled RX must not poison the model");
        };
        disabled.uninstall(())
    }
}

#[test]
fn continuous_receive_has_the_same_capacity_and_recycling_in_feature_builds() {
    let (backend, air) = backend();
    run(run_port(backend, Receive(air))).unwrap();
}

struct Owners;

impl PortBody<ModelBackend> for Owners {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        mut resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend> {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("enable")
        };
        {
            let mut phase = enabled.phase();
            macro_rules! submit {
                ($class:ty, $buffer:expr) => {{
                    let rate = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm6M)).unwrap();
                    let header = AckHeader::new(
                        HeaderSpec::new(
                            FrameControl::new(0x0108).unwrap(),
                            0,
                            IndividualAddress::new(PEER).unwrap(),
                            STATION,
                            PEER,
                            0,
                            None,
                            None,
                            None,
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    let frame = ClassFrame::new(TxFrame::Individual {
                        frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
                        plan: RetryPlan::new(RetryStage {
                            rate,
                            transmissions: NonZeroU8::MIN,
                            protection: Protection::None,
                        }),
                    })
                    .ok()
                    .unwrap();
                    let source = TxSource::Fresh {
                        buffer: $buffer,
                        frame,
                        body: None,
                        protection: TxProtection::Open,
                    };
                    let source = <ModelTypes as TxBufferPool<$class>>::prepare::<Station, _>(
                        source.check().ok().unwrap(),
                    )
                    .ok()
                    .unwrap();
                    let ticket = resources.tickets.pop().unwrap();
                    let index = ticket.index();
                    phase
                        .station()
                        .submit(Attempt {
                            context: (),
                            ticket,
                            source,
                            access_category: WmmAccessCategory::BestEffort,
                            protection: Protection::None,
                            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                            coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                        })
                        .unwrap();
                    index
                }};
            }
            let first = submit!(ModelSmall, resources.small.pop().unwrap());
            let second = submit!(ModelLarge, resources.large.pop().unwrap());
            for (ticket_index, large) in [(first, false), (second, true)] {
                let event = phase.next_event().await.unwrap().unwrap();
                let LowerMacEvent::TxCompleted(TxCompleted::Single(terminal)) = event else {
                    panic!("one Single terminal per submission");
                };
                let oer_ieee80211_lower_mac::TerminalParts {
                    ticket, payload, ..
                } = terminal.into_parts();
                let (buffer, _) = oer_ieee80211_lower_mac::TxMpdu::into_parts(payload);
                assert_eq!(ticket.index(), ticket_index);
                assert_eq!(matches!(buffer, ModelAnyBuffer::LargeSent(_)), large);
                if !large {
                    assert!(matches!(buffer, ModelAnyBuffer::SmallSent(_)));
                }
            }
            assert!(poll_once(phase.next_event()).is_pending());
        }
        let Ok(disabled) = enabled
            .disable(&mut |_| panic!("both terminals were returned"))
            .await
        else {
            panic!("disable")
        };
        disabled.uninstall(())
    }
}

#[test]
fn feature_builds_keep_each_accepted_ticket_with_its_original_buffer() {
    let (backend, _) = backend();
    run(run_port(backend, Owners)).unwrap();
}
