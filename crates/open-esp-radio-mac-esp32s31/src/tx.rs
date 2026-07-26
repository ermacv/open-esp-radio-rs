//! Bounded q0 legacy TX descriptor, submission and completion ownership.

use core::pin::Pin;

use crate::{
    descriptor::{descriptor_address_valid, dma_range_valid, tx_owned_word, Descriptor},
    registers::{
        Mmio, TX_COMPLETE_ALTERNATE_Q0, TX_COMPLETE_AUX_A_Q0, TX_COMPLETE_AUX_B_Q0,
        TX_COMPLETE_AUX_C_Q0, TX_COMPLETE_CLEAR, TX_COMPLETE_PRIMARY_Q0, TX_COMPLETE_Q0,
        TX_COMPLETE_STATE, TX_Q0_CONFIG, TX_Q0_CONTROL, TX_Q0_LENGTH_CONTROL, TX_Q0_PLCP1,
        TX_Q0_POWER, TX_Q0_PPDU_CONTROL, TX_Q0_PROTECTION, TX_Q0_PTI, TX_Q_ENABLE_VALID, TX_STATE,
    },
};

const EXT_ALT_SELECT: u32 = 0x0010_0000;
const Q0_DESCRIPTOR_ADDRESS_MASK: u32 = 0x000f_ffff;
const Q0_LEGACY_PLCP0_BASE: u32 = 0x0160_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxSlotState {
    Free,
    Reserved,
    HardwareOwned,
    Completed,
    ResetRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxCookie(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxCompletion {
    pub cookie: TxCookie,
    pub status: u8,
    pub trigger_flow: bool,
    pub used_alternate: bool,
    pub primary_word: u32,
    pub alternate_word: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxError {
    Busy,
    Invalid,
    QueueActive,
    Stale,
    DetachFailed,
    ResetRequired,
}

/// Inputs for one finite non-HE q0 attempt.
///
/// `rate` is the recovered S31 legacy-rate index `0..=15`. `signal` is the
/// low 12-bit legacy PLCP input; for a raw management MPDU this is the low
/// 12 bits of its first little-endian word. Power values are indices in the
/// PHY gain table, not dBm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyTxConfig {
    pub rate: u8,
    pub rts_rate: u8,
    pub signal: u16,
    pub data_power: u8,
    pub rts_power_low: u8,
    pub rts_power_high: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub interface: u8,
    pub pti: u8,
    pub pti_count: u16,
    pub no_ack: bool,
}

impl LegacyTxConfig {
    /// Conservative 1-Mbit/s management-frame profile used by the first HIL.
    pub const fn management_1m(signal: u16) -> Self {
        Self {
            rate: 0,
            rts_rate: 0,
            signal,
            data_power: 8,
            rts_power_low: 8,
            rts_power_high: 8,
            aifsn: 2,
            contention_window: 0,
            interface: 0,
            pti: 1,
            pti_count: 0,
            no_ack: true,
        }
    }

    const fn valid(self) -> bool {
        self.rate <= 15
            && self.rts_rate <= 15
            && self.signal <= 0x0fff
            && self.aifsn <= 0x0f
            && self.contention_window <= 0x03ff
            && self.interface <= 3
            && self.pti <= 0x0f
            && self.pti_count <= 0x0fff
    }
}

/// Pure register image for one q0 legacy attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyQ0Image {
    pub plcp0: u32,
    pub plcp1: u32,
    pub power: u32,
    pub length_control: u32,
}

pub const fn legacy_q0_image(
    descriptor_address: u32,
    config: LegacyTxConfig,
) -> Option<LegacyQ0Image> {
    if !descriptor_address_valid(descriptor_address) || !config.valid() {
        return None;
    }
    Some(LegacyQ0Image {
        plcp0: (descriptor_address & Q0_DESCRIPTOR_ADDRESS_MASK)
            | if config.no_ack {
                Q0_LEGACY_PLCP0_BASE & !(1 << 24)
            } else {
                Q0_LEGACY_PLCP0_BASE
            },
        plcp1: ((config.rate as u32) << 12) | config.signal as u32,
        power: config.data_power as u32
            | ((config.rts_power_low as u32) << 16)
            | ((config.rts_power_high as u32) << 24),
        length_control: (1 << 22) | ((config.rts_rate as u32) << 6) | 0x04,
    })
}

pub struct TxSlot {
    pub descriptor: Descriptor,
    state: TxSlotState,
    generation_cursor: u32,
    active: TxCookie,
    descriptor_address: u32,
}

impl TxSlot {
    pub const fn new() -> Self {
        Self {
            descriptor: Descriptor::new(),
            state: TxSlotState::Free,
            generation_cursor: 0,
            active: TxCookie(0),
            descriptor_address: 0,
        }
    }

    pub fn state(&self) -> TxSlotState {
        self.state
    }

    pub fn reserve(
        &mut self,
        descriptor_address: u32,
        buffer_address: u32,
        buffer_capacity: u32,
        frame_length: u32,
    ) -> Result<TxCookie, TxError> {
        if self.state != TxSlotState::Free {
            return Err(TxError::Busy);
        }
        if !descriptor_address_valid(descriptor_address)
            || !dma_range_valid(buffer_address, buffer_capacity)
        {
            return Err(TxError::Invalid);
        }
        let word0 = tx_owned_word(buffer_capacity, frame_length).ok_or(TxError::Invalid)?;
        let generation = self
            .generation_cursor
            .checked_add(1)
            .ok_or(TxError::ResetRequired)?;
        self.generation_cursor = generation;
        self.active = TxCookie(generation);
        self.descriptor_address = descriptor_address;
        self.descriptor.publish(word0, buffer_address, 0);
        self.state = TxSlotState::Reserved;
        Ok(self.active)
    }

    /// Programs and starts one legacy q0 attempt.
    ///
    /// Pinning makes the descriptor address checked at `reserve` stable across
    /// the hardware ownership interval. The buffer must likewise remain in
    /// static/pinned DMA-visible SRAM until completion is detached.
    pub fn submit_legacy_q0<M: Mmio>(
        self: Pin<&mut Self>,
        mmio: &M,
        cookie: TxCookie,
        config: LegacyTxConfig,
    ) -> Result<(), TxError> {
        // SAFETY: the method never moves the pinned slot; it changes only
        // scalar ownership fields and the device-owned descriptor words.
        let slot = unsafe { self.get_unchecked_mut() };
        if slot.state != TxSlotState::Reserved || cookie != slot.active {
            return Err(TxError::Stale);
        }
        let actual_address = core::ptr::addr_of!(slot.descriptor).addr() as u32;
        if actual_address != slot.descriptor_address {
            return Err(TxError::Invalid);
        }
        let image = legacy_q0_image(actual_address, config).ok_or(TxError::Invalid)?;
        if mmio.read32(TX_Q0_CONTROL) & TX_Q_ENABLE_VALID != 0 {
            return Err(TxError::QueueActive);
        }

        mmio.write32(TX_COMPLETE_CLEAR, TX_COMPLETE_Q0);
        mmio.write32(TX_Q0_CONTROL, image.plcp0);
        mmio.write32(TX_Q0_PLCP1, image.plcp1);
        mmio.write32(TX_Q0_PPDU_CONTROL, mmio.read32(TX_Q0_PPDU_CONTROL) & !0x08);
        mmio.write32(
            TX_Q0_PROTECTION,
            mmio.read32(TX_Q0_PROTECTION) & 0x7fff_ffff,
        );
        mmio.write32(TX_Q0_LENGTH_CONTROL, image.length_control);
        mmio.write32(TX_Q0_POWER, image.power);

        let mut pti = mmio.read32(TX_Q0_PTI);
        pti = (pti & 0xffff_0fff) | (u32::from(config.pti) << 12);
        pti = (pti & 0xffff_f0ff) | (u32::from(config.pti) << 8);
        pti = (pti & 0xffff_ff0f) | (u32::from(config.pti) << 4);
        pti = (pti & 0xfff0_ffff) | (u32::from(config.pti) << 16);
        pti = (pti & 0x000f_ffff) | (u32::from(config.pti_count) << 20);
        mmio.write32(TX_Q0_PTI, pti);

        let mut queue = mmio.read32(TX_Q0_CONFIG);
        queue = (queue & 0x0fff_ffff) | (u32::from(config.pti) << 28);
        queue = (queue & 0xffff_f000) | 10;
        queue = (queue & 0xf0ff_ffff) | (u32::from(config.aifsn) << 24);
        queue = (queue & 0xffc0_0fff) | (u32::from(config.contention_window) << 12);
        queue = (queue & 0xff3f_ffff) | (u32::from(config.interface) << 22);
        mmio.write32(TX_Q0_CONFIG, queue);

        // Publish the software owner before the final hardware edge. A fast
        // completion can therefore never observe the old Reserved state.
        slot.state = TxSlotState::HardwareOwned;
        mmio.fence();
        mmio.write32(TX_Q0_CONTROL, image.plcp0 | TX_Q_ENABLE_VALID);
        mmio.fence();
        Ok(())
    }

    /// Publishes the software owner immediately before the future management
    /// TX layer performs its final q0 ENABLE|VALID write under MAC-IRQ
    /// exclusion. Full q0 PPDU/rate configuration must precede this call; this
    /// method intentionally performs no incomplete hardware submission.
    pub fn mark_hardware_owned(&mut self, cookie: TxCookie) -> Result<(), TxError> {
        if self.state != TxSlotState::Reserved || cookie != self.active {
            return Err(TxError::Stale);
        }
        self.state = TxSlotState::HardwareOwned;
        Ok(())
    }

    /// Decodes and acknowledges one q0 completion. Storage stays retained in
    /// `Completed`; it is not reusable until `detach_completed` closes q0.
    pub fn acknowledge_q0_completion<M: Mmio>(
        &mut self,
        mmio: &M,
    ) -> Result<Option<TxCompletion>, TxError> {
        if mmio.read32(TX_COMPLETE_STATE) & TX_COMPLETE_Q0 == 0 {
            return Ok(None);
        }

        let aux_a = mmio.read32(TX_COMPLETE_AUX_A_Q0);
        let aux_b = mmio.read32(TX_COMPLETE_AUX_B_Q0);
        let aux_c = mmio.read32(TX_COMPLETE_AUX_C_Q0);
        let ext_word0 =
            ((aux_a & 0x000f_0000) << 12) | (aux_b & 0x001f_e000) | (((aux_b >> 25) & 0x7f) << 21);
        let _ext_word1 = ((aux_a >> 20) & 0x03) | ((aux_c >> 5) & 0x1fc);
        let primary = mmio.read32(TX_COMPLETE_PRIMARY_Q0);
        let alternate = mmio.read32(TX_COMPLETE_ALTERNATE_Q0);
        let used_alternate = ext_word0 & EXT_ALT_SELECT != 0;
        let selected = if used_alternate { alternate } else { primary };
        let status = ((selected >> 12) & 0x0f) as u8;
        let trigger_flow = (mmio.read32(TX_STATE) >> 24) & 1 != 0;

        let clear = mmio.read32(TX_COMPLETE_CLEAR);
        mmio.write32(TX_COMPLETE_CLEAR, clear | TX_COMPLETE_Q0);
        mmio.fence();

        if self.state != TxSlotState::HardwareOwned {
            self.state = TxSlotState::ResetRequired;
            return Err(TxError::Stale);
        }
        self.state = TxSlotState::Completed;
        Ok(Some(TxCompletion {
            cookie: self.active,
            status,
            trigger_flow,
            used_alternate,
            primary_word: primary,
            alternate_word: alternate,
        }))
    }

    /// Makes the completed static slot reusable after disabling q0 and exact
    /// readback. This is normal single-attempt turnover, not a global DMA
    /// release oracle for freeing the backing allocation during teardown.
    pub fn detach_completed<M: Mmio>(&mut self, mmio: &M, cookie: TxCookie) -> Result<(), TxError> {
        if self.state != TxSlotState::Completed || cookie != self.active {
            return Err(TxError::Stale);
        }
        let control = mmio.read32(TX_Q0_CONTROL);
        mmio.write32(TX_Q0_CONTROL, control & !TX_Q_ENABLE_VALID);
        mmio.fence();
        if mmio.read32(TX_Q0_CONTROL) & TX_Q_ENABLE_VALID != 0 {
            self.state = TxSlotState::ResetRequired;
            return Err(TxError::DetachFailed);
        }
        self.active = TxCookie(0);
        self.descriptor_address = 0;
        self.state = TxSlotState::Free;
        Ok(())
    }
}

impl Default for TxSlot {
    fn default() -> Self {
        Self::new()
    }
}
