//! Association epoch, retransmission schedule and terminal response policy.

use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_mac::{
    security::LinkProtection,
    station::{
        AssociationResponse, StaDisconnect, StaSequenceCounter, parse_association_response,
        parse_sta_disconnect,
    },
};

use oer_time::{Duration, Instant};

use super::STA_RESPONSE_TIMEOUT;
use crate::time::deadline_after;

/// Compatibility schedule for Association retransmission inside the vendor
/// one-second state deadline.
///
/// This 160-ms cadence comes from the hardware-qualified pre-transfer open
/// STA runtime, not from a recovered vendor timer body. It remains explicit so
/// later blob comparison can replace one policy value without changing the
/// executor loop.
pub struct StaAssociationRetrySchedule;

impl StaAssociationRetrySchedule {
    pub const INTERVAL: Duration = Duration::from_millis(160);

    /// When the `ordinal`th request (from one) of an epoch is due, measured
    /// from the epoch's start, while it falls before the epoch's deadline.
    pub const fn offset(ordinal: u16) -> Option<Duration> {
        if ordinal == 0 {
            return None;
        }
        let micros = (ordinal as u64 - 1) * Self::INTERVAL.as_micros();
        if micros < STA_RESPONSE_TIMEOUT.as_micros() {
            Some(Duration::from_micros(micros))
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
    /// When the request was due, from the start of its epoch.
    pub offset: Duration,
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

/// Result of observing a management frame.
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
    Terminal,
}

/// What the association asks of its executor at one instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationPoll {
    Idle,
    /// Send this Association Request now.
    Transmit(StaAssociationAttempt),
    Failed {
        failure: StaAssociationFailure,
        total_received_frames: u32,
    },
}

/// Allocation-free owner of one ordinary STA Association epoch.
///
/// A target executor polls it with the time, transmits the request it asks
/// for, reports every completed RX descriptor and supplies extracted
/// management frames, and waits until [`Self::next_deadline`] or the next
/// frame. This type owns the one-second deadline, retransmission cadence,
/// management sequence consumption and terminal response policy; it does not
/// own timers, DMA or MAC registers.
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
    security: LinkProtection,
    /// Start of the current request epoch: `None` before the first poll and
    /// while the station waits out a temporary refusal.
    epoch_start: Option<Instant>,
    /// Requests sent in the current epoch.
    sent: u16,
    terminal: bool,
    received_frames: u32,
    /// When the station comes back after a temporary refusal; the epoch's
    /// deadline and retransmissions pause meanwhile.
    comeback_until: Option<Instant>,
    came_back: bool,
}

/// Status of a temporary association refusal: the access point protects the
/// management frames of an association it still holds and first confirms
/// it with an SA Query.
const REJECTED_TEMPORARILY: u16 = 30;
/// The longest comeback time the vendor waits for.
const MAXIMUM_COMEBACK_TU: u32 = 5_000;
/// The vendor comes back this many TUs after the named comeback time.
const COMEBACK_MARGIN_TU: u32 = 100;
const MICROS_PER_TU: u64 = 1_024;

impl StaAssociationRuntime {
    pub const fn new(local: [u8; 6], bssid: [u8; 6], security: LinkProtection) -> Self {
        Self {
            local,
            bssid,
            security,
            epoch_start: None,
            sent: 0,
            terminal: false,
            received_frames: 0,
            comeback_until: None,
            came_back: false,
        }
    }

    /// Advance to `now`: start the epoch on its first poll, expire its
    /// one-second deadline, or consume a management sequence number exactly
    /// when the retry schedule calls for a new request.
    pub fn poll(
        &mut self,
        now: Instant,
        sequence: &mut StaSequenceCounter,
    ) -> Result<StaAssociationPoll, StaAssociationRuntimeError> {
        if self.terminal {
            return Err(StaAssociationRuntimeError::Terminal);
        }
        if let Some(until) = self.comeback_until {
            if now < until {
                return Ok(StaAssociationPoll::Idle);
            }
            self.comeback_until = None;
        }
        let start = *self.epoch_start.get_or_insert(now);
        if now >= deadline_after(start, STA_RESPONSE_TIMEOUT) {
            self.terminal = true;
            return Ok(StaAssociationPoll::Failed {
                failure: StaAssociationFailure::Timeout,
                total_received_frames: self.received_frames,
            });
        }
        let ordinal = self.sent + 1;
        match StaAssociationRetrySchedule::offset(ordinal) {
            Some(offset) if now >= deadline_after(start, offset) => {
                self.sent = ordinal;
                Ok(StaAssociationPoll::Transmit(StaAssociationAttempt {
                    ordinal,
                    sequence_number: sequence.take(),
                    offset,
                }))
            }
            _ => Ok(StaAssociationPoll::Idle),
        }
    }

    /// When the association next needs a poll: the end of a comeback wait,
    /// the next scheduled request or the epoch's deadline.
    pub fn next_deadline(&self) -> Option<Instant> {
        if self.terminal {
            return None;
        }
        if self.comeback_until.is_some() {
            return self.comeback_until;
        }
        let start = self.epoch_start?;
        let timeout = deadline_after(start, STA_RESPONSE_TIMEOUT);
        Some(
            StaAssociationRetrySchedule::offset(self.sent + 1)
                .map_or(timeout, |offset| deadline_after(start, offset).min(timeout)),
        )
    }

    /// Account for one completed RX descriptor, including a frame which is
    /// not a valid management input.
    pub fn observe_received_frame(&mut self) -> Result<(), StaAssociationRuntimeError> {
        self.require_running()?;
        self.received_frames = self.received_frames.saturating_add(1);
        Ok(())
    }

    /// Classify one extracted management frame for the selected peer,
    /// received at `now`.
    pub fn observe_management_frame(
        &mut self,
        frame: &[u8],
        now: Instant,
    ) -> Result<StaAssociationEvent, StaAssociationRuntimeError> {
        self.require_running()?;
        if let Some(disconnect) = parse_sta_disconnect(frame, self.local, self.bssid) {
            return Ok(self.fail(StaAssociationFailure::PeerDisconnect(disconnect)));
        }
        let Some(response) = parse_association_response(frame, self.local, self.bssid) else {
            return Ok(StaAssociationEvent::Irrelevant);
        };
        if response.status_code == REJECTED_TEMPORARILY
            && let Some(comeback_tu) = response.association_comeback_tu
        {
            return Ok(self.come_back(comeback_tu, now));
        }
        if response.status_code != 0 {
            return Ok(self.fail(StaAssociationFailure::Rejected {
                status_code: response.status_code,
            }));
        }
        if !response.matches_security(self.security) {
            return Ok(self.fail(StaAssociationFailure::SecurityModeMismatch));
        }
        self.terminal = true;
        Ok(StaAssociationEvent::Associated {
            response,
            total_received_frames: self.received_frames,
        })
    }

    pub const fn total_received_frames(&self) -> u32 {
        self.received_frames
    }

    fn require_running(&self) -> Result<(), StaAssociationRuntimeError> {
        if self.terminal {
            Err(StaAssociationRuntimeError::Terminal)
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
    fn come_back(&mut self, comeback_tu: u32, now: Instant) -> StaAssociationEvent {
        if self.came_back || comeback_tu > MAXIMUM_COMEBACK_TU {
            return self.fail(StaAssociationFailure::ComebackRefused { comeback_tu });
        }
        self.came_back = true;
        self.epoch_start = None;
        self.sent = 0;
        let wait = Duration::from_micros((comeback_tu + COMEBACK_MARGIN_TU) as u64 * MICROS_PER_TU);
        self.comeback_until = Some(deadline_after(now, wait));
        StaAssociationEvent::Irrelevant
    }

    fn fail(&mut self, failure: StaAssociationFailure) -> StaAssociationEvent {
        self.terminal = true;
        StaAssociationEvent::Failed {
            failure,
            total_received_frames: self.received_frames,
        }
    }
}

#[cfg(test)]
mod tests;
