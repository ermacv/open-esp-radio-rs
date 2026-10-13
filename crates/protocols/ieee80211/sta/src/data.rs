//! The connected station's data: a sans-IO machine beside the station
//! ([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6131495971)),
//! in the station's and the arbiter's form: an MSDU is an input, an
//! attempt an action ([`StaData::poll_action`]), and
//! [`StaData::poll_deadline`] names when it needs time.
//!
//! An MSDU becomes one MPDU in a buffer of the machine's own: QoS Data by
//! its user priority on a QoS BSS, Data in the station's sequence space
//! otherwise, with the Protected bit and the CCMP header's room on a
//! protected link (the port's backend writes the header). It holds its own
//! tickets and buffers, apart from the station's, so data never takes the
//! station's EAPOL, Null or Deauthentication tokens.
//!
//! A received data MPDU of the station's BSS gives its MSDUs only once the
//! replay check of the association's keys passed it
//! ([`CcmpRxReplayEpoch`](oer_ieee80211_mac::ccmp::CcmpRxReplayEpoch)); the reorder, when it comes, runs before it.
//!
//! The station's link ([`LinkView`]) is only there while data may go: it
//! lends the BSS, the pairwise key (an action carries no key: the service
//! names the lent one when it submits, and only the station removes it),
//! the replay check and, on a non-QoS BSS, the station's sequence space.

use oer_ieee80211_lower_mac::{
    AckHeader, Counter, HeaderSpec, IndividualAddress, MpduLength, ReturnedMpdu, RxCryptoStatus,
    RxEvidence, RxMeta, SizedFrame, TxBufferMemory, TxOutcome,
};
use oer_ieee80211_mac::{
    ccmp::{CcmpHeader, CcmpRxProtection},
    data::{DataInterfaceRole, EthernetFrameParts, decapsulate_data_frames},
    qos::{WmmAccessCategory, WmmUserPriority},
    sequence::SequenceNumber,
    station::StaProtectedDataFrame,
};
use oer_time::Instant;

use crate::epoch::LinkEpoch;
use crate::machine::{LinkMode, LinkProtection, LinkView, StaTokens};
use crate::pool::{Fifo, Pool};

/// The EAPOL ether type, which the station itself carries.
const EAPOL_ETHER_TYPE: u16 = 0x888e;

/// The tokens of the data machine: the port's tickets and the buffers of
/// the class that holds an MSDU's MPDU.
pub trait StaDataTokens {
    /// The checked class carried by the MPDU description and context.
    type Class: oer_ieee80211_lower_mac::TxBufferClass;
    /// A transmit ticket.
    type Ticket;
    /// A transmit buffer: its memory, which the machine encodes into.
    type Buffer: TxBufferMemory;
    /// An immutable published MPDU. Data retains it until retry or retirement.
    type Sent;
    /// Retire an MPDU and release its body before reusing the allocation.
    fn renew(mpdu: Self::Sent) -> Self::Buffer;
}

/// One MSDU from the network stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Msdu<'a> {
    /// Its destination.
    pub destination: [u8; 6],
    /// Its EtherType.
    pub ether_type: u16,
    /// Its user priority.
    pub priority: WmmUserPriority,
    /// Its payload.
    pub payload: &'a [u8],
}

/// One input of the data machine.
pub enum DataInput<'a, 'l, 'v, 'id, T: StaTokens> {
    /// An MSDU from the network stack, on the link it goes on.
    Msdu {
        /// The MSDU.
        msdu: Msdu<'a>,
        /// The station's link.
        link: &'l mut LinkView<'v, 'id, T>,
    },
}

/// Ownership returned from one operation; consuming it cannot refuse resources.
pub enum DataReturn<'id, D: StaDataTokens, K> {
    /// Preparation refused before admission: this MSDU ends, its resources
    /// return and its admission count is released without a radio terminal.
    Refused {
        /// Original operation, moved back from submit.
        context: DataTxContext<'id, K, D::Class>,
        /// Its ticket.
        ticket: D::Ticket,
        /// Its memory.
        buffer: ReturnedMpdu<D::Buffer, D::Sent>,
        /// The explicit preparation failure.
        reason: crate::machine::TxRefusal,
    },
    /// A data MPDU's terminal: its ticket and buffer come back. An attempt
    /// of the current link that ended before any transmission (a closing
    /// gate aborts the ones it holds) goes again: back at the head of its
    /// TID's queue, before newer MSDUs, in its buffer, which comes back
    /// fresh, with its sequence number ([owner decision on #563]). One of
    /// a link that ended only returns its tokens ([`StaData::leave`]).
    ///
    /// [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6132835650
    TxDone {
        /// The original operation, returned unchanged by the port.
        context: DataTxContext<'id, K, D::Class>,
        /// Its ticket.
        ticket: D::Ticket,
        /// Its buffer.
        buffer: ReturnedMpdu<D::Buffer, D::Sent>,
        /// How it ended.
        completion: TxOutcome,
    },
}

/// One action of the data machine.
pub enum DataAction<'id, D: StaDataTokens, K> {
    /// Submit a data MPDU.
    Transmit(DataTransmit<'id, D, K>),
}

/// One data MPDU to submit: its tokens, its kind and its access category.
/// It names no key: on a protected link the service submits it under the
/// link's pairwise key.
pub struct DataTransmit<'id, D: StaDataTokens, K, A = AcPermit> {
    /// The operation identity and parameters, moved through the port.
    pub context: DataTxContext<'id, K, D::Class, A>,
    /// Its ticket.
    pub ticket: D::Ticket,
    /// The buffer holding the MPDU, from its header on.
    pub buffer: ReturnedMpdu<D::Buffer, D::Sent>,
    /// Its kind: an individual frame to the access point.
    pub frame: oer_ieee80211_lower_mac::ClassSizedFrame<D::Class, AckHeader, K>,
    /// The access category of its priority.
    pub access_category: WmmAccessCategory,
    /// It goes under the link's pairwise key.
    pub protected: bool,
}

/// Why an MSDU did not become an MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataRefusal {
    /// The machine holds no ticket or buffer now; the MSDU waits.
    NoTokens,
    /// The MSDU does not fit a buffer.
    TooLong,
}

/// The original data operation. It travels by value with the attempt;
/// completion routing never inspects the returned MPDU's bytes.
/// Its closed, checked frame description survives every unpublished return;
/// neither the MPDU length nor its header is reconstructed at completion.
#[derive(Debug)]
pub struct DataTxContext<'id, K, C, A = AcPermit> {
    admission: A,
    link: LinkEpoch<'id>,
    tid: u8,
    frame: oer_ieee80211_lower_mac::ClassSizedFrame<C, AckHeader, K>,
    access_category: WmmAccessCategory,
    protected: bool,
    order: u32,
}

/// One non-cloneable permission for a data attempt in an access category.
/// Only the data machine's depth pool creates it; the context returns it.
#[derive(Debug)]
pub struct AcPermit {
    category: WmmAccessCategory,
    bit: core::num::NonZeroU8,
}

/// At most eight tickets can exist in this machine. One bit represents
/// one actual permission; taking clears it, returning the owned token sets
/// it. No increment/decrement or metadata lookup reconstructs admission.
struct Admissions {
    best_effort: u8,
    background: u8,
    video: u8,
    voice: u8,
}

impl Admissions {
    fn new(depth: core::num::NonZeroU8, tickets: usize) -> Self {
        let count = usize::from(depth.get()).min(tickets);
        let mask = (0..count).fold(0u8, |mask, _| (mask << 1) | 1);
        Self {
            best_effort: mask,
            background: mask,
            video: mask,
            voice: mask,
        }
    }

    fn available(&self, category: WmmAccessCategory) -> u8 {
        match category {
            WmmAccessCategory::BestEffort => self.best_effort,
            WmmAccessCategory::Background => self.background,
            WmmAccessCategory::Video => self.video,
            WmmAccessCategory::Voice => self.voice,
        }
    }

    fn pool(&mut self, category: WmmAccessCategory) -> &mut u8 {
        match category {
            WmmAccessCategory::BestEffort => &mut self.best_effort,
            WmmAccessCategory::Background => &mut self.background,
            WmmAccessCategory::Video => &mut self.video,
            WmmAccessCategory::Voice => &mut self.voice,
        }
    }

    fn take(&mut self, category: WmmAccessCategory) -> Option<AcPermit> {
        let pool = self.pool(category);
        let bit = core::num::NonZeroU8::new(*pool & pool.wrapping_neg())?;
        *pool &= !bit.get();
        Some(AcPermit { category, bit })
    }

    fn put(&mut self, permit: AcPermit) {
        *self.pool(permit.category) |= permit.bit.get();
    }
}

impl<'id, K, C, A> DataTxContext<'id, K, C, A> {
    fn admission<B>(self, admission: B) -> (DataTxContext<'id, K, C, B>, A) {
        (
            DataTxContext {
                admission,
                link: self.link,
                tid: self.tid,
                frame: self.frame,
                access_category: self.access_category,
                protected: self.protected,
                order: self.order,
            },
            self.admission,
        )
    }
}

/// What the data machine dropped, counted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DataCounters {
    /// Current-link MSDUs ended by a preparation refusal, before admission.
    pub preparation_refused: u32,
    /// Current-link MSDUs retired after an unknown hardware outcome.
    pub indeterminate: u32,
    /// Current-link MSDUs retired after contention budget exhaustion.
    pub contention_limited: u32,
    /// MPDUs a full queue could not hold: their tokens came back, their
    /// MSDUs are gone.
    pub overflows: u32,
    /// Tokens given back beyond the machine's own: dropped.
    pub lost_tokens: u32,
    /// MPDUs of a link that ended before they reached the air: the ones
    /// owed when it ended, and its submitted ones that ended without a
    /// transmission.
    pub left_behind: u32,
}

/// The connected station's data machine. `CAP` sizes each TID's queues of
/// owed MPDUs: each holds a ticket, so the machine's tickets
/// fit (checked at build).
pub struct StaData<'id, D: StaDataTokens, K, const CAP: usize = 8> {
    tickets: Pool<D::Ticket>,
    buffers: Pool<D::Buffer>,
    /// The attempts owed, by TID (a non-QoS link's in the first), each in
    /// its order.
    owed: [Fifo<DataTransmit<'id, D, K, ()>, CAP>; 8],
    admission: Admissions,
    /// The order of the next MSDU.
    next_order: u32,
    /// The current link: [`Self::leave`] ends it.
    link: Option<LinkEpoch<'id>>,
    overflows: Counter,
    left_behind: Counter,
    preparation_refused: Counter,
    indeterminate: Counter,
    contention_limited: Counter,
}

impl<'id, D: StaDataTokens, K: oer_ieee80211_lower_mac::Ieee80211Capabilities, const CAP: usize>
    StaData<'id, D, K, CAP>
{
    /// The machine with the tickets and buffers the composition granted it,
    /// at most eight of a kind and no more tickets than its queues hold,
    /// checked at compile time.
    pub fn new<const TICKETS: usize, const BUFFERS: usize>(
        tickets: [D::Ticket; TICKETS],
        buffers: [D::Buffer; BUFFERS],
        depth: core::num::NonZeroU8,
    ) -> Self {
        const {
            assert!(
                TICKETS <= CAP,
                "each MPDU a data queue holds takes a ticket: the queues hold every ticket"
            );
        };
        Self {
            tickets: Pool::new(tickets),
            buffers: Pool::new(buffers),
            owed: [const { Fifo::new() }; 8],
            admission: Admissions::new(depth, TICKETS),
            next_order: 0,
            link: None,
            preparation_refused: Counter::ZERO,
            indeterminate: Counter::ZERO,
            contention_limited: Counter::ZERO,
            overflows: Counter::ZERO,
            left_behind: Counter::ZERO,
        }
    }

    /// What the machine dropped.
    pub const fn counters(&self) -> DataCounters {
        DataCounters {
            preparation_refused: self.preparation_refused.get(),
            indeterminate: self.indeterminate.get(),
            contention_limited: self.contention_limited.get(),
            overflows: self.overflows.get(),
            lost_tokens: self.tickets.lost().combined(self.buffers.lost()).get(),
            left_behind: self.left_behind.get(),
        }
    }

    /// The station's link ended: the queue belongs to it
    /// ([owner decision on #563]). Every MPDU owed for it drains into its
    /// tokens, counted; its submitted ones return their tokens with their
    /// terminals and never go again.
    ///
    /// [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6133717216
    pub fn leave(&mut self) {
        for tid in 0..8 {
            while let Some(owed) = self.owed[tid].pop() {
                self.left_behind.increment();
                self.tickets.put(owed.ticket);
                self.buffers.put(owed.buffer.into_fresh(D::renew));
            }
        }
        self.link = None;
    }

    /// An MPDU a full queue did not take: its tokens come back, counted.
    fn overflowed(&mut self, transmit: DataTransmit<'id, D, K, ()>) {
        self.overflows.increment();
        self.tickets.put(transmit.ticket);
        self.buffers.put(transmit.buffer.into_fresh(D::renew));
    }

    /// Whether the machine takes one more MSDU now.
    pub fn accepts(&self) -> bool {
        !self.tickets.is_empty() && !self.buffers.is_empty()
    }

    /// Return an accepted terminal or a local refusal and all its owners.
    /// This operation always consumes the resources; it has no refusal result.
    pub fn returned(&mut self, _now: Instant, input: DataReturn<'id, D, K>) {
        match input {
            DataReturn::Refused {
                context,
                ticket,
                buffer,
                reason: _,
            } => {
                let (context, permit) = context.admission(());
                self.admission.put(permit);
                if self.is_current(&context) {
                    self.preparation_refused.increment();
                }
                self.tickets.put(ticket);
                self.buffers.put(buffer.into_fresh(D::renew));
            }
            DataReturn::TxDone {
                context,
                ticket,
                buffer,
                completion,
            } => {
                let (context, permit) = context.admission(());
                self.admission.put(permit);
                if !self.is_current(&context) {
                    if matches!(completion, TxOutcome::Withdrawn(_)) {
                        self.left_behind.increment();
                    }
                    self.tickets.put(ticket);
                    self.buffers.put(buffer.into_fresh(D::renew));
                    return;
                }
                match completion {
                    TxOutcome::Indeterminate(_) => {
                        self.indeterminate.increment();
                    }
                    TxOutcome::Ended {
                        status: oer_ieee80211_lower_mac::TxEndStatus::ContentionLimit,
                        ..
                    } => {
                        self.contention_limited.increment();
                    }
                    _ => {}
                }
                // Withdrawn: preserve the original frame description,
                // TID and order, even after a batch of terminals returns.
                if matches!(completion, TxOutcome::Withdrawn(_)) {
                    let frame = context.frame;
                    let tid = usize::from(context.tid);
                    let again = DataTransmit {
                        ticket,
                        buffer,
                        frame,
                        access_category: context.access_category,
                        protected: context.protected,
                        context,
                    };
                    let next_order = self.next_order;
                    if let Err(again) = self.owed[tid].insert_by(again, |a, b| {
                        a.context.order.wrapping_sub(next_order)
                            < b.context.order.wrapping_sub(next_order)
                    }) {
                        self.overflowed(again);
                    }
                    return;
                }
                self.tickets.put(ticket);
                self.buffers.put(buffer.into_fresh(D::renew));
            }
        }
    }

    /// Take one input. An MSDU goes on its link and becomes an attempt
    /// owed.
    ///
    /// # Errors
    ///
    /// Why an MSDU did not become an MPDU: it waits, or is too long.
    pub fn input<T: StaTokens>(
        &mut self,
        _now: Instant,
        input: DataInput<'_, '_, '_, 'id, T>,
    ) -> Result<(), DataRefusal> {
        match input {
            DataInput::Msdu { msdu, link } => {
                if self.link != Some(link.epoch) {
                    self.leave();
                    self.link = Some(link.epoch);
                }
                let ticket = self.tickets.take().ok_or(DataRefusal::NoTokens)?;
                let Some(mut buffer) = self.buffers.take() else {
                    self.tickets.put(ticket);
                    return Err(DataRefusal::NoTokens);
                };
                let tid = msdu.priority.value();
                let qos = matches!(link.mode, LinkMode::Qos(_));
                // A non-QoS link has one queue, best effort.
                let (slot, access_category) = if qos {
                    (usize::from(tid), msdu.priority.access_category())
                } else {
                    (0, WmmAccessCategory::BestEffort)
                };
                let protected = matches!(link.protection, LinkProtection::Ccmp { .. });
                // A frame that does not fit its buffer takes no sequence
                // number.
                if mpdu_len(qos, protected, msdu.payload.len()) > buffer.bytes_mut().len() {
                    self.tickets.put(ticket);
                    self.buffers.put(buffer);
                    return Err(DataRefusal::TooLong);
                }
                let sequence_number = match &mut link.mode {
                    LinkMode::Qos(counters) => counters[usize::from(tid)].take(),
                    LinkMode::NonQos(counter) => counter.take(),
                };
                let frame = encode_frame::<_, K, D::Class>(
                    link,
                    qos,
                    msdu,
                    sequence_number,
                    protected,
                    buffer.bytes_mut(),
                );
                match frame {
                    Some(frame) => {
                        let order = self.next_order;
                        self.next_order = self.next_order.wrapping_add(1);
                        let context = DataTxContext {
                            admission: (),
                            link: link.epoch,
                            tid: slot as u8,
                            frame,
                            access_category,
                            protected,
                            order,
                        };
                        let owed = DataTransmit {
                            context,
                            ticket,
                            buffer: ReturnedMpdu::Fresh(buffer),
                            frame,
                            access_category,
                            protected,
                        };
                        if let Err(owed) = self.owed[slot].push(owed) {
                            self.overflowed(owed);
                        }
                        Ok(())
                    }
                    None => {
                        self.tickets.put(ticket);
                        self.buffers.put(buffer);
                        Err(DataRefusal::TooLong)
                    }
                }
            }
        }
    }

    /// Free data permissions in this category, bounded by the backend's
    /// declared queue depth and the number of tickets granted to Data.
    pub fn available(&self, category: WmmAccessCategory) -> u32 {
        self.admission.available(category).count_ones()
    }

    /// Whether this operation belongs to the current link. The service
    /// uses the same guard before updating the link's rate selection.
    pub fn is_current<A>(&self, context: &DataTxContext<'id, K, D::Class, A>) -> bool {
        Some(context.link) == self.link
    }

    /// The next action whose category owns a free admission permission.
    /// The context carries that permission until its return; queued work
    /// consumes none. The service takes every action before waiting.
    pub fn poll_action(&mut self, _now: Instant) -> Option<DataAction<'id, D, K>> {
        // The earliest MSDU of an admitted category first, whatever its TID.
        let tid = (0..8)
            .filter_map(|tid| {
                self.owed[tid]
                    .front()
                    .filter(|owed| self.admission.available(owed.access_category) != 0)
                    .map(|owed| (owed.context.order, tid))
            })
            .min_by_key(|&(order, _)| order.wrapping_sub(self.next_order))?
            .1;
        // A queue can be empty or an AC can have no free permit: both are
        // normal polling states. Reserve and pop under the same exclusive
        // borrow; no actor can change either between these operations.
        let category = self.owed[tid].front()?.access_category;
        let permit = self.admission.take(category)?;
        self.owed[tid].pop().map(|transmit| {
            let (context, ()) = transmit.context.admission(permit);
            DataAction::Transmit(DataTransmit {
                context,
                ticket: transmit.ticket,
                buffer: transmit.buffer,
                frame: transmit.frame,
                access_category: transmit.access_category,
                protected: transmit.protected,
            })
        })
    }

    /// When the machine next needs [`Self::poll_action`] without an input:
    /// never, while it keeps no timer.
    pub const fn poll_deadline(&self) -> Option<Instant> {
        None
    }
}

/// The MSDUs of a data MPDU of `link`'s BSS to the station, which `meta`
/// describes: a protected one only once the hardware decrypted and verified
/// it and the replay check passed it. An EAPOL frame is the station's, not
/// the network's.
pub fn received<'f, T: StaTokens>(
    link: &mut LinkView<'_, '_, T>,
    frame: &'f [u8],
    meta: &RxMeta,
) -> Option<impl Iterator<Item = EthernetFrameParts<'f>>> {
    let control = u16::from_le_bytes(frame.get(0..2)?.try_into().ok()?);
    // Data (type 2) from the DS.
    if (control >> 2) & 0b11 != 2 || control & 0x0300 != 0x0200 {
        return None;
    }
    if frame.get(10..16)? != link.bssid {
        return None;
    }
    let destination: [u8; 6] = frame.get(4..10)?.try_into().ok()?;
    let qos = control & 0x0080 != 0;
    let order = control & 0x8000 != 0;
    let mut offset = if qos { 26 } else { 24 };
    if qos && order {
        offset += 4;
    }
    let tid = qos
        .then(|| frame.get(24).map(|control| control & 0x0f))
        .flatten();
    match &mut link.protection {
        LinkProtection::Ccmp { replay, .. } => {
            if control & 0x4000 == 0 {
                return None;
            }
            let verified = matches!(
                meta.crypto,
                RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
                    | RxEvidence::ProtocolValidated(RxCryptoStatus::DecryptedAndIntegrityVerified)
            );
            if !verified {
                return None;
            }
            // The CCMP header stays; the MIC is gone.
            let header = CcmpHeader::parse(frame.get(offset..offset + 8)?.try_into().ok()?).ok()?;
            let protection = if destination == link.address {
                CcmpRxProtection::Pairwise
            } else if destination[0] & 1 == 1 {
                CcmpRxProtection::Group
            } else {
                CcmpRxProtection::Other
            };
            replay.check(protection, tid, header).ok()?;
            offset += 8;
        }
        LinkProtection::Open => {
            if control & 0x4000 != 0 || (destination != link.address && destination[0] & 1 == 0) {
                return None;
            }
        }
    }
    let length = frame.len().checked_sub(offset)?;
    let frames = decapsulate_data_frames(DataInterfaceRole::Station, frame, offset, length).ok()?;
    Some(
        frames
            .filter_map(Result::ok)
            .filter(|parts| parts.ether_type != EAPOL_ETHER_TYPE),
    )
}

/// The length of the MPDU of a payload of `payload` octets: its header,
/// the CCMP header's room on a protected link, the LLC/SNAP header and the
/// payload.
const fn mpdu_len(qos: bool, protected: bool, payload: usize) -> usize {
    let header = if qos { 26 } else { 24 };
    let ccmp = if protected { 8 } else { 0 };
    header + ccmp + 8 + payload
}

/// The MPDU of `msdu` on `link`, written into `output`.
fn encode_frame<
    T: StaTokens,
    K: oer_ieee80211_lower_mac::Ieee80211Capabilities,
    C: oer_ieee80211_lower_mac::TxBufferClass,
>(
    link: &LinkView<'_, '_, T>,
    qos: bool,
    msdu: Msdu<'_>,
    sequence_number: SequenceNumber,
    protected: bool,
    output: &mut [u8],
) -> Option<oer_ieee80211_lower_mac::ClassSizedFrame<C, AckHeader, K>> {
    let len = StaProtectedDataFrame {
        source: link.address,
        bssid: link.bssid,
        destination: msdu.destination,
        sequence_number,
        user_priority: msdu.priority.value(),
        peer_qos: qos,
        // The backend writes the CCMP header at the first transmission.
        ccmp_header: [0; 8],
        ether_type: msdu.ether_type,
        payload: msdu.payload,
    }
    .encode(output)
    .ok()?;
    if !protected {
        // Without a key: no Protected bit and no CCMP header.
        let header = if qos { 26 } else { 24 };
        output[1] &= !0x40;
        output.copy_within(header + 8..len, header);
        return sized(&output[..len - 8]);
    }
    sized(&output[..len])
}

fn sized<
    K: oer_ieee80211_lower_mac::Ieee80211Capabilities,
    C: oer_ieee80211_lower_mac::TxBufferClass,
>(
    frame: &[u8],
) -> Option<oer_ieee80211_lower_mac::ClassSizedFrame<C, AckHeader, K>> {
    let header = HeaderSpec::decode(frame, IndividualAddress::new)?;
    oer_ieee80211_lower_mac::ClassSizedFrame::new(SizedFrame::new(
        AckHeader::new(header)?,
        MpduLength::new(frame.len())?,
    )?)
    .ok()
}

#[cfg(test)]
mod tests;
