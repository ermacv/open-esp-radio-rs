//! A station's transmissions on the port: one frame exchange of the
//! portable [`TxPlanner`] of `oer-ieee80211-upper-mac`, attempt by attempt.
//!
//! Retries, the rate of each retry, protection and the backoff draw are
//! software decisions, so they stay above the port: the planner plans each
//! attempt with the Espressif policy the composition gives it (the retry
//! ladder and HE TXOP budget of `oer-espressif-ieee80211-policy`), the
//! sender submits it through the station's client and feeds its completion
//! back, until the planner reports the exchange's end.

use oer_ieee80211_lower_mac::{
    AckHeader, AttemptKey, CoexPriority, FrameControl, HeaderSpec, IndividualAddress,
    LowerMacClient, LowerMacEvent, MpduLength, PhaseAttempt, PhyRate, QosControl, SizedFrame,
    SupportedBackoff, SupportedCoex, SupportedPower, Tid, TxCompletion, TxFrame, TxFrameBody,
    TxPower, TxResponse, TxSeats,
};
use oer_ieee80211_mac::qos::WmmAccessCategory;
use oer_ieee80211_port::{RadioPortClient, RadioPortEvents};
use oer_ieee80211_softmac::BackoffEntropy;
use oer_ieee80211_upper_mac::{
    AttemptContent, HeTxopRtsBudget, MpduRequest, RateLadder, TxAttemptPlan, TxBody, TxPlanner,
    TxReceiver, TxReport, TxRequest, TxStep,
};

use super::{HeldFrames, SendError, StationOwner, StationParts};

/// The FCS every MPDU ends with.
const FCS_LEN: u32 = 4;
/// The MIC a CCMP-protected MPDU carries.
const CCMP_MIC_LEN: u32 = 8;
/// The Retry bit of Frame Control.
const RETRY: u16 = 0x0800;

/// What one exchange sends: its access category, first rate, coexistence
/// level and transmit power.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Exchange {
    pub access_category: WmmAccessCategory,
    pub rate: PhyRate,
    pub coex: CoexPriority,
    pub power: TxPower,
}

/// The planner and its policy, which every exchange of a station shares.
pub struct PortSender<'s, 'id, B: StationParts, Budget, L, N> {
    pub seats: &'s mut TxSeats<'id, B::QueueKind, B::BufferKind>,
    pub planner: &'s mut TxPlanner<Budget>,
    pub ladder: &'s L,
    pub entropy: N,
    /// Transmissions one MPDU may make, the first included.
    pub retry_limit: u8,
}

/// The header of `encoded` as its encoder wrote it, with the Retry bit
/// when `retry`.
fn header_of(encoded: &[u8], retry: bool) -> Option<(AckHeader, usize)> {
    let field = |at: usize| -> Option<[u8; 6]> { encoded.get(at..at + 6)?.try_into().ok() };
    let word =
        |at: usize| -> Option<u16> { Some(u16::from_le_bytes(encoded.get(at..at + 2)?.try_into().ok()?)) };
    let control = word(0)? | if retry { RETRY } else { 0 };
    // A QoS data frame: type data with the QoS subtype bit.
    let qos = if control & 0x00fc == 0x0088 {
        let field = word(24)?;
        Some(QosControl {
            tid: Tid::new((field & 0x0f) as u8)?,
            end_of_service_period: field & 0x0010 != 0,
            amsdu_present: field & 0x0080 != 0,
            high: (field >> 8) as u8,
        })
    } else {
        None
    };
    let header = HeaderSpec::new(
        FrameControl::new(control)?,
        word(2)?,
        IndividualAddress::new(field(4)?)?,
        field(10)?,
        field(16)?,
        word(22)?,
        None,
        qos,
        None,
    )
    .and_then(AckHeader::new)?;
    let len = header.header().len();
    Some((header, len))
}

impl<'s, 'id, B, Budget, L, N> PortSender<'s, 'id, B, Budget, L, N>
where
    B: StationParts,
    Budget: HeTxopRtsBudget,
    L: RateLadder,
    N: BackoffEntropy,
{
    /// Send the individual frame `encoded` (header and body as its encoder
    /// wrote them), protected with `key` when given, until the planner
    /// reports the exchange's end; frames received meanwhile go to `held`.
    ///
    /// # Errors
    ///
    /// The port is gone or not enabled, or `encoded` makes no checked frame.
    pub async fn send<'p>(
        &mut self,
        owner: &mut StationOwner<'p, 'id, B>,
        encoded: &[u8],
        key: Option<&B::PairwiseKey<'id>>,
        exchange: Exchange,
        held: &mut impl HeldFrames,
    ) -> Result<TxReport, SendError<B::Fault>> {
        let mic = if key.is_some() { CCMP_MIC_LEN } else { 0 };
        let length = u32::try_from(encoded.len()).map_err(|_| SendError::Frame)? + FCS_LEN + mic;
        let request = TxRequest {
            access_category: exchange.access_category,
            initial_rate: exchange.rate,
            receiver: TxReceiver::Individual,
            power: exchange.power,
            coex: exchange.coex,
            mpdu_retry_limit: self.retry_limit,
            body: TxBody::Mpdu(MpduRequest {
                length,
                response: TxResponse::Ack,
            }),
        };
        let (mut planned, mut plan) = self
            .planner
            .begin(request, self.ladder, &mut self.entropy)
            .map_err(|_| SendError::Frame)?;
        loop {
            let AttemptContent::Mpdu { set_retry_bit } = plan.content else {
                return Err(SendError::Frame);
            };
            let completion = self
                .attempt(owner, encoded, set_retry_bit, key, &plan, held)
                .await?;
            let now = {
                let phase = owner.phase().ok_or(SendError::NotRunning)?;
                B::station(&phase.clients)
                    .now()
                    .await
                    .map_err(|poisoned| SendError::Poisoned(poisoned.cause))?
                    .map_err(|_| SendError::Frame)?
            };
            match self.planner.complete(
                &mut planned,
                &completion,
                now,
                self.ladder,
                &mut self.entropy,
            ) {
                TxStep::Attempt(next) => plan = next,
                TxStep::Done(report) => return Ok(report),
            }
        }
    }

    /// One attempt of `plan`, awaited to its completion.
    async fn attempt<'p>(
        &mut self,
        owner: &mut StationOwner<'p, 'id, B>,
        encoded: &[u8],
        retry: bool,
        key: Option<&B::PairwiseKey<'id>>,
        plan: &TxAttemptPlan,
        held: &mut impl HeldFrames,
    ) -> Result<TxCompletion, SendError<B::Fault>> {
        let (header, header_len) = header_of(encoded, retry).ok_or(SendError::Frame)?;
        let mut phase = owner.phase().ok_or(SendError::NotRunning)?;
        let station = B::station(&phase.clients);
        let rate = station.rate(plan.rate).ok_or(SendError::Frame)?;
        let frame = MpduLength::new(encoded.len())
            .and_then(|len| SizedFrame::new(header, len))
            .ok_or(SendError::Frame)?;
        let category = plan.access_category;
        let mut buffer = station
            .tx_buffer(TxFrame::Individual { frame, rate }, self.seats.buffer(category))
            .await;
        buffer.body_mut().copy_from_slice(&encoded[header_len..]);
        let queue = station.queue(self.seats.queue(category)).await;
        let attempt = PhaseAttempt {
            queue,
            frame: buffer,
            body: None,
            protection: plan.port_protection(),
            key: key.map(AttemptKey::Pairwise),
            power: SupportedPower::new(plan.power).ok_or(SendError::Frame)?,
            backoff: SupportedBackoff::new(plan.backoff).ok_or(SendError::Frame)?,
            coex: SupportedCoex::new(plan.coex)
                .or_else(|| SupportedCoex::new(CoexPriority::Normal))
                .ok_or(SendError::Frame)?,
        };
        let work = station
            .submit(attempt)
            .map_err(|poisoned| SendError::Poisoned(poisoned.cause))?;
        let id = work.id();
        loop {
            match phase.events.next_event().await {
                Ok(Ok(event)) => match B::view(&event) {
                    LowerMacEvent::TxCompleted(completion) if completion.id == id => {
                        return Ok(completion);
                    }
                    LowerMacEvent::Received { frame, .. } => held.hold(frame),
                    _ => {}
                },
                Ok(Err(_lost)) => {}
                Err(poisoned) => return Err(SendError::Poisoned(poisoned.cause)),
            }
        }
    }
}
