//! The connected station's data: a sans-IO machine beside the station
//! ([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6131495971)).
//!
//! It encapsulates an MSDU the network stack hands it into one MPDU in a
//! buffer of its own (QoS Data by the MSDU's priority on a QoS BSS, Data
//! otherwise; the Protected bit and the CCMP header's room on a protected
//! link, whose header the port's backend writes), and decapsulates a data
//! MPDU of the station's BSS into the MSDUs it carries. It holds its own
//! tickets and buffers, apart from the station's, so data never takes the
//! station's EAPOL, Null or Deauthentication tokens.
//!
//! The station's link ([`LinkView`]) lends it the BSS and the pairwise key:
//! an action of the machine carries no key, the service names the lent one
//! when it submits, and only the station removes it. While the link may not
//! transmit (an absence, the station asleep, a protected link without its
//! key) the service keeps the MSDUs in their queue.
//!
//! Not yet: A-MPDUs, Block Ack agreements, the receive reorder and replay
//! check, and power save; they come with the move of the connected receive
//! path and the modem-sleep manager into layer 2.

use oer_ieee80211_lower_mac::{
    AckHeader, HeaderSpec, IndividualAddress, MpduLength, RxCryptoStatus, RxEvidence, RxMeta,
    SizedFrame, TxBufferMemory,
};
use oer_ieee80211_mac::{
    data::{DataInterfaceRole, EthernetFrameParts, decapsulate_data_frames},
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
    station::{StaProtectedDataFrame, StaTxSequenceCounters},
};

use crate::machine::{LinkView, StaTokens};
use crate::pool::Pool;

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
    /// Its 802.1D user priority, 0 to 7.
    pub priority: u8,
    /// Its payload.
    pub payload: &'a [u8],
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
    /// The link may not transmit now; the MSDU waits in its queue.
    NotNow,
    /// The machine holds no ticket or buffer now; the MSDU waits.
    NoTokens,
    /// The MSDU does not fit a buffer, or its priority is not 0 to 7.
    Invalid,
}

/// The connected station's data machine.
pub struct StaData<D: StaDataTokens> {
    tickets: Pool<D::Ticket>,
    buffers: Pool<D::Buffer>,
    /// The QoS sequence spaces, one per TID.
    sequences: StaTxSequenceCounters,
}

impl<D: StaDataTokens> StaData<D> {
    /// The machine with the tickets and buffers the composition granted it,
    /// at most eight of a kind, checked at compile time.
    pub fn new<const TICKETS: usize, const BUFFERS: usize>(
        tickets: [D::Ticket; TICKETS],
        buffers: [D::Buffer; BUFFERS],
    ) -> Self {
        Self {
            tickets: Pool::new(tickets),
            buffers: Pool::new(buffers),
            sequences: StaTxSequenceCounters::new(SequenceNumber::ZERO),
        }
    }

    /// Encapsulate `msdu` for `link` into an MPDU in one of the machine's
    /// buffers. A non-QoS link's data takes `non_qos`, the next number of
    /// the station's non-QoS space ([`Station::take_sequence`](crate::machine::Station::take_sequence)),
    /// which it shares with the station's management frames; a QoS link's
    /// takes the machine's own space of its TID.
    ///
    /// # Errors
    ///
    /// Why it did not: the MSDU waits, or is invalid.
    pub fn encode<T: StaTokens, K: oer_ieee80211_lower_mac::Ieee80211Capabilities>(
        &mut self,
        link: &LinkView<'_, T>,
        msdu: Msdu<'_>,
        non_qos: Option<SequenceNumber>,
    ) -> Result<DataTransmit<D, K>, DataRefusal> {
        if !link.transmit {
            return Err(DataRefusal::NotNow);
        }
        if msdu.priority > 7 || (!link.qos && non_qos.is_none()) {
            return Err(DataRefusal::Invalid);
        }
        let ticket = self.tickets.take().ok_or(DataRefusal::NoTokens)?;
        let Some(mut buffer) = self.buffers.take() else {
            self.tickets.put(ticket);
            return Err(DataRefusal::NoTokens);
        };
        let tid = msdu.priority;
        let sequence_number = match non_qos {
            Some(number) if !link.qos => number,
            _ => self.sequences.take_qos(tid).unwrap_or(SequenceNumber::ZERO),
        };
        let protected = link.pairwise.is_some();
        let encoded = encode_frame(link, msdu, sequence_number, protected, buffer.bytes_mut());
        match encoded {
            Some(frame) => Ok(DataTransmit {
                ticket,
                buffer,
                frame,
                access_category: access_category(tid),
                protected,
            }),
            None => {
                self.tickets.put(ticket);
                self.buffers.put(buffer);
                Err(DataRefusal::Invalid)
            }
        }
    }

    /// A data MPDU's terminal: its ticket and buffer come back.
    pub fn sent(&mut self, ticket: D::Ticket, buffer: D::Buffer) {
        self.tickets.put(ticket);
        self.buffers.put(buffer);
    }

    /// Whether the machine holds a ticket and a buffer for one more MPDU.
    pub fn can_send(&self) -> bool {
        !self.tickets.is_empty() && !self.buffers.is_empty()
    }

    /// The MSDUs of a data MPDU of `link`'s BSS to the station, which `meta`
    /// describes; a protected one only once the hardware decrypted and
    /// verified it. An EAPOL frame is the station's, not the network's.
    pub fn received<'f, T: StaTokens>(
        link: &LinkView<'_, T>,
        frame: &'f [u8],
        meta: &RxMeta,
    ) -> Option<impl Iterator<Item = EthernetFrameParts<'f>>> {
        let control = u16::from_le_bytes(frame.get(0..2)?.try_into().ok()?);
        // Data (type 2) from the DS.
        if (control >> 2) & 0b11 != 2 || control & 0x0300 != 0x0200 {
            return None;
        }
        if frame.get(4..10)? != link.address || frame.get(10..16)? != link.bssid {
            return None;
        }
        let qos = control & 0x0080 != 0;
        let order = control & 0x8000 != 0;
        let mut offset = if qos { 26 } else { 24 };
        if qos && order {
            offset += 4;
        }
        if control & 0x4000 != 0 {
            let verified = matches!(
                meta.crypto,
                RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
                    | RxEvidence::ProtocolValidated(RxCryptoStatus::DecryptedAndIntegrityVerified)
            );
            if !verified {
                return None;
            }
            // The CCMP header stays; the MIC is gone.
            offset += 8;
        }
        let length = frame.len().checked_sub(offset)?;
        let frames = decapsulate_data_frames(DataInterfaceRole::Station, frame, offset, length).ok()?;
        Some(
            frames
                .filter_map(Result::ok)
                .filter(|parts| parts.ether_type != EAPOL_ETHER_TYPE),
        )
    }
}

/// The MPDU of `msdu` on `link`, written into `output`.
fn encode_frame<T: StaTokens, K: oer_ieee80211_lower_mac::Ieee80211Capabilities>(
    link: &LinkView<'_, T>,
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
        user_priority: msdu.priority,
        peer_qos: link.qos,
        // The backend writes the CCMP header at the first transmission.
        ccmp_header: [0; 8],
        ether_type: msdu.ether_type,
        payload: msdu.payload,
    }
    .encode(output)
    .ok()?;
    if !protected {
        // Without a key: no Protected bit and no CCMP header.
        let header = if link.qos { 26 } else { 24 };
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

/// The access category of the user priority `priority` (IEEE 802.11-2020
/// Table 10-1).
const fn access_category(priority: u8) -> WmmAccessCategory {
    match priority {
        1 | 2 => WmmAccessCategory::Background,
        4 | 5 => WmmAccessCategory::Video,
        6 | 7 => WmmAccessCategory::Voice,
        _ => WmmAccessCategory::BestEffort,
    }
}

#[cfg(test)]
mod tests;
