//! Role-neutral ownership handoff from physical RX DMA to protocol roles.

use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender},
};
use open_esp_radio_esp32s31_wifi_mac::{
    rx::RxPhyInfo,
    rx_pool::{NetworkRxFrame, VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
};
use open_esp_radio_wifi_softmac::MacRxMetadata;

/// Unique owner of one staged RX unit.
pub type Esp32s31StagedRxFrame<
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> = NetworkRxFrame<'pool, SLOTS, CAPACITY>;

/// Static bounded storage for the physical-RX to protocol-role handoff.
pub struct Esp32s31StagedRxQueue<
    'pool,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    frames: Channel<M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
}

impl<'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    Esp32s31StagedRxQueue<'pool, M, DEPTH, CAPACITY, SLOTS>
{
    pub const fn new() -> Self {
        assert!(DEPTH != 0, "staged RX queue must not be empty");
        assert!(
            DEPTH <= SLOTS,
            "staged RX queue cannot outgrow its ownership pool"
        );
        Self {
            frames: Channel::new(),
        }
    }

    pub fn split(
        &self,
    ) -> (
        Sender<'_, M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
        Receiver<'_, M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    ) {
        (self.frames.sender(), self.frames.receiver())
    }
}

impl<'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize> Default
    for Esp32s31StagedRxQueue<'pool, M, DEPTH, CAPACITY, SLOTS>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Borrow-free Ethernet publication captured while a role validates a frame.
#[derive(Clone, Copy, Debug)]
pub struct StagedEthernetPublication {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload_offset: usize,
    pub payload_length: usize,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagedRxDisposition {
    Released,
    RetainedByNetwork,
}
