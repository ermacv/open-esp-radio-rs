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
pub enum DataInput<'a, 'l, 'v, D: StaDataTokens, T: StaTokens> {
    /// An MSDU from the network stack, on the link it goes on.
    Msdu {
        /// The MSDU.
        msdu: Msdu<'a>,
        /// The station's link.
        link: &'l mut LinkView<'v, T>,
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
        /// A ticket, if one came back.
        ticket: Option<D::Ticket>,
        /// A buffer, if one came back.
        buffer: Option<D::Buffer>,
    },
}

/// One action of the data machine.
pub enum DataAction<D: StaDataTokens, K> {
    /// Submit a data MPDU.
    Transmit(DataTransmit<D, K>),
}

/// One data MPDU to submit: its tokens, its kind and its access category.
/// It names no key: on a protected link the service submits it under the
/// link's pairwise key.
pub struct DataTransmit<D: StaDataTokens, K> {
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

/// An attempt owed, with its place in the order MSDUs came.
struct Owed<D: StaDataTokens, K> {
    transmit: DataTransmit<D, K>,
    order: u32,
}

/// What the machine keeps of a submitted MPDU to send it again: its
/// buffer holds the frame.
struct InFlight {
    /// The link it was encoded for.
    link: u32,
    len: usize,
    access_category: WmmAccessCategory,
    protected: bool,
    order: u32,
}

/// What the data machine dropped, counted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DataCounters {
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
/// owed and submitted MPDUs: each holds a ticket, so the machine's tickets
/// fit (checked at build).
pub struct StaData<D: StaDataTokens, K, const CAP: usize = 8> {
    tickets: Pool<D::Ticket>,
    buffers: Pool<D::Buffer>,
    /// The attempts owed, by TID (a non-QoS link's in the first), each in
    /// its order.
    owed: [Fifo<Owed<D, K>, CAP>; 8],
    /// The submitted MPDUs, by TID, in their order: a port ends a queue's
    /// attempts in order.
    in_flight: [Fifo<InFlight, CAP>; 8],
    /// The order of the next MSDU.
    next_order: u32,
    /// The current link: [`Self::leave`] ends it.
    link: u32,
    overflows: u32,
    left_behind: u32,
}

impl<D: StaDataTokens, K: oer_ieee80211_lower_mac::Ieee80211Capabilities, const CAP: usize>
    StaData<D, K, CAP>
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
            in_flight: [const { Fifo::new() }; 8],
            next_order: 0,
            link: 0,
            overflows: 0,
            left_behind: 0,
        }
    }

    /// What the machine dropped.
    pub const fn counters(&self) -> DataCounters {
        DataCounters {
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
                self.tickets.put(owed.transmit.ticket);
                self.buffers.put(owed.transmit.buffer);
            }
        }
        self.link = self.link.wrapping_add(1);
    }

    /// An MPDU a full queue did not take: its tokens come back, counted.
    fn overflowed(&mut self, transmit: DataTransmit<D, K>) {
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
        input: DataInput<'_, '_, '_, D, T>,
    ) -> Result<(), DataRefusal> {
        match input {
            DataInput::TxDone {
                ticket,
                mut buffer,
                completion,
            } => {
                let tid = tid_of(buffer.bytes_mut());
                let in_flight = self.in_flight[tid].pop();
                // Of a link that ended: its tokens come back, nothing more.
                if let Some(InFlight { link, .. }) = in_flight
                    && link != self.link
                {
                    if completion.transmissions == 0 {
                        self.left_behind = self.left_behind.saturating_add(1);
                    }
                    self.tickets.put(ticket);
                    self.buffers.put(buffer);
                    return Ok(());
                }
                // Never on the air: the same MPDU goes again, first.
                if completion.transmissions == 0
                    && let Some(in_flight) = in_flight
                    && let Some(frame) = sized::<K>(&buffer.bytes_mut()[..in_flight.len])
                {
                    let again = Owed {
                        transmit: DataTransmit {
                            ticket,
                            buffer,
                            frame,
                            access_category: in_flight.access_category,
                            protected: in_flight.protected,
                        },
                        order: in_flight.order,
                    };
                    if let Err(again) = self.owed[tid].push_front(again) {
                        self.overflowed(again.transmit);
                    }
                    return Ok(());
                }
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Ok(())
            }
            DataInput::Returned { ticket, buffer } => {
                if let Some(ticket) = ticket {
                    self.tickets.put(ticket);
                }
                if let Some(buffer) = buffer {
                    self.buffers.put(buffer);
                }
                Ok(())
            }
            DataInput::Msdu { msdu, link } => {
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
                        let owed = Owed {
                            transmit: DataTransmit {
                                ticket,
                                buffer,
                                frame,
                                access_category,
                                protected,
                            },
                            order,
                        };
                        if let Err(owed) = self.owed[slot].push(owed) {
                            self.overflowed(owed.transmit);
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
        self.in_flight
            .iter()
            .flat_map(Fifo::iter)
            .filter(|in_flight| in_flight.access_category == access_category)
            .count()
    }

    /// The next action at `now`, if any, of a category `admit` takes: the
    /// service admits a category while its data at the port is below the
    /// backend's queue depth. The service takes every action before it
    /// waits again.
    pub fn poll_action(
        &mut self,
        _now: Instant,
        admit: impl Fn(WmmAccessCategory) -> bool,
    ) -> Option<DataAction<D, K>> {
        // The earliest MSDU of an admitted category first, whatever its TID.
        let tid = (0..8)
            .filter_map(|tid| {
                self.owed[tid]
                    .front()
                    .filter(|owed| admit(owed.transmit.access_category))
                    .map(|owed| (owed.order, tid))
            })
            .min_by_key(|&(order, _)| order.wrapping_sub(self.next_order))?
            .1;
        let Owed { transmit, order } = self.owed[tid].pop()?;
        let in_flight = InFlight {
            link: self.link,
            len: transmit.frame.len().get(),
            access_category: transmit.access_category,
            protected: transmit.protected,
            order,
        };
        // Untracked, an MPDU could not go again: it does not go.
        if self.in_flight[tid].push(in_flight).is_err() {
            self.overflowed(transmit);
            return None;
        }
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
    link: &mut LinkView<'_, T>,
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
    link: &LinkView<'_, T>,
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

/// The queue of an MPDU in its buffer: its TID on QoS Data, the first
/// otherwise.
fn tid_of(frame: &[u8]) -> usize {
    match frame {
        [control, _, ..] if control & 0x80 != 0 => {
            frame.get(24).map_or(0, |qos| usize::from(qos & 0x07))
        }
        _ => 0,
    }
}

fn sized<K: oer_ieee80211_lower_mac::Ieee80211Capabilities>(
    frame: &[u8],
) -> Option<SizedFrame<AckHeader, K>> {
    let header = HeaderSpec::decode(frame, IndividualAddress::new)?;
    SizedFrame::new(AckHeader::new(header)?, MpduLength::new(frame.len())?)
}

#[cfg(test)]
mod tests;
