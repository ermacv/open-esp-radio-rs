//! Checked Ethernet-II storage at the network queue boundary.

use oer_ieee80211_mac::data::EthernetFrameParts;

/// Why a complete Ethernet frame cannot cross the network boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EthernetFrameError {
    HeaderTooShort { available: usize },
    PayloadTooLong { available: usize },
    OutputTooShort { available: usize, required: usize },
}

/// One complete Ethernet-II header and a bounded, owned payload.
///
/// No independent length can disagree with the storage. Construction rejects
/// a truncated header or a payload exceeding 1500 bytes; it never clips bytes.
#[derive(Debug)]
pub struct EthernetFrame {
    header: [u8; 14],
    payload: heapless::Vec<u8, 1500>,
}

impl EthernetFrame {
    pub fn new(bytes: &[u8]) -> Result<Self, EthernetFrameError> {
        let (header, payload) =
            bytes
                .split_first_chunk::<14>()
                .ok_or(EthernetFrameError::HeaderTooShort {
                    available: bytes.len(),
                })?;
        let payload =
            heapless::Vec::from_slice(payload).map_err(|_| EthernetFrameError::PayloadTooLong {
                available: payload.len(),
            })?;
        Ok(Self {
            header: *header,
            payload,
        })
    }

    pub fn from_parts(parts: EthernetFrameParts<'_>) -> Result<Self, EthernetFrameError> {
        let payload = heapless::Vec::from_slice(parts.payload).map_err(|_| {
            EthernetFrameError::PayloadTooLong {
                available: parts.payload.len(),
            }
        })?;
        let [d0, d1, d2, d3, d4, d5] = parts.destination;
        let [s0, s1, s2, s3, s4, s5] = parts.source;
        let [e0, e1] = parts.ether_type.to_be_bytes();
        Ok(Self {
            header: [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, e0, e1],
            payload,
        })
    }

    pub fn parts(&self) -> EthernetFrameParts<'_> {
        let [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, e0, e1] = self.header;
        EthernetFrameParts {
            destination: [d0, d1, d2, d3, d4, d5],
            source: [s0, s1, s2, s3, s4, s5],
            ether_type: u16::from_be_bytes([e0, e1]),
            payload: &self.payload,
        }
    }

    pub fn bytes(&self) -> impl Iterator<Item = u8> + '_ {
        self.header.iter().chain(self.payload.iter()).copied()
    }

    pub fn length(&self) -> usize {
        14_usize.saturating_add(self.payload.len())
    }

    /// Validate the whole destination before writing any bytes.
    pub fn copy_to(&self, output: &mut [u8]) -> Result<usize, EthernetFrameError> {
        let available = output.len();
        let required = self.length();
        let (header, rest) =
            output
                .split_first_chunk_mut::<14>()
                .ok_or(EthernetFrameError::OutputTooShort {
                    available,
                    required,
                })?;
        let (payload, _) = rest.split_at_mut_checked(self.payload.len()).ok_or(
            EthernetFrameError::OutputTooShort {
                available,
                required,
            },
        )?;
        *header = self.header;
        for (to, from) in payload.iter_mut().zip(&self.payload) {
            *to = *from;
        }
        Ok(required)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_maximum_payload_are_proved_once_without_truncation() {
        assert!(matches!(
            EthernetFrame::new(&[0; 13]),
            Err(EthernetFrameError::HeaderTooShort { .. })
        ));
        let bytes = [0xab; 1514];
        let frame = EthernetFrame::new(&bytes).unwrap();
        assert_eq!(frame.parts().payload.len(), 1500);
        assert_eq!(frame.bytes().collect::<std::vec::Vec<_>>(), bytes);
        assert!(matches!(
            EthernetFrame::new(&[0; 1515]),
            Err(EthernetFrameError::PayloadTooLong { .. })
        ));
        let mut short = [0x55; 1513];
        assert!(matches!(
            frame.copy_to(&mut short),
            Err(EthernetFrameError::OutputTooShort { .. })
        ));
        assert_eq!(short, [0x55; 1513]);
        let mut output = [0; 1514];
        assert_eq!(frame.copy_to(&mut output), Ok(1514));
        assert_eq!(output, bytes);
    }
}
