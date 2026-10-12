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
    SizedFrame, TxBufferMemory,
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
    /// A data MPDU's terminal: its ticket and buffer come back.
    TxDone {
        /// Its ticket.
        ticket: D::Ticket,
        /// Its buffer.
        buffer: D::Buffer,
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

/// The connected station's data machine.
pub struct StaData<D: StaDataTokens, K> {
    tickets: Pool<D::Ticket>,
    buffers: Pool<D::Buffer>,
    /// The attempts owed, in their order.
    owed: Fifo<DataTransmit<D, K>>,
}

impl<D: StaDataTokens, K: oer_ieee80211_lower_mac::Ieee80211Capabilities> StaData<D, K> {
    /// The machine with the tickets and buffers the composition granted it,
    /// at most eight of a kind, checked at compile time.
    pub fn new<const TICKETS: usize, const BUFFERS: usize>(
        tickets: [D::Ticket; TICKETS],
        buffers: [D::Buffer; BUFFERS],
    ) -> Self {
        Self {
            tickets: Pool::new(tickets),
            buffers: Pool::new(buffers),
            owed: Fifo::new(),
        }
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
            DataInput::TxDone { ticket, buffer } => {
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
                        self.owed.push(DataTransmit {
                            ticket,
                            buffer,
                            frame,
                            access_category: msdu.priority.access_category(),
                            protected,
                        });
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

    /// The next action at `now`, if any. The service takes every action
    /// before it waits again.
    pub fn poll_action(&mut self, _now: Instant) -> Option<DataAction<D, K>> {
        self.owed.pop().map(DataAction::Transmit)
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

fn sized<K: oer_ieee80211_lower_mac::Ieee80211Capabilities>(
    frame: &[u8],
) -> Option<SizedFrame<AckHeader, K>> {
    let header = HeaderSpec::decode(frame, IndividualAddress::new)?;
    SizedFrame::new(AckHeader::new(header)?, MpduLength::new(frame.len())?)
}

#[cfg(test)]
mod tests;
