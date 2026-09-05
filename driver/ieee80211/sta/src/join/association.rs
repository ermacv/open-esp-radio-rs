//! Association epoch, retransmission schedule and terminal response policy.

use open_esp_radio_ieee80211::{
    security::WifiSecurityMode,
    station::{
        AssociationResponse, StaDisconnect, StaSequenceCounter, parse_association_response,
        parse_sta_disconnect,
    },
};

use super::STA_RESPONSE_TIMEOUT_MS;

/// Compatibility schedule for Association retransmission inside the vendor
/// one-second state deadline.
///
/// This 160-ms cadence comes from the hardware-qualified pre-transfer open
/// STA runtime, not from a recovered vendor timer body. It remains explicit so
/// later blob comparison can replace one policy value without changing the
/// executor loop.
pub struct StaAssociationRetrySchedule;

impl StaAssociationRetrySchedule {
    pub const INTERVAL_MS: u32 = 160;

    pub const fn attempt_at(elapsed_ms: u32) -> Option<u16> {
        if elapsed_ms < STA_RESPONSE_TIMEOUT_MS && elapsed_ms.is_multiple_of(Self::INTERVAL_MS) {
            Some((elapsed_ms / Self::INTERVAL_MS + 1) as u16)
        } else {
            None
        }
    }
}

/// One uniquely numbered Association transmission inside a state epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociationAttempt {
    pub ordinal: u16,
    pub sequence_number: u16,
    pub elapsed_ms: u32,
}

/// Protocol-level reason why an Association epoch ended unsuccessfully.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationFailure {
    Timeout,
    PeerDisconnect(StaDisconnect),
    Rejected {
        status_code: u16,
    },
    /// A successful response contradicted the exact security mode selected
    /// from the scan record. Treating this as association success would make
    /// the following key/plaintext transition an implicit downgrade.
    SecurityModeMismatch,
}

/// Result of observing a management frame or completing one millisecond tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationEvent {
    Irrelevant,
    Associated {
        response: AssociationResponse,
        total_received_frames: u32,
    },
    Failed {
        failure: StaAssociationFailure,
        total_received_frames: u32,
    },
}

/// Invalid executor interaction with [`StaAssociationRuntime`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationRuntimeError {
    TickAlreadyActive,
    NoActiveTick,
    Terminal,
}

/// Allocation-free owner of one ordinary STA Association epoch.
///
/// A target executor begins one tick, optionally transmits the returned
/// attempt, reports every completed RX descriptor, supplies extracted
/// management frames, then finishes the tick. This type owns the one-second
/// deadline, retransmission cadence, management sequence consumption and
/// terminal response policy; it does not own timers, DMA or MAC registers.
///
/// SOURCE: complete `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state` Association branch arms the 1,000-ms state timer.
/// The 160-ms retransmission cadence is the hardware-qualified open STA policy
/// previously owned by the ESP32-S31 HIL and remains isolated in
/// [`StaAssociationRetrySchedule`] pending recovery of the vendor timer body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociationRuntime {
    local: [u8; 6],
    bssid: [u8; 6],
    security: WifiSecurityMode,
    elapsed_ms: u32,
    tick_active: bool,
    terminal: bool,
    received_frames: u32,
}

impl StaAssociationRuntime {
    pub const fn new(local: [u8; 6], bssid: [u8; 6], security: WifiSecurityMode) -> Self {
        Self {
            local,
            bssid,
            security,
            elapsed_ms: 0,
            tick_active: false,
            terminal: false,
            received_frames: 0,
        }
    }

    /// Begin the current millisecond tick and consume a management sequence
    /// number exactly when the retry schedule calls for a new MPDU.
    pub fn begin_tick(
        &mut self,
        sequence: &mut StaSequenceCounter,
    ) -> Result<Option<StaAssociationAttempt>, StaAssociationRuntimeError> {
        if self.terminal || self.elapsed_ms >= STA_RESPONSE_TIMEOUT_MS {
            return Err(StaAssociationRuntimeError::Terminal);
        }
        if self.tick_active {
            return Err(StaAssociationRuntimeError::TickAlreadyActive);
        }
        self.tick_active = true;
        Ok(
            StaAssociationRetrySchedule::attempt_at(self.elapsed_ms).map(|ordinal| {
                StaAssociationAttempt {
                    ordinal,
                    sequence_number: sequence.take(),
                    elapsed_ms: self.elapsed_ms,
                }
            }),
        )
    }

    /// Account for one completed RX descriptor, including a frame which is
    /// not a valid management input.
    pub fn observe_received_frame(&mut self) -> Result<(), StaAssociationRuntimeError> {
        self.require_active_tick()?;
        self.received_frames = self.received_frames.saturating_add(1);
        Ok(())
    }

    /// Classify one extracted management frame for the selected peer.
    pub fn observe_management_frame(
        &mut self,
        frame: &[u8],
    ) -> Result<StaAssociationEvent, StaAssociationRuntimeError> {
        self.require_active_tick()?;
        if let Some(disconnect) = parse_sta_disconnect(frame, self.local, self.bssid) {
            return Ok(self.fail(StaAssociationFailure::PeerDisconnect(disconnect)));
        }
        let Some(response) = parse_association_response(frame, self.local, self.bssid) else {
            return Ok(StaAssociationEvent::Irrelevant);
        };
        if response.status_code != 0 {
            return Ok(self.fail(StaAssociationFailure::Rejected {
                status_code: response.status_code,
            }));
        }
        if !response.matches_security(self.security) {
            return Ok(self.fail(StaAssociationFailure::SecurityModeMismatch));
        }
        self.tick_active = false;
        self.terminal = true;
        Ok(StaAssociationEvent::Associated {
            response,
            total_received_frames: self.received_frames,
        })
    }

    /// Complete the current millisecond tick and expire the complete vendor
    /// state deadline after exactly 1,000 ticks.
    pub fn finish_tick(&mut self) -> Result<StaAssociationEvent, StaAssociationRuntimeError> {
        self.require_active_tick()?;
        self.tick_active = false;
        self.elapsed_ms = self.elapsed_ms.saturating_add(1);
        if self.elapsed_ms >= STA_RESPONSE_TIMEOUT_MS {
            Ok(self.fail(StaAssociationFailure::Timeout))
        } else {
            Ok(StaAssociationEvent::Irrelevant)
        }
    }

    pub const fn elapsed_ms(&self) -> u32 {
        self.elapsed_ms
    }

    pub const fn total_received_frames(&self) -> u32 {
        self.received_frames
    }

    fn require_active_tick(&self) -> Result<(), StaAssociationRuntimeError> {
        if self.terminal {
            Err(StaAssociationRuntimeError::Terminal)
        } else if !self.tick_active {
            Err(StaAssociationRuntimeError::NoActiveTick)
        } else {
            Ok(())
        }
    }

    fn fail(&mut self, failure: StaAssociationFailure) -> StaAssociationEvent {
        self.tick_active = false;
        self.terminal = true;
        StaAssociationEvent::Failed {
            failure,
            total_received_frames: self.received_frames,
        }
    }
}

#[cfg(test)]
mod tests;
