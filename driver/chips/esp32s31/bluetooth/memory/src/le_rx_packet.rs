//! Shared ESP32-S31 LE receive-packet SRAM codec.
//!
//! Role graphs own allocation topology and publication. This module owns only
//! the common controller buffer header, packet prefix and bounded semantic
//! extraction used by scanning and response-capable advertising.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use vcell::VolatileCell;

use crate::sram_link::BluetoothControllerSramLinkAddress;

/// Bytes preceding a received Link Layer PDU in one controller allocation.
pub const BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES: usize = 0x1e;
/// Maximum Link Layer payload admitted by the shared receive allocation.
pub const BLUETOOTH_LE_RX_PAYLOAD_CAPACITY: usize = u8::MAX as usize;
/// Complete logical receive-packet allocation size.
pub const BLUETOOTH_LE_RX_PACKET_BYTES: usize =
    BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES + BLUETOOTH_LE_RX_PAYLOAD_CAPACITY;

const BUFFER_HEADER_BYTES: usize = 0x18;
const BUFFER_HEADER_WORDS: usize = BUFFER_HEADER_BYTES / 4;
const RX_PACKET_WORDS: usize = BLUETOOTH_LE_RX_PACKET_BYTES.div_ceil(4);
const RX_PACKET_LAST_ALIGNED_OFFSET: u32 = ((BLUETOOTH_LE_RX_PACKET_BYTES as u32 - 1) / 4) * 4;

/// Opaque raw-controller-time observation attached to one received LE packet.
///
/// The value is not scheduler time and is not yet the on-air packet start. It
/// can enter only the chip-private epoch and PHY-calibration operation before
/// protocol timing consumes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLePacketCapturedTime(u32);

impl BluetoothLePacketCapturedTime {
    fn from_controller_sram_word(word: u32) -> Self {
        Self(word)
    }

    /// Borrow the wrapping tick position for the chip-private epoch projector.
    #[doc(hidden)]
    pub const fn wrapping_controller_ticks(self) -> u32 {
        self.0
    }
}

/// One bounded Link Layer PDU copied from a completed controller packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLeReceivedPdu {
    bytes: [u8; BLUETOOTH_LE_RX_PAYLOAD_CAPACITY + 2],
    length: u16,
    rssi_dbm: i8,
    captured_time: BluetoothLePacketCapturedTime,
}

impl BluetoothLeReceivedPdu {
    /// Complete two-byte Link Layer header and declared payload.
    pub const fn as_bytes(&self) -> &[u8] {
        self.bytes.split_at(self.length as usize).0
    }

    /// Number of copied Link Layer PDU octets.
    pub const fn len(&self) -> usize {
        self.length as usize
    }

    /// Whether the copied PDU is empty. A valid hardware result is never empty.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Signed receive-strength byte supplied by the controller packet prefix.
    pub const fn rssi_dbm(&self) -> i8 {
        self.rssi_dbm
    }

    /// Controller-time observation captured by hardware for this exact PDU.
    pub const fn captured_time(&self) -> BluetoothLePacketCapturedTime {
        self.captured_time
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothLeRxPacketError {
    ProducerSentinelRetained,
    EpochSentinelRetained,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothLeRxPacketAddress(BluetoothControllerSramAddress);

impl BluetoothLeRxPacketAddress {
    pub(crate) fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        let address = BluetoothControllerSramAddress::new(address)?;
        let tail = address
            .address()
            .checked_add(RX_PACKET_LAST_ALIGNED_OFFSET)
            .ok_or(BluetoothControllerSramAddressError::OutsideEncodableWindow)?;
        BluetoothControllerSramAddress::new(tail)?;
        Ok(Self(address))
    }

    pub(crate) const fn compressed_image(self) -> u32 {
        self.0.compressed_image()
    }
}

/// Private controller-shared receive-buffer header.
#[repr(C, align(4))]
pub(crate) struct BluetoothLeRxBufferHeaderStorage {
    words: [VolatileCell<u32>; BUFFER_HEADER_WORDS],
}

impl BluetoothLeRxBufferHeaderStorage {
    const LINK_MASK: u32 = 0x000f_ffff;
    const ROTATION_MARKER: u32 = 1;
    const COMPLETION_GATE: u32 = 1 << 31;

    pub(crate) const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BUFFER_HEADER_WORDS],
        }
    }

    pub(crate) fn install(
        &self,
        packet: BluetoothLeRxPacketAddress,
        successor: Option<BluetoothControllerSramLinkAddress>,
        predecessor: Option<BluetoothControllerSramAddress>,
        rotates_into_successor: bool,
    ) {
        let successor = successor.map_or(0, BluetoothControllerSramLinkAddress::compressed_image);
        let predecessor = predecessor.map_or(0, BluetoothControllerSramAddress::address);
        let rotation = if rotates_into_successor {
            Self::ROTATION_MARKER
        } else {
            0
        };
        let image = [
            successor,
            packet.compressed_image(),
            0x8080_0000,
            0,
            rotation,
            predecessor,
        ];
        for (cell, word) in self.words.iter().zip(image) {
            cell.set(word);
        }
    }

    pub(crate) fn completion_observed(&self) -> bool {
        self.words[3].get() & Self::COMPLETION_GATE != 0
    }

    #[cfg(test)]
    pub(crate) fn emulate_hardware_completion(&self) {
        self.words[3].set(self.words[3].get() | Self::COMPLETION_GATE);
    }

    pub(crate) fn retains_packet(&self, packet: BluetoothLeRxPacketAddress) -> bool {
        self.words[1].get() & Self::LINK_MASK == packet.compressed_image()
    }

    pub(crate) fn successor(&self) -> Option<u32> {
        let image = self.words[0].get() & Self::LINK_MASK;
        (image != 0).then_some(image)
    }

    pub(crate) fn predecessor(&self) -> Option<u32> {
        let address = self.words[5].get();
        (address != 0).then_some(address)
    }

    pub(crate) fn rotates_into_successor(&self) -> bool {
        self.words[4].get() & Self::ROTATION_MARKER != 0
    }
}

/// Private controller-shared receive packet allocation.
#[repr(C, align(4))]
pub(crate) struct BluetoothLeRxPacketStorage {
    words: [VolatileCell<u32>; RX_PACKET_WORDS],
}

impl BluetoothLeRxPacketStorage {
    const CAPACITY_WORD: usize = 1;
    const RESULT_WORD: usize = 3;
    const CAPTURED_TIME_WORD: usize = 4;
    const EPOCH_WORD: usize = 6;
    const RESULT_REARM_SENTINEL: u32 = 0x00ff_ffff;
    const EPOCH_REARM_SENTINEL: u32 = 0x0000_ffff;
    const CAPACITY_IMAGE: u32 = 0x0001_0100;

    pub(crate) const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; RX_PACKET_WORDS],
        }
    }

    pub(crate) fn initialize(&self) {
        for word in &self.words {
            word.set(0);
        }
        self.words[Self::CAPACITY_WORD].set(Self::CAPACITY_IMAGE);
        self.rearm();
    }

    pub(crate) fn rearm(&self) {
        self.words[Self::RESULT_WORD]
            .set(self.words[Self::RESULT_WORD].get() | Self::RESULT_REARM_SENTINEL);
        self.words[Self::EPOCH_WORD]
            .set(self.words[Self::EPOCH_WORD].get() | Self::EPOCH_REARM_SENTINEL);
    }

    pub(crate) fn received_pdu(&self) -> Result<BluetoothLeReceivedPdu, BluetoothLeRxPacketError> {
        let result = self.words[Self::RESULT_WORD].get();
        if result & Self::RESULT_REARM_SENTINEL == Self::RESULT_REARM_SENTINEL {
            return Err(BluetoothLeRxPacketError::ProducerSentinelRetained);
        }
        let epoch = self.words[Self::EPOCH_WORD].get();
        if epoch & Self::EPOCH_REARM_SENTINEL == Self::EPOCH_REARM_SENTINEL {
            return Err(BluetoothLeRxPacketError::EpochSentinelRetained);
        }

        let payload_length = self.read_byte(BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES - 1);
        let length = usize::from(payload_length) + 2;
        let mut bytes = [0; BLUETOOTH_LE_RX_PAYLOAD_CAPACITY + 2];
        let mut index = 0;
        while index < length {
            bytes[index] = self.read_byte(BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES - 2 + index);
            index += 1;
        }
        Ok(BluetoothLeReceivedPdu {
            bytes,
            length: length as u16,
            rssi_dbm: self.read_byte(BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES - 15) as i8,
            captured_time: BluetoothLePacketCapturedTime::from_controller_sram_word(
                self.words[Self::CAPTURED_TIME_WORD].get(),
            ),
        })
    }

    fn read_byte(&self, offset: usize) -> u8 {
        let word = self.words[offset / 4].get();
        ((word >> ((offset % 4) * 8)) & 0xff) as u8
    }

    #[cfg(test)]
    pub(crate) fn emulate_hardware_receive(&self, pdu: &[u8], rssi_dbm: i8, captured_time: u32) {
        assert!((2..=BLUETOOTH_LE_RX_PAYLOAD_CAPACITY + 2).contains(&pdu.len()));
        assert_eq!(usize::from(pdu[1]) + 2, pdu.len());
        self.words[Self::RESULT_WORD].set(0);
        self.words[Self::CAPTURED_TIME_WORD].set(captured_time);
        self.words[Self::EPOCH_WORD].set(0);
        for (offset, byte) in pdu.iter().copied().enumerate() {
            self.write_byte(BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES - 2 + offset, byte);
        }
        self.write_byte(BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES - 15, rssi_dbm as u8);
    }

    #[cfg(test)]
    fn write_byte(&self, offset: usize, value: u8) {
        let shift = (offset % 4) * 8;
        let word = self.words[offset / 4].get();
        self.words[offset / 4].set((word & !(0xff << shift)) | (u32::from(value) << shift));
    }

    pub(crate) fn is_armed(&self) -> bool {
        self.words[Self::RESULT_WORD].get() & Self::RESULT_REARM_SENTINEL
            == Self::RESULT_REARM_SENTINEL
            && self.words[Self::EPOCH_WORD].get() & Self::EPOCH_REARM_SENTINEL
                == Self::EPOCH_REARM_SENTINEL
    }
}

#[repr(C)]
pub(crate) struct BluetoothLeRxNodeStorage {
    pub(crate) header: BluetoothLeRxBufferHeaderStorage,
    pub(crate) packet: BluetoothLeRxPacketStorage,
}

impl BluetoothLeRxNodeStorage {
    pub(crate) const fn new() -> Self {
        Self {
            header: BluetoothLeRxBufferHeaderStorage::new(),
            packet: BluetoothLeRxPacketStorage::new(),
        }
    }
}
