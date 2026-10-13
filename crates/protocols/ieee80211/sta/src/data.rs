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
    AckHeader, HeaderSpec, IndividualAddress, MpduLength, RxCryptoStatus, RxEvidence, RxMeta,
    SizedFrame, TxBufferMemory, TxCompletion,
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
    /// A transmit ticket.
    type Ticket;
    /// A transmit buffer: its memory, which the machine encodes into.
    type Buffer: TxBufferMemory;
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
pub enum DataInput<'a, 'l, 'v, 'id, D: StaDataTokens, T: StaTokens, K> {
    /// An MSDU from the network stack, on the link it goes on.
    Msdu {
        /// The MSDU.
        msdu: Msdu<'a>,
        /// The station's link.
        link: &'l mut LinkView<'v, 'id, T>,
    },
    /// Preparation refused before admission: this MSDU ends, its resources
    /// return and its admission count is released without a radio terminal.
    Refused {
        /// Original operation, moved back from submit.
        context: DataTxContext<'id, K>,
        /// Its ticket.
        ticket: D::Ticket,
        /// Its memory.
        buffer: D::Buffer,
        /// The explicit preparation failure.
        reason: oer_ieee80211_lower_mac::PreparationError,
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
        context: DataTxContext<'id, K>,
        /// Its ticket.
        ticket: D::Ticket,
        /// Its buffer.
        buffer: D::Buffer,
        /// How it ended.
        completion: TxCompletion,
    },
    /// Tokens that came back without an MPDU's terminal: an MPDU the
    /// service did not send, or a terminal that carried a buffer more or
    /// none. Its MSDU, if any, is gone; the service counts it.
    Returned {
        /// An operation that ended locally, when these are its resources.
        context: Option<DataTxContext<'id, K>>,
        /// A ticket, if one came back.
        ticket: Option<D::Ticket>,
        /// A buffer, if one came back.
        buffer: Option<D::Buffer>,
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
pub struct DataTransmit<'id, D: StaDataTokens, K> {
    /// The operation identity and parameters, moved through the port.
    pub context: DataTxContext<'id, K>,
    /// Its ticket.
    pub ticket: D::Ticket,
    /// The buffer holding the MPDU, from its header on.
    pub buffer: D::Buffer,
    /// Its kind: an individual frame to the access point.
    pub frame: SizedFrame<AckHeader, K>,
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
pub struct DataTxContext<'id, K> {
    link: LinkEpoch<'id>,
    tid: u8,
    frame: SizedFrame<AckHeader, K>,
    access_category: WmmAccessCategory,
    protected: bool,
    order: u32,
}

/// What the data machine dropped, counted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DataCounters {
    /// Current-link MSDUs ended by a preparation refusal, before admission.
    pub preparation_refused: u32,
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
    owed: [Fifo<DataTransmit<'id, D, K>, CAP>; 8],
    /// Admission counts only; operation metadata belongs to the attempt.
    at_port: [usize; 4],
    /// The order of the next MSDU.
    next_order: u32,
    /// The current link: [`Self::leave`] ends it.
    link: Option<LinkEpoch<'id>>,
    overflows: u32,
    left_behind: u32,
    preparation_refused: u32,
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
            at_port: [0; 4],
            next_order: 0,
            link: None,
            preparation_refused: 0,
            overflows: 0,
            left_behind: 0,
        }
    }

    /// What the machine dropped.
    pub const fn counters(&self) -> DataCounters {
        DataCounters {
            preparation_refused: self.preparation_refused,
            overflows: self.overflows,
            lost_tokens: self.tickets.lost().saturating_add(self.buffers.lost()),
            left_behind: self.left_behind,
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
                self.left_behind = self.left_behind.saturating_add(1);
                self.tickets.put(owed.ticket);
                self.buffers.put(owed.buffer);
            }
        }
        self.link = None;
    }

    /// An MPDU a full queue did not take: its tokens come back, counted.
    fn overflowed(&mut self, transmit: DataTransmit<'id, D, K>) {
        self.overflows = self.overflows.saturating_add(1);
        self.tickets.put(transmit.ticket);
        self.buffers.put(transmit.buffer);
    }

    /// Whether the machine takes one more MSDU now.
    pub fn accepts(&self) -> bool {
        !self.tickets.is_empty() && !self.buffers.is_empty()
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
        input: DataInput<'_, '_, '_, 'id, D, T, K>,
    ) -> Result<(), DataRefusal> {
        match input {
            DataInput::Refused {
                context,
                ticket,
                buffer,
                reason: _,
            } => {
                self.at_port[context.access_category as usize] -= 1;
                if self.is_current(&context) {
                    self.preparation_refused = self.preparation_refused.saturating_add(1);
                }
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Ok(())
            }
            DataInput::TxDone {
                context,
                ticket,
                buffer,
                completion,
            } => {
                self.at_port[context.access_category as usize] -= 1;
                if !self.is_current(&context) {
                    if completion.transmissions == 0 {
                        self.left_behind = self.left_behind.saturating_add(1);
                    }
                    self.tickets.put(ticket);
                    self.buffers.put(buffer);
                    return Ok(());
                }
                // Never published: preserve the original frame description,
                // TID and order, even after a batch of terminals returns.
                if completion.transmissions == 0 {
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
                    return Ok(());
                }
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Ok(())
            }
            DataInput::Returned {
                context,
                ticket,
                buffer,
            } => {
                if let Some(context) = context {
                    self.at_port[context.access_category as usize] -= 1;
                }
                if let Some(ticket) = ticket {
                    self.tickets.put(ticket);
                }
                if let Some(buffer) = buffer {
                    self.buffers.put(buffer);
                }
                Ok(())
            }
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
                let frame = encode_frame(
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
                            buffer,
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

    /// The machine's MPDUs of `access_category` at the port: submitted and
    /// not yet ended, of the current link or an earlier one.
    pub fn at_port(&self, access_category: WmmAccessCategory) -> usize {
        self.at_port[access_category as usize]
    }

    /// Whether this operation belongs to the current link. The service
    /// uses the same guard before updating the link's rate selection.
    pub fn is_current(&self, context: &DataTxContext<'id, K>) -> bool {
        Some(context.link) == self.link
    }

    /// The next action at `now`, if any, of a category `admit` takes: the
    /// service admits a category while its data at the port is below the
    /// backend's queue depth. The service takes every action before it
    /// waits again.
    pub fn poll_action(
        &mut self,
        _now: Instant,
        admit: impl Fn(WmmAccessCategory) -> bool,
    ) -> Option<DataAction<'id, D, K>> {
        // The earliest MSDU of an admitted category first, whatever its TID.
        let tid = (0..8)
            .filter_map(|tid| {
                self.owed[tid]
                    .front()
                    .filter(|owed| admit(owed.access_category))
                    .map(|owed| (owed.context.order, tid))
            })
            .min_by_key(|&(order, _)| order.wrapping_sub(self.next_order))?
            .1;
        let transmit = self.owed[tid].pop()?;
        self.at_port[transmit.access_category as usize] += 1;
        Some(DataAction::Transmit(transmit))
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
fn encode_frame<T: StaTokens, K: oer_ieee80211_lower_mac::Ieee80211Capabilities>(
    link: &LinkView<'_, '_, T>,
    qos: bool,
    msdu: Msdu<'_>,
    sequence_number: SequenceNumber,
    protected: bool,
    output: &mut [u8],
) -> Option<SizedFrame<AckHeader, K>> {
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

fn sized<K: oer_ieee80211_lower_mac::Ieee80211Capabilities>(
    frame: &[u8],
) -> Option<SizedFrame<AckHeader, K>> {
    let header = HeaderSpec::decode(frame, IndividualAddress::new)?;
    SizedFrame::new(AckHeader::new(header)?, MpduLength::new(frame.len())?)
}

#[cfg(test)]
mod tests;
