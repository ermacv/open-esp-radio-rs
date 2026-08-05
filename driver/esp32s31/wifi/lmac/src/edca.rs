//! Owned EDCA contention state for the four ordinary TX queues.

use open_esp_radio_ieee80211::wmm::{WmmAcParameters, WmmAccessCategory, WmmParameterSet};

use crate::tx::LegacyTxQueue;

/// The S31 ordinary-TX queue stores the selected backoff as a ten-bit slot
/// count, so an exponent above ten cannot be represented without truncation.
pub const MAX_HARDWARE_ECW_EXPONENT: u8 = 10;

/// Invalid contention parameters that cannot be represented by the S31 queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdcaParametersError {
    AifsnOutOfRange(u8),
    MinimumExponentOutOfRange(u8),
    MaximumExponentOutOfRange(u8),
    InvertedExponentRange { minimum: u8, maximum: u8 },
}

/// One validated AIFS/contention-window policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaContentionParameters {
    aifsn: u8,
    minimum_exponent: u8,
    maximum_exponent: u8,
}

impl EdcaContentionParameters {
    pub const fn new(
        aifsn: u8,
        minimum_exponent: u8,
        maximum_exponent: u8,
    ) -> Result<Self, EdcaParametersError> {
        if aifsn > 0x0f {
            return Err(EdcaParametersError::AifsnOutOfRange(aifsn));
        }
        if minimum_exponent > MAX_HARDWARE_ECW_EXPONENT {
            return Err(EdcaParametersError::MinimumExponentOutOfRange(
                minimum_exponent,
            ));
        }
        if maximum_exponent > MAX_HARDWARE_ECW_EXPONENT {
            return Err(EdcaParametersError::MaximumExponentOutOfRange(
                maximum_exponent,
            ));
        }
        if minimum_exponent > maximum_exponent {
            return Err(EdcaParametersError::InvertedExponentRange {
                minimum: minimum_exponent,
                maximum: maximum_exponent,
            });
        }
        Ok(Self {
            aifsn,
            minimum_exponent,
            maximum_exponent,
        })
    }

    /// Convert a parsed WMM AC Parameter Record without retaining its
    /// independent admission-control or TXOP policy.
    pub const fn from_wmm(parameters: WmmAcParameters) -> Result<Self, EdcaParametersError> {
        Self::new(parameters.aifsn, parameters.ecw_min, parameters.ecw_max)
    }

    pub const fn aifsn(self) -> u8 {
        self.aifsn
    }

    pub const fn minimum_exponent(self) -> u8 {
        self.minimum_exponent
    }

    pub const fn maximum_exponent(self) -> u8 {
        self.maximum_exponent
    }

    const fn vendor_default(queue: LegacyTxQueue) -> Self {
        // SOURCE: complete `libpp.a[lmac.o]::lmacInit` and
        // `lmacInitAc`. The five arguments are queue, AIFSN, ECWmin, ECWmax,
        // and TXOP. Ordinary queues are VO=(2,2,3), VI=(2,3,4),
        // BE=(3,4,10), and BK=(7,4,10).
        match queue {
            LegacyTxQueue::Voice => Self {
                aifsn: 2,
                minimum_exponent: 2,
                maximum_exponent: 3,
            },
            LegacyTxQueue::Video => Self {
                aifsn: 2,
                minimum_exponent: 3,
                maximum_exponent: 4,
            },
            LegacyTxQueue::BestEffort => Self {
                aifsn: 3,
                minimum_exponent: 4,
                maximum_exponent: 10,
            },
            LegacyTxQueue::Background => Self {
                aifsn: 7,
                minimum_exponent: 4,
                maximum_exponent: 10,
            },
        }
    }
}

/// Runtime contention state for one access category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaBackoffState {
    parameters: EdcaContentionParameters,
    current_exponent: u8,
}

impl EdcaBackoffState {
    pub const fn new(parameters: EdcaContentionParameters) -> Self {
        Self {
            parameters,
            current_exponent: parameters.minimum_exponent,
        }
    }

    pub const fn parameters(self) -> EdcaContentionParameters {
        self.parameters
    }

    pub const fn current_exponent(self) -> u8 {
        self.current_exponent
    }

    /// Install a new parameter set while retaining a still-valid current CW.
    pub fn reconfigure(&mut self, parameters: EdcaContentionParameters) {
        // SOURCE: complete `libpp.a[lmac.o]::lmacSetAcParam`.
        // It replaces AIFSN/min/max, clamps current down to a lower new max,
        // clamps it up to a higher new min, and otherwise retains it.
        self.parameters = parameters;
        self.current_exponent = self
            .current_exponent
            .clamp(parameters.minimum_exponent, parameters.maximum_exponent);
    }

    /// Select the hardware slot count from caller-supplied entropy.
    pub const fn select_slot(self, entropy: u32) -> u16 {
        // SOURCE: complete `libpp.a[lmac.o]::lmacTxFrame`
        // +0x10e..0x12c calls `hal_random`, masks it with
        // `(1 << current_exponent) - 1`, stores the u16 result at AC+0x06,
        // and passes it to `hal_mac_tx_config_edca`.
        let mask = (1_u32 << self.current_exponent) - 1;
        (entropy & mask) as u16
    }

    /// Advance the current CW after an attempt that will be retried.
    pub fn record_retry_failure(&mut self) {
        // SOURCE: complete `libpp.a[lmac.o]::
        // {lmacProcessLongRetryFail,lmacProcessShortRetryFail}`. Both raise
        // AC+0x08 by one while it is below the active maximum.
        if self.current_exponent < self.parameters.maximum_exponent {
            self.current_exponent += 1;
        }
    }

    /// Reset contention after a successful frame exchange.
    pub fn record_success(&mut self) {
        // SOURCE: complete `libpp.a[lmac.o]::
        // {lmacProcessLongFrameSuccess,lmacProcessShortFrameSuccess}`. Both
        // copy AC+0x09 (ECWmin) to AC+0x08 (current exponent).
        self.current_exponent = self.parameters.minimum_exponent;
    }

    /// Reset a terminal exchange before a new MSDU starts.
    pub fn reset_terminal_exchange(&mut self) {
        // SOURCE: the retry-limit branches in complete
        // `libpp.a[lmac.o]::
        // {lmacProcessLongRetryFail,lmacProcessShortRetryFail}` restore the
        // active minimum before discarding or completing the exchange.
        self.current_exponent = self.parameters.minimum_exponent;
    }
}

/// Owned contention state for all four ordinary hardware queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaQueues {
    queues: [EdcaBackoffState; 4],
}

impl EdcaQueues {
    pub const fn vendor_defaults() -> Self {
        Self {
            queues: [
                EdcaBackoffState::new(EdcaContentionParameters::vendor_default(
                    LegacyTxQueue::Voice,
                )),
                EdcaBackoffState::new(EdcaContentionParameters::vendor_default(
                    LegacyTxQueue::Video,
                )),
                EdcaBackoffState::new(EdcaContentionParameters::vendor_default(
                    LegacyTxQueue::BestEffort,
                )),
                EdcaBackoffState::new(EdcaContentionParameters::vendor_default(
                    LegacyTxQueue::Background,
                )),
            ],
        }
    }

    pub const fn queue(&self, queue: LegacyTxQueue) -> &EdcaBackoffState {
        &self.queues[queue as usize]
    }

    pub fn select_slot(&self, queue: LegacyTxQueue, entropy: u32) -> u16 {
        self.queue(queue).select_slot(entropy)
    }

    pub fn record_retry_failure(&mut self, queue: LegacyTxQueue) {
        self.queues[queue as usize].record_retry_failure();
    }

    pub fn record_success(&mut self, queue: LegacyTxQueue) {
        self.queues[queue as usize].record_success();
    }

    pub fn reset_terminal_exchange(&mut self, queue: LegacyTxQueue) {
        self.queues[queue as usize].reset_terminal_exchange();
    }

    /// Atomically validate and install all four WMM AC records.
    pub fn configure_from_wmm(
        &mut self,
        parameters: WmmParameterSet,
    ) -> Result<(), EdcaParametersError> {
        let voice = EdcaContentionParameters::from_wmm(
            parameters.access_category(WmmAccessCategory::Voice),
        )?;
        let video = EdcaContentionParameters::from_wmm(
            parameters.access_category(WmmAccessCategory::Video),
        )?;
        let best_effort = EdcaContentionParameters::from_wmm(
            parameters.access_category(WmmAccessCategory::BestEffort),
        )?;
        let background = EdcaContentionParameters::from_wmm(
            parameters.access_category(WmmAccessCategory::Background),
        )?;

        self.queues[LegacyTxQueue::Voice as usize].reconfigure(voice);
        self.queues[LegacyTxQueue::Video as usize].reconfigure(video);
        self.queues[LegacyTxQueue::BestEffort as usize].reconfigure(best_effort);
        self.queues[LegacyTxQueue::Background as usize].reconfigure(background);
        Ok(())
    }
}

impl Default for EdcaQueues {
    fn default() -> Self {
        Self::vendor_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EdcaBackoffState, EdcaContentionParameters, EdcaParametersError, EdcaQueues,
        MAX_HARDWARE_ECW_EXPONENT,
    };
    use crate::tx::LegacyTxQueue;
    use open_esp_radio_ieee80211::wmm::parse_wmm_parameter_element;

    const STANDARD_WMM: [u8; 26] = [
        221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42,
        0x43, 94, 0, 0x72, 0x32, 47, 0,
    ];

    #[test]
    fn vendor_defaults_match_all_four_lmac_init_records() {
        let queues = EdcaQueues::vendor_defaults();
        assert_eq!(
            queues.queue(LegacyTxQueue::Voice).parameters(),
            EdcaContentionParameters::new(2, 2, 3).unwrap()
        );
        assert_eq!(
            queues.queue(LegacyTxQueue::Video).parameters(),
            EdcaContentionParameters::new(2, 3, 4).unwrap()
        );
        assert_eq!(
            queues.queue(LegacyTxQueue::BestEffort).parameters(),
            EdcaContentionParameters::new(3, 4, 10).unwrap()
        );
        assert_eq!(
            queues.queue(LegacyTxQueue::Background).parameters(),
            EdcaContentionParameters::new(7, 4, 10).unwrap()
        );
    }

    #[test]
    fn retry_expands_cw_to_maximum_and_success_restores_minimum() {
        let mut state = EdcaBackoffState::new(EdcaContentionParameters::new(3, 4, 6).unwrap());
        assert_eq!(state.select_slot(u32::MAX), 15);
        state.record_retry_failure();
        assert_eq!(state.current_exponent(), 5);
        assert_eq!(state.select_slot(u32::MAX), 31);
        state.record_retry_failure();
        state.record_retry_failure();
        assert_eq!(state.current_exponent(), 6);
        assert_eq!(state.select_slot(u32::MAX), 63);
        state.record_success();
        assert_eq!(state.current_exponent(), 4);
    }

    #[test]
    fn reconfigure_clamps_current_to_both_new_bounds() {
        let mut state = EdcaBackoffState::new(EdcaContentionParameters::new(3, 4, 10).unwrap());
        state.record_retry_failure();
        state.record_retry_failure();
        assert_eq!(state.current_exponent(), 6);
        state.reconfigure(EdcaContentionParameters::new(2, 2, 4).unwrap());
        assert_eq!(state.current_exponent(), 4);
        state.reconfigure(EdcaContentionParameters::new(2, 7, 9).unwrap());
        assert_eq!(state.current_exponent(), 7);
    }

    #[test]
    fn wmm_update_is_validated_before_any_queue_changes() {
        let mut queues = EdcaQueues::vendor_defaults();
        let before = queues;
        let mut invalid = STANDARD_WMM;
        invalid[11] = (11 << 4) | 4;
        let parameters = parse_wmm_parameter_element(&invalid).unwrap();
        assert_eq!(
            queues.configure_from_wmm(parameters),
            Err(EdcaParametersError::MaximumExponentOutOfRange(11))
        );
        assert_eq!(queues, before);

        queues
            .configure_from_wmm(parse_wmm_parameter_element(&STANDARD_WMM).unwrap())
            .unwrap();
        assert_eq!(
            queues.queue(LegacyTxQueue::BestEffort).parameters(),
            EdcaContentionParameters::new(3, 4, 10).unwrap()
        );
    }

    #[test]
    fn rejects_values_wider_than_the_queue_slot_field() {
        assert_eq!(MAX_HARDWARE_ECW_EXPONENT, 10);
        assert_eq!(
            EdcaContentionParameters::new(3, 4, 11),
            Err(EdcaParametersError::MaximumExponentOutOfRange(11))
        );
        assert_eq!(
            EdcaContentionParameters::new(3, 5, 4),
            Err(EdcaParametersError::InvertedExponentRange {
                minimum: 5,
                maximum: 4,
            })
        );
    }
}
