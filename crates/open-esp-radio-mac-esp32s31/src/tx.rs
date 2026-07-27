//! Bounded q0 legacy TX descriptor, submission and completion ownership.

use core::pin::Pin;

use crate::{
    descriptor::{descriptor_address_valid, dma_range_valid, tx_owned_word, Descriptor},
    registers::{
        Mmio, TX_CCA_CONTROL, TX_CCA_FORCE_DISABLE, TX_CCA_FORCE_MASK, TX_COMPLETE_ALTERNATE,
        TX_COMPLETE_AUX_A, TX_COMPLETE_AUX_B, TX_COMPLETE_AUX_C, TX_COMPLETE_CLEAR,
        TX_COMPLETE_PRIMARY, TX_COMPLETE_STATE, TX_Q_CONFIG, TX_Q_CONTROL, TX_Q_ENABLE,
        TX_Q_ENABLE_VALID, TX_Q_LENGTH_CONTROL, TX_Q_PLCP1, TX_Q_POWER, TX_Q_PPDU_CONTROL,
        TX_Q_PROTECTION, TX_Q_PTI, TX_Q_VALID, TX_STATE, TX_STATE_CLEAR, TX_TIMEOUT_SHIFT,
    },
    tx_plcp::{basic_length_control_word, basic_non_he_plcp1_word},
};

const EXT_ALT_SELECT: u32 = 0x0010_0000;
const Q0_DESCRIPTOR_ADDRESS_MASK: u32 = 0x000f_ffff;
const Q0_LEGACY_PLCP0_BASE: u32 = 0x0160_0000;
const LEGACY_FCS_LENGTH: u16 = 4;

/// The four ordinary EDCA hardware queues recovered from `ppTxPkt`.
///
/// The names follow the standard user-priority mapping: priorities 6/7 use
/// voice, 4/5 video, 0/3 best effort, and 1/2 background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum LegacyTxQueue {
    #[default]
    Voice = 0,
    Video = 1,
    BestEffort = 2,
    Background = 3,
}

impl LegacyTxQueue {
    const fn index(self) -> usize {
        self as usize
    }

    const fn completion_mask(self) -> u32 {
        1 << self.index()
    }
}

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
    TimeoutNotPending,
    ResetRequired,
}

/// Inputs for one finite non-HE q0 attempt.
///
/// `rate` is the recovered S31 legacy-rate index `0..=15`. For the direct raw
/// q0 management path, `signal` is the transmitted `MPDU + FCS` byte length
/// written to `TX_Q_PLCP1`; it is not a vendor descriptor snapshot. Power
/// values are indices in the PHY gain table, not dBm.
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
    pub timeout: u16,
    pub interface: u8,
    pub pti: u8,
    pub pti_count: u16,
    pub no_ack: bool,
    /// Low byte of the recovered descriptor control word. Zero is plaintext;
    /// protected STA pairwise traffic uses its owned hardware key slot.
    pub hardware_key_selector: u8,
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
            timeout: 100,
            interface: 0,
            pti: 1,
            pti_count: 0,
            no_ack: true,
            hardware_key_selector: 0,
        }
    }

    /// Builds the direct q0 profile from the owned MPDU length.
    ///
    /// Hardware appends the four-byte FCS. For example, a 30-byte open
    /// authentication MPDU requires PLCP1 `0x22`. The S31 HIL proved that
    /// replaying the vendor-context value `0x00b6` instead produces TX status
    /// four and no authentication response.
    pub const fn management_1m_from_mpdu_length(mpdu_length: u16) -> Option<Self> {
        let Some(signal) = mpdu_length.checked_add(LEGACY_FCS_LENGTH) else {
            return None;
        };
        if signal > 0x0fff {
            return None;
        }
        Some(Self::management_1m(signal))
    }

    const fn valid(self) -> bool {
        self.rate <= 15
            && self.rts_rate <= 15
            && self.signal <= 0x0fff
            && self.aifsn <= 0x0f
            && self.contention_window <= 0x03ff
            && self.timeout <= 0x0fff
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
        plcp1: basic_non_he_plcp1_word(
            config.rate,
            0,
            config.hardware_key_selector,
            0,
            config.signal as u32,
        ),
        power: config.data_power as u32
            | ((config.rts_power_low as u32) << 16)
            | ((config.rts_power_high as u32) << 24),
        length_control: basic_length_control_word(
            config.rts_rate,
            1,
            config.hardware_key_selector as u32,
        ),
    })
}

pub struct TxSlot {
    pub descriptor: Descriptor,
    state: TxSlotState,
    generation_cursor: u32,
    active: TxCookie,
    descriptor_address: u32,
    queue: LegacyTxQueue,
}

impl TxSlot {
    pub const fn new() -> Self {
        Self {
            descriptor: Descriptor::new(),
            state: TxSlotState::Free,
            generation_cursor: 0,
            active: TxCookie(0),
            descriptor_address: 0,
            queue: LegacyTxQueue::Voice,
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
            || frame_length > buffer_capacity
        {
            return Err(TxError::Invalid);
        }
        // The low field retains allocation capacity while the high field
        // publishes the populated transfer length. They remain distinct even
        // for a direct buffer without an ESF-private prefix.
        let word0 = tx_owned_word(buffer_capacity, frame_length).ok_or(TxError::Invalid)?;
        let generation = self
            .generation_cursor
            .checked_add(1)
            .ok_or(TxError::ResetRequired)?;
        self.generation_cursor = generation;
        self.active = TxCookie(generation);
        self.descriptor_address = descriptor_address;
        self.queue = LegacyTxQueue::Voice;
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
        mmio: &mut M,
        cookie: TxCookie,
        config: LegacyTxConfig,
    ) -> Result<(), TxError> {
        self.submit_legacy(mmio, cookie, LegacyTxQueue::Voice, config)
    }

    /// Programs and starts one legacy attempt on an ordinary EDCA queue.
    pub fn submit_legacy<M: Mmio>(
        self: Pin<&mut Self>,
        mmio: &mut M,
        cookie: TxCookie,
        queue: LegacyTxQueue,
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
        let index = queue.index();
        if mmio.read32(TX_Q_CONTROL[index]) & TX_Q_ENABLE_VALID != 0 {
            return Err(TxError::QueueActive);
        }

        // Match `lmacSetTxFrame`: timeout is published before the PPDU
        // formatter touches the queue's PLCP/vector registers.
        let mut queue_config = mmio.read32(TX_Q_CONFIG[index]);
        queue_config = (queue_config & 0xffff_f000) | u32::from(config.timeout);
        mmio.write32(TX_Q_CONFIG[index], queue_config);

        mmio.write32(TX_Q_CONTROL[index], image.plcp0);
        mmio.write32(TX_Q_PLCP1[index], image.plcp1);
        let ppdu_control = mmio.read32(TX_Q_PPDU_CONTROL[index]);
        mmio.write32(TX_Q_PPDU_CONTROL[index], ppdu_control & !0x08);
        let protection = mmio.read32(TX_Q_PROTECTION[index]);
        mmio.write32(TX_Q_PROTECTION[index], protection & 0x7fff_ffff);
        mmio.write32(TX_Q_LENGTH_CONTROL[index], image.length_control);
        mmio.write32(TX_Q_POWER[index], image.power);

        // `mac_tx_set_pti` publishes the queue priority first, then performs
        // one read/modify/write edge for each PTI field.
        queue_config = mmio.read32(TX_Q_CONFIG[index]);
        queue_config = (queue_config & 0x0fff_ffff) | (u32::from(config.pti) << 28);
        mmio.write32(TX_Q_CONFIG[index], queue_config);

        let mut pti = mmio.read32(TX_Q_PTI[index]);
        pti = (pti & 0xffff_0fff) | (u32::from(config.pti) << 12);
        mmio.write32(TX_Q_PTI[index], pti);
        pti = mmio.read32(TX_Q_PTI[index]);
        pti = (pti & 0xffff_f0ff) | (u32::from(config.pti) << 8);
        mmio.write32(TX_Q_PTI[index], pti);
        pti = mmio.read32(TX_Q_PTI[index]);
        pti = (pti & 0xffff_ff0f) | (u32::from(config.pti) << 4);
        mmio.write32(TX_Q_PTI[index], pti);
        pti = mmio.read32(TX_Q_PTI[index]);
        pti = (pti & 0xfff0_ffff) | (u32::from(config.pti) << 16);
        mmio.write32(TX_Q_PTI[index], pti);
        pti = mmio.read32(TX_Q_PTI[index]);
        pti = (pti & 0x000f_ffff) | (u32::from(config.pti_count) << 20);
        mmio.write32(TX_Q_PTI[index], pti);

        // Match `hal_mac_tx_config_edca`: its three fields are separate MMIO
        // edges after PPDU/PTI formatting.
        queue_config = mmio.read32(TX_Q_CONFIG[index]);
        queue_config = (queue_config & 0xf0ff_ffff) | (u32::from(config.aifsn) << 24);
        mmio.write32(TX_Q_CONFIG[index], queue_config);
        queue_config = mmio.read32(TX_Q_CONFIG[index]);
        queue_config = (queue_config & 0xffc0_0fff) | (u32::from(config.contention_window) << 12);
        mmio.write32(TX_Q_CONFIG[index], queue_config);
        queue_config = mmio.read32(TX_Q_CONFIG[index]);
        queue_config = (queue_config & 0xff3f_ffff) | (u32::from(config.interface) << 22);
        mmio.write32(TX_Q_CONFIG[index], queue_config);

        // Publish the software owner before the final hardware edge. A fast
        // completion can therefore never observe the old Reserved state.
        slot.queue = queue;
        slot.state = TxSlotState::HardwareOwned;
        mmio.fence();
        mmio.write32(TX_Q_CONTROL[index], image.plcp0 | TX_Q_ENABLE_VALID);
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
        mmio: &mut M,
    ) -> Result<Option<TxCompletion>, TxError> {
        self.acknowledge_completion(mmio)
    }

    /// Decodes and acknowledges the completion for the queue retained by the
    /// hardware-owned slot.
    pub fn acknowledge_completion<M: Mmio>(
        &mut self,
        mmio: &mut M,
    ) -> Result<Option<TxCompletion>, TxError> {
        let index = self.queue.index();
        let completion_mask = self.queue.completion_mask();
        if mmio.read32(TX_COMPLETE_STATE) & completion_mask == 0 {
            return Ok(None);
        }

        let aux_a = mmio.read32(TX_COMPLETE_AUX_A[index]);
        let aux_b = mmio.read32(TX_COMPLETE_AUX_B[index]);
        let aux_c = mmio.read32(TX_COMPLETE_AUX_C[index]);
        let ext_word0 =
            ((aux_a & 0x000f_0000) << 12) | (aux_b & 0x001f_e000) | (((aux_b >> 25) & 0x7f) << 21);
        let _ext_word1 = ((aux_a >> 20) & 0x03) | ((aux_c >> 5) & 0x1fc);
        let primary = mmio.read32(TX_COMPLETE_PRIMARY[index]);
        let alternate = mmio.read32(TX_COMPLETE_ALTERNATE[index]);
        let used_alternate = ext_word0 & EXT_ALT_SELECT != 0;
        let selected = if used_alternate { alternate } else { primary };
        let status = ((selected >> 12) & 0x0f) as u8;
        let trigger_flow = (mmio.read32(TX_STATE) >> (24 + index)) & 1 != 0;

        let clear = mmio.read32(TX_COMPLETE_CLEAR);
        mmio.write32(TX_COMPLETE_CLEAR, clear | completion_mask);
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

    /// Starts the recovered two-phase abort for this queue's TX-timeout edge.
    ///
    /// `migration/lmac.rs::begin_tx_timeout` forces CCA to three before its
    /// fixed 16-us settling interval. `Ok(false)` means that this queue has no
    /// timeout edge and leaves all registers untouched.
    pub fn begin_timeout_abort<M: Mmio>(
        &mut self,
        mmio: &mut M,
        cookie: TxCookie,
    ) -> Result<bool, TxError> {
        if self.state != TxSlotState::HardwareOwned || cookie != self.active {
            return Err(TxError::Stale);
        }
        let timeout_mask = self.queue.completion_mask() << TX_TIMEOUT_SHIFT;
        if mmio.read32(TX_STATE) & timeout_mask == 0 {
            return Ok(false);
        }

        let cca = mmio.read32(TX_CCA_CONTROL);
        mmio.write32(
            TX_CCA_CONTROL,
            (cca & !TX_CCA_FORCE_MASK) | TX_CCA_FORCE_DISABLE,
        );
        mmio.fence();
        Ok(true)
    }

    /// Finishes a timed-out queue abort after at least 16 us of settling.
    ///
    /// The caller owns the one timer edge between this method and
    /// [`begin_timeout_abort`](Self::begin_timeout_abort). The register order
    /// matches the recovered migration path: invalidate, release forced CCA,
    /// disable a queue that was still valid, then clear its timeout bit.
    pub fn finish_timeout_abort<M: Mmio>(
        &mut self,
        mmio: &mut M,
        cookie: TxCookie,
    ) -> Result<(), TxError> {
        if self.state != TxSlotState::HardwareOwned || cookie != self.active {
            return Err(TxError::Stale);
        }
        let index = self.queue.index();
        let timeout_mask = self.queue.completion_mask() << TX_TIMEOUT_SHIFT;
        if mmio.read32(TX_STATE) & timeout_mask == 0 {
            return Err(TxError::TimeoutNotPending);
        }

        let control_register = TX_Q_CONTROL[index];
        let control = mmio.read32(control_register);
        let was_valid = control & TX_Q_VALID != 0;
        mmio.write32(control_register, control & !TX_Q_VALID);

        let cca = mmio.read32(TX_CCA_CONTROL);
        mmio.write32(TX_CCA_CONTROL, cca & !TX_CCA_FORCE_MASK);
        if was_valid {
            let invalid = mmio.read32(control_register);
            mmio.write32(control_register, invalid & !TX_Q_ENABLE);
        }
        mmio.write32(TX_STATE_CLEAR, timeout_mask);
        mmio.fence();

        if mmio.read32(control_register) & TX_Q_ENABLE_VALID != 0 {
            self.state = TxSlotState::ResetRequired;
            return Err(TxError::DetachFailed);
        }
        self.active = TxCookie(0);
        self.descriptor_address = 0;
        self.state = TxSlotState::Free;
        Ok(())
    }

    /// Makes the completed static slot reusable after disabling q0 and exact
    /// readback. This is normal single-attempt turnover, not a global DMA
    /// release oracle for freeing the backing allocation during teardown.
    pub fn detach_completed<M: Mmio>(
        &mut self,
        mmio: &mut M,
        cookie: TxCookie,
    ) -> Result<(), TxError> {
        if self.state != TxSlotState::Completed || cookie != self.active {
            return Err(TxError::Stale);
        }
        let control_register = TX_Q_CONTROL[self.queue.index()];
        let control = mmio.read32(control_register);
        mmio.write32(control_register, control & !TX_Q_ENABLE_VALID);
        mmio.fence();
        if mmio.read32(control_register) & TX_Q_ENABLE_VALID != 0 {
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
