//! Association epoch, retransmission schedule and terminal response policy.

use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_mac::{
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
    pub sequence_number: SequenceNumber,
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
    /// The access point refused the association temporarily with a comeback
    /// time the station does not wait for: longer than 5000 TUs, or a second
    /// refusal after the station already came back once.
    ComebackRefused {
        comeback_tu: u32,
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
/// SOURCE(esp32s31): complete `libnet80211.a[ieee80211_sta.o]::
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
    /// Milliseconds left until the station comes back after a temporary
    /// refusal; the association deadline and retransmissions pause meanwhile.
    comeback_remaining_ms: u32,
    came_back: bool,
    /// Completed millisecond ticks of the whole epoch, including a comeback
    /// wait; the executor paces its ticks by this count.
    ticks: u32,
}

/// Status of a temporary association refusal: the access point protects the
/// management frames of an association it still holds and first confirms
/// it with an SA Query.
const REJECTED_TEMPORARILY: u16 = 30;
/// The longest comeback time the vendor waits for.
const MAXIMUM_COMEBACK_TU: u32 = 5_000;
/// The vendor comes back this many TUs after the named comeback time.
const COMEBACK_MARGIN_TU: u32 = 100;
const MICROS_PER_TU: u32 = 1_024;

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
            comeback_remaining_ms: 0,
            came_back: false,
            ticks: 0,
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
        if self.comeback_remaining_ms != 0 {
            return Ok(None);
        }
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
        if response.status_code == REJECTED_TEMPORARILY
            && let Some(comeback_tu) = response.association_comeback_tu
        {
            return Ok(self.come_back(comeback_tu));
        }
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
        self.ticks = self.ticks.saturating_add(1);
        if self.comeback_remaining_ms != 0 {
            self.comeback_remaining_ms -= 1;
            return Ok(StaAssociationEvent::Irrelevant);
        }
        self.elapsed_ms = self.elapsed_ms.saturating_add(1);
        if self.elapsed_ms >= STA_RESPONSE_TIMEOUT_MS {
            Ok(self.fail(StaAssociationFailure::Timeout))
        } else {
            Ok(StaAssociationEvent::Irrelevant)
        }
    }

    /// Milliseconds of the current request's deadline.
    pub const fn elapsed_ms(&self) -> u32 {
        self.elapsed_ms
    }

    /// Completed ticks of the whole epoch, including a comeback wait.
    pub const fn ticks(&self) -> u32 {
        self.ticks
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

    /// Wait out one temporary refusal, then send a fresh Association Request
    /// under a fresh deadline, as the vendor's `sta_recv_assoc` does: it
    /// disarms the association timer and arms `sta_assoc_comeback` for the
    /// comeback time plus 100 TUs. A second refusal, or one longer than
    /// 5000 TUs, ends the association.
    ///
    /// SOURCE(esp32s31): complete pinned `libnet80211.a[ieee80211_sta.o]::
    /// sta_recv_assoc` and `sta_assoc_comeback`.
    fn come_back(&mut self, comeback_tu: u32) -> StaAssociationEvent {
        if self.came_back || comeback_tu > MAXIMUM_COMEBACK_TU {
            return self.fail(StaAssociationFailure::ComebackRefused { comeback_tu });
        }
        self.came_back = true;
        self.elapsed_ms = 0;
        self.comeback_remaining_ms =
            ((comeback_tu + COMEBACK_MARGIN_TU) * MICROS_PER_TU).div_ceil(1_000);
        StaAssociationEvent::Irrelevant
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
