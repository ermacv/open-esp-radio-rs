//! Fallible, side-effect-free HT/HE register-image preparation.

use open_esp_radio_esp32s31_pac::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid, MacHeTxProgram,
    MacHtTxProgram,
};

use super::{HtAmpduLength, HtAmpduTxError, HtAmpduTxFormat, HtAmpduTxStorage};
use crate::tx::{HeAmpduTxConfig, HtAmpduTxConfig, LegacyTxQueue, TxCookie, TxSlotState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PreparedHeTrigger {
    pub(super) policy: MacHeTbTidLimit,
    pub(super) reservation: MacHeTbLinkReservation,
    pub(super) tid: MacHeTid,
    pub(super) queued_msdu_bytes: u32,
}

pub(super) struct PreparedHtSubmission {
    pub(super) aggregate: HtAmpduLength,
    pub(super) program: MacHtTxProgram,
    pub(super) plcp0: u32,
}

pub(super) struct PreparedHeSubmission {
    pub(super) aggregate: HtAmpduLength,
    pub(super) program: MacHeTxProgram,
    pub(super) plcp0: u32,
    pub(super) trigger: Option<PreparedHeTrigger>,
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    pub(super) fn prepared_ht_submission(
        &self,
        descriptor_head: u32,
        cookie: TxCookie,
        config: HtAmpduTxConfig,
    ) -> Result<PreparedHtSubmission, HtAmpduTxError> {
        if self.state != TxSlotState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if self.count == 0 {
            return Err(HtAmpduTxError::TooFewFrames);
        }
        let aggregate = self.calculate_aggregate()?;
        if config.aggregate_length != aggregate.bytes || config.subframes != aggregate.subframes {
            return Err(HtAmpduTxError::RegisterImageMismatch);
        }
        let image = crate::tx::ht_ampdu_q0_image(descriptor_head, config).ok_or(
            HtAmpduTxError::TxImageUnavailable {
                format: HtAmpduTxFormat::HtAmpdu,
            },
        )?;
        Ok(PreparedHtSubmission {
            aggregate,
            program: MacHtTxProgram {
                plcp0: image.plcp0,
                plcp1: image.plcp1,
                ht_signal: image.ht_signal,
                data_length: image.data_length,
                power: image.power,
                length_control: image.length_control,
                descriptor_count_a: image.descriptor_count_a,
                descriptor_count_b: image.descriptor_count_b,
                protection_spacing: image.protection_spacing,
                timeout: config.timeout,
                scheduler_priority: config.scheduler_priority,
                packet_priority: config.pti,
                priority_count: config.pti_count,
                aifsn: config.aifsn,
                contention_window: config.contention_window,
                interface: config.interface,
            },
            plcp0: image.plcp0,
        })
    }

    pub(super) fn prepared_he_submission(
        &self,
        descriptor_head: u32,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HeAmpduTxConfig,
    ) -> Result<PreparedHeSubmission, HtAmpduTxError> {
        if self.state != TxSlotState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if self.count == 0 {
            return Err(HtAmpduTxError::TooFewFrames);
        }
        let aggregate = self.calculate_aggregate()?;
        if config.aggregate_length != aggregate.bytes || config.subframes != aggregate.subframes {
            return Err(HtAmpduTxError::RegisterImageMismatch);
        }
        let trigger = self.prepared_he_trigger(queue, config)?;
        let image = crate::tx::he_ampdu_q0_image(descriptor_head, config).ok_or(
            HtAmpduTxError::TxImageUnavailable {
                format: HtAmpduTxFormat::HeAmpdu,
            },
        )?;
        Ok(PreparedHeSubmission {
            aggregate,
            program: MacHeTxProgram {
                plcp0: image.plcp0,
                plcp1: image.plcp1,
                he_signal_a1: image.he_signal_a1,
                he_signal_a2_length: image.he_signal_a2_length,
                software_he_control: None,
                power: image.power,
                length_control: image.length_control,
                descriptor_count_a: image.descriptor_count_a,
                descriptor_count_b: image.descriptor_count_b,
                protection_spacing: image.protection_spacing,
                timeout: config.timeout,
                scheduler_priority: config.scheduler_priority,
                packet_priority: config.pti,
                priority_count: config.pti_count,
                aifsn: config.aifsn,
                contention_window: config.contention_window,
                interface: config.interface,
            },
            plcp0: image.plcp0,
            trigger,
        })
    }

    pub(super) fn prepared_he_trigger(
        &self,
        queue: LegacyTxQueue,
        config: HeAmpduTxConfig,
    ) -> Result<Option<PreparedHeTrigger>, HtAmpduTxError> {
        let Some(trigger) = config.trigger_based() else {
            return Ok(None);
        };
        let count = self.count;
        let reservation =
            MacHeTbLinkReservation::for_queue(trigger.tid_limit(), queue.index(), count).ok_or(
                HtAmpduTxError::InvalidTriggerReservation {
                    queue: queue.index(),
                    subframes: count,
                },
            )?;
        if self.psdu_lengths[..usize::from(count)]
            .iter()
            .any(|length| *length == 0 || *length > 0x3fff)
        {
            return Err(HtAmpduTxError::TriggerBased(
                MacHeTbProgramError::InvalidMpduLength,
            ));
        }
        let mut queued_msdu_bytes = 0_u32;
        for msdu_length in &self.msdu_lengths[..usize::from(count)] {
            if *msdu_length == 0 {
                return Err(HtAmpduTxError::TriggerMsduLengthUnavailable);
            }
            queued_msdu_bytes = queued_msdu_bytes
                .checked_add(u32::from(*msdu_length))
                .ok_or(HtAmpduTxError::TriggerBased(
                    MacHeTbProgramError::QueuedBytesTooLarge,
                ))?;
        }
        if queued_msdu_bytes > 0x000f_ffff {
            return Err(HtAmpduTxError::TriggerBased(
                MacHeTbProgramError::QueuedBytesTooLarge,
            ));
        }
        Ok(Some(PreparedHeTrigger {
            policy: trigger.tid_limit(),
            reservation,
            tid: trigger.tid(),
            queued_msdu_bytes,
        }))
    }
}
