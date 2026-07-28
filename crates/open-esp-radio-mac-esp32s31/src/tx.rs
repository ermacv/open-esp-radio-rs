//! Bounded q0 legacy TX descriptor, submission and completion ownership.

use core::pin::Pin;

use open_esp_radio_pac_esp32s31::{MacLegacyTxProgram, MacTxCompletionRegisters, RadioRegisters};

use crate::{
    descriptor::{descriptor_address_valid, dma_range_valid, tx_owned_word, Descriptor},
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
    const fn index(self) -> u8 {
        self as u8
    }
}

/// Finite ordinary-queue hardware authority used by one owned TX slot.
pub trait TxHardware {
    fn prepare_legacy_tx(&mut self, queue: u8, program: MacLegacyTxProgram) -> bool;
    fn start_legacy_tx(&mut self, queue: u8, plcp0: u32);
    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionRegisters>;
    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool;
    fn finish_tx_timeout_abort(&mut self, queue: u8) -> Option<bool>;
    fn detach_completed_tx(&mut self, queue: u8) -> bool;
}

impl TxHardware for RadioRegisters {
    fn prepare_legacy_tx(&mut self, queue: u8, program: MacLegacyTxProgram) -> bool {
        self.prepare_legacy_mac_tx(queue, program)
    }

    fn start_legacy_tx(&mut self, queue: u8, plcp0: u32) {
        self.start_legacy_mac_tx(queue, plcp0);
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionRegisters> {
        self.take_mac_tx_completion(queue)
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        self.begin_mac_tx_timeout_abort(queue)
    }

    fn finish_tx_timeout_abort(&mut self, queue: u8) -> Option<bool> {
        self.finish_mac_tx_timeout_abort(queue)
    }

    fn detach_completed_tx(&mut self, queue: u8) -> bool {
        self.detach_completed_mac_tx(queue)
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
    pub fn submit_legacy_q0<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        config: LegacyTxConfig,
    ) -> Result<(), TxError> {
        self.submit_legacy(hardware, cookie, LegacyTxQueue::Voice, config)
    }

    /// Programs and starts one legacy attempt on an ordinary EDCA queue.
    pub fn submit_legacy<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
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
        let program = MacLegacyTxProgram {
            plcp0: image.plcp0,
            plcp1: image.plcp1,
            power: image.power,
            length_control: image.length_control,
            timeout: config.timeout,
            priority: config.pti,
            priority_count: config.pti_count,
            aifsn: config.aifsn,
            contention_window: config.contention_window,
            interface: config.interface,
        };
        if !hardware.prepare_legacy_tx(index, program) {
            return Err(TxError::QueueActive);
        }

        // Publish the software owner before the final hardware edge. A fast
        // completion can therefore never observe the old Reserved state.
        slot.queue = queue;
        slot.state = TxSlotState::HardwareOwned;
        hardware.start_legacy_tx(index, image.plcp0);
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
    pub fn acknowledge_q0_completion<H: TxHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<Option<TxCompletion>, TxError> {
        self.acknowledge_completion(hardware)
    }

    /// Decodes and acknowledges the completion for the queue retained by the
    /// hardware-owned slot.
    pub fn acknowledge_completion<H: TxHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<Option<TxCompletion>, TxError> {
        let index = self.queue.index();
        let Some(registers) = hardware.take_tx_completion(index) else {
            return Ok(None);
        };

        let ext_word0 = ((registers.aux_a & 0x000f_0000) << 12)
            | (registers.aux_b & 0x001f_e000)
            | (((registers.aux_b >> 25) & 0x7f) << 21);
        let _ext_word1 = ((registers.aux_a >> 20) & 0x03) | ((registers.aux_c >> 5) & 0x1fc);
        let primary = registers.primary;
        let alternate = registers.alternate;
        let used_alternate = ext_word0 & EXT_ALT_SELECT != 0;
        let selected = if used_alternate { alternate } else { primary };
        let status = ((selected >> 12) & 0x0f) as u8;

        if self.state != TxSlotState::HardwareOwned {
            self.state = TxSlotState::ResetRequired;
            return Err(TxError::Stale);
        }
        self.state = TxSlotState::Completed;
        Ok(Some(TxCompletion {
            cookie: self.active,
            status,
            trigger_flow: registers.trigger_flow,
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
    pub fn begin_timeout_abort<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, TxError> {
        if self.state != TxSlotState::HardwareOwned || cookie != self.active {
            return Err(TxError::Stale);
        }
        if !hardware.begin_tx_timeout_abort(self.queue.index()) {
            return Ok(false);
        }
        Ok(true)
    }

    /// Finishes a timed-out queue abort after at least 16 us of settling.
    ///
    /// The caller owns the one timer edge between this method and
    /// [`begin_timeout_abort`](Self::begin_timeout_abort). The register order
    /// matches the recovered migration path: invalidate, release forced CCA,
    /// disable a queue that was still valid, then clear its timeout bit.
    pub fn finish_timeout_abort<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), TxError> {
        if self.state != TxSlotState::HardwareOwned || cookie != self.active {
            return Err(TxError::Stale);
        }
        let Some(detached) = hardware.finish_tx_timeout_abort(self.queue.index()) else {
            return Err(TxError::TimeoutNotPending);
        };
        if !detached {
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
    pub fn detach_completed<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), TxError> {
        if self.state != TxSlotState::Completed || cookie != self.active {
            return Err(TxError::Stale);
        }
        if !hardware.detach_completed_tx(self.queue.index()) {
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
