//! Pure shared-PHY client-set and PLL-tracking scheduler model.
//!
//! The reviewed ESP-IDF software protocol stores Wi-Fi, Bluetooth, and IEEE
//! 802.15.4 as independent bits. It gives Wi-Fi one tracking timestamp and
//! Bluetooth plus IEEE 802.15.4 a shared timestamp. This module reproduces only
//! those software decisions. It performs no MMIO, does not arm a real timer,
//! and emits an opaque [`PhyParamTrackRequest`] instead of claiming that the
//! vendor-opaque PLL adjustment completed.
//!
//! Timer arm/stop and the PHY lock are represented only as atomic model facts.
//! There is no fallible timer executor, rollback protocol, or target lock in
//! this module, so the model is deliberately not connectable to hardware yet.
//! The public source samples its timer separately during the due checks and
//! timestamp refreshes; this pure model intentionally accepts one atomic
//! `now_micros` snapshot. It therefore models ordering and client-set decisions,
//! not instruction-level timing equivalence at a period boundary.

#![allow(
    dead_code,
    reason = "the closed model awaits a fallible target executor and PHY-lock binding"
)]

use core::fmt;

const WIFI_BIT: u8 = 1;
const BLUETOOTH_BIT: u8 = 2;
const IEEE802154_BIT: u8 = 4;
const VALID_CLIENT_BITS: u8 = WIFI_BIT | BLUETOOTH_BIT | IEEE802154_BIT;

/// Source-reviewed default periodic PLL-tracking interval.
pub const DEFAULT_PLL_TRACK_PERIOD_MICROS: u64 = 1_000_000;

/// One typed user of the shared PHY software client set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyModemClient {
    Wifi,
    Bluetooth,
    Ieee802154,
}

impl PhyModemClient {
    const fn bit(self) -> u8 {
        match self {
            Self::Wifi => WIFI_BIT,
            Self::Bluetooth => BLUETOOTH_BIT,
            Self::Ieee802154 => IEEE802154_BIT,
        }
    }
}

/// Tracking timestamp class selected by the reviewed scheduler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPllTrackClass {
    Wifi,
    BluetoothIeee802154,
}

/// Copyable observation of software state without a raw-mask mutation path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyClientSnapshot {
    wifi: bool,
    bluetooth: bool,
    ieee802154: bool,
    tracker_model_armed: bool,
    wifi_previous_micros: u64,
    bluetooth_ieee802154_previous_micros: u64,
    period_micros: u64,
}

impl PhyClientSnapshot {
    pub const fn contains(self, client: PhyModemClient) -> bool {
        match client {
            PhyModemClient::Wifi => self.wifi,
            PhyModemClient::Bluetooth => self.bluetooth,
            PhyModemClient::Ieee802154 => self.ieee802154,
        }
    }

    pub const fn is_empty(self) -> bool {
        !self.wifi && !self.bluetooth && !self.ieee802154
    }

    /// Whether the pure model carries the first-client timer obligation.
    ///
    /// This is not evidence that a target timer was created or started.
    pub const fn tracker_model_armed(self) -> bool {
        self.tracker_model_armed
    }

    pub const fn previous_micros(self, class: PhyPllTrackClass) -> u64 {
        match class {
            PhyPllTrackClass::Wifi => self.wifi_previous_micros,
            PhyPllTrackClass::BluetoothIeee802154 => self.bluetooth_ieee802154_previous_micros,
        }
    }

    pub const fn period_micros(self) -> u64 {
        self.period_micros
    }
}

/// Unique owner of the source-reviewed software client and scheduler state.
///
/// This type is deliberately neither `Clone` nor `Copy`. Its private bit image
/// cannot be supplied or changed by safe callers. It is not a hardware lease or
/// an RF/PLL readiness proof.
#[must_use = "the PHY client model owns its unique software state"]
pub struct PhyClientState {
    bits: u8,
    tracker_model_armed: bool,
    wifi_previous_micros: u64,
    bluetooth_ieee802154_previous_micros: u64,
    period_micros: u64,
}

impl PhyClientState {
    /// Mint an empty model at the crate-controlled physical-epoch boundary.
    ///
    /// The caller must not use this constructor as evidence about a target
    /// global or vendor-owned client mask.
    #[cfg(test)]
    const fn new_empty(period_micros: u64) -> Self {
        Self {
            bits: 0,
            tracker_model_armed: false,
            wifi_previous_micros: 0,
            bluetooth_ieee802154_previous_micros: 0,
            period_micros,
        }
    }

    pub const fn snapshot(&self) -> PhyClientSnapshot {
        PhyClientSnapshot {
            wifi: self.bits & WIFI_BIT != 0,
            bluetooth: self.bits & BLUETOOTH_BIT != 0,
            ieee802154: self.bits & IEEE802154_BIT != 0,
            tracker_model_armed: self.tracker_model_armed,
            wifi_previous_micros: self.wifi_previous_micros,
            bluetooth_ieee802154_previous_micros: self.bluetooth_ieee802154_previous_micros,
            period_micros: self.period_micros,
        }
    }

    /// Consume the owner and acquire one client bit.
    ///
    /// Duplicate acquisition is rejected before any model state changes. A
    /// first aggregate acquisition records the reviewed arm-before-set-before-
    /// evaluate ordering. Later acquisitions record set-before-evaluate. The
    /// arm is an infallible model fact, not evidence of a target timer.
    pub fn acquire(
        mut self,
        client: PhyModemClient,
        now_micros: u64,
    ) -> Result<PhyClientAcquireOutcome, PhyClientAcquireFailure> {
        let bit = client.bit();
        if self.bits & bit != 0 {
            return Err(PhyClientAcquireFailure {
                owner: self,
                error: PhyClientAcquireError::AlreadyAcquired(client),
            });
        }

        let was_empty = self.bits == 0;
        let prospective_bits = self.bits | bit;
        let evaluation = match self.evaluate_for_bits(prospective_bits, now_micros) {
            Ok(evaluation) => evaluation,
            Err(error) => {
                return Err(PhyClientAcquireFailure {
                    owner: self,
                    error: PhyClientAcquireError::TrackingTime(error),
                });
            }
        };

        if was_empty {
            self.tracker_model_armed = true;
        }
        self.bits = prospective_bits;
        self.apply_evaluation(evaluation);

        debug_assert_eq!(self.bits & !VALID_CLIENT_BITS, 0);
        debug_assert_eq!(self.tracker_model_armed, self.bits != 0);

        Ok(PhyClientAcquireOutcome {
            client,
            was_empty,
            ordering: if was_empty {
                PhyClientAcquireOrdering::ArmThenSetThenEvaluate
            } else {
                PhyClientAcquireOrdering::SetThenEvaluate
            },
            continuation: TrackContinuation::new(self, evaluation),
        })
    }

    /// Consume the owner and release one client bit.
    ///
    /// Missing release is rejected without changing the owner. `is_last`
    /// matches the reviewed saved-mask equality test; unrelated bits survive.
    /// Last-user timer stop is an infallible model fact here, not a target
    /// executor or rollback contract.
    pub fn release(
        mut self,
        client: PhyModemClient,
    ) -> Result<PhyClientReleaseOutcome, PhyClientReleaseFailure> {
        let bit = client.bit();
        if self.bits & bit == 0 {
            return Err(PhyClientReleaseFailure {
                owner: self,
                error: PhyClientReleaseError::NotAcquired(client),
            });
        }

        let saved_bits = self.bits;
        self.bits &= !bit;
        let is_last = saved_bits == bit;
        if is_last {
            self.tracker_model_armed = false;
        }

        debug_assert_eq!(self.bits & !VALID_CLIENT_BITS, 0);
        debug_assert_eq!(self.tracker_model_armed, self.bits != 0);

        Ok(PhyClientReleaseOutcome {
            owner: self,
            client,
            is_last,
        })
    }

    /// Apply the reviewed immediate-on-enable due check without performing PLL
    /// hardware work.
    pub fn evaluate_immediate_tracking(
        mut self,
        now_micros: u64,
    ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure> {
        let evaluation = match self.evaluate_for_bits(self.bits, now_micros) {
            Ok(evaluation) => evaluation,
            Err(error) => {
                return Err(PhyTrackEvaluationFailure { owner: self, error });
            }
        };
        self.apply_evaluation(evaluation);
        Ok(PhyTrackEvaluation {
            continuation: TrackContinuation::new(self, evaluation),
        })
    }

    /// Apply one reviewed periodic callback without performing PLL hardware
    /// work.
    ///
    /// Unlike [`Self::evaluate_immediate_tracking`], the timer callback invokes
    /// the internal scheduler unconditionally. An active class therefore emits
    /// a request and refreshes its timestamp on every callback.
    pub fn evaluate_periodic_tracking(
        mut self,
        now_micros: u64,
    ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure> {
        if let Err(error) = self.validate_active_timestamps(self.bits, now_micros) {
            return Err(PhyTrackEvaluationFailure { owner: self, error });
        }

        let wifi_active = self.bits & WIFI_BIT != 0;
        let bluetooth_ieee802154_active = self.bits & (BLUETOOTH_BIT | IEEE802154_BIT) != 0;
        let evaluation = TrackEvaluation {
            wifi_active,
            bluetooth_ieee802154_active,
            request_due: wifi_active || bluetooth_ieee802154_active,
            now_micros,
        };
        self.apply_evaluation(evaluation);
        Ok(PhyTrackEvaluation {
            continuation: TrackContinuation::new(self, evaluation),
        })
    }

    fn evaluate_for_bits(
        &self,
        bits: u8,
        now_micros: u64,
    ) -> Result<TrackEvaluation, PhyTrackTimeError> {
        let wifi_active = bits & WIFI_BIT != 0;
        let bluetooth_ieee802154_active = bits & (BLUETOOTH_BIT | IEEE802154_BIT) != 0;

        self.validate_active_timestamps(bits, now_micros)?;

        let wifi_due = wifi_active && now_micros - self.wifi_previous_micros > self.period_micros;
        let bluetooth_ieee802154_due = bluetooth_ieee802154_active
            && now_micros - self.bluetooth_ieee802154_previous_micros > self.period_micros;

        Ok(TrackEvaluation {
            wifi_active,
            bluetooth_ieee802154_active,
            request_due: wifi_due || bluetooth_ieee802154_due,
            now_micros,
        })
    }

    fn validate_active_timestamps(
        &self,
        bits: u8,
        now_micros: u64,
    ) -> Result<(), PhyTrackTimeError> {
        let wifi_active = bits & WIFI_BIT != 0;
        let bluetooth_ieee802154_active = bits & (BLUETOOTH_BIT | IEEE802154_BIT) != 0;

        if wifi_active && now_micros < self.wifi_previous_micros {
            return Err(PhyTrackTimeError::TimeReversed {
                class: PhyPllTrackClass::Wifi,
                previous_micros: self.wifi_previous_micros,
                now_micros,
            });
        }
        if bluetooth_ieee802154_active && now_micros < self.bluetooth_ieee802154_previous_micros {
            return Err(PhyTrackTimeError::TimeReversed {
                class: PhyPllTrackClass::BluetoothIeee802154,
                previous_micros: self.bluetooth_ieee802154_previous_micros,
                now_micros,
            });
        }
        Ok(())
    }

    fn apply_evaluation(&mut self, evaluation: TrackEvaluation) {
        if !evaluation.request_due {
            return;
        }
        if evaluation.wifi_active {
            self.wifi_previous_micros = evaluation.now_micros;
        }
        if evaluation.bluetooth_ieee802154_active {
            self.bluetooth_ieee802154_previous_micros = evaluation.now_micros;
        }
    }
}

#[derive(Clone, Copy)]
struct TrackEvaluation {
    wifi_active: bool,
    bluetooth_ieee802154_active: bool,
    request_due: bool,
    now_micros: u64,
}

impl TrackEvaluation {
    fn request(self) -> Option<PhyParamTrackRequest> {
        self.request_due.then_some(PhyParamTrackRequest {
            wifi: self.wifi_active,
            bluetooth_ieee802154: self.bluetooth_ieee802154_active,
        })
    }
}

enum TrackContinuation {
    Settled(PhyClientState),
    Pending(PhyPendingTrack),
}

impl TrackContinuation {
    fn new(owner: PhyClientState, evaluation: TrackEvaluation) -> Self {
        match evaluation.request() {
            Some(request) => Self::Pending(PhyPendingTrack { owner, request }),
            None => Self::Settled(owner),
        }
    }

    const fn request(&self) -> Option<&PhyParamTrackRequest> {
        match self {
            Self::Settled(_) => None,
            Self::Pending(pending) => Some(pending.request()),
        }
    }

    const fn owner(&self) -> &PhyClientState {
        match self {
            Self::Settled(owner) => owner,
            Self::Pending(pending) => &pending.owner,
        }
    }

    fn into_owner(self) -> Result<PhyClientState, PhyPendingTrack> {
        match self {
            Self::Settled(owner) => Ok(owner),
            Self::Pending(pending) => Err(pending),
        }
    }
}

/// Reviewed ordering of one successful acquisition's software obligations.
///
/// These variants describe atomic pure-model facts; they are not timer or lock
/// completion tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyClientAcquireOrdering {
    ArmThenSetThenEvaluate,
    SetThenEvaluate,
}

/// Opaque request to the still-unreviewed PLL hardware leaf.
///
/// Possessing this value proves only that the software scheduler found work
/// due. It is not a completion token or a PLL/RF readiness proof.
#[must_use = "the opaque hardware request must be handled by a later target transition"]
pub struct PhyParamTrackRequest {
    wifi: bool,
    bluetooth_ieee802154: bool,
}

/// Owner held while the opaque PLL hardware request remains unresolved.
///
/// Safe code can inspect the request but cannot recover or operate on the
/// client owner. Dropping this value destroys access to that software epoch.
/// The only production exit currently poisons the state explicitly; a future
/// target executor must add an identity-bound successful completion.
///
#[must_use = "the PLL hardware request retains the unique PHY client owner"]
pub struct PhyPendingTrack {
    owner: PhyClientState,
    request: PhyParamTrackRequest,
}

impl fmt::Debug for PhyPendingTrack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyPendingTrack")
            .field("wifi", &self.request.wifi)
            .field("bluetooth_ieee802154", &self.request.bluetooth_ieee802154)
            .finish_non_exhaustive()
    }
}

impl PhyPendingTrack {
    pub const fn request(&self) -> &PhyParamTrackRequest {
        &self.request
    }

    pub const fn snapshot(&self) -> PhyClientSnapshot {
        self.owner.snapshot()
    }

    /// Explicitly poison a model request that target hardware did not complete.
    pub fn fail(self) -> PhyTrackPoisoned {
        PhyTrackPoisoned {
            owner: self.owner,
            request: self.request,
        }
    }

    #[cfg(test)]
    fn complete_for_test(self) -> PhyClientState {
        self.owner
    }
}

/// Terminal model owner after an opaque PLL request failed or was cancelled.
///
/// No ordinary-owner extractor is provided because the model has no reviewed
/// rollback for partially executed hardware work.
#[must_use = "failed opaque PLL work poisons the PHY client model"]
pub struct PhyTrackPoisoned {
    owner: PhyClientState,
    request: PhyParamTrackRequest,
}

impl PhyTrackPoisoned {
    pub const fn request(&self) -> &PhyParamTrackRequest {
        &self.request
    }

    pub const fn snapshot(&self) -> PhyClientSnapshot {
        self.owner.snapshot()
    }
}

impl PhyParamTrackRequest {
    pub const fn wifi(&self) -> bool {
        self.wifi
    }

    pub const fn bluetooth_ieee802154(&self) -> bool {
        self.bluetooth_ieee802154
    }
}

/// Fail-closed monotonic-clock validation error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTrackTimeError {
    TimeReversed {
        class: PhyPllTrackClass,
        previous_micros: u64,
        now_micros: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyClientAcquireError {
    AlreadyAcquired(PhyModemClient),
    TrackingTime(PhyTrackTimeError),
}

/// Failed acquisition retaining the exact unchanged owner.
#[must_use = "failure retains the unique PHY client owner"]
pub struct PhyClientAcquireFailure {
    owner: PhyClientState,
    error: PhyClientAcquireError,
}

impl fmt::Debug for PhyClientAcquireFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyClientAcquireFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl PhyClientAcquireFailure {
    pub const fn error(&self) -> PhyClientAcquireError {
        self.error
    }

    pub const fn owner(&self) -> &PhyClientState {
        &self.owner
    }

    pub fn into_owner(self) -> PhyClientState {
        self.owner
    }
}

/// Successful pure acquisition and its source-reviewed facts.
#[must_use = "the outcome retains the unique PHY client owner"]
pub struct PhyClientAcquireOutcome {
    client: PhyModemClient,
    was_empty: bool,
    ordering: PhyClientAcquireOrdering,
    continuation: TrackContinuation,
}

impl fmt::Debug for PhyClientAcquireOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyClientAcquireOutcome")
            .field("client", &self.client)
            .field("was_empty", &self.was_empty)
            .field("ordering", &self.ordering)
            .field("track_requested", &self.request().is_some())
            .finish_non_exhaustive()
    }
}

impl PhyClientAcquireOutcome {
    pub const fn client(&self) -> PhyModemClient {
        self.client
    }

    pub const fn was_empty(&self) -> bool {
        self.was_empty
    }

    pub const fn ordering(&self) -> PhyClientAcquireOrdering {
        self.ordering
    }

    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.continuation.request()
    }

    pub const fn owner(&self) -> &PhyClientState {
        self.continuation.owner()
    }

    /// Recover the ordinary owner only when no opaque request is pending.
    pub fn into_owner(self) -> Result<PhyClientState, PhyPendingTrack> {
        self.continuation.into_owner()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyClientReleaseError {
    NotAcquired(PhyModemClient),
}

/// Failed release retaining the exact unchanged owner.
#[must_use = "failure retains the unique PHY client owner"]
pub struct PhyClientReleaseFailure {
    owner: PhyClientState,
    error: PhyClientReleaseError,
}

impl fmt::Debug for PhyClientReleaseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyClientReleaseFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl PhyClientReleaseFailure {
    pub const fn error(&self) -> PhyClientReleaseError {
        self.error
    }

    pub const fn owner(&self) -> &PhyClientState {
        &self.owner
    }

    pub fn into_owner(self) -> PhyClientState {
        self.owner
    }
}

/// Successful pure release and its saved-mask last-user fact.
#[must_use = "the outcome retains the unique PHY client owner"]
pub struct PhyClientReleaseOutcome {
    owner: PhyClientState,
    client: PhyModemClient,
    is_last: bool,
}

impl fmt::Debug for PhyClientReleaseOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyClientReleaseOutcome")
            .field("client", &self.client)
            .field("is_last", &self.is_last)
            .finish_non_exhaustive()
    }
}

impl PhyClientReleaseOutcome {
    pub const fn client(&self) -> PhyModemClient {
        self.client
    }

    pub const fn is_last(&self) -> bool {
        self.is_last
    }

    pub const fn owner(&self) -> &PhyClientState {
        &self.owner
    }

    pub fn into_owner(self) -> PhyClientState {
        self.owner
    }
}

/// Failed scheduler evaluation retaining the exact unchanged owner.
#[must_use = "failure retains the unique PHY client owner"]
pub struct PhyTrackEvaluationFailure {
    owner: PhyClientState,
    error: PhyTrackTimeError,
}

impl fmt::Debug for PhyTrackEvaluationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyTrackEvaluationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl PhyTrackEvaluationFailure {
    pub const fn error(&self) -> PhyTrackTimeError {
        self.error
    }

    pub const fn owner(&self) -> &PhyClientState {
        &self.owner
    }

    pub fn into_owner(self) -> PhyClientState {
        self.owner
    }
}

/// Pure scheduler result retaining the unique software owner.
#[must_use = "the evaluation retains the unique PHY client owner"]
pub struct PhyTrackEvaluation {
    continuation: TrackContinuation,
}

impl fmt::Debug for PhyTrackEvaluation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyTrackEvaluation")
            .field("track_requested", &self.request().is_some())
            .finish_non_exhaustive()
    }
}

impl PhyTrackEvaluation {
    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.continuation.request()
    }

    pub const fn owner(&self) -> &PhyClientState {
        self.continuation.owner()
    }

    /// Recover the ordinary owner only when no opaque request is pending.
    pub fn into_owner(self) -> Result<PhyClientState, PhyPendingTrack> {
        self.continuation.into_owner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENTS: [PhyModemClient; 3] = [
        PhyModemClient::Wifi,
        PhyModemClient::Bluetooth,
        PhyModemClient::Ieee802154,
    ];

    fn complete_acquire_for_test(outcome: PhyClientAcquireOutcome) -> PhyClientState {
        match outcome.into_owner() {
            Ok(owner) => owner,
            Err(pending) => pending.complete_for_test(),
        }
    }

    fn state_for_mask(mask: u8, now_micros: u64) -> PhyClientState {
        assert!(mask <= VALID_CLIENT_BITS);
        let mut state = PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS);
        for client in CLIENTS {
            if mask & client.bit() != 0 {
                state = complete_acquire_for_test(state.acquire(client, now_micros).unwrap());
            }
        }
        state
    }

    fn observed_mask(snapshot: PhyClientSnapshot) -> u8 {
        CLIENTS.into_iter().fold(0, |mask, client| {
            mask | if snapshot.contains(client) {
                client.bit()
            } else {
                0
            }
        })
    }

    #[test]
    fn exhaustive_acquire_preserves_unrelated_bits_and_reports_first_user() {
        for mask in 0..=VALID_CLIENT_BITS {
            for client in CLIENTS {
                let state = state_for_mask(mask, 0);
                let before = state.snapshot();
                if mask & client.bit() != 0 {
                    let failure = state.acquire(client, 0).unwrap_err();
                    assert_eq!(
                        failure.error(),
                        PhyClientAcquireError::AlreadyAcquired(client)
                    );
                    assert_eq!(failure.owner().snapshot(), before);
                    continue;
                }

                let outcome = state.acquire(client, 0).unwrap();
                assert_eq!(outcome.was_empty(), mask == 0);
                assert_eq!(
                    outcome.ordering(),
                    if mask == 0 {
                        PhyClientAcquireOrdering::ArmThenSetThenEvaluate
                    } else {
                        PhyClientAcquireOrdering::SetThenEvaluate
                    }
                );
                assert_eq!(
                    observed_mask(outcome.owner().snapshot()),
                    mask | client.bit()
                );
                assert!(outcome.owner().snapshot().tracker_model_armed());
            }
        }
    }

    #[test]
    fn exhaustive_release_preserves_unrelated_bits_and_reports_last_user() {
        for mask in 0..=VALID_CLIENT_BITS {
            for client in CLIENTS {
                let state = state_for_mask(mask, 0);
                let before = state.snapshot();
                if mask & client.bit() == 0 {
                    let failure = state.release(client).unwrap_err();
                    assert_eq!(failure.error(), PhyClientReleaseError::NotAcquired(client));
                    assert_eq!(failure.owner().snapshot(), before);
                    continue;
                }

                let outcome = state.release(client).unwrap();
                assert_eq!(outcome.is_last(), mask == client.bit());
                assert_eq!(
                    observed_mask(outcome.owner().snapshot()),
                    mask & !client.bit()
                );
                assert_eq!(
                    outcome.owner().snapshot().tracker_model_armed(),
                    mask & !client.bit() != 0
                );
            }
        }
    }

    #[test]
    fn strict_threshold_equality_does_not_request_but_greater_does() {
        let state = complete_acquire_for_test(
            PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
                .acquire(PhyModemClient::Ieee802154, DEFAULT_PLL_TRACK_PERIOD_MICROS)
                .unwrap(),
        );
        let equal = state
            .evaluate_immediate_tracking(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .unwrap();
        assert!(equal.request().is_none());

        let greater = equal
            .into_owner()
            .unwrap()
            .evaluate_immediate_tracking(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
            .unwrap();
        let request = greater.request().unwrap();
        assert!(!request.wifi());
        assert!(request.bluetooth_ieee802154());
    }

    #[test]
    fn bluetooth_and_ieee_share_timestamp_and_request_class() {
        let state = complete_acquire_for_test(
            PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
                .acquire(
                    PhyModemClient::Bluetooth,
                    DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
                )
                .unwrap(),
        );
        assert_eq!(
            state
                .snapshot()
                .previous_micros(PhyPllTrackClass::BluetoothIeee802154),
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 1
        );

        let outcome = state
            .acquire(
                PhyModemClient::Ieee802154,
                DEFAULT_PLL_TRACK_PERIOD_MICROS + 2,
            )
            .unwrap();
        assert!(outcome.request().is_none());
        assert_eq!(
            outcome
                .owner()
                .snapshot()
                .previous_micros(PhyPllTrackClass::BluetoothIeee802154),
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 1
        );
    }

    #[test]
    fn request_booleans_describe_every_active_class() {
        for mask in 1..=VALID_CLIENT_BITS {
            let evaluation = state_for_mask(mask, 0)
                .evaluate_immediate_tracking(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
                .unwrap();
            let request = evaluation.request().unwrap();
            assert_eq!(request.wifi(), mask & WIFI_BIT != 0);
            assert_eq!(
                request.bluetooth_ieee802154(),
                mask & (BLUETOOTH_BIT | IEEE802154_BIT) != 0
            );
        }
    }

    #[test]
    fn one_due_class_refreshes_and_requests_all_active_classes() {
        let state = complete_acquire_for_test(
            PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
                .acquire(PhyModemClient::Wifi, DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
                .unwrap(),
        );
        let state = state.release(PhyModemClient::Wifi).unwrap().into_owner();
        let state = complete_acquire_for_test(
            state
                .acquire(
                    PhyModemClient::Ieee802154,
                    DEFAULT_PLL_TRACK_PERIOD_MICROS + 2,
                )
                .unwrap(),
        );
        let state = complete_acquire_for_test(
            state
                .acquire(PhyModemClient::Wifi, DEFAULT_PLL_TRACK_PERIOD_MICROS + 2)
                .unwrap(),
        );
        let evaluation = state
            .evaluate_immediate_tracking(2 * DEFAULT_PLL_TRACK_PERIOD_MICROS + 2)
            .unwrap();
        let request = evaluation.request().unwrap();
        assert!(request.wifi());
        assert!(request.bluetooth_ieee802154());
        let snapshot = evaluation.owner().snapshot();
        assert_eq!(
            snapshot.previous_micros(PhyPllTrackClass::Wifi),
            2 * DEFAULT_PLL_TRACK_PERIOD_MICROS + 2
        );
        assert_eq!(
            snapshot.previous_micros(PhyPllTrackClass::BluetoothIeee802154),
            2 * DEFAULT_PLL_TRACK_PERIOD_MICROS + 2
        );
    }

    #[test]
    fn periodic_callback_requests_active_classes_without_due_check() {
        let state = state_for_mask(IEEE802154_BIT, 0);
        let evaluation = state.evaluate_periodic_tracking(1).unwrap();
        let request = evaluation.request().unwrap();
        assert!(!request.wifi());
        assert!(request.bluetooth_ieee802154());
        assert_eq!(
            evaluation
                .owner()
                .snapshot()
                .previous_micros(PhyPllTrackClass::BluetoothIeee802154),
            1
        );
    }

    #[test]
    fn pending_request_cannot_release_owner_without_explicit_resolution() {
        let outcome = PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .acquire(
                PhyModemClient::Ieee802154,
                DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
            )
            .unwrap();
        let pending = match outcome.into_owner() {
            Ok(_) => panic!("due hardware work released the owner"),
            Err(pending) => pending,
        };
        assert!(pending.request().bluetooth_ieee802154());

        let before = pending.snapshot();
        let poisoned = pending.fail();
        assert_eq!(poisoned.snapshot(), before);
        assert!(poisoned.request().bluetooth_ieee802154());
    }

    #[test]
    fn pending_periodic_request_cannot_release_owner() {
        let evaluation = state_for_mask(IEEE802154_BIT, 0)
            .evaluate_periodic_tracking(1)
            .unwrap();
        let pending = match evaluation.into_owner() {
            Ok(_) => panic!("periodic hardware work released the owner"),
            Err(pending) => pending,
        };
        assert_eq!(
            observed_mask(pending.snapshot()),
            IEEE802154_BIT,
            "the pending request must retain the exact client state"
        );
    }

    #[test]
    fn time_reversal_rejects_acquire_and_restores_exact_owner() {
        let state = complete_acquire_for_test(
            PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
                .acquire(PhyModemClient::Wifi, DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
                .unwrap(),
        );
        let before = state.snapshot();
        let failure = state.acquire(PhyModemClient::Ieee802154, 0).unwrap_err();
        assert_eq!(
            failure.error(),
            PhyClientAcquireError::TrackingTime(PhyTrackTimeError::TimeReversed {
                class: PhyPllTrackClass::Wifi,
                previous_micros: DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
                now_micros: 0,
            })
        );
        assert_eq!(failure.owner().snapshot(), before);
    }

    #[test]
    fn time_reversal_rejects_immediate_evaluation_and_restores_exact_owner() {
        let state = complete_acquire_for_test(
            PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
                .acquire(
                    PhyModemClient::Ieee802154,
                    DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
                )
                .unwrap(),
        );
        let before = state.snapshot();
        let failure = state.evaluate_immediate_tracking(0).unwrap_err();
        assert_eq!(
            failure.error(),
            PhyTrackTimeError::TimeReversed {
                class: PhyPllTrackClass::BluetoothIeee802154,
                previous_micros: DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
                now_micros: 0,
            }
        );
        assert_eq!(failure.owner().snapshot(), before);
    }

    #[test]
    fn time_reversal_rejects_periodic_callback_and_restores_exact_owner() {
        let state = complete_acquire_for_test(
            PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
                .acquire(
                    PhyModemClient::Ieee802154,
                    DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
                )
                .unwrap(),
        );
        let before = state.snapshot();
        let failure = state.evaluate_periodic_tracking(0).unwrap_err();
        assert_eq!(
            failure.error(),
            PhyTrackTimeError::TimeReversed {
                class: PhyPllTrackClass::BluetoothIeee802154,
                previous_micros: DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
                now_micros: 0,
            }
        );
        assert_eq!(failure.owner().snapshot(), before);
    }
}
