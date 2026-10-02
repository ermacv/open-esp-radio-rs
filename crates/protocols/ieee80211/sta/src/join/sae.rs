//! SAE Authentication of a WPA3-Personal association.
//!
//! The station sends one SAE Commit and waits for the access point's Commit;
//! it then sends its Confirm and waits for the access point's Confirm, which
//! authenticates both peers and yields the PMK. An anti-clogging refusal
//! (status 76) makes the station repeat the same commit with the access
//! point's token. There are no retransmissions inside one exchange: the
//! vendor arms a 4000 ms authentication timer for the commit exchange and
//! rearms it for 2000 ms after sending the Confirm, and a timeout ends the
//! attempt.
//!
//! SOURCE(esp32s31): complete pinned `libnet80211.a[ieee80211_sta.o]::sta_auth_sae`
//! and the SAE branch of `ieee80211_sta_new_state`; ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/wpa_supplicant/esp_supplicant/src/esp_wpa3.c`.

use oer_ieee80211_mac::security::SaePwe;
use oer_time::{Duration, Instant};

use crate::time::deadline_after;
use oer_ieee80211_mac::station::{
    SAE_COMMIT_TRANSACTION, SAE_CONFIRM_TRANSACTION, StaDisconnect, parse_sae_authentication,
    parse_sta_disconnect,
};
use oer_ieee80211_rsn::sae::{
    SAE_COMMIT_LEN, SAE_CONFIRM_LEN, SAE_KEY_LEN, SAE_PMKID_LEN, SaeCommit, SaeCommitValues,
    SaeError, SaeKeys, anti_clogging_token,
};

/// Authentication timer of the commit exchange.
pub const STA_SAE_COMMIT_TIMEOUT: Duration = Duration::from_secs(4);
/// Authentication timer rearmed after the station sent its Confirm.
pub const STA_SAE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);
/// The longest anti-clogging token the station repeats.
pub const STA_SAE_TOKEN_CAPACITY: usize = 64;
/// The longest SAE body the station sends: a commit with a token container.
pub const STA_SAE_BODY_CAPACITY: usize = SAE_COMMIT_LEN + 3 + STA_SAE_TOKEN_CAPACITY;

const STATUS_SUCCESS: u16 = 0;
const STATUS_ANTI_CLOGGING_TOKEN_REQUIRED: u16 = 76;
const STATUS_SAE_HASH_TO_ELEMENT: u16 = 126;

/// One SAE Authentication frame body the station sends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaSaeTransmission {
    pub transaction: u16,
    pub status_code: u16,
    body: [u8; STA_SAE_BODY_CAPACITY],
    length: usize,
}

impl StaSaeTransmission {
    pub fn body(&self) -> &[u8] {
        &self.body[..self.length]
    }
}

/// The PMK and PMKID an accepted SAE exchange yields.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct StaSaePmk {
    pub pmk: [u8; SAE_KEY_LEN],
    pub pmkid: [u8; SAE_PMKID_LEN],
}

impl core::fmt::Debug for StaSaePmk {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("StaSaePmk")
    }
}

/// Why one SAE Authentication attempt ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaSaeFailure {
    Timeout,
    PeerDisconnect(StaDisconnect),
    Rejected {
        status_code: u16,
    },
    Protocol(SaeError),
    /// The anti-clogging token exceeds what the station repeats.
    TokenTooLong,
}

/// Result of observing a frame or completing a millisecond.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaSaeEvent {
    Irrelevant,
    Transmit(StaSaeTransmission),
    Authenticated(StaSaePmk),
    Failed(StaSaeFailure),
}

#[expect(
    clippy::large_enum_variant,
    reason = "the allocation-free exchange keeps the derived keys inline"
)]
enum StaSaePhase {
    Committed,
    Confirmed(SaeKeys),
    Terminal,
}

/// The SAE Authentication exchange of one attempt.
// CAPABILITY: wifi-security-wpa3-personal-wpa3-enterprise
pub struct StaSaeAuthentication {
    local: [u8; 6],
    bssid: [u8; 6],
    commit: SaeCommit,
    pwe: SaePwe,
    token: [u8; STA_SAE_TOKEN_CAPACITY],
    token_length: usize,
    phase: StaSaePhase,
    /// When the authentication timer of the current phase expires; `None`
    /// until [`Self::start`] after the commit left.
    deadline: Option<Instant>,
}

impl StaSaeAuthentication {
    /// Start with this station's commit, derived with the password element
    /// method `pwe`.
    pub fn new(local: [u8; 6], bssid: [u8; 6], commit: SaeCommit, pwe: SaePwe) -> Self {
        Self {
            local,
            bssid,
            commit,
            pwe,
            token: [0; STA_SAE_TOKEN_CAPACITY],
            token_length: 0,
            phase: StaSaePhase::Committed,
            deadline: None,
        }
    }

    /// Arm the commit timer once the station's commit left at `now`.
    pub fn start(&mut self, now: Instant) {
        self.deadline = Some(deadline_after(now, STA_SAE_COMMIT_TIMEOUT));
    }

    /// When the authentication timer expires, while the exchange runs.
    pub fn next_deadline(&self) -> Option<Instant> {
        match self.phase {
            StaSaePhase::Terminal => None,
            _ => self.deadline,
        }
    }

    /// Whether the exchange uses the hash-to-element frame format.
    const fn hash_to_element(&self) -> bool {
        matches!(self.pwe, SaePwe::HashToElement)
    }

    /// The commit to send, with the current anti-clogging token.
    pub fn commit(&self) -> StaSaeTransmission {
        let mut body = [0; STA_SAE_BODY_CAPACITY];
        let token = (self.token_length != 0).then_some(&self.token[..self.token_length]);
        let length = self
            .commit
            .values()
            .encode(token, self.hash_to_element(), &mut body)
            .expect("the body capacity holds a commit with the largest token");
        StaSaeTransmission {
            transaction: SAE_COMMIT_TRANSACTION,
            status_code: self.commit_status(),
            body,
            length,
        }
    }

    const fn commit_status(&self) -> u16 {
        if self.hash_to_element() {
            STATUS_SAE_HASH_TO_ELEMENT
        } else {
            STATUS_SUCCESS
        }
    }

    /// Classify one management frame from the access point, received at
    /// `now`.
    pub fn observe_management_frame(&mut self, frame: &[u8], now: Instant) -> StaSaeEvent {
        if matches!(self.phase, StaSaePhase::Terminal) {
            return StaSaeEvent::Irrelevant;
        }
        if let Some(disconnect) = parse_sta_disconnect(frame, self.local, self.bssid) {
            return self.fail(StaSaeFailure::PeerDisconnect(disconnect));
        }
        let Some(frame) = parse_sae_authentication(frame, self.local, self.bssid) else {
            return StaSaeEvent::Irrelevant;
        };
        match (frame.transaction, &self.phase) {
            (SAE_COMMIT_TRANSACTION, StaSaePhase::Committed) => {
                self.receive_commit(frame.status_code, frame.body, now)
            }
            (SAE_CONFIRM_TRANSACTION, StaSaePhase::Confirmed(_)) => {
                self.receive_confirm(frame.status_code, frame.body)
            }
            // A commit after this station confirmed, or a confirm before it
            // committed, is discarded as the vendor discards it.
            _ => StaSaeEvent::Irrelevant,
        }
    }

    fn receive_commit(&mut self, status_code: u16, body: &[u8], now: Instant) -> StaSaeEvent {
        if status_code == STATUS_ANTI_CLOGGING_TOKEN_REQUIRED {
            let token = match anti_clogging_token(body, self.hash_to_element()) {
                Ok(token) => token,
                Err(error) => return self.fail(StaSaeFailure::Protocol(error)),
            };
            if token.len() > STA_SAE_TOKEN_CAPACITY {
                return self.fail(StaSaeFailure::TokenTooLong);
            }
            self.token[..token.len()].copy_from_slice(token);
            self.token_length = token.len();
            return StaSaeEvent::Transmit(self.commit());
        }
        if status_code != self.commit_status() {
            return self.fail(
                if matches!(status_code, STATUS_SUCCESS | STATUS_SAE_HASH_TO_ELEMENT) {
                    StaSaeFailure::Protocol(SaeError::Malformed)
                } else {
                    StaSaeFailure::Rejected { status_code }
                },
            );
        }
        let peer = match SaeCommitValues::parse(body, self.hash_to_element()) {
            Ok(peer) => peer,
            Err(error) => return self.fail(StaSaeFailure::Protocol(error)),
        };
        let keys = match self.commit.process(peer) {
            Ok(keys) => keys,
            // A reflected commit is silently discarded.
            Err(SaeError::Reflection) => return StaSaeEvent::Irrelevant,
            Err(error) => return self.fail(StaSaeFailure::Protocol(error)),
        };
        let confirm = keys.own_confirm(1);
        let mut body = [0; STA_SAE_BODY_CAPACITY];
        body[..SAE_CONFIRM_LEN].copy_from_slice(&confirm);
        self.phase = StaSaePhase::Confirmed(keys);
        self.deadline = Some(deadline_after(now, STA_SAE_CONFIRM_TIMEOUT));
        StaSaeEvent::Transmit(StaSaeTransmission {
            transaction: SAE_CONFIRM_TRANSACTION,
            status_code: STATUS_SUCCESS,
            body,
            length: SAE_CONFIRM_LEN,
        })
    }

    fn receive_confirm(&mut self, status_code: u16, body: &[u8]) -> StaSaeEvent {
        if status_code != STATUS_SUCCESS {
            return self.fail(StaSaeFailure::Rejected { status_code });
        }
        let StaSaePhase::Confirmed(keys) = &self.phase else {
            unreachable!("confirms are received only after this station confirmed");
        };
        match keys.verify_peer_confirm(body) {
            Ok(_) => {
                let pmk = StaSaePmk {
                    pmk: keys.pmk,
                    pmkid: keys.pmkid,
                };
                self.phase = StaSaePhase::Terminal;
                StaSaeEvent::Authenticated(pmk)
            }
            Err(error) => self.fail(StaSaeFailure::Protocol(error)),
        }
    }

    /// Expire the authentication timer when `now` reached its deadline.
    pub fn on_deadline(&mut self, now: Instant) -> StaSaeEvent {
        match self.next_deadline() {
            Some(deadline) if now >= deadline => self.fail(StaSaeFailure::Timeout),
            _ => StaSaeEvent::Irrelevant,
        }
    }

    fn fail(&mut self, failure: StaSaeFailure) -> StaSaeEvent {
        self.phase = StaSaePhase::Terminal;
        StaSaeEvent::Failed(failure)
    }
}

#[cfg(test)]
mod tests;
