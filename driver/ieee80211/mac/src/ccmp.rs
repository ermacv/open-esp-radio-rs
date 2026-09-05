//! Hardware-independent CCMP header and receive replay ownership.
//!
//! Packet-number allocation, key-slot mapping and hardware publication belong
//! to the concrete MAC backend. This module validates and encodes the public
//! eight-byte header and owns a two-phase receive replay frontier. A caller
//! must prepare replay only after its per-TID BlockAck reorder release, then
//! commit only after cryptographic authentication and peer admission.

pub const CCMP_HEADER_LEN: usize = 8;
pub const CCMP_PACKET_NUMBER_MAX: u64 = (1_u64 << 48) - 1;
pub const CCMP_REPLAY_LANE_COUNT: usize = 17;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CcmpPacketNumber(u64);

impl CcmpPacketNumber {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Option<Self> {
        if value <= CCMP_PACKET_NUMBER_MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn from_header_bytes(bytes: [u8; CCMP_HEADER_LEN]) -> Self {
        Self(
            (bytes[0] as u64)
                | ((bytes[1] as u64) << 8)
                | ((bytes[4] as u64) << 16)
                | ((bytes[5] as u64) << 24)
                | ((bytes[6] as u64) << 32)
                | ((bytes[7] as u64) << 40),
        )
    }

    /// Decode the little-endian six-octet packet number carried by an EAPOL
    /// Key RSC. The two unused high octets must be zero; accepting them would
    /// silently truncate an install-time replay frontier.
    pub const fn from_receive_sequence(
        sequence: [u8; 8],
    ) -> Result<Self, CcmpReceiveSequenceError> {
        if sequence[6] != 0 || sequence[7] != 0 {
            return Err(CcmpReceiveSequenceError::HighBitsSet);
        }
        Ok(Self(
            (sequence[0] as u64)
                | ((sequence[1] as u64) << 8)
                | ((sequence[2] as u64) << 16)
                | ((sequence[3] as u64) << 24)
                | ((sequence[4] as u64) << 32)
                | ((sequence[5] as u64) << 40),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpReceiveSequenceError {
    HighBitsSet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcmpKeyId(u8);

impl CcmpKeyId {
    pub const PAIRWISE: Self = Self(0);

    pub const fn new(value: u8) -> Option<Self> {
        if value <= 3 { Some(Self(value)) } else { None }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    pub const fn header_bits(self) -> u8 {
        self.0 << 6
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcmpHeader {
    packet_number: CcmpPacketNumber,
    key_id: CcmpKeyId,
}

impl CcmpHeader {
    pub const fn new(packet_number: CcmpPacketNumber, key_id: CcmpKeyId) -> Self {
        Self {
            packet_number,
            key_id,
        }
    }

    pub const fn parse(bytes: [u8; CCMP_HEADER_LEN]) -> Result<Self, CcmpHeaderError> {
        if bytes[2] != 0 {
            return Err(CcmpHeaderError::ReservedOctet);
        }
        if bytes[3] & 0x20 == 0 {
            return Err(CcmpHeaderError::ExtIvMissing);
        }
        if bytes[3] & 0x1f != 0 {
            return Err(CcmpHeaderError::ReservedKeyBits);
        }
        Ok(Self {
            packet_number: CcmpPacketNumber::from_header_bytes(bytes),
            key_id: CcmpKeyId((bytes[3] >> 6) & 0x03),
        })
    }

    pub const fn encode(self) -> [u8; CCMP_HEADER_LEN] {
        let packet_number = self.packet_number.value();
        [
            packet_number as u8,
            (packet_number >> 8) as u8,
            0,
            self.key_id.header_bits() | 0x20,
            (packet_number >> 16) as u8,
            (packet_number >> 24) as u8,
            (packet_number >> 32) as u8,
            (packet_number >> 40) as u8,
        ]
    }

    pub const fn packet_number(self) -> CcmpPacketNumber {
        self.packet_number
    }

    pub const fn key_id(self) -> CcmpKeyId {
        self.key_id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpHeaderError {
    ReservedOctet,
    ExtIvMissing,
    ReservedKeyBits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpReplayLane {
    NonQos,
    Tid(u8),
}

impl CcmpReplayLane {
    const fn index(self) -> Option<usize> {
        match self {
            Self::NonQos => Some(0),
            Self::Tid(tid) if tid < 16 => Some(tid as usize + 1),
            Self::Tid(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcmpRxReplayCandidate {
    lane: CcmpReplayLane,
    packet_number: CcmpPacketNumber,
    expected_revision: u32,
}

impl CcmpRxReplayCandidate {
    pub const fn lane(self) -> CcmpReplayLane {
        self.lane
    }

    pub const fn packet_number(self) -> CcmpPacketNumber {
        self.packet_number
    }
}

/// Per-key CCMP receive frontiers for non-QoS and TID 0 through 15.
///
/// This is deliberately not a sliding arrival-order window. The 802.11
/// sequence-space owner must first release frames in order within each TID;
/// only then is a strictly increasing CCMP PN meaningful. Separate lanes keep
/// unrelated access categories from rejecting each other's valid PNs.
#[derive(Debug, Eq, PartialEq)]
pub struct CcmpRxReplayState {
    highest: [CcmpPacketNumber; CCMP_REPLAY_LANE_COUNT],
    revisions: [u32; CCMP_REPLAY_LANE_COUNT],
}

impl CcmpRxReplayState {
    pub const fn new(initial: CcmpPacketNumber) -> Self {
        Self {
            highest: [initial; CCMP_REPLAY_LANE_COUNT],
            revisions: [0; CCMP_REPLAY_LANE_COUNT],
        }
    }

    pub const fn from_receive_sequence(
        sequence: [u8; 8],
    ) -> Result<Self, CcmpReceiveSequenceError> {
        match CcmpPacketNumber::from_receive_sequence(sequence) {
            Ok(initial) => Ok(Self::new(initial)),
            Err(error) => Err(error),
        }
    }

    pub fn prepare(
        &self,
        lane: CcmpReplayLane,
        packet_number: CcmpPacketNumber,
    ) -> Result<CcmpRxReplayCandidate, CcmpReplayError> {
        let index = lane.index().ok_or(CcmpReplayError::InvalidTid)?;
        let highest = self.highest[index];
        if packet_number <= highest {
            return Err(CcmpReplayError::Replayed {
                packet_number,
                highest,
            });
        }
        Ok(CcmpRxReplayCandidate {
            lane,
            packet_number,
            expected_revision: self.revisions[index],
        })
    }

    pub fn commit(&mut self, candidate: CcmpRxReplayCandidate) -> Result<(), CcmpReplayError> {
        let index = candidate.lane.index().ok_or(CcmpReplayError::InvalidTid)?;
        if self.revisions[index] != candidate.expected_revision
            || candidate.packet_number <= self.highest[index]
        {
            return Err(CcmpReplayError::StaleCandidate);
        }
        let revision = self.revisions[index]
            .checked_add(1)
            .ok_or(CcmpReplayError::RevisionExhausted)?;
        self.highest[index] = candidate.packet_number;
        self.revisions[index] = revision;
        Ok(())
    }

    /// Check and advance one lane while the caller holds its unique replay
    /// owner across both operations.
    ///
    /// Split prepare/commit remains required when authentication, reassembly
    /// or publication can interleave. An already authenticated ordinary MPDU
    /// has no such edge, so it need not construct and then revalidate a
    /// revision-bound candidate.
    #[inline(always)]
    pub fn commit_immediate(
        &mut self,
        lane: CcmpReplayLane,
        packet_number: CcmpPacketNumber,
    ) -> Result<(), CcmpReplayError> {
        let index = lane.index().ok_or(CcmpReplayError::InvalidTid)?;
        let highest = self.highest[index];
        if packet_number <= highest {
            return Err(CcmpReplayError::Replayed {
                packet_number,
                highest,
            });
        }
        let revision = self.revisions[index]
            .checked_add(1)
            .ok_or(CcmpReplayError::RevisionExhausted)?;
        self.highest[index] = packet_number;
        self.revisions[index] = revision;
        Ok(())
    }

    pub const fn highest(&self, lane: CcmpReplayLane) -> Option<CcmpPacketNumber> {
        match lane.index() {
            Some(index) => Some(self.highest[index]),
            None => None,
        }
    }
}

impl Default for CcmpRxReplayState {
    fn default() -> Self {
        Self::new(CcmpPacketNumber::ZERO)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpReplayError {
    InvalidTid,
    Replayed {
        packet_number: CcmpPacketNumber,
        highest: CcmpPacketNumber,
    },
    StaleCandidate,
    RevisionExhausted,
}

pub const fn ccmp_header(low: u32, high: u32, key_id_bits: u8) -> [u8; CCMP_HEADER_LEN] {
    [
        low as u8,
        (low >> 8) as u8,
        0,
        key_id_bits | 0x20,
        (low >> 16) as u8,
        (low >> 24) as u8,
        high as u8,
        (high >> 8) as u8,
    ]
}

#[cfg(test)]
mod tests;
