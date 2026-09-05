//! IEEE 802.15.4 frame-buffer layout.

pub const FRAME_BUFFER_SIZE: usize = 128;
pub const MIN_PHR_LENGTH: u8 = 3;
pub const MAX_PHR_LENGTH: u8 = 127;
pub const MIN_MAC_FRAME_SIZE: usize = MIN_PHR_LENGTH as usize - 2;
pub const MAX_MAC_FRAME_SIZE: usize = MAX_PHR_LENGTH as usize - 2;

const FRAME_TYPE_MASK: u8 = 0x07;
const MAX_SUPPORTED_FRAME_TYPE: u8 = 0x03;
const ACKNOWLEDGEMENT_REQUEST_BIT: u8 = 0x20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxFrameError {
    MacLengthOutOfRange { length: usize },
    UnsupportedFrameType { frame_type: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxFrameError {
    PhrLengthOutOfRange { length: u8 },
}

/// Immutable view of one prepared transmit buffer.
#[derive(Clone, Copy, Debug)]
pub struct TxFrameView<'buffer> {
    bytes: &'buffer [u8; FRAME_BUFFER_SIZE],
    phr_length: u8,
}

impl<'buffer> TxFrameView<'buffer> {
    pub(crate) const fn new(bytes: &'buffer [u8; FRAME_BUFFER_SIZE], phr_length: u8) -> Self {
        Self { bytes, phr_length }
    }

    pub const fn phr_length(self) -> u8 {
        self.phr_length
    }

    pub fn mac_bytes(self) -> &'buffer [u8] {
        &self.bytes[1..self.phr_length as usize - 1]
    }

    pub fn reserved_fcs(self) -> &'buffer [u8] {
        &self.bytes[self.phr_length as usize - 1..=self.phr_length as usize]
    }

    /// Number of bytes in the DMA-visible PHR + PSDU image.
    pub const fn dma_length(self) -> usize {
        self.phr_length as usize + 1
    }

    pub const fn buffer(self) -> &'buffer [u8; FRAME_BUFFER_SIZE] {
        self.bytes
    }

    /// Return the ACK-request bit from the immutable DMA-visible FCF image.
    ///
    /// Construction is possible only after the ESP32-S31-supported frame-type
    /// check, so this value is the complete source-driver ACK predicate for the
    /// prepared image before the retained global `rx_auto_ack` policy is
    /// applied.
    pub const fn acknowledgement_requested(self) -> bool {
        self.bytes[1] & ACKNOWLEDGEMENT_REQUEST_BIT != 0
    }
}

/// Parsed receive image after hardware replaced the two FCS bytes with
/// RSSI/LQI metadata.
#[derive(Clone, Copy, Debug)]
pub struct RxFrameView<'buffer> {
    bytes: &'buffer [u8; FRAME_BUFFER_SIZE],
    phr_length: u8,
}

impl<'buffer> RxFrameView<'buffer> {
    pub fn parse(bytes: &'buffer [u8; FRAME_BUFFER_SIZE]) -> Result<Self, RxFrameError> {
        let phr_length = bytes[0];
        if !(MIN_PHR_LENGTH..=MAX_PHR_LENGTH).contains(&phr_length) {
            return Err(RxFrameError::PhrLengthOutOfRange { length: phr_length });
        }
        Ok(Self { bytes, phr_length })
    }

    pub const fn phr_length(self) -> u8 {
        self.phr_length
    }

    pub fn mac_bytes(self) -> &'buffer [u8] {
        &self.bytes[1..self.phr_length as usize - 1]
    }

    pub fn rssi(self) -> i8 {
        self.bytes[self.phr_length as usize - 1] as i8
    }

    pub fn lqi(self) -> u8 {
        self.bytes[self.phr_length as usize]
    }

    pub const fn buffer(self) -> &'buffer [u8; FRAME_BUFFER_SIZE] {
        self.bytes
    }
}

pub(crate) fn prepare_tx(
    bytes: &mut [u8; FRAME_BUFFER_SIZE],
    mac_frame: &[u8],
) -> Result<u8, TxFrameError> {
    if !(MIN_MAC_FRAME_SIZE..=MAX_MAC_FRAME_SIZE).contains(&mac_frame.len()) {
        return Err(TxFrameError::MacLengthOutOfRange {
            length: mac_frame.len(),
        });
    }

    let frame_type = mac_frame[0] & FRAME_TYPE_MASK;
    if frame_type > MAX_SUPPORTED_FRAME_TYPE {
        return Err(TxFrameError::UnsupportedFrameType { frame_type });
    }

    bytes.fill(0);
    let phr_length = u8::try_from(mac_frame.len() + 2)
        .expect("the checked IEEE 802.15.4 MAC length always fits in u8");
    bytes[0] = phr_length;
    bytes[1..=mac_frame.len()].copy_from_slice(mac_frame);
    Ok(phr_length)
}

#[cfg(test)]
mod tests;
