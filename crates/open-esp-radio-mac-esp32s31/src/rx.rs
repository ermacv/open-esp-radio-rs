//! RX descriptor metadata decoding and bounded raw MPDU extraction.

use crate::descriptor::{
    descriptor_address_valid, dma_range_valid, length as descriptor_length, rx_armed_word, rx_done,
    rx_rearm_word, size as descriptor_size, Descriptor, BIT_31, DESCRIPTOR_BYTES,
};
use open_esp_radio_pac_esp32s31::RadioRegisters;

/// Semantic ownership boundary for the S31 RX descriptor walker.
///
/// Production uses the generated PAC implementation below. Host tests model
/// these finite operations without receiving arbitrary register identities.
pub trait RxDma {
    fn last_descriptor_low(&mut self) -> u32;
    fn next_descriptor_low(&mut self) -> u32;
    fn walker_enabled(&mut self) -> bool;
    fn reload_pending(&mut self) -> bool;
    fn set_descriptor_high_window(&mut self, address_high: u16);
    fn write_descriptor_base(&mut self, address: u32);
    fn publish_walker_enable(&mut self);
    fn request_reload(&mut self);
    fn try_enable_walker(&mut self) -> bool;
    fn try_disable_walker(&mut self) -> bool;
    fn fence(&mut self);
}

impl RxDma for RadioRegisters {
    fn last_descriptor_low(&mut self) -> u32 {
        self.mac_rx_last_descriptor_low()
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.mac_rx_next_descriptor_low()
    }

    fn walker_enabled(&mut self) -> bool {
        self.mac_rx_walker_enabled()
    }

    fn reload_pending(&mut self) -> bool {
        self.mac_rx_reload_pending()
    }

    fn set_descriptor_high_window(&mut self, address_high: u16) {
        self.set_mac_rx_descriptor_high_window(address_high);
    }

    fn write_descriptor_base(&mut self, address: u32) {
        self.write_mac_rx_descriptor_base(address);
    }

    fn publish_walker_enable(&mut self) {
        self.publish_mac_rx_walker_enable();
    }

    fn request_reload(&mut self) {
        self.request_mac_rx_descriptor_reload();
    }

    fn try_enable_walker(&mut self) -> bool {
        self.try_enable_mac_rx_walker()
    }

    fn try_disable_walker(&mut self) -> bool {
        self.try_disable_mac_rx_walker()
    }

    fn fence(&mut self) {
        RadioRegisters::fence(self);
    }
}

pub const INGRESS_STRICT_RXEND: u32 = 0x01;
pub const INGRESS_STRICT_DUMP: u32 = 0x02;
pub const INGRESS_VALID_FLAGS: u32 = 0x03;

pub const FIXED_PREFIX_SIZE: usize = 0x38;
pub const DYNAMIC_TAIL_SIZE: usize = 0x08;
pub const PUBLIC_HEADER_SIZE: usize = 0x40;
pub const FCS_SIZE: usize = 4;
pub const CCMP_HEADER_SIZE: usize = 8;
pub const CCMP_MIC_SIZE: usize = 8;
/// Guard value restored by the ROM RX recycler at both DMA-buffer bounds.
///
/// SOURCE[ROM_REV0_WDEV_APPEND_RX_BLOCKS]: `_oracles/esp32s31_rev0_rom.elf`,
/// `wDev_AppendRxBlocks` at `0x2f838a7e`, complete size `0x132`, writes
/// `0xdead_beef` at `buffer` and `buffer + descriptor_capacity`.
/// The trailing word lives immediately after the descriptor-advertised
/// capacity, so backing storage must reserve four additional bytes.
pub const RX_BUFFER_SENTINEL: u32 = 0xdead_beef;
pub const PREFIX_RXEND_STATE_OFFSET: usize = 0x08;
pub const TAIL_STATE_OFFSET: usize = 0x04;
pub const TAIL_INTERNAL_OFFSET: usize = 0x05;

const RX_PHY_RATE_OFFSET: usize = 0x01;
const RX_PHY_HE_SIGA1_OFFSET: usize = 0x04;
const RX_PHY_HE_SIGA2_OFFSET: usize = 0x09;
const RX_PHY_BB_FORMAT_OFFSET: usize = 0x25;

const CSI_LENGTH_LOW_OFFSET: usize = 0x26;
const CSI_LENGTH_FLAGS_OFFSET: usize = 0x27;
const OPTION_LENGTH_HINT_OFFSET: usize = 0x2a;
const OPTION_LENGTH_FLAGS_OFFSET: usize = 0x2b;
const CSI_APPEND_ENABLE: u32 = 0x0080_0000;

const MLME_HEADER_SIZE: usize = 24;
const MLME_AUTH_BODY_SIZE: usize = 6;
const SUBTYPE_ASSOC_RESPONSE: u8 = 0x10;
const SUBTYPE_REASSOC_RESPONSE: u8 = 0x30;
const SUBTYPE_AUTH: u8 = 0xb0;
const RX_DESCRIPTOR_ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
const RX_DESCRIPTOR_RELOAD_POLL_LIMIT: usize = 0x0001_86a1;

#[derive(Clone, Copy, Debug)]
pub struct RxSegment<'a> {
    pub descriptor_address: u32,
    pub descriptor_word0: u32,
    pub buffer: &'a [u8],
    pub next_descriptor_address: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxIngressConfig {
    pub ring_entry_limit: usize,
    pub csi_config: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxMpduFrame {
    pub length: usize,
    pub tail_offset: usize,
    pub signal_length: u16,
    pub dump_length: u16,
    pub rxend_state: u8,
    pub rx_state: u8,
    pub internal_state: u8,
    pub dump_length_matches: bool,
}

/// Geometry decoded from the first completed S31 RX descriptor.
///
/// The fields mirror the recovered `wDev_IndicateFrame` boundary retained in
/// `migration/esp32s31-hybrid-runtime/src/rx_descriptor.rs`: the descriptor
/// publishes its received byte count independently from the 14-bit on-air
/// `sig_len`, and the latter includes the four-byte FCS. Keeping both values
/// visible is necessary for protected RX, where the MAC may consume cipher
/// trailer bytes before publishing the DMA view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxFirstSegmentLayout {
    pub received_length: usize,
    pub tail_offset: usize,
    pub frame_offset: usize,
    pub signal_length: usize,
    pub dump_length: usize,
    pub expected_frame_length: usize,
    pub available_frame_bytes: usize,
    pub frame_shortfall: usize,
    pub rxend_state: u8,
    pub rx_state: u8,
    pub internal_state: u8,
    pub dump_length_matches: bool,
}

/// Stable radio fields from the public 64-byte ESP32-S31 RX-control prefix.
///
/// SOURCE: esp-wifi-sys commit
/// `72b97e6fe55307aa92c8c1edf3fdb3f4df816e80`,
/// `esp-wifi-sys-esp32s31/src/include.rs` sha256
/// `de6ecd8853cc1925389f89e768a8a865ad58db6025f94a63584096eb8ad5f1dc`,
/// generated `esp_wifi_rxctrl_t`. The packed ABI places the five-bit `rate`
/// at byte 1, HE-SIGA1 at bytes 4..8, HE-SIGA2 at bytes 9..11 and the
/// four-bit `cur_bb_format` in the high nibble of byte 37.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxPhyInfo {
    pub rate: u8,
    pub bb_format: u8,
    pub he_siga1: u32,
    pub he_siga2: u16,
}

/// Decode the finite PHY-rate view without interpreting the format-specific
/// HE-SIG bitfields.
pub fn decode_rx_phy_info(buffer: &[u8]) -> Option<RxPhyInfo> {
    Some(RxPhyInfo {
        rate: *buffer.get(RX_PHY_RATE_OFFSET)? & 0x1f,
        bb_format: *buffer.get(RX_PHY_BB_FORMAT_OFFSET)? >> 4,
        he_siga1: u32::from_le_bytes(
            buffer
                .get(RX_PHY_HE_SIGA1_OFFSET..RX_PHY_HE_SIGA1_OFFSET + 4)?
                .try_into()
                .ok()?,
        ),
        he_siga2: u16::from_le_bytes(
            buffer
                .get(RX_PHY_HE_SIGA2_OFFSET..RX_PHY_HE_SIGA2_OFFSET + 2)?
                .try_into()
                .ok()?,
        ),
    })
}

pub type RxManagementFrame = RxMpduFrame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxDataFrame {
    pub mpdu: RxMpduFrame,
    pub payload_offset: usize,
}

/// Hardware-verified CCMP data MPDU before 802.11-to-Ethernet decapsulation.
///
/// The S31 MAC decrypts the payload in place and reports MIC failure through
/// RX metadata. It leaves the eight-byte CCMP header in the DMA view. The
/// The eight-byte MIC can be wholly or partially absent from the completed
/// DMA view. The recovered migration path passed the logical `sig_len - FCS`
/// to `ccmp_decap`, which removed the complete MIC without reading it.
/// [`mic_bytes_in_dma`](Self::mic_bytes_in_dma) records the bounded physical
/// view while [`mic_offset`](Self::mic_offset) remains the logical payload
/// boundary.
///
/// These offsets reproduce the finite `ccmp_decap` pointer/length adjustment
/// from the pinned net80211 oracle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxCcmpDataFrame {
    pub mpdu: RxMpduFrame,
    pub ccmp_header_offset: usize,
    pub payload_offset: usize,
    pub payload_length: usize,
    pub mic_offset: usize,
    pub mic_bytes_in_dma: usize,
    pub mic_present_in_dma: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxError {
    Invalid,
    Chain,
    Bounds,
    RxFailure,
    MicFailure,
    Quarantined,
    OutputSmall,
    Metadata,
    Ignored,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxRingError {
    Empty,
    Count,
    Address,
    Size,
    Overflow,
    Busy,
    Corrupt,
}

/// One descriptor whose completion ownership has moved from the MAC to Rust.
///
/// The value is a snapshot. Taking it through [`RxRingLive::take_completed`]
/// also records that this descriptor must not be exposed a second time before
/// its recycle group has been rearmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxCompletedDescriptor {
    pub index: usize,
    pub descriptor_address: u32,
    pub word0: u32,
    pub next_descriptor_address: u32,
}

/// One live append accepted for publication to the RX descriptor walker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxLiveAppend {
    pub head_index: usize,
    pub head_address: u32,
    pub tail_index: usize,
}

/// Prepared zero-terminated RX ring while the hardware walker is stopped.
///
/// This type owns the right to start the descriptor walker. Consuming
/// [`start`](Self::start) transfers that authority into [`RxRingLive`].
pub struct RxRingStopped<'a, const COUNT: usize> {
    descriptors: &'a [Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &'a [u32; COUNT],
    initial_start: usize,
    accepted_tail: usize,
    retained_last_low: u32,
}

/// Sole software owner of one running S31 RX descriptor frontier.
///
/// The owner tracks three distinct states recovered from
/// `wDev_AppendRxBlocks`: descriptors observed as CPU-owned, the last tail
/// accepted by hardware, and a future tail whose reload doorbell is still in
/// flight. No allocator, global `wDevCtrl`, C ABI or vendor callback is needed.
pub struct RxRingLive<'a, const COUNT: usize> {
    descriptors: &'a [Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &'a [u32; COUNT],
    observed_mask: u32,
    recycle_start: usize,
    accepted_tail: usize,
    pending_tail: Option<usize>,
}

impl<'a, const COUNT: usize> RxRingStopped<'a, COUNT> {
    /// Stops the walker, prepares all buffers and publishes a rotated cold
    /// list beginning after the descriptor retained by the previous owner.
    ///
    /// `prepare_buffer` must restore any buffer-side DMA contract for `index`;
    /// for the S31 ROM layout this means the two `0xdead_beef` sentinels. It is
    /// invoked only while the walker is confirmed stopped.
    ///
    /// SOURCE[ROM_REV0_WDEV_APPEND_RX_BLOCKS,ROM_REV0_HAL_MAC_RX_GATE,
    /// ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR]; the rotated handoff is qualified
    /// by HIL_OPEN_RX_LIVE_APPEND_2026_07_27.
    pub fn prepare<M, F>(
        mmio: &mut M,
        descriptors: &'a [Descriptor; COUNT],
        descriptor_base: u32,
        buffer_addresses: &'a [u32; COUNT],
        buffer_size: u32,
        mut prepare_buffer: F,
    ) -> Result<Self, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        validate_live_ring_geometry::<COUNT>()?;
        let retained_last_low = mmio.last_descriptor_low();
        let initial_start =
            descriptor_index(retained_last_low, descriptor_base, COUNT).map_or(0, |index| {
                if index + 1 == COUNT {
                    0
                } else {
                    index + 1
                }
            });

        if mmio.walker_enabled() {
            disable_receive(mmio)?;
        }
        for index in 0..COUNT {
            prepare_buffer(index)?;
        }
        build_cold_ring(descriptors, descriptor_base, buffer_addresses, buffer_size)?;
        relink_rotated_ring(
            descriptors,
            descriptor_base,
            buffer_addresses,
            initial_start,
        )?;
        let head = descriptor_address(descriptor_base, initial_start)?;
        publish_cold_ring(mmio, head, false)?;

        Ok(Self {
            descriptors,
            descriptor_base,
            buffer_addresses,
            initial_start,
            accepted_tail: wrap_sub_one::<COUNT>(initial_start),
            retained_last_low,
        })
    }

    pub const fn initial_start(&self) -> usize {
        self.initial_start
    }

    pub const fn accepted_tail(&self) -> usize {
        self.accepted_tail
    }

    pub const fn retained_last_low(&self) -> u32 {
        self.retained_last_low
    }

    /// Opens the walker and consumes the stopped-state authority.
    ///
    /// The caller owns any platform-specific settle delay between
    /// [`prepare`](Self::prepare) and this edge.
    pub fn start<M: RxDma>(self, mmio: &mut M) -> Result<RxRingLive<'a, COUNT>, RxRingError> {
        enable_receive(mmio)?;
        Ok(RxRingLive {
            descriptors: self.descriptors,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
            observed_mask: 0,
            recycle_start: self.initial_start,
            accepted_tail: self.accepted_tail,
            pending_tail: None,
        })
    }
}

impl<const COUNT: usize> RxRingLive<'_, COUNT> {
    /// Takes one newly completed descriptor exactly once for this ring epoch.
    ///
    /// Kept in internal SRAM for PSRAM-code profiles: this is invoked once for
    /// every descriptor slot on every receive poll. HIL at HE20 showed that
    /// executing the complete poll/copy path from PSRAM capped useful UDP RX
    /// near 65 Mbit/s.
    #[inline(never)]
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".rwtext.open_radio_rx_hot")
    )]
    pub fn take_completed(&mut self, index: usize) -> Option<RxCompletedDescriptor> {
        if index >= COUNT {
            return None;
        }
        let bit = 1_u32 << index;
        if self.observed_mask & bit != 0 {
            return None;
        }
        let descriptor = &self.descriptors[index];
        let word0 = descriptor.word0();
        if !rx_done(word0) {
            return None;
        }
        self.observed_mask |= bit;
        Some(RxCompletedDescriptor {
            index,
            descriptor_address: descriptor_address(self.descriptor_base, index).ok()?,
            word0,
            next_descriptor_address: descriptor.next_address(),
        })
    }

    /// Settles a prior append and, when the next half is entirely CPU-owned,
    /// rearms and appends it without stopping the live walker.
    ///
    /// This is the allocation-free Rust ownership form of the recovered
    /// `wDevCtrl.head/tail` transaction. The future tail is kept private until
    /// RX_CONTROL bit 0 self-clears. If the walker exhausted the old frontier
    /// during reload, the exact ROM base-repair rule is applied before the new
    /// tail becomes accepted.
    #[inline(never)]
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".rwtext.open_radio_rx_hot")
    )]
    pub fn recycle_completed_half<M, F>(
        &mut self,
        mmio: &mut M,
        mut prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if mmio.reload_pending() {
            return Ok(None);
        }
        self.settle_reload(mmio)?;

        let group_mask = recycle_group_mask::<COUNT>(self.recycle_start);
        if self.observed_mask & group_mask != group_mask {
            return Ok(None);
        }

        let half = COUNT / 2;
        for step in 0..half {
            let index = wrap_add::<COUNT>(self.recycle_start, step);
            if !rx_done(self.descriptors[index].word0()) {
                return Err(RxRingError::Corrupt);
            }
        }
        for step in 0..half {
            prepare_buffer(wrap_add::<COUNT>(self.recycle_start, step))?;
        }
        for step in 0..half {
            let index = wrap_add::<COUNT>(self.recycle_start, step);
            let descriptor = &self.descriptors[index];
            let next = if step + 1 < half {
                descriptor_address(self.descriptor_base, wrap_add::<COUNT>(index, 1))?
            } else {
                0
            };
            descriptor.publish(
                rx_rearm_word(descriptor.word0()).ok_or(RxRingError::Size)?,
                self.buffer_addresses[index],
                next,
            );
        }

        let head_index = self.recycle_start;
        let head_address = descriptor_address(self.descriptor_base, head_index)?;
        let tail_index = wrap_add::<COUNT>(head_index, half - 1);
        let accepted_tail = &self.descriptors[self.accepted_tail];
        if accepted_tail.next_address() != 0 {
            return Err(RxRingError::Corrupt);
        }
        // SAFETY: this type is the sole publication authority. All descriptors
        // in the appended half were observed complete, rearmed and remain
        // unreachable until this old-tail link and the following doorbell.
        unsafe { accepted_tail.publish_next_address(head_address) };
        mmio.fence();
        mmio.request_reload();
        mmio.fence();

        self.pending_tail = Some(tail_index);
        self.observed_mask &= !group_mask;
        self.recycle_start = wrap_add::<COUNT>(self.recycle_start, half);
        Ok(Some(RxLiveAppend {
            head_index,
            head_address,
            tail_index,
        }))
    }

    pub const fn observed_mask(&self) -> u32 {
        self.observed_mask
    }

    pub const fn recycle_start(&self) -> usize {
        self.recycle_start
    }

    pub const fn accepted_tail(&self) -> usize {
        self.accepted_tail
    }

    pub const fn reload_pending(&self) -> bool {
        self.pending_tail.is_some()
    }

    /// Complete one live-append doorbell before returning to frame processing.
    ///
    /// SOURCE[ROM_REV0_WDEV_APPEND_RX_BLOCKS]: the complete ROM body at
    /// `0x2f83_8a7e` spins on `RX_CONTROL.APPEND_DESCRIPTOR_RELOAD` with the
    /// exact `0x186a1` bound, then immediately samples `RX_NEXT_DESCRIPTOR`
    /// and repairs `RX_DESCRIPTOR_BASE` from `last->next` when the old
    /// frontier was exhausted. Deferring this suffix until after another RX
    /// processing pass is observably unsafe: the appended half can itself
    /// reach the terminal descriptor first, making `RX_LAST_DESCRIPTOR`
    /// indistinguishable from the no-repair case.
    pub fn finish_pending_reload<M: RxDma>(&mut self, mmio: &mut M) -> Result<(), RxRingError> {
        if self.pending_tail.is_none() {
            return Ok(());
        }
        for _ in 0..RX_DESCRIPTOR_RELOAD_POLL_LIMIT {
            if !mmio.reload_pending() {
                return self.settle_reload(mmio);
            }
            core::hint::spin_loop();
        }
        Err(RxRingError::Busy)
    }

    fn settle_reload<M: RxDma>(&mut self, mmio: &mut M) -> Result<(), RxRingError> {
        let Some(pending_tail) = self.pending_tail else {
            return Ok(());
        };
        if mmio.next_descriptor_low() == 0 {
            let last_low = mmio.last_descriptor_low();
            let last_index = descriptor_index(last_low, self.descriptor_base, COUNT)
                .ok_or(RxRingError::Corrupt)?;
            if last_index != pending_tail {
                let repair_head = self.descriptors[last_index].next_address();
                if repair_head == 0 {
                    return Err(RxRingError::Corrupt);
                }
                mmio.write_descriptor_base(repair_head);
                mmio.fence();
            }
        }
        self.accepted_tail = pending_tail;
        self.pending_tail = None;
        Ok(())
    }
}

fn validate_live_ring_geometry<const COUNT: usize>() -> Result<(), RxRingError> {
    if COUNT < 2 || COUNT > 32 || COUNT % 2 != 0 {
        Err(RxRingError::Count)
    } else {
        Ok(())
    }
}

fn descriptor_address(descriptor_base: u32, index: usize) -> Result<u32, RxRingError> {
    let index = u32::try_from(index).map_err(|_| RxRingError::Overflow)?;
    descriptor_base
        .checked_add(
            index
                .checked_mul(DESCRIPTOR_BYTES)
                .ok_or(RxRingError::Overflow)?,
        )
        .ok_or(RxRingError::Overflow)
}

fn descriptor_index(low_address: u32, descriptor_base: u32, count: usize) -> Option<usize> {
    let base_low = descriptor_base & RX_DESCRIPTOR_ADDRESS_LOW_MASK;
    let offset = low_address.checked_sub(base_low)?;
    if offset % DESCRIPTOR_BYTES != 0 {
        return None;
    }
    let index = usize::try_from(offset / DESCRIPTOR_BYTES).ok()?;
    (index < count).then_some(index)
}

fn wrap_add<const COUNT: usize>(index: usize, amount: usize) -> usize {
    (index + amount) % COUNT
}

fn wrap_sub_one<const COUNT: usize>(index: usize) -> usize {
    if index == 0 {
        COUNT - 1
    } else {
        index - 1
    }
}

fn recycle_group_mask<const COUNT: usize>(start: usize) -> u32 {
    let mut mask = 0_u32;
    for step in 0..COUNT / 2 {
        mask |= 1_u32 << wrap_add::<COUNT>(start, step);
    }
    mask
}

fn relink_rotated_ring<const COUNT: usize>(
    descriptors: &[Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &[u32; COUNT],
    start: usize,
) -> Result<(), RxRingError> {
    for step in 0..COUNT {
        let index = wrap_add::<COUNT>(start, step);
        let next = if step + 1 < COUNT {
            descriptor_address(descriptor_base, wrap_add::<COUNT>(index, 1))?
        } else {
            0
        };
        let word0 = descriptors[index].word0();
        descriptors[index].publish(word0, buffer_addresses[index], next);
    }
    Ok(())
}

/// Restores the two guard words required by the recovered RX recycle path.
///
/// SOURCE[ROM_REV0_WDEV_APPEND_RX_BLOCKS] and the preserved Rust transcription
/// in `migration/esp32s31-hybrid-runtime/src/wdev.rs::
/// prepare_rx_recycle_chain`.
///
/// `buffer` is the complete allocation, including the four-byte trailing
/// guard. `capacity` is the byte count published in the DMA descriptor.
pub fn prepare_recycled_buffer(buffer: &mut [u8], capacity: usize) -> Result<(), RxRingError> {
    if capacity < core::mem::size_of::<u32>()
        || capacity
            .checked_add(core::mem::size_of::<u32>())
            .is_none_or(|required| required > buffer.len())
    {
        return Err(RxRingError::Size);
    }
    let sentinel = RX_BUFFER_SENTINEL.to_le_bytes();
    buffer[..sentinel.len()].copy_from_slice(&sentinel);
    buffer[capacity..capacity + sentinel.len()].copy_from_slice(&sentinel);
    Ok(())
}

/// Builds the cold, zero-terminated list used by the recovered S31 RX path.
pub fn build_cold_ring(
    descriptors: &[Descriptor],
    descriptor_dma_base: u32,
    buffer_addresses: &[u32],
    buffer_size: u32,
) -> Result<(), RxRingError> {
    if descriptors.is_empty() {
        return Err(RxRingError::Empty);
    }
    if descriptors.len() != buffer_addresses.len() {
        return Err(RxRingError::Count);
    }
    let word0 = rx_armed_word(buffer_size).ok_or(RxRingError::Size)?;
    let count = u32::try_from(descriptors.len()).map_err(|_| RxRingError::Overflow)?;
    let span = count
        .checked_mul(DESCRIPTOR_BYTES)
        .ok_or(RxRingError::Overflow)?;
    if descriptor_dma_base & 3 != 0 || !dma_range_valid(descriptor_dma_base, span) {
        return Err(RxRingError::Address);
    }
    for &buffer in buffer_addresses {
        if !dma_range_valid(buffer, buffer_size) {
            return Err(RxRingError::Address);
        }
    }
    for (index, descriptor) in descriptors.iter().enumerate() {
        let next = if index + 1 < descriptors.len() {
            descriptor_dma_base + (index as u32 + 1) * DESCRIPTOR_BYTES
        } else {
            0
        };
        descriptor.publish(word0, buffer_addresses[index], next);
    }
    Ok(())
}

/// Publishes a previously built cold ring using the instruction-confirmed
/// fence/high-window/base/enable/fence sequence.
pub fn publish_cold_ring<M: RxDma>(
    mmio: &mut M,
    descriptor_dma_base: u32,
    enable_rx: bool,
) -> Result<(), RxRingError> {
    if !descriptor_address_valid(descriptor_dma_base) {
        return Err(RxRingError::Address);
    }
    mmio.fence();
    mmio.set_descriptor_high_window(0x02f0);
    mmio.write_descriptor_base(descriptor_dma_base);
    if enable_rx {
        mmio.publish_walker_enable();
    }
    mmio.fence();
    Ok(())
}

/// Opens the RX walker after a cold ring base has already been published.
///
/// The vendor cold path keeps these operations separate:
/// `wDev_AppendRxBlocks` publishes the first descriptor base, while the later
/// `chip_enable` path calls `hal_mac_rx_enable`. Keeping this as a distinct
/// operation preserves that ordering and gives the base register time to
/// settle while the caller completes channel/MAC setup.
pub fn enable_receive<M: RxDma>(mmio: &mut M) -> Result<(), RxRingError> {
    if mmio.try_enable_walker() {
        Ok(())
    } else {
        Err(RxRingError::Busy)
    }
}

/// Stop the RX walker and confirm that the peripheral released its enable
/// edge before the owner rebuilds descriptor words or links.
///
/// The pinned `hal_mac_rx_disable` body is exactly this bit clear. The fence
/// and readback turn that raw leaf into an explicit Rust ownership boundary:
/// callers may mutate the ring only after this function returns `Ok(())`.
pub fn disable_receive<M: RxDma>(mmio: &mut M) -> Result<(), RxRingError> {
    if mmio.try_disable_walker() {
        Ok(())
    } else {
        Err(RxRingError::Busy)
    }
}

/// Returns one CPU-owned completed descriptor to the cold/live ring.
pub fn rearm_descriptor(
    descriptor: &Descriptor,
    expected_buffer_address: u32,
    expected_next_address: u32,
) -> Result<(), RxRingError> {
    let word0 = descriptor.word0();
    let capacity = descriptor_size(word0);
    if descriptor.buffer_address() != expected_buffer_address
        || descriptor.next_address() != expected_next_address
        || !dma_range_valid(expected_buffer_address, capacity)
    {
        return Err(RxRingError::Corrupt);
    }
    if !rx_done(word0) {
        return Err(RxRingError::Busy);
    }
    descriptor.write_word0(rx_rearm_word(word0).ok_or(RxRingError::Size)?);
    Ok(())
}

#[inline]
fn read_le32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

fn dynamic_tail_offset(prefix: &[u8], csi_config: u32) -> Option<usize> {
    if prefix.len() < FIXED_PREFIX_SIZE {
        return None;
    }
    let mut offset = FIXED_PREFIX_SIZE;
    let option_flags = *prefix.get(OPTION_LENGTH_FLAGS_OFFSET)?;
    if option_flags & 0x80 != 0 {
        let hint = *prefix.get(OPTION_LENGTH_HINT_OFFSET)?;
        let option_length = usize::from(option_flags & 0x7f) + usize::from(hint >> 5 != 0);
        offset = offset.checked_add((option_length + 3) & !3)?;
    }
    if csi_config & CSI_APPEND_ENABLE != 0 {
        let low = *prefix.get(CSI_LENGTH_LOW_OFFSET)?;
        let flags = *prefix.get(CSI_LENGTH_FLAGS_OFFSET)?;
        let csi_length = usize::from(low) | (usize::from(flags & 0x03) << 8);
        if flags & 0x04 != 0 || csi_length != 0 {
            offset = offset.checked_add((csi_length + 3) & 0x7fc)?;
        }
    }
    Some(offset)
}

fn segment_valid(segment: &RxSegment<'_>) -> bool {
    let capacity = descriptor_size(segment.descriptor_word0) as usize;
    let length = descriptor_length(segment.descriptor_word0) as usize;
    descriptor_address_valid(segment.descriptor_address)
        && segment.descriptor_word0 & BIT_31 != 0
        && capacity != 0
        && length != 0
        && length <= capacity
        && length <= segment.buffer.len()
}

fn copy_frame(
    segments: &[RxSegment<'_>],
    first_offset: usize,
    length: usize,
    output: &mut [u8],
) -> bool {
    let mut copied = 0;
    for (index, segment) in segments.iter().enumerate() {
        if copied == length {
            break;
        }
        let segment_length = descriptor_length(segment.descriptor_word0) as usize;
        let offset = if index == 0 { first_offset } else { 0 };
        let Some(available) = segment_length.checked_sub(offset) else {
            return false;
        };
        let take = available.min(length - copied);
        let Some(source) = segment.buffer.get(offset..offset + take) else {
            return false;
        };
        output[copied..copied + take].copy_from_slice(source);
        copied += take;
    }
    copied == length
}

/// Decode the first-descriptor RX layout without requiring the complete MPDU
/// to fit in that descriptor.
///
/// This is also the diagnostic boundary for a split hardware unit: callers
/// can distinguish malformed metadata from a valid first segment whose
/// remaining bytes are carried by following descriptors.
pub fn first_segment_layout(
    segment: &RxSegment<'_>,
    config: RxIngressConfig,
) -> Result<RxFirstSegmentLayout, RxError> {
    if config.flags & !INGRESS_VALID_FLAGS != 0 || !segment_valid(segment) {
        return Err(RxError::Invalid);
    }

    let received_length = descriptor_length(segment.descriptor_word0) as usize;
    let tail_offset = dynamic_tail_offset(&segment.buffer[..received_length], config.csi_config)
        .ok_or(RxError::Bounds)?;
    if tail_offset < FIXED_PREFIX_SIZE
        || tail_offset > received_length
        || received_length - tail_offset < DYNAMIC_TAIL_SIZE
    {
        return Err(RxError::Bounds);
    }

    let tail_word = read_le32(&segment.buffer[tail_offset..]).ok_or(RxError::Bounds)?;
    let signal_length = (tail_word & 0x3fff) as usize;
    let dump_length = ((tail_word & 0x3fff_0000) >> 16) as usize;
    let rxend_state = segment.buffer[PREFIX_RXEND_STATE_OFFSET];
    let rx_state = segment.buffer[tail_offset + TAIL_STATE_OFFSET];
    match rx_state {
        0xf5 => return Err(RxError::MicFailure),
        0xc6 => return Err(RxError::Quarantined),
        0 => {}
        _ => return Err(RxError::RxFailure),
    }
    if config.flags & INGRESS_STRICT_RXEND != 0 && rxend_state != 0 {
        return Err(RxError::RxFailure);
    }

    let dump_length_matches = signal_length
        .checked_add(FCS_SIZE)
        .is_some_and(|expected| dump_length == expected);
    if config.flags & INGRESS_STRICT_DUMP != 0 && !dump_length_matches {
        return Err(RxError::Metadata);
    }
    if signal_length < 2 + FCS_SIZE {
        return Err(RxError::Bounds);
    }

    let frame_offset = tail_offset + DYNAMIC_TAIL_SIZE;
    let available_frame_bytes = received_length - frame_offset;
    let expected_frame_length = signal_length - FCS_SIZE;
    Ok(RxFirstSegmentLayout {
        received_length,
        tail_offset,
        frame_offset,
        signal_length,
        dump_length,
        expected_frame_length,
        available_frame_bytes,
        frame_shortfall: expected_frame_length.saturating_sub(available_frame_bytes),
        rxend_state,
        rx_state,
        internal_state: segment.buffer[tail_offset + TAIL_INTERNAL_OFFSET],
        dump_length_matches,
    })
}

#[inline(never)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_rx_hot")
)]
fn extract_mpdu_allowing_consumed_trailer(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
    maximum_consumable_trailer_length: usize,
) -> Result<(RxMpduFrame, usize), RxError> {
    if segments.is_empty()
        || config.ring_entry_limit == 0
        || segments.len() > config.ring_entry_limit
        || config.flags & !INGRESS_VALID_FLAGS != 0
    {
        return Err(RxError::Invalid);
    }

    for (index, segment) in segments.iter().enumerate() {
        if !segment_valid(segment)
            || segments[..index]
                .iter()
                .any(|old| old.descriptor_address == segment.descriptor_address)
        {
            return Err(RxError::Chain);
        }
        if let Some(next) = segments.get(index + 1) {
            if rx_done(segment.descriptor_word0)
                || segment.next_descriptor_address != next.descriptor_address
            {
                return Err(RxError::Chain);
            }
        }
    }
    if !rx_done(segments.last().unwrap().descriptor_word0) {
        return Err(RxError::Chain);
    }

    let layout = first_segment_layout(&segments[0], config)?;
    let mut available = layout.available_frame_bytes;
    for segment in &segments[1..] {
        available = available
            .checked_add(descriptor_length(segment.descriptor_word0) as usize)
            .ok_or(RxError::Bounds)?;
    }
    let expected_frame_length = layout.expected_frame_length;
    let consumed_trailer_length = expected_frame_length.saturating_sub(available);
    let frame_length = if consumed_trailer_length == 0 {
        expected_frame_length
    } else if consumed_trailer_length <= maximum_consumable_trailer_length {
        available
    } else {
        return Err(RxError::Bounds);
    };
    if frame_length > output.len() {
        return Err(RxError::OutputSmall);
    }
    if !copy_frame(segments, layout.frame_offset, frame_length, output) {
        return Err(RxError::Bounds);
    }

    Ok((
        RxMpduFrame {
            length: frame_length,
            tail_offset: layout.tail_offset,
            signal_length: layout.signal_length as u16,
            dump_length: layout.dump_length as u16,
            rxend_state: layout.rxend_state,
            rx_state: layout.rx_state,
            internal_state: layout.internal_state,
            dump_length_matches: layout.dump_length_matches,
        },
        consumed_trailer_length,
    ))
}

fn extract_mpdu(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
) -> Result<RxMpduFrame, RxError> {
    extract_mpdu_allowing_consumed_trailer(segments, config, output, 0).map(|(frame, _)| frame)
}

/// Validates one completed chain, strips the four-byte FCS and copies one
/// unfragmented, unprotected management MPDU into caller-owned storage.
pub fn extract_management(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
) -> Result<RxManagementFrame, RxError> {
    let frame = extract_mpdu(segments, config, output)?;
    if frame.length < MLME_HEADER_SIZE {
        return Err(RxError::Bounds);
    }
    if output[0] & 0x0f != 0 {
        return Err(RxError::Ignored);
    }
    if output[1] & (0x04 | 0x40) != 0 || output[22] & 0x0f != 0 {
        return Err(RxError::Unsupported);
    }
    let subtype = output[0] & 0xf0;
    if matches!(
        subtype,
        SUBTYPE_AUTH | SUBTYPE_ASSOC_RESPONSE | SUBTYPE_REASSOC_RESPONSE
    ) && frame.length < MLME_HEADER_SIZE + MLME_AUTH_BODY_SIZE
    {
        return Err(RxError::Bounds);
    }
    Ok(frame)
}

/// Extracts one unfragmented, unprotected 802.11 data MPDU and reports the
/// LLC/SNAP payload offset.
///
/// The header length accounts for address 4, QoS control and HT control. The
/// WPA2 four-way handshake starts before pairwise keys exist, so EAPOL M1 must
/// arrive as an unprotected data frame; protected traffic is rejected here.
pub fn extract_data(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
) -> Result<RxDataFrame, RxError> {
    let frame = extract_mpdu(segments, config, output)?;
    if frame.length < MLME_HEADER_SIZE {
        return Err(RxError::Bounds);
    }
    let frame_control = u16::from_le_bytes([output[0], output[1]]);
    if frame_control & 0x0003 != 0 || frame_control & 0x000c != 0x0008 {
        return Err(RxError::Ignored);
    }
    if frame_control & (1 << 10 | 1 << 14) != 0 || output[22] & 0x0f != 0 {
        return Err(RxError::Unsupported);
    }

    let to_ds = frame_control & (1 << 8) != 0;
    let from_ds = frame_control & (1 << 9) != 0;
    let qos = frame_control & 0x0080 != 0;
    let ordered = frame_control & (1 << 15) != 0;
    let mut payload_offset = if to_ds && from_ds {
        MLME_HEADER_SIZE + 6
    } else {
        MLME_HEADER_SIZE
    };
    if qos {
        payload_offset += 2;
        if ordered {
            payload_offset += 4;
        }
    }
    if frame.length < payload_offset {
        return Err(RxError::Bounds);
    }
    Ok(RxDataFrame {
        mpdu: frame,
        payload_offset,
    })
}

/// Extracts one unfragmented CCMP data MPDU after hardware MIC verification.
///
/// Unlike [`extract_data`], this entry requires the Protected bit. A returned
/// frame therefore has both a successful RX crypto status and a valid CCMP
/// ExtIV/header shape. The payload bytes between `payload_offset` and
/// `mic_offset` are the hardware-decrypted LLC/SNAP payload.
///
/// S31 HIL shows that successful hardware verification can publish a DMA view
/// ending anywhere inside the eight-byte CCMP MIC while `sig_len` still
/// describes the complete on-air MPDU. This reproduces the migration
/// `ccmp_decap` invariant: only bytes after the logical payload boundary may
/// be absent. Unprotected extraction remains strict.
#[inline(never)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_rx_hot")
)]
pub fn extract_ccmp_data(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
) -> Result<RxCcmpDataFrame, RxError> {
    let (frame, consumed_trailer_length) =
        extract_mpdu_allowing_consumed_trailer(segments, config, output, CCMP_MIC_SIZE)?;
    if frame.length < MLME_HEADER_SIZE {
        return Err(RxError::Bounds);
    }
    let frame_control = u16::from_le_bytes([output[0], output[1]]);
    if frame_control & 0x0003 != 0 || frame_control & 0x000c != 0x0008 {
        return Err(RxError::Ignored);
    }
    if frame_control & (1 << 10) != 0 || frame_control & (1 << 14) == 0 || output[22] & 0x0f != 0 {
        return Err(RxError::Unsupported);
    }

    let to_ds = frame_control & (1 << 8) != 0;
    let from_ds = frame_control & (1 << 9) != 0;
    let qos = frame_control & 0x0080 != 0;
    let ordered = frame_control & (1 << 15) != 0;
    let mut ccmp_header_offset = if to_ds && from_ds {
        MLME_HEADER_SIZE + 6
    } else {
        MLME_HEADER_SIZE
    };
    if qos {
        ccmp_header_offset += 2;
        if ordered {
            ccmp_header_offset += 4;
        }
    }
    let payload_offset = ccmp_header_offset
        .checked_add(CCMP_HEADER_SIZE)
        .ok_or(RxError::Bounds)?;
    let expected_frame_length = usize::from(frame.signal_length)
        .checked_sub(FCS_SIZE)
        .ok_or(RxError::Bounds)?;
    let mic_offset = expected_frame_length
        .checked_sub(CCMP_MIC_SIZE)
        .ok_or(RxError::Bounds)?;
    let mic_bytes_in_dma = CCMP_MIC_SIZE - consumed_trailer_length;
    let mic_present_in_dma = mic_bytes_in_dma == CCMP_MIC_SIZE;
    if payload_offset > mic_offset {
        return Err(RxError::Bounds);
    }
    if mic_offset
        .checked_add(mic_bytes_in_dma)
        .is_none_or(|physical_end| physical_end > frame.length)
    {
        return Err(RxError::Bounds);
    }
    // `ccmp_decap` rejects a protected frame whose CCMP ExtIV bit is clear.
    if output
        .get(ccmp_header_offset + 3)
        .is_none_or(|value| value & 0x20 == 0)
    {
        return Err(RxError::Unsupported);
    }

    Ok(RxCcmpDataFrame {
        mpdu: frame,
        ccmp_header_offset,
        payload_offset,
        payload_length: mic_offset - payload_offset,
        mic_offset,
        mic_bytes_in_dma,
        mic_present_in_dma,
    })
}
