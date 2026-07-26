//! Minimal q0 TX descriptor and completion ownership state.

use crate::{
    descriptor::{descriptor_address_valid, dma_range_valid, tx_owned_word, Descriptor},
    registers::{
        Mmio, TX_COMPLETE_ALTERNATE_Q0, TX_COMPLETE_AUX_A_Q0, TX_COMPLETE_AUX_B_Q0,
        TX_COMPLETE_AUX_C_Q0, TX_COMPLETE_CLEAR, TX_COMPLETE_PRIMARY_Q0, TX_COMPLETE_Q0,
        TX_COMPLETE_STATE, TX_Q0_CONTROL, TX_Q_ENABLE_VALID, TX_STATE,
    },
};

const EXT_ALT_SELECT: u32 = 0x0010_0000;

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
    Stale,
    DetachFailed,
    ResetRequired,
}

pub struct TxSlot {
    pub descriptor: Descriptor,
    state: TxSlotState,
    generation_cursor: u32,
    active: TxCookie,
}

impl TxSlot {
    pub const fn new() -> Self {
        Self {
            descriptor: Descriptor::new(),
            state: TxSlotState::Free,
            generation_cursor: 0,
            active: TxCookie(0),
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
        self.descriptor.publish(word0, buffer_address, 0);
        self.state = TxSlotState::Reserved;
        Ok(self.active)
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
        self.state = TxSlotState::Free;
        Ok(())
    }
}

impl Default for TxSlot {
    fn default() -> Self {
        Self::new()
    }
}
