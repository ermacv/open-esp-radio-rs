//! Allocation-free data-fragment reassembly.
//!
//! Fragmentation is an MSDU transform, not a receive-BlockAck sequence
//! space. This owner therefore accepts only individually delivered data MPDUs
//! and binds every retained byte to the complete Sequence Control, QoS/TID,
//! three-address identity and protection class. CCMP fragments additionally
//! retain the exact per-fragment packet number and authenticated plaintext so
//! Retry can suppress only a byte-identical retransmission. Replay-frontier
//! ownership remains outside this module: a caller commits each new PN only
//! after the corresponding fragment has reached this bounded owner.

use crate::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber};
use crate::data::{DataInterfaceRole, EthernetFrameParts, LLC_SNAP_HEADER_LEN};

const DATA: u16 = 0x0008;
const QOS_DATA: u16 = 0x0088;
const TYPE_AND_SUBTYPE: u16 = 0x00fc;
const TO_DS: u16 = 0x0100;
const FROM_DS: u16 = 0x0200;
const MORE_FRAGMENTS: u16 = 0x0400;
const RETRY: u16 = 0x0800;
const PROTECTED: u16 = 0x4000;
const ORDER: u16 = 0x8000;
const QOS_AMSDU_PRESENT: u8 = 0x80;
const RFC1042_LLC_SNAP_PREFIX: [u8; 6] = [0xaa, 0xaa, 0x03, 0, 0, 0];

/// Payload bytes retained for one ordinary 1,500-byte Ethernet payload plus
/// its LLC/SNAP header. The Ethernet header is reconstructed from the exact
/// admitted 802.11 address tuple and therefore consumes no retained bytes.
pub const OPEN_DATA_REASSEMBLY_CAPACITY: usize = 1_500 + LLC_SNAP_HEADER_LEN;

/// Software resource lifetime for an incomplete Open MSDU.
///
/// This is deliberately not presented as an ESP32-S31 hardware value. It is
/// a finite host-owned eviction policy used only when a runtime clock sample
/// accompanies the received fragment.
pub const OPEN_DATA_FRAGMENT_TIMEOUT_MICROS: u64 = 1_000_000;

/// Cryptographic class bound to one complete fragment identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataFragmentProtection {
    Open,
    Ccmp { key_id: CcmpKeyId },
}

/// Exact identity shared by every fragment of one Open data MSDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenDataFragmentIdentity {
    role: DataInterfaceRole,
    protection: DataFragmentProtection,
    receiver_address: [u8; 6],
    transmitter_address: [u8; 6],
    address3: [u8; 6],
    sequence_number: u16,
    qos_control: Option<u16>,
}

impl OpenDataFragmentIdentity {
    pub const fn role(self) -> DataInterfaceRole {
        self.role
    }

    pub const fn protection(self) -> DataFragmentProtection {
        self.protection
    }

    pub const fn receiver_address(self) -> [u8; 6] {
        self.receiver_address
    }

    pub const fn transmitter_address(self) -> [u8; 6] {
        self.transmitter_address
    }

    pub const fn address3(self) -> [u8; 6] {
        self.address3
    }

    pub const fn sequence_number(self) -> u16 {
        self.sequence_number
    }

    pub const fn tid(self) -> Option<u8> {
        match self.qos_control {
            Some(control) => Some((control & 0x000f) as u8),
            None => None,
        }
    }

    pub const fn destination(self) -> [u8; 6] {
        match self.role {
            DataInterfaceRole::Station => self.receiver_address,
            DataInterfaceRole::AccessPoint => self.address3,
        }
    }

    pub const fn source(self) -> [u8; 6] {
        match self.role {
            DataInterfaceRole::Station => self.address3,
            DataInterfaceRole::AccessPoint => self.transmitter_address,
        }
    }

    fn same_sequence_space(self, other: Self) -> bool {
        self.role == other.role
            && self.protection == other.protection
            && self.transmitter_address == other.transmitter_address
            && self.sequence_number == other.sequence_number
            && self.tid() == other.tid()
    }
}

/// One strictly parsed Open-network data fragment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenDataFragment<'frame> {
    identity: OpenDataFragmentIdentity,
    sequence_control: u16,
    fragment_number: u8,
    more_fragments: bool,
    retry: bool,
    packet_number: Option<CcmpPacketNumber>,
    payload: &'frame [u8],
}

impl<'frame> OpenDataFragment<'frame> {
    pub const fn identity(self) -> OpenDataFragmentIdentity {
        self.identity
    }

    pub const fn sequence_control(self) -> u16 {
        self.sequence_control
    }

    pub const fn fragment_number(self) -> u8 {
        self.fragment_number
    }

    pub const fn more_fragments(self) -> bool {
        self.more_fragments
    }

    pub const fn retry(self) -> bool {
        self.retry
    }

    pub const fn packet_number(self) -> Option<CcmpPacketNumber> {
        self.packet_number
    }

    pub const fn payload(self) -> &'frame [u8] {
        self.payload
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenDataFragmentError {
    Truncated,
    NotData,
    NotFragmented,
    Protected,
    Unprotected,
    OrderedUnsupported,
    RoleMismatch,
    InvalidReceiver,
    InvalidDestination,
    InvalidTransmitter,
    AmsduUnsupported,
    EmptyPayload,
    ClockUnavailable,
    NoReassemblyContexts,
    Orphan {
        fragment_number: u8,
    },
    IdentityMismatch,
    MoreFragmentsMismatch,
    RetryPacketNumberMismatch {
        fragment_number: u8,
        expected: CcmpPacketNumber,
        observed: CcmpPacketNumber,
    },
    RetryPayloadMismatch {
        fragment_number: u8,
    },
    PacketNumberNotIncreasing {
        previous: CcmpPacketNumber,
        observed: CcmpPacketNumber,
    },
    OutOfOrder {
        expected: u8,
        observed: u8,
    },
    TooManyFragments,
    ReassembledTooLarge {
        capacity: usize,
    },
    InvalidLlcSnap,
}

/// Parse an Open data fragment from one normalized (FCS-free) MPDU.
///
/// A successful value proves exact three-address role mapping with an
/// individual receiver and logical destination, an unprotected
/// Data/QoS-Data subtype, no HT-Control or A-MSDU, and a nonempty fragment
/// body. Unfragmented MPDUs remain with the ordinary decapsulation path.
pub fn parse_open_data_fragment(
    role: DataInterfaceRole,
    mpdu: &[u8],
) -> Result<OpenDataFragment<'_>, OpenDataFragmentError> {
    let header = parse_data_header(role, mpdu, DataFragmentProtection::Open)?;
    parse_data_fragment(header, mpdu, &mpdu[header.header_length..], None)
}

/// Parse one hardware-authenticated CCMP data fragment.
///
/// The caller must supply only the plaintext payload slice released by a
/// successful CCMP MIC-verification result. This codec validates all public
/// 802.11 identity/AAD fields and binds the parsed Key ID and PN to the
/// returned fragment; it does not itself own or advance a replay frontier.
pub fn parse_ccmp_data_fragment<'frame>(
    role: DataInterfaceRole,
    mpdu: &'frame [u8],
    authenticated_payload: &'frame [u8],
    ccmp_header: CcmpHeader,
) -> Result<OpenDataFragment<'frame>, OpenDataFragmentError> {
    let header = parse_data_header(
        role,
        mpdu,
        DataFragmentProtection::Ccmp {
            key_id: ccmp_header.key_id(),
        },
    )?;
    parse_data_fragment(
        header,
        mpdu,
        authenticated_payload,
        Some(ccmp_header.packet_number()),
    )
}

fn parse_data_fragment<'frame>(
    header: ParsedOpenDataHeader,
    mpdu: &[u8],
    payload: &'frame [u8],
    packet_number: Option<CcmpPacketNumber>,
) -> Result<OpenDataFragment<'frame>, OpenDataFragmentError> {
    if header.qos_amsdu_present {
        return Err(OpenDataFragmentError::AmsduUnsupported);
    }
    if header.identity.receiver_address == [0; 6] || header.identity.receiver_address[0] & 1 != 0 {
        // IEEE fragmentation is defined only for an individual Address 1.
        // Ordinary group-addressed MPDUs remain valid and therefore this
        // check belongs to the fragment constructor, not the shared identity
        // parser used by unfragmented receive.
        return Err(OpenDataFragmentError::InvalidReceiver);
    }
    let destination = header.identity.destination();
    if destination == [0; 6] || destination[0] & 1 != 0 {
        // In To-DS/AP direction Address 1 is the individual BSSID while
        // Address 3 is the MSDU destination. Fragmentation is forbidden for
        // that group-addressed MSDU even though its immediate receiver is
        // individual.
        return Err(OpenDataFragmentError::InvalidDestination);
    }
    let sequence_control = u16::from_le_bytes([mpdu[22], mpdu[23]]);
    let fragment_number = (sequence_control & 0x000f) as u8;
    let more_fragments = header.frame_control & MORE_FRAGMENTS != 0;
    if fragment_number == 0 && !more_fragments {
        return Err(OpenDataFragmentError::NotFragmented);
    }
    if payload.is_empty() {
        return Err(OpenDataFragmentError::EmptyPayload);
    }
    Ok(OpenDataFragment {
        identity: header.identity,
        sequence_control,
        fragment_number,
        more_fragments,
        retry: header.frame_control & RETRY != 0,
        packet_number,
        payload,
    })
}

/// Parse the exact role/address/sequence identity of any Open Data or
/// QoS-Data MPDU, including an unfragmented unit.
///
/// Receive dispatchers consult this identity before ordinary decapsulation
/// so clearing More Fragments on a retry cannot bypass an active fragment
/// context and publish a partial MSDU as standalone Ethernet.
pub fn parse_open_data_identity(
    role: DataInterfaceRole,
    mpdu: &[u8],
) -> Result<OpenDataFragmentIdentity, OpenDataFragmentError> {
    parse_data_header(role, mpdu, DataFragmentProtection::Open).map(|header| header.identity)
}

/// Parse the role/address/sequence identity of an authenticated CCMP data
/// MPDU without exposing or retaining its payload.
pub fn parse_ccmp_data_identity(
    role: DataInterfaceRole,
    mpdu: &[u8],
    ccmp_header: CcmpHeader,
) -> Result<OpenDataFragmentIdentity, OpenDataFragmentError> {
    parse_data_header(
        role,
        mpdu,
        DataFragmentProtection::Ccmp {
            key_id: ccmp_header.key_id(),
        },
    )
    .map(|header| header.identity)
}

#[derive(Clone, Copy)]
struct ParsedOpenDataHeader {
    identity: OpenDataFragmentIdentity,
    frame_control: u16,
    header_length: usize,
    qos_amsdu_present: bool,
}

fn parse_data_header(
    role: DataInterfaceRole,
    mpdu: &[u8],
    protection: DataFragmentProtection,
) -> Result<ParsedOpenDataHeader, OpenDataFragmentError> {
    if mpdu.len() < 24 {
        return Err(OpenDataFragmentError::Truncated);
    }
    let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    let subtype = frame_control & TYPE_AND_SUBTYPE;
    if frame_control & 0x0003 != 0 || (subtype != DATA && subtype != QOS_DATA) {
        return Err(OpenDataFragmentError::NotData);
    }
    match (protection, frame_control & PROTECTED != 0) {
        (DataFragmentProtection::Open, true) => {
            return Err(OpenDataFragmentError::Protected);
        }
        (DataFragmentProtection::Ccmp { .. }, false) => {
            return Err(OpenDataFragmentError::Unprotected);
        }
        _ => {}
    }
    if frame_control & ORDER != 0 {
        return Err(OpenDataFragmentError::OrderedUnsupported);
    }
    let role_matches = matches!(
        (role, frame_control & (TO_DS | FROM_DS)),
        (DataInterfaceRole::Station, FROM_DS) | (DataInterfaceRole::AccessPoint, TO_DS)
    );
    if !role_matches {
        return Err(OpenDataFragmentError::RoleMismatch);
    }

    let qos = subtype == QOS_DATA;
    let header_length = if qos { 26 } else { 24 };
    if mpdu.len() < header_length {
        return Err(OpenDataFragmentError::Truncated);
    }
    let qos_control = if qos {
        Some(u16::from_le_bytes([mpdu[24], mpdu[25]]))
    } else {
        None
    };
    let sequence_control = u16::from_le_bytes([mpdu[22], mpdu[23]]);
    let receiver_address = mpdu[4..10]
        .try_into()
        .expect("validated receiver-address width");
    let transmitter_address: [u8; 6] = mpdu[10..16]
        .try_into()
        .expect("validated transmitter-address width");
    if transmitter_address == [0; 6] || transmitter_address[0] & 1 != 0 {
        return Err(OpenDataFragmentError::InvalidTransmitter);
    }
    let address3 = mpdu[16..22]
        .try_into()
        .expect("validated third-address width");
    Ok(ParsedOpenDataHeader {
        identity: OpenDataFragmentIdentity {
            role,
            protection,
            receiver_address,
            transmitter_address,
            address3,
            sequence_number: sequence_control >> 4,
            qos_control,
        },
        frame_control,
        header_length,
        qos_amsdu_present: qos && mpdu[24] & QOS_AMSDU_PRESENT != 0,
    })
}

/// Borrowed Ethernet view of one fully reassembled and LLC-validated MSDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenReassembledData<'payload> {
    identity: OpenDataFragmentIdentity,
    msdu: &'payload [u8],
}

impl<'payload> OpenReassembledData<'payload> {
    pub const fn identity(self) -> OpenDataFragmentIdentity {
        self.identity
    }

    pub fn ethernet_frame(self) -> EthernetFrameParts<'payload> {
        EthernetFrameParts {
            destination: self.identity.destination(),
            source: self.identity.source(),
            ether_type: u16::from_be_bytes([self.msdu[6], self.msdu[7]]),
            payload: &self.msdu[LLC_SNAP_HEADER_LEN..],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenDataDefragmentation<R> {
    Buffered {
        expired: u8,
        evicted: Option<OpenDataFragmentIdentity>,
    },
    Duplicate {
        expired: u8,
    },
    Complete {
        expired: u8,
        value: R,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenDataUnfragmentedAdmission {
    Admitted { expired: u8 },
    Duplicate { expired: u8 },
}

/// Side-effect-free fragment classification after timeout expiry.
///
/// An admitted token keeps the exact reassembly owner mutably borrowed while
/// an external CCMP owner prepares (but does not commit) its replay candidate.
/// This prevents a replay-invalid fragment from evicting or modifying a live
/// train before the caller has proved that its PN can advance.
pub enum OpenDataFragmentPreflight<'owner, 'frame, const CONTEXTS: usize, const CAPACITY: usize> {
    Duplicate { expired: u8 },
    Admitted(OpenDataFragmentAdmission<'owner, 'frame, CONTEXTS, CAPACITY>),
}

pub struct OpenDataFragmentAdmission<'owner, 'frame, const CONTEXTS: usize, const CAPACITY: usize> {
    owner: &'owner mut OpenDataDefragmenter<CONTEXTS, CAPACITY>,
    fragment: OpenDataFragment<'frame>,
    admission_epoch: u64,
    now_micros: u64,
    expired: u8,
}

impl<'frame, const CONTEXTS: usize, const CAPACITY: usize>
    OpenDataFragmentAdmission<'_, 'frame, CONTEXTS, CAPACITY>
{
    /// Durably retain this already preflighted fragment. For a final fragment,
    /// `complete` is the only edge at which the reassembled MSDU is borrowed.
    pub fn ingest<R>(
        self,
        complete: impl FnOnce(OpenReassembledData<'_>) -> R,
    ) -> Result<OpenDataDefragmentation<R>, OpenDataFragmentError> {
        self.owner.ingest_admitted(
            self.fragment,
            self.admission_epoch,
            self.now_micros,
            self.expired,
            complete,
        )
    }
}

struct ReassemblySlot<const CAPACITY: usize> {
    identity: Option<OpenDataFragmentIdentity>,
    admission_epoch: u64,
    completed: bool,
    expected_fragment: u8,
    final_fragment: u8,
    started_at_micros: u64,
    length: usize,
    fragment_offsets: [usize; 16],
    fragment_lengths: [usize; 16],
    packet_numbers: [Option<CcmpPacketNumber>; 16],
    bytes: [u8; CAPACITY],
}

impl<const CAPACITY: usize> ReassemblySlot<CAPACITY> {
    const fn new() -> Self {
        Self {
            identity: None,
            admission_epoch: 0,
            completed: false,
            expected_fragment: 0,
            final_fragment: 0,
            started_at_micros: 0,
            length: 0,
            fragment_offsets: [0; 16],
            fragment_lengths: [0; 16],
            packet_numbers: [None; 16],
            bytes: [0; CAPACITY],
        }
    }

    fn clear(&mut self) {
        self.identity = None;
        self.admission_epoch = 0;
        self.completed = false;
        self.expected_fragment = 0;
        self.final_fragment = 0;
        self.started_at_micros = 0;
        self.length = 0;
        self.fragment_offsets.fill(0);
        self.fragment_lengths.fill(0);
        self.packet_numbers.fill(None);
    }

    fn is_active(&self) -> bool {
        self.identity.is_some() && !self.completed
    }

    fn verify_retry(&self, fragment: OpenDataFragment<'_>) -> Result<(), OpenDataFragmentError> {
        let number = usize::from(fragment.fragment_number);
        let final_fragment = if self.completed {
            self.final_fragment
        } else {
            self.expected_fragment.saturating_sub(1)
        };
        if fragment.fragment_number > final_fragment {
            return Err(OpenDataFragmentError::OutOfOrder {
                expected: self.expected_fragment,
                observed: fragment.fragment_number,
            });
        }
        let expected_more = fragment.fragment_number < final_fragment || !self.completed;
        if fragment.more_fragments != expected_more {
            return Err(OpenDataFragmentError::MoreFragmentsMismatch);
        }
        match (self.packet_numbers[number], fragment.packet_number) {
            (Some(expected), Some(observed)) if expected != observed => {
                return Err(OpenDataFragmentError::RetryPacketNumberMismatch {
                    fragment_number: fragment.fragment_number,
                    expected,
                    observed,
                });
            }
            (None, None) | (Some(_), Some(_)) => {}
            _ => return Err(OpenDataFragmentError::IdentityMismatch),
        }
        let offset = self.fragment_offsets[number];
        let length = self.fragment_lengths[number];
        if fragment.payload.len() != length
            || self.bytes.get(offset..offset + length) != Some(fragment.payload)
        {
            return Err(OpenDataFragmentError::RetryPayloadMismatch {
                fragment_number: fragment.fragment_number,
            });
        }
        Ok(())
    }
}

/// Fixed-capacity owner of incomplete Open or CCMP data MSDUs.
///
/// The oldest live context is evicted deterministically when all slots are
/// occupied. Callers must provide a monotonic runtime timestamp; synthetic
/// users without one fail closed rather than creating immortal retained data.
pub struct OpenDataDefragmenter<const CONTEXTS: usize, const CAPACITY: usize> {
    slots: [ReassemblySlot<CAPACITY>; CONTEXTS],
    timeout_micros: u64,
}

impl<const CONTEXTS: usize, const CAPACITY: usize> OpenDataDefragmenter<CONTEXTS, CAPACITY> {
    pub const fn new(timeout_micros: u64) -> Self {
        Self {
            slots: [const { ReassemblySlot::new() }; CONTEXTS],
            timeout_micros,
        }
    }

    pub fn active_contexts(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_active()).count()
    }

    /// Revoke every incomplete MSDU and retry fingerprint at an epoch edge.
    pub fn clear(&mut self) -> usize {
        let active = self.active_contexts();
        for slot in &mut self.slots {
            slot.clear();
        }
        active
    }

    /// Revoke contexts owned by one transmitter at an AP peer-close edge.
    pub fn forget_transmitter(&mut self, transmitter: [u8; 6]) -> usize {
        let mut discarded = 0;
        for slot in &mut self.slots {
            if slot
                .identity
                .is_some_and(|identity| identity.transmitter_address == transmitter)
            {
                discarded += usize::from(slot.is_active());
                slot.clear();
            }
        }
        discarded
    }

    /// Discard one exact train after an external replay transaction failed.
    pub fn discard(&mut self, identity: OpenDataFragmentIdentity, admission_epoch: u64) -> bool {
        let Some(slot) = self.slots.iter_mut().find(|slot| {
            slot.identity == Some(identity) && slot.admission_epoch == admission_epoch
        }) else {
            return false;
        };
        slot.clear();
        true
    }

    /// Fence an ordinary Open MPDU against incomplete fragment state.
    pub fn admit_unfragmented(
        &mut self,
        identity: OpenDataFragmentIdentity,
        retry: bool,
        now_micros: Option<u64>,
    ) -> Result<OpenDataUnfragmentedAdmission, OpenDataFragmentError> {
        self.admit_unfragmented_in_epoch(identity, 0, retry, now_micros)
    }

    /// Fence an ordinary Open MPDU in the caller's association epoch.
    pub fn admit_unfragmented_in_epoch(
        &mut self,
        identity: OpenDataFragmentIdentity,
        admission_epoch: u64,
        retry: bool,
        now_micros: Option<u64>,
    ) -> Result<OpenDataUnfragmentedAdmission, OpenDataFragmentError> {
        let relevant_active = self.slots.iter().any(|slot| {
            slot.is_active()
                && slot
                    .identity
                    .is_some_and(|active| active.same_sequence_space(identity))
        });
        let relevant_completed = retry
            && self.slots.iter().any(|slot| {
                slot.completed
                    && slot
                        .identity
                        .is_some_and(|entry| entry.same_sequence_space(identity))
            });
        let expired = match now_micros {
            Some(now_micros) => self.expire(now_micros),
            None if relevant_active || relevant_completed => {
                return Err(OpenDataFragmentError::ClockUnavailable);
            }
            None => 0,
        };
        if self.slots.iter().any(|slot| {
            slot.is_active()
                && slot.identity == Some(identity)
                && slot.admission_epoch == admission_epoch
        }) {
            return Err(OpenDataFragmentError::MoreFragmentsMismatch);
        }
        if self.slots.iter().any(|slot| {
            slot.is_active()
                && slot
                    .identity
                    .is_some_and(|active| active.same_sequence_space(identity))
        }) {
            return Err(OpenDataFragmentError::IdentityMismatch);
        }
        if retry
            && self.slots.iter().any(|slot| {
                slot.completed
                    && slot.identity == Some(identity)
                    && slot.admission_epoch == admission_epoch
            })
        {
            return match identity.protection {
                DataFragmentProtection::Open => {
                    Ok(OpenDataUnfragmentedAdmission::Duplicate { expired })
                }
                DataFragmentProtection::Ccmp { .. } => {
                    Err(OpenDataFragmentError::MoreFragmentsMismatch)
                }
            };
        }
        if retry
            && self.slots.iter().any(|slot| {
                slot.completed
                    && slot
                        .identity
                        .is_some_and(|entry| entry.same_sequence_space(identity))
            })
        {
            return Err(OpenDataFragmentError::IdentityMismatch);
        }
        if !retry {
            // A fresh ordinary MPDU supersedes a retained fragment-completion
            // fingerprint in the same 12-bit sequence space. At high packet
            // rates the sequence number can legitimately wrap inside the
            // completion-history timeout; keeping the old address identity
            // would then reject the new ordinary MPDU's retry before the
            // role-local duplicate filter can recognize it.
            for slot in &mut self.slots {
                if slot.completed
                    && slot
                        .identity
                        .is_some_and(|entry| entry.same_sequence_space(identity))
                {
                    slot.clear();
                }
            }
        }
        Ok(OpenDataUnfragmentedAdmission::Admitted { expired })
    }

    /// Ingest one parsed fragment and synchronously borrow a completed MSDU.
    ///
    /// The completion callback must copy or consume its borrowed Ethernet
    /// view before returning. The backing bytes remain privately retained as
    /// an exact Retry fingerprint until timeout/reuse, so no borrowed payload
    /// can escape this ownership edge.
    pub fn ingest<R>(
        &mut self,
        fragment: OpenDataFragment<'_>,
        now_micros: u64,
        complete: impl FnOnce(OpenReassembledData<'_>) -> R,
    ) -> Result<OpenDataDefragmentation<R>, OpenDataFragmentError> {
        self.ingest_in_epoch(fragment, 0, now_micros, complete)
    }

    /// Ingest within an external association/key admission epoch.
    ///
    /// AP integrations bind this value to the live association generation so
    /// an AID/address reuse cannot complete bytes retained by its predecessor.
    /// Single-peer STA epochs can use [`Self::ingest`], because the complete
    /// defragmenter is already destroyed or cleared on association teardown.
    pub fn ingest_in_epoch<R>(
        &mut self,
        fragment: OpenDataFragment<'_>,
        admission_epoch: u64,
        now_micros: u64,
        complete: impl FnOnce(OpenReassembledData<'_>) -> R,
    ) -> Result<OpenDataDefragmentation<R>, OpenDataFragmentError> {
        match self.preflight_in_epoch(fragment, admission_epoch, now_micros)? {
            OpenDataFragmentPreflight::Duplicate { expired } => {
                Ok(OpenDataDefragmentation::Duplicate { expired })
            }
            OpenDataFragmentPreflight::Admitted(admission) => admission.ingest(complete),
        }
    }

    /// Classify a fragment before an external CCMP replay candidate is
    /// prepared. Only timeout expiry can mutate the owner at this edge.
    pub fn preflight_in_epoch<'owner, 'frame>(
        &'owner mut self,
        fragment: OpenDataFragment<'frame>,
        admission_epoch: u64,
        now_micros: u64,
    ) -> Result<OpenDataFragmentPreflight<'owner, 'frame, CONTEXTS, CAPACITY>, OpenDataFragmentError>
    {
        if CONTEXTS == 0 {
            return Err(OpenDataFragmentError::NoReassemblyContexts);
        }
        let expired = self.expire(now_micros);
        let identity = fragment.identity;

        if fragment.retry {
            if let Some(slot) = self.slots.iter().find(|slot| {
                slot.completed
                    && slot.identity == Some(identity)
                    && slot.admission_epoch == admission_epoch
            }) {
                slot.verify_retry(fragment)?;
                return Ok(OpenDataFragmentPreflight::Duplicate { expired });
            }
            if self.slots.iter().any(|slot| {
                slot.completed
                    && slot
                        .identity
                        .is_some_and(|entry| entry.same_sequence_space(identity))
            }) {
                return Err(OpenDataFragmentError::IdentityMismatch);
            }
        }

        let exact = self.slots.iter().position(|slot| {
            slot.is_active()
                && slot.identity == Some(identity)
                && slot.admission_epoch == admission_epoch
        });
        if fragment.fragment_number == 0
            && fragment.retry
            && let Some(index) = exact
        {
            self.slots[index].verify_retry(fragment)?;
            return Ok(OpenDataFragmentPreflight::Duplicate { expired });
        }
        if fragment.fragment_number == 0
            && fragment.retry
            && self.slots.iter().any(|slot| {
                slot.is_active()
                    && slot
                        .identity
                        .is_some_and(|active| active.same_sequence_space(identity))
            })
        {
            return Err(OpenDataFragmentError::IdentityMismatch);
        }
        if fragment.fragment_number != 0 {
            let Some(index) = exact else {
                if self.slots.iter().any(|slot| {
                    slot.is_active()
                        && slot
                            .identity
                            .is_some_and(|active| active.same_sequence_space(identity))
                }) {
                    return Err(OpenDataFragmentError::IdentityMismatch);
                }
                return Err(OpenDataFragmentError::Orphan {
                    fragment_number: fragment.fragment_number,
                });
            };
            let slot = &mut self.slots[index];
            if fragment.retry && fragment.fragment_number < slot.expected_fragment {
                slot.verify_retry(fragment)?;
                return Ok(OpenDataFragmentPreflight::Duplicate { expired });
            }
            if fragment.fragment_number != slot.expected_fragment {
                return Err(OpenDataFragmentError::OutOfOrder {
                    expected: slot.expected_fragment,
                    observed: fragment.fragment_number,
                });
            }
            if fragment.more_fragments && fragment.fragment_number == 15 {
                return Err(OpenDataFragmentError::TooManyFragments);
            }
        }
        Ok(OpenDataFragmentPreflight::Admitted(
            OpenDataFragmentAdmission {
                owner: self,
                fragment,
                admission_epoch,
                now_micros,
                expired,
            },
        ))
    }

    fn ingest_admitted<R>(
        &mut self,
        fragment: OpenDataFragment<'_>,
        admission_epoch: u64,
        now_micros: u64,
        expired: u8,
        complete: impl FnOnce(OpenReassembledData<'_>) -> R,
    ) -> Result<OpenDataDefragmentation<R>, OpenDataFragmentError> {
        let identity = fragment.identity;
        let exact = self.slots.iter().position(|slot| {
            slot.is_active()
                && slot.identity == Some(identity)
                && slot.admission_epoch == admission_epoch
        });
        if fragment.fragment_number != 0 {
            let Some(index) = exact else {
                return Err(OpenDataFragmentError::Orphan {
                    fragment_number: fragment.fragment_number,
                });
            };
            let slot = &mut self.slots[index];
            if let Err(error) = append_fragment(slot, fragment) {
                slot.clear();
                return Err(error);
            }
            if fragment.more_fragments {
                slot.expected_fragment += 1;
                return Ok(OpenDataDefragmentation::Buffered {
                    expired,
                    evicted: None,
                });
            }

            let length = slot.length;
            if length < LLC_SNAP_HEADER_LEN
                || slot.bytes[..RFC1042_LLC_SNAP_PREFIX.len()] != RFC1042_LLC_SNAP_PREFIX
            {
                slot.clear();
                return Err(OpenDataFragmentError::InvalidLlcSnap);
            }
            let value = complete(OpenReassembledData {
                identity,
                msdu: &slot.bytes[..length],
            });
            slot.completed = true;
            slot.final_fragment = fragment.fragment_number;
            slot.started_at_micros = now_micros;
            return Ok(OpenDataDefragmentation::Complete { expired, value });
        }

        let same_sequence = self.slots.iter().position(|slot| {
            slot.is_active()
                && slot
                    .identity
                    .is_some_and(|active| active.same_sequence_space(identity))
        });
        let (index, evicted) = if let Some(index) = exact.or(same_sequence) {
            let evicted = self.slots[index].identity;
            self.slots[index].clear();
            (index, evicted)
        } else if let Some(index) = self.slots.iter().position(|slot| slot.identity.is_none()) {
            (index, None)
        } else if let Some(index) = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.completed)
            .min_by_key(|(index, slot)| (slot.started_at_micros, *index))
            .map(|(index, _)| index)
        {
            self.slots[index].clear();
            (index, None)
        } else {
            let index = self
                .slots
                .iter()
                .enumerate()
                .min_by_key(|(index, slot)| (slot.started_at_micros, *index))
                .map(|(index, _)| index)
                .expect("nonzero context count has an oldest slot");
            let evicted = self.slots[index].identity;
            self.slots[index].clear();
            (index, evicted)
        };
        for (other_index, slot) in self.slots.iter_mut().enumerate() {
            if other_index != index
                && slot.completed
                && slot
                    .identity
                    .is_some_and(|entry| entry.same_sequence_space(identity))
            {
                slot.clear();
            }
        }
        let slot = &mut self.slots[index];
        slot.identity = Some(identity);
        slot.admission_epoch = admission_epoch;
        slot.expected_fragment = 1;
        slot.started_at_micros = now_micros;
        if let Err(error) = append_fragment(slot, fragment) {
            slot.clear();
            return Err(error);
        }
        Ok(OpenDataDefragmentation::Buffered { expired, evicted })
    }

    fn expire(&mut self, now_micros: u64) -> u8 {
        let mut expired = 0_u8;
        for slot in &mut self.slots {
            if slot.identity.is_some()
                && timestamp_expired(now_micros, slot.started_at_micros, self.timeout_micros)
            {
                expired = expired.saturating_add(u8::from(slot.is_active()));
                slot.clear();
            }
        }
        expired
    }
}

fn append_fragment<const CAPACITY: usize>(
    slot: &mut ReassemblySlot<CAPACITY>,
    fragment: OpenDataFragment<'_>,
) -> Result<(), OpenDataFragmentError> {
    let number = usize::from(fragment.fragment_number);
    if number != 0
        && let (Some(previous), Some(observed)) =
            (slot.packet_numbers[number - 1], fragment.packet_number)
        && observed <= previous
    {
        return Err(OpenDataFragmentError::PacketNumberNotIncreasing { previous, observed });
    }
    let payload = fragment.payload;
    let end = slot
        .length
        .checked_add(payload.len())
        .ok_or(OpenDataFragmentError::ReassembledTooLarge { capacity: CAPACITY })?;
    let Some(destination) = slot.bytes.get_mut(slot.length..end) else {
        return Err(OpenDataFragmentError::ReassembledTooLarge { capacity: CAPACITY });
    };
    destination.copy_from_slice(payload);
    slot.fragment_offsets[number] = slot.length;
    slot.fragment_lengths[number] = payload.len();
    slot.packet_numbers[number] = fragment.packet_number;
    slot.length = end;
    Ok(())
}

const fn timestamp_expired(now: u64, then: u64, timeout: u64) -> bool {
    timeout == 0 || now < then || now - then >= timeout
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    const STA: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const AP: [u8; 6] = [2, 0, 0, 0, 0, 2];
    const SOURCE: [u8; 6] = [2, 0, 0, 0, 0, 3];

    fn fragment(
        role: DataInterfaceRole,
        sequence: u16,
        number: u8,
        more: bool,
        retry: bool,
        payload: &[u8],
    ) -> [u8; 64] {
        let mut mpdu = [0_u8; 64];
        let mut control = DATA
            | match role {
                DataInterfaceRole::Station => FROM_DS,
                DataInterfaceRole::AccessPoint => TO_DS,
            };
        if more {
            control |= MORE_FRAGMENTS;
        }
        if retry {
            control |= RETRY;
        }
        mpdu[..2].copy_from_slice(&control.to_le_bytes());
        match role {
            DataInterfaceRole::Station => {
                mpdu[4..10].copy_from_slice(&STA);
                mpdu[10..16].copy_from_slice(&AP);
                mpdu[16..22].copy_from_slice(&SOURCE);
            }
            DataInterfaceRole::AccessPoint => {
                mpdu[4..10].copy_from_slice(&AP);
                mpdu[10..16].copy_from_slice(&STA);
                mpdu[16..22].copy_from_slice(&SOURCE);
            }
        }
        mpdu[22..24].copy_from_slice(&((sequence << 4) | u16::from(number)).to_le_bytes());
        mpdu[24..24 + payload.len()].copy_from_slice(payload);
        mpdu
    }

    fn parsed<'a>(
        role: DataInterfaceRole,
        frame: &'a [u8; 64],
        payload_len: usize,
    ) -> OpenDataFragment<'a> {
        parse_open_data_fragment(role, &frame[..24 + payload_len]).unwrap()
    }

    #[test]
    fn station_fragments_reassemble_one_exact_ethernet_view() {
        let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];
        let second_payload = [3, 4, 5];
        let first = fragment(
            DataInterfaceRole::Station,
            0x123,
            0,
            true,
            false,
            &first_payload,
        );
        let second = fragment(
            DataInterfaceRole::Station,
            0x123,
            1,
            false,
            false,
            &second_payload,
        );
        let mut state = OpenDataDefragmenter::<2, 32>::new(100);
        assert_eq!(
            state.ingest(
                parsed(DataInterfaceRole::Station, &first, first_payload.len()),
                1,
                |_| ()
            ),
            Ok(OpenDataDefragmentation::Buffered {
                expired: 0,
                evicted: None,
            })
        );
        let outcome = state
            .ingest(
                parsed(DataInterfaceRole::Station, &second, second_payload.len()),
                2,
                |data| {
                    let frame = data.ethernet_frame();
                    (
                        frame.destination,
                        frame.source,
                        frame.ether_type,
                        frame.payload.to_vec(),
                    )
                },
            )
            .unwrap();
        assert_eq!(
            outcome,
            OpenDataDefragmentation::Complete {
                expired: 0,
                value: (STA, SOURCE, 0x0800, std::vec![1, 2, 3, 4, 5]),
            }
        );
        assert_eq!(state.active_contexts(), 0);
    }

    #[test]
    fn changed_address_cannot_splice_an_active_sequence() {
        let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let last_payload = [2];
        let first = fragment(
            DataInterfaceRole::Station,
            7,
            0,
            true,
            false,
            &first_payload,
        );
        let mut changed = fragment(
            DataInterfaceRole::Station,
            7,
            1,
            false,
            false,
            &last_payload,
        );
        changed[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
        let last = fragment(
            DataInterfaceRole::Station,
            7,
            1,
            false,
            false,
            &last_payload,
        );
        let mut state = OpenDataDefragmenter::<1, 16>::new(100);
        state
            .ingest(
                parsed(DataInterfaceRole::Station, &first, first_payload.len()),
                1,
                |_| (),
            )
            .unwrap();
        assert_eq!(
            state.ingest(parsed(DataInterfaceRole::Station, &changed, 1), 2, |_| ()),
            Err(OpenDataFragmentError::IdentityMismatch)
        );
        assert!(matches!(
            state.ingest(parsed(DataInterfaceRole::Station, &last, 1), 3, |_| ()),
            Ok(OpenDataDefragmentation::Complete { .. })
        ));
    }

    #[test]
    fn retry_out_of_order_timeout_and_oldest_eviction_are_bounded() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let first = fragment(DataInterfaceRole::Station, 1, 0, true, false, &payload);
        let retry = fragment(DataInterfaceRole::Station, 1, 0, true, true, &payload);
        let third = fragment(DataInterfaceRole::Station, 1, 2, false, false, &[2]);
        let other = fragment(DataInterfaceRole::Station, 2, 0, true, false, &payload);
        let newest = fragment(DataInterfaceRole::Station, 3, 0, true, false, &payload);
        let mut state = OpenDataDefragmenter::<2, 32>::new(10);
        state
            .ingest(
                parsed(DataInterfaceRole::Station, &first, payload.len()),
                1,
                |_| (),
            )
            .unwrap();
        assert_eq!(
            state.ingest(
                parsed(DataInterfaceRole::Station, &retry, payload.len()),
                2,
                |_| ()
            ),
            Ok(OpenDataDefragmentation::Duplicate { expired: 0 })
        );
        assert_eq!(
            state.ingest(parsed(DataInterfaceRole::Station, &third, 1), 3, |_| ()),
            Err(OpenDataFragmentError::OutOfOrder {
                expected: 1,
                observed: 2,
            })
        );
        state
            .ingest(
                parsed(DataInterfaceRole::Station, &other, payload.len()),
                4,
                |_| (),
            )
            .unwrap();
        let outcome = state
            .ingest(
                parsed(DataInterfaceRole::Station, &newest, payload.len()),
                5,
                |_| (),
            )
            .unwrap();
        assert!(matches!(
            outcome,
            OpenDataDefragmentation::Buffered {
                evicted: Some(identity),
                ..
            } if identity.sequence_number() == 1
        ));
        let expired = fragment(DataInterfaceRole::Station, 4, 0, true, false, &payload);
        assert!(matches!(
            state.ingest(
                parsed(DataInterfaceRole::Station, &expired, payload.len()),
                20,
                |_| ()
            ),
            Ok(OpenDataDefragmentation::Buffered { expired: 2, .. })
        ));
    }

    #[test]
    fn protected_amsdu_overflow_and_lifecycle_edges_fail_closed() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let mut protected = fragment(DataInterfaceRole::Station, 1, 0, true, false, &payload);
        protected[1] |= 0x40;
        assert_eq!(
            parse_open_data_fragment(DataInterfaceRole::Station, &protected[..33]),
            Err(OpenDataFragmentError::Protected)
        );

        let mut qos = fragment(DataInterfaceRole::Station, 1, 0, true, false, &payload);
        qos[0] = QOS_DATA as u8;
        qos[24] = QOS_AMSDU_PRESENT;
        assert_eq!(
            parse_open_data_fragment(DataInterfaceRole::Station, &qos[..35]),
            Err(OpenDataFragmentError::AmsduUnsupported)
        );

        let mut group = fragment(DataInterfaceRole::Station, 2, 0, true, false, &payload);
        group[4..10].fill(0xff);
        assert_eq!(
            parse_open_data_fragment(DataInterfaceRole::Station, &group[..33]),
            Err(OpenDataFragmentError::InvalidReceiver)
        );
        assert!(
            parse_open_data_identity(DataInterfaceRole::Station, &group[..33]).is_ok(),
            "ordinary group-address identity remains valid"
        );

        let mut group_destination =
            fragment(DataInterfaceRole::AccessPoint, 2, 0, true, false, &payload);
        group_destination[16..22].fill(0xff);
        assert_eq!(
            parse_open_data_fragment(DataInterfaceRole::AccessPoint, &group_destination[..33]),
            Err(OpenDataFragmentError::InvalidDestination)
        );
        assert!(
            parse_open_data_identity(DataInterfaceRole::AccessPoint, &group_destination[..33])
                .is_ok(),
            "ordinary To-DS group destination remains valid"
        );

        let first = fragment(DataInterfaceRole::AccessPoint, 3, 0, true, false, &payload);
        let second = fragment(DataInterfaceRole::AccessPoint, 3, 1, false, false, &[2, 3]);
        let mut state = OpenDataDefragmenter::<1, 10>::new(100);
        state
            .ingest(
                parsed(DataInterfaceRole::AccessPoint, &first, payload.len()),
                1,
                |_| (),
            )
            .unwrap();
        assert_eq!(
            state.ingest(
                parsed(DataInterfaceRole::AccessPoint, &second, 2),
                2,
                |_| ()
            ),
            Err(OpenDataFragmentError::ReassembledTooLarge { capacity: 10 })
        );
        assert_eq!(state.active_contexts(), 0);

        state
            .ingest(
                parsed(DataInterfaceRole::AccessPoint, &first, payload.len()),
                3,
                |_| (),
            )
            .unwrap();
        assert_eq!(state.forget_transmitter(STA), 1);
        assert_eq!(state.clear(), 0);
    }

    #[test]
    fn fresh_ordinary_wrap_replaces_stale_fragment_completion_identity() {
        let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let first = fragment(
            DataInterfaceRole::Station,
            7,
            0,
            true,
            false,
            &first_payload,
        );
        let final_fragment = fragment(DataInterfaceRole::Station, 7, 1, false, false, &[2]);
        let mut state = OpenDataDefragmenter::<1, 32>::new(100);
        state
            .ingest(
                parsed(DataInterfaceRole::Station, &first, first_payload.len()),
                1,
                |_| (),
            )
            .unwrap();
        assert!(matches!(
            state.ingest(
                parsed(DataInterfaceRole::Station, &final_fragment, 1),
                2,
                |_| (),
            ),
            Ok(OpenDataDefragmentation::Complete { .. })
        ));

        // A 12-bit sequence number can wrap while the bounded completion
        // fingerprint is still live. A fresh ordinary MPDU with a different
        // third address owns the reused sequence; its retry is left to the
        // ordinary duplicate filter instead of colliding with stale fragment
        // identity here.
        let mut ordinary = fragment(DataInterfaceRole::Station, 7, 0, false, false, &[9]);
        ordinary[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
        let ordinary_identity =
            parse_open_data_identity(DataInterfaceRole::Station, &ordinary[..25]).unwrap();
        assert_eq!(
            state.admit_unfragmented(ordinary_identity, false, Some(3)),
            Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
        );
        assert_eq!(
            state.admit_unfragmented(ordinary_identity, true, Some(4)),
            Ok(OpenDataUnfragmentedAdmission::Admitted { expired: 0 })
        );
    }

    #[test]
    fn clock_wrap_expires_retained_bytes_before_sequence_reuse() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let first = fragment(DataInterfaceRole::Station, 9, 0, true, false, &payload);
        let final_fragment = fragment(DataInterfaceRole::Station, 9, 1, false, false, &[2]);
        let mut state = OpenDataDefragmenter::<1, 32>::new(100);
        state
            .ingest(
                parsed(DataInterfaceRole::Station, &first, payload.len()),
                u64::MAX - 1,
                |_| (),
            )
            .unwrap();

        assert_eq!(
            state.ingest(
                parsed(DataInterfaceRole::Station, &final_fragment, 1),
                1,
                |_| (),
            ),
            Err(OpenDataFragmentError::Orphan { fragment_number: 1 })
        );
        assert_eq!(state.active_contexts(), 0);
    }
}
