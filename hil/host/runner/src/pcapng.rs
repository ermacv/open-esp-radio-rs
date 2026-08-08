//! Host-only PCAPNG serialization for normalized IEEE 802.11 captures.

use std::io::{self, Write};

const SECTION_HEADER_BLOCK: u32 = 0x0a0d_0d0a;
const INTERFACE_DESCRIPTION_BLOCK: u32 = 0x0000_0001;
const ENHANCED_PACKET_BLOCK: u32 = 0x0000_0006;
const BYTE_ORDER_MAGIC: u32 = 0x1a2b_3c4d;
const LINKTYPE_IEEE802_11: u16 = 105;
const SECTION_HEADER_LENGTH: u32 = 28;
const INTERFACE_DESCRIPTION_LENGTH: u32 = 20;
const ENHANCED_PACKET_FIXED_LENGTH: u32 = 32;

/// Incremental PCAPNG writer for one raw IEEE 802.11 interface.
///
/// Timestamps use PCAPNG's default microsecond interface resolution. The
/// target transports only frame bytes and typed metadata; block layout,
/// padding and file ownership remain entirely on the host.
pub struct PcapNgWriter<W> {
    output: W,
    snap_length: u32,
    packets: u64,
}

impl<W: Write> PcapNgWriter<W> {
    pub fn new(mut output: W, snap_length: u32) -> io::Result<Self> {
        write_u32(&mut output, SECTION_HEADER_BLOCK)?;
        write_u32(&mut output, SECTION_HEADER_LENGTH)?;
        write_u32(&mut output, BYTE_ORDER_MAGIC)?;
        write_u16(&mut output, 1)?;
        write_u16(&mut output, 0)?;
        output.write_all(&u64::MAX.to_le_bytes())?;
        write_u32(&mut output, SECTION_HEADER_LENGTH)?;

        write_u32(&mut output, INTERFACE_DESCRIPTION_BLOCK)?;
        write_u32(&mut output, INTERFACE_DESCRIPTION_LENGTH)?;
        write_u16(&mut output, LINKTYPE_IEEE802_11)?;
        write_u16(&mut output, 0)?;
        write_u32(&mut output, snap_length)?;
        write_u32(&mut output, INTERFACE_DESCRIPTION_LENGTH)?;

        Ok(Self {
            output,
            snap_length,
            packets: 0,
        })
    }

    pub const fn packet_count(&self) -> u64 {
        self.packets
    }

    /// Append one frame reassembled and validated by the host transport.
    ///
    /// `original_length` is the logical MPDU length reported by the driver.
    /// It may exceed `bytes.len()` when the selected capture snap length
    /// truncated the packet or hardware consumed a protected trailer.
    pub fn write_packet(
        &mut self,
        timestamp_micros: u64,
        bytes: &[u8],
        original_length: u32,
    ) -> io::Result<()> {
        let captured_length = u32::try_from(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "capture is too large"))?;
        if captured_length > self.snap_length || original_length < captured_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid captured/original IEEE 802.11 lengths",
            ));
        }
        let padded_length = captured_length
            .checked_add(3)
            .map(|length| length & !3)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "capture is too large"))?;
        let block_length = ENHANCED_PACKET_FIXED_LENGTH
            .checked_add(padded_length)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "capture is too large"))?;

        write_u32(&mut self.output, ENHANCED_PACKET_BLOCK)?;
        write_u32(&mut self.output, block_length)?;
        write_u32(&mut self.output, 0)?; // interface ID
        write_u32(&mut self.output, (timestamp_micros >> 32) as u32)?;
        write_u32(&mut self.output, timestamp_micros as u32)?;
        write_u32(&mut self.output, captured_length)?;
        write_u32(&mut self.output, original_length)?;
        self.output.write_all(bytes)?;
        const ZERO_PADDING: [u8; 3] = [0; 3];
        self.output
            .write_all(&ZERO_PADDING[..(padded_length - captured_length) as usize])?;
        write_u32(&mut self.output, block_length)?;
        self.packets = self.packets.saturating_add(1);
        Ok(())
    }

    pub fn into_inner(self) -> W {
        self.output
    }
}

fn write_u16(output: &mut impl Write, value: u16) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

fn write_u32(output: &mut impl Write, value: u32) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn writes_host_owned_section_interface_and_padded_packet_blocks() {
        let mut writer = PcapNgWriter::new(Vec::new(), 4096).unwrap();
        writer
            .write_packet(0x0000_0001_0000_0002, &[0x80, 1, 2, 3, 4], 9)
            .unwrap();
        assert_eq!(writer.packet_count(), 1);
        let bytes = writer.into_inner();

        assert_eq!(u32_at(&bytes, 0), SECTION_HEADER_BLOCK);
        assert_eq!(u32_at(&bytes, 4), SECTION_HEADER_LENGTH);
        assert_eq!(u32_at(&bytes, 24), SECTION_HEADER_LENGTH);
        assert_eq!(u32_at(&bytes, 28), INTERFACE_DESCRIPTION_BLOCK);
        assert_eq!(u32_at(&bytes, 32), INTERFACE_DESCRIPTION_LENGTH);
        assert_eq!(u16::from_le_bytes(bytes[36..38].try_into().unwrap()), 105);
        assert_eq!(u32_at(&bytes, 40), 4096);

        let packet = 48;
        assert_eq!(u32_at(&bytes, packet), ENHANCED_PACKET_BLOCK);
        assert_eq!(u32_at(&bytes, packet + 4), 40);
        assert_eq!(u32_at(&bytes, packet + 12), 1);
        assert_eq!(u32_at(&bytes, packet + 16), 2);
        assert_eq!(u32_at(&bytes, packet + 20), 5);
        assert_eq!(u32_at(&bytes, packet + 24), 9);
        assert_eq!(&bytes[packet + 28..packet + 33], &[0x80, 1, 2, 3, 4]);
        assert_eq!(&bytes[packet + 33..packet + 36], &[0, 0, 0]);
        assert_eq!(u32_at(&bytes, packet + 36), 40);
        assert_eq!(bytes.len(), 88);
    }

    #[test]
    fn rejects_lengths_which_cannot_describe_one_capture() {
        let mut writer = PcapNgWriter::new(Vec::new(), 4).unwrap();
        assert_eq!(
            writer.write_packet(0, &[0; 5], 5).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            writer.write_packet(0, &[0; 4], 3).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(writer.packet_count(), 0);
    }
}
