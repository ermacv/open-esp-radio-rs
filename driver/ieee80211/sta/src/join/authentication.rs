//! Open Authentication attempts, deadline results and retry ownership.

use open_esp_radio_ieee80211::station::{
    StaDisconnect, StaSequenceCounter, parse_open_authentication_response, parse_sta_disconnect,
};

use super::STA_RESPONSE_TIMEOUT_MS;

/// Bounded open Authentication attempts retained by the qualified STA path.
pub const STA_AUTHENTICATION_ATTEMPT_LIMIT: u16 = 3;

/// One uniquely numbered Open Authentication transmission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAuthenticationAttempt {
    pub ordinal: u16,
    pub sequence_number: u16,
    pub response_timeout_ms: u32,
}

/// Protocol-level reason why one Authentication attempt ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAuthenticationFailure {
    Timeout,
    PeerDisconnect(StaDisconnect),
    Rejected { status_code: u16 },
}

/// Result of observing a management frame or expiring an attempt deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAuthenticationEvent {
    Irrelevant,
    Authenticated {
        attempt: u16,
        total_received_frames: u32,
    },
    Retry {
        attempt: u16,
        failure: StaAuthenticationFailure,
        received_frames: u32,
        total_received_frames: u32,
    },
    Failed {
        attempts: u16,
        failure: StaAuthenticationFailure,
        total_received_frames: u32,
    },
}

/// Invalid executor interaction with [`StaAuthenticationRuntime`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAuthenticationRuntimeError {
    AttemptAlreadyActive,
    NoActiveAttempt,
    Terminal,
}

/// Allocation-free owner of the ordinary Open Authentication retry epoch.
///
/// This type owns protocol policy and state only. A target executor arms RX,
/// submits the returned sequence number, reports every received descriptor,
/// and supplies extracted management frames. It therefore remains independent
/// of Embassy, DMA layout and the ESP32-S31 MAC.
///
/// SOURCE: complete `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state` ordinary Authentication branch arms the 1,000-ms
/// state timer. The three-attempt bound is the hardware-qualified open STA
/// policy previously owned by the ESP32-S31 HIL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAuthenticationRuntime {
    local: [u8; 6],
    bssid: [u8; 6],
    attempts_started: u16,
    active: bool,
    terminal: bool,
    received_frames: u32,
    total_received_frames: u32,
}

impl StaAuthenticationRuntime {
    pub const fn new(local: [u8; 6], bssid: [u8; 6]) -> Self {
        Self {
            local,
            bssid,
            attempts_started: 0,
            active: false,
            terminal: false,
            received_frames: 0,
            total_received_frames: 0,
        }
    }

    /// Start the next bounded attempt and consume exactly one management
    /// sequence number. Hardware retransmission of the encoded request does
    /// not call this method again.
    pub fn begin_attempt(
        &mut self,
        sequence: &mut StaSequenceCounter,
    ) -> Result<StaAuthenticationAttempt, StaAuthenticationRuntimeError> {
        if self.terminal || self.attempts_started >= STA_AUTHENTICATION_ATTEMPT_LIMIT {
            return Err(StaAuthenticationRuntimeError::Terminal);
        }
        if self.active {
            return Err(StaAuthenticationRuntimeError::AttemptAlreadyActive);
        }
        self.attempts_started += 1;
        self.active = true;
        self.received_frames = 0;
        Ok(StaAuthenticationAttempt {
            ordinal: self.attempts_started,
            sequence_number: sequence.take(),
            response_timeout_ms: STA_RESPONSE_TIMEOUT_MS,
        })
    }

    /// Account for one completed RX descriptor, including a frame which is
    /// not a valid management input. This preserves the diagnostic count while
    /// keeping frame parsing separately typed.
    pub fn observe_received_frame(&mut self) -> Result<(), StaAuthenticationRuntimeError> {
        if !self.active {
            return Err(StaAuthenticationRuntimeError::NoActiveAttempt);
        }
        self.received_frames = self.received_frames.saturating_add(1);
        Ok(())
    }

    /// Classify one extracted management frame for the active peer exchange.
    pub fn observe_management_frame(
        &mut self,
        frame: &[u8],
    ) -> Result<StaAuthenticationEvent, StaAuthenticationRuntimeError> {
        if !self.active {
            return Err(StaAuthenticationRuntimeError::NoActiveAttempt);
        }
        if let Some(disconnect) = parse_sta_disconnect(frame, self.local, self.bssid) {
            return Ok(self.finish_retryable(StaAuthenticationFailure::PeerDisconnect(disconnect)));
        }
        let Some(response) = parse_open_authentication_response(frame, self.local, self.bssid)
        else {
            return Ok(StaAuthenticationEvent::Irrelevant);
        };
        self.finish_attempt();
        self.terminal = true;
        if response.status_code == 0 {
            Ok(StaAuthenticationEvent::Authenticated {
                attempt: self.attempts_started,
                total_received_frames: self.total_received_frames,
            })
        } else {
            Ok(StaAuthenticationEvent::Failed {
                attempts: self.attempts_started,
                failure: StaAuthenticationFailure::Rejected {
                    status_code: response.status_code,
                },
                total_received_frames: self.total_received_frames,
            })
        }
    }

    /// Expire the complete vendor state deadline for the active attempt.
    pub fn response_timed_out(
        &mut self,
    ) -> Result<StaAuthenticationEvent, StaAuthenticationRuntimeError> {
        if !self.active {
            return Err(StaAuthenticationRuntimeError::NoActiveAttempt);
        }
        Ok(self.finish_retryable(StaAuthenticationFailure::Timeout))
    }

    pub const fn total_received_frames(&self) -> u32 {
        self.total_received_frames
            .saturating_add(self.received_frames)
    }

    pub const fn active_received_frames(&self) -> u32 {
        self.received_frames
    }

    fn finish_retryable(&mut self, failure: StaAuthenticationFailure) -> StaAuthenticationEvent {
        let received_frames = self.received_frames;
        self.finish_attempt();
        if self.attempts_started < STA_AUTHENTICATION_ATTEMPT_LIMIT {
            StaAuthenticationEvent::Retry {
                attempt: self.attempts_started,
                failure,
                received_frames,
                total_received_frames: self.total_received_frames,
            }
        } else {
            self.terminal = true;
            StaAuthenticationEvent::Failed {
                attempts: self.attempts_started,
                failure,
                total_received_frames: self.total_received_frames,
            }
        }
    }

    fn finish_attempt(&mut self) {
        self.total_received_frames = self
            .total_received_frames
            .saturating_add(self.received_frames);
        self.received_frames = 0;
        self.active = false;
    }
}

#[cfg(test)]
mod tests;
