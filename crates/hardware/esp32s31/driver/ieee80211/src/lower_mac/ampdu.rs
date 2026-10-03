//! HT A-MPDU attempts of the lower-MAC core.
//!
//! One aggregate is one attempt: [`RetainedDmaAmpduTx`] begins it, commits
//! every subframe with `commit_ht` and publishes it once with `submit`. Its
//! completion reports the recipient's BlockAck; the aggregate is never
//! retained for a retry (`retain_for_ampdu_retry`), since selecting what a
//! later aggregate retransmits is the portable layer's.
//!
//! # Buffers
//!
//! A lent [`Esp32s31AmpduBuffer`] is one idle aggregate owner together with
//! the subframes the caller appended. Each subframe is a
//! [`StableDmaBacking`] lease of the integrator's [`AmpduBackingSource`],
//! the same retained-lease memory the station and access-point aggregate
//! paths publish from: the MPDU is written after the eight-byte metadata
//! prefix at the start of the lease's region, and `commit_ht` publishes it
//! where it was written. The owner retains every lease until the attempt's
//! completion releases it, when dropping the lease returns it to its source.
//! The ordinary TX slots cannot serve here: each is a single descriptor with
//! its own buffer, while an aggregate descriptor chain references leases.
//!
//! # Limits
//!
//! HT only, at most `SLOTS` subframes and [`ESP32S31_AMPDU_MAX_LENGTH`]
//! octets. HE A-MPDU and Trigger-based aggregates stay outside the declared
//! formats: the HE owner path needs the recipient's HE TXOP and Trigger
//! reservations the port does not carry.

use core::fmt;

use oer_esp32s31_ieee80211_mac::tx::{
    HtAmpduDensity, HtChannelWidth, HtGuardInterval, HtMcs, HtProtectionSpacing, HtRate,
    LegacyTxQueue, TxCookie,
    ampdu::{
        AmpduFrameLayout, AmpduFrameSize, HtAmpduFrameRequest, HtAmpduHardware,
        HtAmpduTxCompletion, HtAmpduTxError, RetainedDmaAmpduTx, TX_AMPDU_METADATA_SIZE,
    },
};
use oer_ieee80211_lower_mac::{
    AmpduAttempt, AmpduBuffer, AmpduCapabilities, BlockAckReport, PhyFormatSet, TxCompletion, TxId,
    TxStatus,
};
use oer_memory::{PinnedDmaTxRadioLease, StableDmaBacking};

use crate::{
    ampdu_tx::{AmpduTxRoleAdapter, HtAmpduPublicationInputs, ht_ampdu_publication_config},
    ordinary_tx::{TX_ABORT_SETTLE, TX_CCMP_MIC_SIZE, TX_FCS_SIZE},
    tx::WifiTxWake,
};

/// The longest aggregate the core publishes: the vendor's per-rate ceiling
/// of HT MCS 0 with the long guard interval (`rx11NRate2AMPDULimit`, see
/// [`HtRate::vendor_ampdu_byte_limit`]), the lowest of every HT rate and
/// below the 8191 octets every HT recipient accepts. The aggregate owner is
/// configured to it, so an aggregate is refused exactly when it is longer.
pub const ESP32S31_AMPDU_MAX_LENGTH: u16 = match HtRate::new(
    HtMcs::Mcs0,
    HtGuardInterval::Long800Ns,
    HtChannelWidth::Mhz20,
)
.vendor_ampdu_byte_limit()
{
    Some(limit) => limit,
    None => 0x1fff,
};

/// The limits of the core's aggregates of up to `slots` subframes.
pub const fn esp32s31_ampdu_capabilities(slots: usize) -> AmpduCapabilities {
    AmpduCapabilities {
        max_subframes: if slots > u16::MAX as usize {
            u16::MAX
        } else {
            slots as u16
        },
        formats: PhyFormatSet::HT,
        max_length: ESP32S31_AMPDU_MAX_LENGTH as u32,
    }
}

/// The memory type of aggregate subframes: what an aggregate owner of the
/// core retains.
pub trait AmpduBacking {
    /// One subframe's memory. Its region must start word-aligned, as every
    /// S31 TX descriptor buffer does.
    type Backing: StableDmaBacking;
}

/// Stable DMA memory the core lends as aggregate subframes.
///
/// Dropping a backing returns it to its source, as
/// `oer_memory::ReturningStableDmaBacking` returns a pool slot.
pub trait AmpduBackingSource: AmpduBacking {
    /// A free backing, or `None` when every one is in use.
    fn backing(&self) -> Option<Self::Backing>;
}

/// The subframe memory of a core without aggregates. It is no
/// [`AmpduBackingSource`], so such a core lends no aggregate and its port
/// does not implement `LowerMacAmpdu`; its backing type is never
/// constructed.
#[derive(Debug)]
pub enum NoAmpdu {}

impl AmpduBacking for NoAmpdu {
    type Backing = PinnedDmaTxRadioLease<'static, 0, 0, 0>;
}

/// The aggregate owner behind one lent buffer. The port publishes only
/// referenced subframes, so its formatter workspace is empty.
pub type Esp32s31AmpduOwner<'slot, B, const SLOTS: usize> = RetainedDmaAmpduTx<'slot, B, SLOTS, 0>;

struct Subframe<B> {
    backing: B,
    len: usize,
}

/// One lent aggregate: an idle aggregate owner and the subframes appended so
/// far. It goes back to the core by submission or
/// [`LowerMacCore::release_ampdu_buffer`](super::LowerMacCore::release_ampdu_buffer);
/// a dropped buffer returns its subframes to their source and loses its
/// aggregate owner until the core is rebuilt.
pub struct Esp32s31AmpduBuffer<'slot, S: AmpduBacking, const SLOTS: usize> {
    pub(super) owner: Esp32s31AmpduOwner<'slot, S::Backing, SLOTS>,
    source: &'slot S,
    subframes: [Option<Subframe<S::Backing>>; SLOTS],
    count: usize,
}

impl<'slot, S: AmpduBacking, const SLOTS: usize> Esp32s31AmpduBuffer<'slot, S, SLOTS> {
    pub(super) fn new(
        owner: Esp32s31AmpduOwner<'slot, S::Backing, SLOTS>,
        source: &'slot S,
    ) -> Self {
        Self {
            owner,
            source,
            subframes: [const { None }; SLOTS],
            count: 0,
        }
    }

    /// The appended MPDU lengths, in order.
    pub(super) fn lengths(&self) -> impl Iterator<Item = usize> + '_ {
        self.subframes.iter().flatten().map(|subframe| subframe.len)
    }

    /// Return the subframes to their source and give the owner back.
    pub(super) fn into_owner(self) -> Esp32s31AmpduOwner<'slot, S::Backing, SLOTS> {
        self.owner
    }
}

impl<S: AmpduBacking, const SLOTS: usize> Esp32s31AmpduBuffer<'_, S, SLOTS> {
    /// The first MPDU, for the checks of admission.
    pub(super) fn first_mpdu(&mut self) -> Option<&[u8]> {
        let subframe = self.subframes.first_mut()?.as_mut()?;
        let len = subframe.len;
        let region = subframe.backing.stable_dma_region().into_mut_slice();
        Some(&region[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + len])
    }
}

/// The region one subframe of `len` MPDU bytes needs: the metadata prefix,
/// the MPDU, room for the hardware CCMP MIC and FCS, word-aligned.
const fn subframe_capacity(len: usize) -> Option<usize> {
    match (TX_AMPDU_METADATA_SIZE + TX_CCMP_MIC_SIZE + TX_FCS_SIZE + 3).checked_add(len) {
        Some(capacity) => Some(capacity & !3),
        None => None,
    }
}

impl<S: AmpduBackingSource, const SLOTS: usize> AmpduBuffer for Esp32s31AmpduBuffer<'_, S, SLOTS> {
    /// `None` also when the source has no free backing or its backing is
    /// too short for the MPDU with its metadata, MIC and FCS.
    fn push_mpdu(&mut self, len: usize) -> Option<&mut [u8]> {
        if self.count >= SLOTS || len == 0 {
            return None;
        }
        let capacity = subframe_capacity(len)?;
        let mut backing = self.source.backing()?;
        if backing.stable_dma_region().len() < capacity {
            // Dropping the backing returns it.
            return None;
        }
        let index = self.count;
        let subframe = self.subframes[index].insert(Subframe { backing, len });
        self.count += 1;
        let region = subframe.backing.stable_dma_region().into_mut_slice();
        Some(&mut region[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + len])
    }

    fn subframes(&self) -> usize {
        self.count
    }
}

impl<S: AmpduBacking, const SLOTS: usize> fmt::Debug for Esp32s31AmpduBuffer<'_, S, SLOTS> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Esp32s31AmpduBuffer")
            .field("subframes", &self.count)
            .finish_non_exhaustive()
    }
}

/// An aggregate submission of the core.
pub type Esp32s31AmpduAttempt<'slot, S, const SLOTS: usize> =
    AmpduAttempt<Esp32s31AmpduBuffer<'slot, S, SLOTS>>;

/// The queue-state choices of one admitted aggregate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AmpduPlan {
    pub queue: LegacyTxQueue,
    pub rate: HtRate,
    pub role: AmpduTxRoleAdapter,
    pub hardware_mic_length: u8,
    /// Everything but the aggregate's length and subframe count, which the
    /// owner computes as the subframes are committed.
    pub inputs: HtAmpduPublicationInputs,
}

/// The owner's protection spacing for a recipient's Minimum MPDU Start
/// Spacing encoding (IEEE 0-7, bits 4:2 of its A-MPDU Parameters).
pub(super) const fn protection_spacing(min_mpdu_start_spacing: u8) -> HtProtectionSpacing {
    HtProtectionSpacing::from_density(HtAmpduDensity::from_ampdu_parameters(
        (min_mpdu_start_spacing & 0x07) << 2,
    ))
}

/// A published aggregate on its queue.
pub(super) struct PublishedAmpdu<'slot, B, const SLOTS: usize> {
    owner: Esp32s31AmpduOwner<'slot, B, SLOTS>,
    cookie: TxCookie,
    /// The publication deadline, or the end of the abort settle.
    deadline: oer_time::Instant,
    settling: bool,
}

impl<B, const SLOTS: usize> PublishedAmpdu<'_, B, SLOTS> {
    pub const fn deadline(&self) -> oer_time::Instant {
        self.deadline
    }

    pub const fn abort_settling(&self) -> bool {
        self.settling
    }

    /// The aggregate owner, for observation.
    #[cfg(test)]
    pub(super) const fn owner(&self) -> &Esp32s31AmpduOwner<'_, B, SLOTS> {
        &self.owner
    }
}

/// How a published aggregate ended.
pub(super) enum AmpduOutcome {
    Completed(HtAmpduTxCompletion),
    Collision,
    /// The queue's hardware timeout and its abort.
    Aborted,
}

impl AmpduOutcome {
    /// The portable completion. A valid BlockAck is the solicited response,
    /// so the attempt succeeded whatever the completion status says
    /// ([`HtAmpduTxCompletion::valid_block_ack`]); a completion without one
    /// is its failure, and a success status without one is `AckTimeout`.
    pub fn completion(self, id: TxId) -> TxCompletion {
        let (status, ack_snr_db, block_ack) = match self {
            Self::Completed(completion) => {
                let block_ack = completion.valid_block_ack();
                let status = match (block_ack, completion.tx.tx_status()) {
                    (Some(_), _) => TxStatus::Success,
                    (None, TxStatus::Success) => TxStatus::AckTimeout,
                    (None, failure) => failure,
                };
                (
                    status,
                    completion.tx.ack_snr_sample(),
                    block_ack.map(|block_ack| BlockAckReport {
                        start_sequence: block_ack.starting_sequence,
                        bitmap: block_ack.bitmap,
                    }),
                )
            }
            Self::Collision => (TxStatus::Collision, None, None),
            Self::Aborted => (TxStatus::Aborted, None, None),
        };
        TxCompletion {
            id,
            status,
            ack_rssi_dbm: None,
            ack_snr_db,
            block_ack,
        }
    }
}

pub(super) enum AmpduProgress<'slot, B, const SLOTS: usize> {
    Pending(PublishedAmpdu<'slot, B, SLOTS>),
    /// The aggregate ended and released its subframes; the owner is idle.
    Complete {
        outcome: AmpduOutcome,
        owner: Esp32s31AmpduOwner<'slot, B, SLOTS>,
    },
}

/// Commit the buffer's subframes to its owner and publish the aggregate
/// once. An error leaves the owner to be dropped: a reserved aggregate
/// returns its subframes, a published one is quarantined.
pub(super) fn publish<'slot, S, H, const SLOTS: usize>(
    hardware: &mut H,
    buffer: Esp32s31AmpduBuffer<'slot, S, SLOTS>,
    plan: AmpduPlan,
    now: oer_time::Instant,
    publication_timeout: oer_time::Duration,
) -> Result<PublishedAmpdu<'slot, S::Backing, SLOTS>, HtAmpduTxError>
where
    S: AmpduBacking,
    H: HtAmpduHardware,
{
    let Esp32s31AmpduBuffer {
        mut owner,
        subframes,
        ..
    } = buffer;
    let deadline = now
        .checked_add(publication_timeout)
        .ok_or(HtAmpduTxError::ResetRequired)?;
    let cookie = owner.begin()?;
    for subframe in subframes.into_iter().flatten() {
        let layout = AmpduFrameLayout::new(
            0,
            AmpduFrameSize::new(subframe.len, plan.hardware_mic_length),
        )
        .ok_or(HtAmpduTxError::FrameTooLong)?;
        owner.commit_ht(
            cookie,
            subframe.backing,
            HtAmpduFrameRequest::new(layout, 0, plan.rate),
        )?;
    }
    let aggregate = owner.prepared_aggregate(cookie)?;
    let config = ht_ampdu_publication_config(
        plan.role,
        HtAmpduPublicationInputs {
            aggregate_length: aggregate.bytes,
            subframes: aggregate.subframes,
            ..plan.inputs
        },
    )
    .ok_or(HtAmpduTxError::AggregateConfigurationMismatch)?;
    owner.submit(hardware, cookie, plan.queue, config)?;
    Ok(PublishedAmpdu {
        owner,
        cookie,
        deadline,
        settling: false,
    })
}

/// Consume one interrupt or deadline edge several queues share, as
/// `OrdinaryTxOwner::service_queued_single_attempt` does for an MPDU: the
/// queue's BlockAck completion, then its collision detach on a collision
/// edge, then its timeout abort on a timeout edge or the expired deadline,
/// begun only while no other queue's abort forces CCA.
pub(super) fn service<'slot, B, H, const SLOTS: usize>(
    hardware: &mut H,
    mut published: PublishedAmpdu<'slot, B, SLOTS>,
    wake: WifiTxWake,
    may_begin_timeout_abort: bool,
    now: oer_time::Instant,
) -> Result<AmpduProgress<'slot, B, SLOTS>, HtAmpduTxError>
where
    B: StableDmaBacking,
    H: HtAmpduHardware,
{
    use oer_esp32s31_ieee80211_mac::irq::{EVENT_COLLISION, EVENT_TX_TIMEOUT};

    let cookie = published.cookie;
    if published.settling {
        if now < published.deadline {
            return Ok(AmpduProgress::Pending(published));
        }
        published.owner.finish_timeout_abort(hardware, cookie)?;
        return Ok(AmpduProgress::Complete {
            outcome: AmpduOutcome::Aborted,
            owner: published.owner,
        });
    }
    if let Some(completion) = published.owner.acknowledge_completion(hardware)? {
        published.owner.detach_completed(hardware, cookie)?;
        published.owner.release_completed(cookie)?;
        return Ok(AmpduProgress::Complete {
            outcome: AmpduOutcome::Completed(completion),
            owner: published.owner,
        });
    }
    let events = match wake {
        WifiTxWake::Interrupt { events } => events,
        WifiTxWake::Deadline => 0,
    };
    if events & EVENT_COLLISION != 0 && published.owner.abort_collision(hardware, cookie)? {
        return Ok(AmpduProgress::Complete {
            outcome: AmpduOutcome::Collision,
            owner: published.owner,
        });
    }
    let expired = matches!(wake, WifiTxWake::Deadline) && now >= published.deadline;
    if (events & EVENT_TX_TIMEOUT != 0 || expired) && may_begin_timeout_abort {
        if published.owner.begin_timeout_abort(hardware, cookie)? {
            published.settling = true;
            published.deadline = now
                .checked_add(TX_ABORT_SETTLE)
                .ok_or(HtAmpduTxError::ResetRequired)?;
            return Ok(AmpduProgress::Pending(published));
        }
        if expired {
            // No qualified timeout edge: the leases stay retained until a
            // radio reset.
            published.owner.require_reset(cookie)?;
            return Err(HtAmpduTxError::ResetRequired);
        }
    }
    Ok(AmpduProgress::Pending(published))
}
