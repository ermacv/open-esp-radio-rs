//! Allocation-free IEEE 802.11 management-frame construction.

pub const MANAGEMENT_HEADER_LEN: usize = 24;
pub const MAX_SSID_LEN: usize = 32;
pub const MAX_SUPPORTED_RATES_LEN: usize = 24;

const PROBE_REQUEST_FRAME_CONTROL: u16 = 0x0040;
pub const BROADCAST_ADDRESS: [u8; 6] = [0xff; 6];
const SSID_ELEMENT_ID: u8 = 0;
const SUPPORTED_RATES_ELEMENT_ID: u8 = 1;
const EXTENDED_SUPPORTED_RATES_ELEMENT_ID: u8 = 50;
const SUPPORTED_RATES_ELEMENT_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeRequestError {
    SsidTooLong,
    NoSupportedRates,
    TooManySupportedRates,
    SequenceNumberOutOfRange,
    OutputTooSmall { required: usize },
}

/// A borrowed Probe Request description.
///
/// The lifetime ties `ssid` and `supported_rates` to their caller-owned
/// storage. Encoding copies the finished frame into caller-owned DMA-capable
/// memory, so this type needs neither allocation nor global buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeRequest<'a> {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub bssid: [u8; 6],
    pub sequence_number: u16,
    pub ssid: &'a [u8],
    pub supported_rates: &'a [u8],
}

impl ProbeRequest<'_> {
    /// Encode a wildcard or directed Probe Request into `output`.
    ///
    /// `sequence_number` is the 12-bit 802.11 sequence number; the fragment
    /// number is emitted as zero. Rate bytes use the standard 500 kbit/s
    /// representation, including any basic-rate bit supplied by the caller.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ProbeRequestError> {
        if self.ssid.len() > MAX_SSID_LEN {
            return Err(ProbeRequestError::SsidTooLong);
        }
        if self.supported_rates.is_empty() {
            return Err(ProbeRequestError::NoSupportedRates);
        }
        if self.supported_rates.len() > MAX_SUPPORTED_RATES_LEN {
            return Err(ProbeRequestError::TooManySupportedRates);
        }
        if self.sequence_number > 0x0fff {
            return Err(ProbeRequestError::SequenceNumberOutOfRange);
        }

        let first_rates_len = self
            .supported_rates
            .len()
            .min(SUPPORTED_RATES_ELEMENT_CAPACITY);
        let extended_rates_len = self.supported_rates.len() - first_rates_len;
        let required = MANAGEMENT_HEADER_LEN
            + 2
            + self.ssid.len()
            + 2
            + first_rates_len
            + usize::from(extended_rates_len != 0) * (2 + extended_rates_len);
        if output.len() < required {
            return Err(ProbeRequestError::OutputTooSmall { required });
        }

        let frame = &mut output[..required];
        frame.fill(0);
        frame[0..2].copy_from_slice(&PROBE_REQUEST_FRAME_CONTROL.to_le_bytes());
        frame[4..10].copy_from_slice(&self.destination);
        frame[10..16].copy_from_slice(&self.source);
        frame[16..22].copy_from_slice(&self.bssid);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());

        let mut offset = MANAGEMENT_HEADER_LEN;
        frame[offset] = SSID_ELEMENT_ID;
        frame[offset + 1] = self.ssid.len() as u8;
        offset += 2;
        frame[offset..offset + self.ssid.len()].copy_from_slice(self.ssid);
        offset += self.ssid.len();

        frame[offset] = SUPPORTED_RATES_ELEMENT_ID;
        frame[offset + 1] = first_rates_len as u8;
        offset += 2;
        frame[offset..offset + first_rates_len]
            .copy_from_slice(&self.supported_rates[..first_rates_len]);
        offset += first_rates_len;

        if extended_rates_len != 0 {
            frame[offset] = EXTENDED_SUPPORTED_RATES_ELEMENT_ID;
            frame[offset + 1] = extended_rates_len as u8;
            offset += 2;
            frame[offset..offset + extended_rates_len]
                .copy_from_slice(&self.supported_rates[first_rates_len..]);
            offset += extended_rates_len;
        }

        debug_assert_eq!(offset, required);
        Ok(required)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: [u8; 6] = [0x02, 0x00, 0x00, 0x12, 0x34, 0x56];

    #[test]
    fn encodes_wildcard_probe_request() {
        let mut output = [0xa5; 64];
        let length = ProbeRequest {
            destination: BROADCAST_ADDRESS,
            source: SOURCE,
            bssid: BROADCAST_ADDRESS,
            sequence_number: 0x123,
            ssid: b"",
            supported_rates: &[0x82, 0x84, 0x8b, 0x96],
        }
        .encode(&mut output)
        .unwrap();

        assert_eq!(length, 32);
        assert_eq!(&output[0..2], &[0x40, 0x00]);
        assert_eq!(&output[2..4], &[0, 0]);
        assert_eq!(&output[4..10], &[0xff; 6]);
        assert_eq!(&output[10..16], &SOURCE);
        assert_eq!(&output[16..22], &[0xff; 6]);
        assert_eq!(&output[22..24], &[0x30, 0x12]);
        assert_eq!(&output[24..32], &[0, 0, 1, 4, 0x82, 0x84, 0x8b, 0x96]);
        assert_eq!(output[32], 0xa5);
    }

    #[test]
    fn encodes_directed_request_and_extended_rates() {
        let rates = [
            0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
        ];
        let mut output = [0; 64];
        let length = ProbeRequest {
            destination: BROADCAST_ADDRESS,
            source: SOURCE,
            bssid: BROADCAST_ADDRESS,
            sequence_number: 0,
            ssid: b"open",
            supported_rates: &rates,
        }
        .encode(&mut output)
        .unwrap();

        assert_eq!(length, 46);
        assert_eq!(&output[24..30], &[0, 4, b'o', b'p', b'e', b'n']);
        assert_eq!(
            &output[30..40],
            &[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]
        );
        assert_eq!(&output[40..46], &[50, 4, 0x30, 0x48, 0x60, 0x6c]);
    }

    #[test]
    fn rejects_invalid_inputs_before_touching_output() {
        let mut output = [0xa5; 31];
        let error = ProbeRequest {
            destination: BROADCAST_ADDRESS,
            source: SOURCE,
            bssid: BROADCAST_ADDRESS,
            sequence_number: 0,
            ssid: b"",
            supported_rates: &[0x82, 0x84, 0x8b, 0x96],
        }
        .encode(&mut output)
        .unwrap_err();
        assert_eq!(error, ProbeRequestError::OutputTooSmall { required: 32 });
        assert_eq!(output, [0xa5; 31]);

        assert_eq!(
            ProbeRequest {
                destination: BROADCAST_ADDRESS,
                source: SOURCE,
                bssid: BROADCAST_ADDRESS,
                sequence_number: 0x1000,
                ssid: b"",
                supported_rates: &[0x82],
            }
            .encode(&mut [0; 64]),
            Err(ProbeRequestError::SequenceNumberOutOfRange)
        );
        assert_eq!(
            ProbeRequest {
                destination: BROADCAST_ADDRESS,
                source: SOURCE,
                bssid: BROADCAST_ADDRESS,
                sequence_number: 0,
                ssid: &[0; MAX_SSID_LEN + 1],
                supported_rates: &[0x82],
            }
            .encode(&mut [0; 64]),
            Err(ProbeRequestError::SsidTooLong)
        );
        assert_eq!(
            ProbeRequest {
                destination: BROADCAST_ADDRESS,
                source: SOURCE,
                bssid: BROADCAST_ADDRESS,
                sequence_number: 0,
                ssid: b"",
                supported_rates: &[],
            }
            .encode(&mut [0; 64]),
            Err(ProbeRequestError::NoSupportedRates)
        );
    }
}
