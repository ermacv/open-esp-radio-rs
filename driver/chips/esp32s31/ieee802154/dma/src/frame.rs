//! IEEE 802.15.4 frame-buffer layout.

pub const FRAME_BUFFER_SIZE: usize = 128;
pub const MIN_PHR_LENGTH: u8 = 3;
pub const MAX_PHR_LENGTH: u8 = 127;
pub const MIN_MAC_FRAME_SIZE: usize = MIN_PHR_LENGTH as usize - 2;
pub const MAX_MAC_FRAME_SIZE: usize = MAX_PHR_LENGTH as usize - 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxFrameError {
    MacLengthOutOfRange { length: usize },
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

    bytes.fill(0);
    let phr_length = u8::try_from(mac_frame.len() + 2)
        .expect("the checked IEEE 802.15.4 MAC length always fits in u8");
    bytes[0] = phr_length;
    bytes[1..=mac_frame.len()].copy_from_slice(mac_frame);
    Ok(phr_length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_legal_tx_length_has_exact_layout_and_zero_fcs() {
        let mut bytes = [0xa5; FRAME_BUFFER_SIZE];
        let source = [0x5a; MAX_MAC_FRAME_SIZE];

        for length in MIN_MAC_FRAME_SIZE..=MAX_MAC_FRAME_SIZE {
            let phr = prepare_tx(&mut bytes, &source[..length]).unwrap();
            let view = TxFrameView::new(&bytes, phr);
            assert_eq!(usize::from(phr), length + 2);
            assert_eq!(view.mac_bytes(), &source[..length]);
            assert_eq!(view.reserved_fcs(), &[0, 0]);
            assert_eq!(view.dma_length(), length + 3);
            assert!(
                view.buffer()[view.dma_length()..]
                    .iter()
                    .all(|byte| *byte == 0)
            );
        }
    }

    #[test]
    fn invalid_tx_lengths_do_not_modify_destination() {
        let mut bytes = [0xa5; FRAME_BUFFER_SIZE];
        let original = bytes;
        assert_eq!(
            prepare_tx(&mut bytes, &[]),
            Err(TxFrameError::MacLengthOutOfRange { length: 0 })
        );
        assert_eq!(bytes, original);
        assert_eq!(
            prepare_tx(&mut bytes, &[0; MAX_MAC_FRAME_SIZE + 1]),
            Err(TxFrameError::MacLengthOutOfRange {
                length: MAX_MAC_FRAME_SIZE + 1
            })
        );
        assert_eq!(bytes, original);
    }

    #[test]
    fn rx_minimum_and_maximum_layouts_are_exact() {
        for phr in [MIN_PHR_LENGTH, MAX_PHR_LENGTH] {
            let mut bytes = [0; FRAME_BUFFER_SIZE];
            bytes[0] = phr;
            bytes[phr as usize - 1] = (-42_i8) as u8;
            bytes[phr as usize] = 211;
            let view = RxFrameView::parse(&bytes).unwrap();
            assert_eq!(view.phr_length(), phr);
            assert_eq!(view.mac_bytes().len(), phr as usize - 2);
            assert_eq!(view.rssi(), -42);
            assert_eq!(view.lqi(), 211);
        }
    }

    #[test]
    fn rx_rejects_every_out_of_range_phr() {
        for phr in 0_u8..=u8::MAX {
            let mut bytes = [0; FRAME_BUFFER_SIZE];
            bytes[0] = phr;
            let result = RxFrameView::parse(&bytes);
            assert_eq!(
                result.is_ok(),
                (MIN_PHR_LENGTH..=MAX_PHR_LENGTH).contains(&phr)
            );
        }
    }
}
