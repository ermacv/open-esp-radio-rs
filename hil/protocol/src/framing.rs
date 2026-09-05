use core::fmt;

use crc::{CRC_32_ISCSI, Crc};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{Envelope, EvidenceRecord, PROTOCOL_VERSION, WireBody};

pub const MAX_POSTCARD_BYTES: usize = 480;
pub const WIRE_MAGIC: [u8; 4] = *b"ORHL";
pub const FRAMING_VERSION: u8 = 1;
pub const WIRE_HEADER_BYTES: usize = 34;
const CHECKSUM_BYTES: usize = size_of::<u32>();
const MAX_RAW_FRAME_BYTES: usize = WIRE_HEADER_BYTES + MAX_POSTCARD_BYTES + CHECKSUM_BYTES;
const MAX_COBS_FRAME_BYTES: usize = cobs::max_encoding_length(MAX_RAW_FRAME_BYTES);
pub const MAX_WIRE_FRAME_BYTES: usize = 2 + MAX_COBS_FRAME_BYTES + 1;

const CRC32C: Crc<u32> = Crc::<u32>::new(&CRC_32_ISCSI);

/// Computes the digest carried by [`crate::Finished`] for an ordered evidence
/// set.
///
/// The digest covers the canonical postcard representation, including the
/// slice length. Both the target and host use this helper so missing, reordered
/// or mismatched evidence cannot satisfy a `Finished` event. Session identity
/// is checked separately from the surrounding [`crate::Envelope`].
pub fn evidence_crc32c(evidence: &[EvidenceRecord]) -> Result<u32, EncodeError> {
    let mut encoded = [0_u8; MAX_POSTCARD_BYTES];
    let payload = postcard::to_slice(evidence, &mut encoded).map_err(|_| EncodeError::Serialize)?;
    Ok(CRC32C.checksum(payload))
}

/// Computes the transfer-integrity digest of one opaque startup artifact.
///
/// This checksum is not an identity or qualification hash. It only prevents
/// a receiver from accepting an incomplete or internally inconsistent UART
/// transfer.
pub fn startup_artifact_crc32c(bytes: &[u8]) -> u32 {
    CRC32C.checksum(bytes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    Serialize,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize => {
                formatter.write_str("HIL message exceeds the wire frame or cannot be serialized")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Cobs,
    TooShort,
    Magic,
    FramingVersion,
    MessageKind,
    ProtocolVersion,
    PayloadLength,
    Checksum,
    Deserialize,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cobs => formatter.write_str("invalid COBS frame"),
            Self::TooShort => {
                formatter.write_str("HIL frame does not contain a complete header and checksum")
            }
            Self::Magic => formatter.write_str("invalid HIL wire magic"),
            Self::FramingVersion => formatter.write_str("unsupported HIL framing version"),
            Self::MessageKind => formatter.write_str("HIL frame has the wrong message direction"),
            Self::ProtocolVersion => formatter.write_str("unsupported HIL protocol version"),
            Self::PayloadLength => {
                formatter.write_str("HIL frame payload length does not match its header")
            }
            Self::Checksum => formatter.write_str("HIL frame checksum mismatch"),
            Self::Deserialize => formatter.write_str("invalid HIL postcard payload"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecodeCounters {
    pub frames: u32,
    pub cobs_errors: u32,
    pub too_short: u32,
    pub header_errors: u32,
    pub framing_version_errors: u32,
    pub message_kind_errors: u32,
    pub protocol_version_errors: u32,
    pub payload_length_errors: u32,
    pub checksum_errors: u32,
    pub deserialize_errors: u32,
    pub overflows: u32,
}

pub struct FrameEncoder {
    raw: [u8; MAX_RAW_FRAME_BYTES],
    wire: [u8; MAX_WIRE_FRAME_BYTES],
}

impl Default for FrameEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameEncoder {
    pub const fn new() -> Self {
        Self {
            raw: [0; MAX_RAW_FRAME_BYTES],
            wire: [0; MAX_WIRE_FRAME_BYTES],
        }
    }

    /// Encodes one independently resynchronizable frame.
    ///
    /// A leading delimiter discards any preceding ROM or text output. The
    /// trailing delimiter terminates this frame for an incremental decoder.
    pub fn encode<T>(&mut self, message: &Envelope<T>) -> Result<&[u8], EncodeError>
    where
        T: Serialize + WireBody,
    {
        let payload_length = postcard::to_slice(
            &message.body,
            &mut self.raw[WIRE_HEADER_BYTES..WIRE_HEADER_BYTES + MAX_POSTCARD_BYTES],
        )
        .map_err(|_| EncodeError::Serialize)?
        .len();
        self.raw[..4].copy_from_slice(&WIRE_MAGIC);
        self.raw[4] = FRAMING_VERSION;
        self.raw[5] = T::WIRE_KIND as u8;
        self.raw[6..8].copy_from_slice(&message.protocol_version.to_le_bytes());
        self.raw[8..16].copy_from_slice(&message.boot_id.to_le_bytes());
        self.raw[16..20].copy_from_slice(&message.message_sequence.to_le_bytes());
        self.raw[20..28].copy_from_slice(&message.session_id.to_le_bytes());
        self.raw[28..32].copy_from_slice(&message.request_id.to_le_bytes());
        self.raw[32..34].copy_from_slice(
            &u16::try_from(payload_length)
                .map_err(|_| EncodeError::Serialize)?
                .to_le_bytes(),
        );
        let protected_length = WIRE_HEADER_BYTES + payload_length;
        let checksum = CRC32C.checksum(&self.raw[..protected_length]);
        self.raw[protected_length..protected_length + CHECKSUM_BYTES]
            .copy_from_slice(&checksum.to_le_bytes());

        // Two leading delimiters recover even when arbitrary pre-protocol
        // binary output contained a zero and made the decoder enter a false
        // frame. In the normal case both simply keep it armed for a body.
        self.wire[0] = 0;
        self.wire[1] = 0;
        let encoded_length = cobs::encode(
            &self.raw[..protected_length + CHECKSUM_BYTES],
            &mut self.wire[2..2 + MAX_COBS_FRAME_BYTES],
        );
        self.wire[2 + encoded_length] = 0;
        Ok(&self.wire[..encoded_length + 3])
    }
}

impl Drop for FrameEncoder {
    fn drop(&mut self) {
        self.raw.zeroize();
        self.wire.zeroize();
    }
}

pub struct FrameDecoder {
    encoded: [u8; MAX_COBS_FRAME_BYTES],
    length: usize,
    inside_frame: bool,
    discard_until_delimiter: bool,
    counters: DecodeCounters,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub const fn new() -> Self {
        Self {
            encoded: [0; MAX_COBS_FRAME_BYTES],
            length: 0,
            inside_frame: false,
            discard_until_delimiter: false,
            counters: DecodeCounters {
                frames: 0,
                cobs_errors: 0,
                too_short: 0,
                header_errors: 0,
                framing_version_errors: 0,
                message_kind_errors: 0,
                protocol_version_errors: 0,
                payload_length_errors: 0,
                checksum_errors: 0,
                deserialize_errors: 0,
                overflows: 0,
            },
        }
    }

    pub const fn counters(&self) -> DecodeCounters {
        self.counters
    }

    /// Feeds arbitrary serial chunks and calls `receive` for every complete
    /// frame. Empty delimiters are ignored, so senders may prefix every frame
    /// with a delimiter to recover from unframed boot output.
    pub fn feed<T>(
        &mut self,
        bytes: &[u8],
        mut receive: impl FnMut(Result<Envelope<T>, DecodeError>),
    ) where
        T: for<'de> Deserialize<'de> + WireBody,
    {
        for &byte in bytes {
            if byte == 0 {
                if self.discard_until_delimiter {
                    self.discard_until_delimiter = false;
                    self.inside_frame = false;
                    self.length = 0;
                    continue;
                }
                if !self.inside_frame {
                    self.inside_frame = true;
                    self.length = 0;
                    continue;
                }
                if self.length == 0 {
                    // Repeated leading delimiters deliberately keep the
                    // decoder armed for a body.
                    continue;
                }
                let result = self.decode();
                self.encoded[..self.length].zeroize();
                self.inside_frame = false;
                self.length = 0;
                receive(result);
                continue;
            }

            if self.discard_until_delimiter || !self.inside_frame {
                continue;
            }
            if self.length == self.encoded.len() {
                self.counters.overflows = self.counters.overflows.saturating_add(1);
                self.encoded[..self.length].zeroize();
                self.discard_until_delimiter = true;
                self.length = 0;
                continue;
            }
            self.encoded[self.length] = byte;
            self.length += 1;
        }
    }

    fn decode<T>(&mut self) -> Result<Envelope<T>, DecodeError>
    where
        T: for<'de> Deserialize<'de> + WireBody,
    {
        let decoded_length =
            cobs::decode_in_place(&mut self.encoded[..self.length]).map_err(|_| {
                self.counters.cobs_errors = self.counters.cobs_errors.saturating_add(1);
                DecodeError::Cobs
            })?;
        if decoded_length < WIRE_HEADER_BYTES + CHECKSUM_BYTES {
            self.counters.too_short = self.counters.too_short.saturating_add(1);
            return Err(DecodeError::TooShort);
        }
        if self.encoded[..4] != WIRE_MAGIC {
            self.counters.header_errors = self.counters.header_errors.saturating_add(1);
            return Err(DecodeError::Magic);
        }
        if self.encoded[4] != FRAMING_VERSION {
            self.counters.framing_version_errors =
                self.counters.framing_version_errors.saturating_add(1);
            return Err(DecodeError::FramingVersion);
        }
        if self.encoded[5] != T::WIRE_KIND as u8 {
            self.counters.message_kind_errors = self.counters.message_kind_errors.saturating_add(1);
            return Err(DecodeError::MessageKind);
        }
        let protocol_version = u16::from_le_bytes([self.encoded[6], self.encoded[7]]);
        if protocol_version != PROTOCOL_VERSION {
            self.counters.protocol_version_errors =
                self.counters.protocol_version_errors.saturating_add(1);
            return Err(DecodeError::ProtocolVersion);
        }
        let payload_length = usize::from(u16::from_le_bytes([self.encoded[32], self.encoded[33]]));
        let protected_length = WIRE_HEADER_BYTES + payload_length;
        if protected_length + CHECKSUM_BYTES != decoded_length {
            self.counters.payload_length_errors =
                self.counters.payload_length_errors.saturating_add(1);
            return Err(DecodeError::PayloadLength);
        }
        let expected = u32::from_le_bytes(
            self.encoded[protected_length..decoded_length]
                .try_into()
                .expect("checksum length is fixed"),
        );
        if CRC32C.checksum(&self.encoded[..protected_length]) != expected {
            self.counters.checksum_errors = self.counters.checksum_errors.saturating_add(1);
            return Err(DecodeError::Checksum);
        }
        let body = postcard::from_bytes(&self.encoded[WIRE_HEADER_BYTES..protected_length])
            .map_err(|_| {
                self.counters.deserialize_errors =
                    self.counters.deserialize_errors.saturating_add(1);
                DecodeError::Deserialize
            })?;
        self.counters.frames = self.counters.frames.saturating_add(1);
        Ok(Envelope {
            protocol_version,
            boot_id: u64::from_le_bytes(
                self.encoded[8..16]
                    .try_into()
                    .expect("header range is fixed"),
            ),
            message_sequence: u32::from_le_bytes(
                self.encoded[16..20]
                    .try_into()
                    .expect("header range is fixed"),
            ),
            session_id: u64::from_le_bytes(
                self.encoded[20..28]
                    .try_into()
                    .expect("header range is fixed"),
            ),
            request_id: u32::from_le_bytes(
                self.encoded[28..32]
                    .try_into()
                    .expect("header range is fixed"),
            ),
            body,
        })
    }
}

impl Drop for FrameDecoder {
    fn drop(&mut self) {
        self.encoded.zeroize();
    }
}

#[cfg(test)]
mod tests;
