//! Pure shared-PHY client-set and PLL-tracking scheduler model.
//!
//! The reviewed ESP-IDF software protocol stores Wi-Fi, Bluetooth, and IEEE
//! 802.15.4 as independent bits. It gives Wi-Fi one tracking timestamp and
//! Bluetooth plus IEEE 802.15.4 a shared timestamp. This module reproduces only
//! those software decisions. It performs no MMIO and does not arm a real
//! timer. A due request retains the unique owner while the exact outer
//! [`crate::phy_param_tracking::PhyParamTrackingTransition`] is executed.
//! Its recovered TX-power, Wi-Fi PHY-I2C, calibration, RFPLL and temperature
//! children compose into live-state owners; calibration hardware effects stay
//! explicit unresolved actions until their target bindings are owned.
//!
//! Timer arm/stop and the PHY lock are represented only as atomic model facts.
//! There is no fallible timer executor, rollback protocol, or target lock in
//! this module, so the resulting transition is not an RF-readiness proof.
//! Clock sampling is nevertheless exact: immediate evaluation preserves the
//! public source's short-circuit due checks and then samples each active class
//! again while refreshing timestamps; the periodic callback performs only the
//! per-class refresh samples.

#![allow(
    dead_code,
    reason = "the model awaits fallible timer, PHY-lock, and tracking-child executors"
)]

use core::fmt;

use crate::phy_param_tracking::{
    PhyParamTrackRequest, PhyParamTrackingAction, PhyParamTrackingCalibrationTransition,
    PhyParamTrackingChildError, PhyParamTrackingCompletion, PhyParamTrackingPolicy,
    PhyParamTrackingRfpllTransition, PhyParamTrackingTemperatureTransition,
    PhyParamTrackingTransition, PhyParamTrackingTransitionError, PhyParamTrackingTxPowerTransition,
    PhyParamTrackingWifiI2cTransition,
};

const WIFI_BIT: u8 = 1;
const BLUETOOTH_BIT: u8 = 2;
const IEEE802154_BIT: u8 = 4;
const VALID_CLIENT_BITS: u8 = WIFI_BIT | BLUETOOTH_BIT | IEEE802154_BIT;

/// Source-reviewed default periodic PLL-tracking interval.
pub const DEFAULT_PLL_TRACK_PERIOD_MICROS: u64 = 1_000_000;

/// Monotonic microsecond clock sampled by the shared-PHY scheduler.
///
/// The source reads its timer at several distinct points. Accepting a port
/// instead of one caller-supplied timestamp prevents those reads from being
/// collapsed into an atomic snapshot at a tracking-period boundary.
pub trait PhyPllTrackClock {
    fn now_micros(&mut self) -> u64;
}

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
    /// Mint an empty scheduler for one freshly registered hardware epoch.
    ///
    /// This constructor is crate-controlled so application code cannot create
    /// an unrelated client mask and present it beside registered hardware.
    pub(crate) const fn for_registered_epoch(period_micros: u64) -> Self {
        Self {
            bits: 0,
            tracker_model_armed: false,
            wifi_previous_micros: 0,
            bluetooth_ieee802154_previous_micros: 0,
            period_micros,
        }
    }

    /// Mint an empty model at the crate-controlled physical-epoch boundary.
    ///
    /// The caller must not use this constructor as evidence about a target
    /// global or vendor-owned client mask.
    #[cfg(test)]
    const fn new_empty(period_micros: u64) -> Self {
        Self::for_registered_epoch(period_micros)
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
        clock: &mut impl PhyPllTrackClock,
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
        let evaluation = match self.evaluate_for_bits(prospective_bits, clock) {
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
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure> {
        let evaluation = match self.evaluate_for_bits(self.bits, clock) {
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
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure> {
        let wifi_active = self.bits & WIFI_BIT != 0;
        let bluetooth_ieee802154_active = self.bits & (BLUETOOTH_BIT | IEEE802154_BIT) != 0;
        let evaluation = match self.build_evaluation(
            wifi_active,
            bluetooth_ieee802154_active,
            wifi_active || bluetooth_ieee802154_active,
            clock,
        ) {
            Ok(evaluation) => evaluation,
            Err(error) => return Err(PhyTrackEvaluationFailure { owner: self, error }),
        };
        self.apply_evaluation(evaluation);
        Ok(PhyTrackEvaluation {
            continuation: TrackContinuation::new(self, evaluation),
        })
    }

    fn evaluate_for_bits(
        &self,
        bits: u8,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<TrackEvaluation, PhyTrackTimeError> {
        let wifi_active = bits & WIFI_BIT != 0;
        let bluetooth_ieee802154_active = bits & (BLUETOOTH_BIT | IEEE802154_BIT) != 0;

        let mut request_due = false;
        if wifi_active {
            let now_micros = clock.now_micros();
            self.validate_timestamp(
                PhyPllTrackClass::Wifi,
                self.wifi_previous_micros,
                now_micros,
            )?;
            request_due = now_micros - self.wifi_previous_micros > self.period_micros;
        }
        // Preserve the two source assignments containing `need_track_pll ||`.
        // Once Wi-Fi is due, C short-circuiting skips the BT/154 due sample.
        if bluetooth_ieee802154_active && !request_due {
            let now_micros = clock.now_micros();
            self.validate_timestamp(
                PhyPllTrackClass::BluetoothIeee802154,
                self.bluetooth_ieee802154_previous_micros,
                now_micros,
            )?;
            request_due =
                now_micros - self.bluetooth_ieee802154_previous_micros > self.period_micros;
        }

        self.build_evaluation(wifi_active, bluetooth_ieee802154_active, request_due, clock)
    }

    fn build_evaluation(
        &self,
        wifi_active: bool,
        bluetooth_ieee802154_active: bool,
        request_due: bool,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<TrackEvaluation, PhyTrackTimeError> {
        if !request_due {
            return Ok(TrackEvaluation {
                wifi_active,
                bluetooth_ieee802154_active,
                wifi_refresh_micros: None,
                bluetooth_ieee802154_refresh_micros: None,
            });
        }

        let wifi_refresh_micros = if wifi_active {
            let now_micros = clock.now_micros();
            self.validate_timestamp(
                PhyPllTrackClass::Wifi,
                self.wifi_previous_micros,
                now_micros,
            )?;
            Some(now_micros)
        } else {
            None
        };
        let bluetooth_ieee802154_refresh_micros = if bluetooth_ieee802154_active {
            let now_micros = clock.now_micros();
            self.validate_timestamp(
                PhyPllTrackClass::BluetoothIeee802154,
                self.bluetooth_ieee802154_previous_micros,
                now_micros,
            )?;
            Some(now_micros)
        } else {
            None
        };

        Ok(TrackEvaluation {
            wifi_active,
            bluetooth_ieee802154_active,
            wifi_refresh_micros,
            bluetooth_ieee802154_refresh_micros,
        })
    }

    fn validate_timestamp(
        &self,
        class: PhyPllTrackClass,
        previous_micros: u64,
        now_micros: u64,
    ) -> Result<(), PhyTrackTimeError> {
        if now_micros < previous_micros {
            Err(PhyTrackTimeError::TimeReversed {
                class,
                previous_micros,
                now_micros,
            })
        } else {
            Ok(())
        }
    }

    fn apply_evaluation(&mut self, evaluation: TrackEvaluation) {
        if let Some(now_micros) = evaluation.wifi_refresh_micros {
            self.wifi_previous_micros = now_micros;
        }
        if let Some(now_micros) = evaluation.bluetooth_ieee802154_refresh_micros {
            self.bluetooth_ieee802154_previous_micros = now_micros;
        }
    }
}

#[derive(Clone, Copy)]
struct TrackEvaluation {
    wifi_active: bool,
    bluetooth_ieee802154_active: bool,
    wifi_refresh_micros: Option<u64>,
    bluetooth_ieee802154_refresh_micros: Option<u64>,
}

impl TrackEvaluation {
    fn request(self) -> Option<PhyParamTrackRequest> {
        (self.wifi_refresh_micros.is_some() || self.bluetooth_ieee802154_refresh_micros.is_some())
            .then_some(PhyParamTrackRequest::new(
                self.wifi_active,
                self.bluetooth_ieee802154_active,
            ))
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

/// Owner held while the source-reviewed tracking request remains unresolved.
///
/// Safe code can inspect the request but cannot recover or operate on the
/// client owner. It can begin the exact outer tracking transition or poison
/// the epoch explicitly.
#[must_use = "the tracking request retains the unique PHY client owner"]
pub struct PhyPendingTrack {
    owner: PhyClientState,
    request: PhyParamTrackRequest,
}

impl fmt::Debug for PhyPendingTrack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyPendingTrack")
            .field("wifi", &self.request.wifi())
            .field("bluetooth_ieee802154", &self.request.bluetooth_ieee802154())
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

    /// Bind a registered-epoch policy to the exact outer tracking transition.
    pub(crate) fn begin_tracking(self, policy: PhyParamTrackingPolicy) -> PhyPendingTracking {
        PhyPendingTracking {
            owner: self.owner,
            request: self.request,
            transition: PhyParamTrackingTransition::new(self.request, policy),
        }
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

/// Unique client owner retained through one exact outer tracking transition.
///
/// Wrong completions preserve both this owner and the current action. Ordinary
/// access is restored only after the terminal action is visible.
#[must_use = "the in-flight tracking transition retains the unique PHY client owner"]
pub struct PhyPendingTracking {
    owner: PhyClientState,
    request: PhyParamTrackRequest,
    transition: PhyParamTrackingTransition,
}

impl fmt::Debug for PhyPendingTracking {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhyPendingTracking")
            .field("request", &self.request)
            .field("action", &self.transition.action())
            .finish_non_exhaustive()
    }
}

impl PhyPendingTracking {
    #[cfg(test)]
    pub(crate) fn for_test(request: PhyParamTrackRequest, policy: PhyParamTrackingPolicy) -> Self {
        Self {
            owner: PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS),
            request,
            transition: PhyParamTrackingTransition::new(request, policy),
        }
    }

    pub const fn action(&self) -> PhyParamTrackingAction {
        self.transition.action()
    }

    pub fn advance(
        &mut self,
        completion: PhyParamTrackingCompletion,
    ) -> Result<(), PhyParamTrackingTransitionError> {
        self.transition.advance(completion)
    }

    pub const fn snapshot(&self) -> PhyClientSnapshot {
        self.owner.snapshot()
    }

    /// Lower the current RFPLL-cap action into its complete typed child.
    pub fn begin_rfpll_cap_tracking<'state>(
        &self,
        state: &'state mut crate::phy_state::PhyState,
    ) -> Result<PhyParamTrackingRfpllTransition<'state>, PhyParamTrackingChildError> {
        self.transition.begin_rfpll_cap_tracking(state)
    }

    /// Lower the current calibration action while retaining all three live
    /// semantic temperature references until terminal commit.
    pub fn begin_calibration_tracking<'state>(
        &self,
        state: &'state mut crate::phy_state::PhyState,
    ) -> Result<PhyParamTrackingCalibrationTransition<'state>, PhyParamTrackingChildError> {
        self.transition.begin_calibration_tracking(state)
    }

    /// Lower the current outer TX-power action into its complete typed child.
    /// Every other outer action fails closed instead of becoming a no-op.
    pub fn begin_tx_power_tracking<'state>(
        &self,
        state: &'state mut crate::phy_state::PhyState,
    ) -> Result<PhyParamTrackingTxPowerTransition<'state>, PhyParamTrackingChildError> {
        self.transition.begin_tx_power_tracking(state)
    }

    /// Lower the current Wi-Fi PHY-I2C action into its complete typed child.
    pub fn begin_wifi_i2c_tracking<'state>(
        &self,
        state: &'state mut crate::phy_state::PhyState,
    ) -> Result<PhyParamTrackingWifiI2cTransition<'state>, PhyParamTrackingChildError> {
        self.transition.begin_wifi_i2c_tracking(state)
    }

    /// Lower the current final sensor action into its complete typed child.
    pub fn begin_temperature_read<'state>(
        &self,
        state: &'state mut crate::phy_state::PhyState,
    ) -> Result<PhyParamTrackingTemperatureTransition<'state>, PhyParamTrackingChildError> {
        self.transition.begin_temperature_read(state)
    }

    /// Recover the ordinary owner only after every outer action was confirmed.
    pub fn into_owner(self) -> Result<PhyClientState, Self> {
        if matches!(
            self.transition.action(),
            PhyParamTrackingAction::Complete(_)
        ) {
            Ok(self.owner)
        } else {
            Err(self)
        }
    }

    /// Poison an interrupted or failed hardware tracking operation.
    pub fn fail(self) -> PhyTrackPoisoned {
        PhyTrackPoisoned {
            owner: self.owner,
            request: self.request,
        }
    }
}

/// Terminal model owner after tracking hardware failed or was cancelled.
///
/// No ordinary-owner extractor is provided because the model has no reviewed
/// rollback for partially executed hardware work.
#[must_use = "failed tracking hardware work poisons the PHY client model"]
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

    /// Recover the ordinary owner only when no tracking request is pending.
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

    /// Recover the ordinary owner only when no tracking request is pending.
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

    struct FixedClock(u64);

    impl PhyPllTrackClock for FixedClock {
        fn now_micros(&mut self) -> u64 {
            self.0
        }
    }

    struct ScriptedClock<const COUNT: usize> {
        samples: [u64; COUNT],
        next: usize,
    }

    impl<const COUNT: usize> ScriptedClock<COUNT> {
        const fn new(samples: [u64; COUNT]) -> Self {
            Self { samples, next: 0 }
        }

        const fn samples_consumed(&self) -> usize {
            self.next
        }
    }

    impl<const COUNT: usize> PhyPllTrackClock for ScriptedClock<COUNT> {
        fn now_micros(&mut self) -> u64 {
            let sample = self.samples[self.next];
            self.next += 1;
            sample
        }
    }

    trait PhyClientStateTestExt: Sized {
        fn acquire_at(
            self,
            client: PhyModemClient,
            now_micros: u64,
        ) -> Result<PhyClientAcquireOutcome, PhyClientAcquireFailure>;

        fn evaluate_immediate_at(
            self,
            now_micros: u64,
        ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure>;

        fn evaluate_periodic_at(
            self,
            now_micros: u64,
        ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure>;
    }

    impl PhyClientStateTestExt for PhyClientState {
        fn acquire_at(
            self,
            client: PhyModemClient,
            now_micros: u64,
        ) -> Result<PhyClientAcquireOutcome, PhyClientAcquireFailure> {
            self.acquire(client, &mut FixedClock(now_micros))
        }

        fn evaluate_immediate_at(
            self,
            now_micros: u64,
        ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure> {
            self.evaluate_immediate_tracking(&mut FixedClock(now_micros))
        }

        fn evaluate_periodic_at(
            self,
            now_micros: u64,
        ) -> Result<PhyTrackEvaluation, PhyTrackEvaluationFailure> {
            self.evaluate_periodic_tracking(&mut FixedClock(now_micros))
        }
    }

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
                state = complete_acquire_for_test(state.acquire_at(client, now_micros).unwrap());
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
                    let failure = state.acquire_at(client, 0).unwrap_err();
                    assert_eq!(
                        failure.error(),
                        PhyClientAcquireError::AlreadyAcquired(client)
                    );
                    assert_eq!(failure.owner().snapshot(), before);
                    continue;
                }

                let outcome = state.acquire_at(client, 0).unwrap();
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
                .acquire_at(PhyModemClient::Ieee802154, DEFAULT_PLL_TRACK_PERIOD_MICROS)
                .unwrap(),
        );
        let equal = state
            .evaluate_immediate_at(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .unwrap();
        assert!(equal.request().is_none());

        let greater = equal
            .into_owner()
            .unwrap()
            .evaluate_immediate_at(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
            .unwrap();
        let request = greater.request().unwrap();
        assert!(!request.wifi());
        assert!(request.bluetooth_ieee802154());
    }

    #[test]
    fn bluetooth_and_ieee_share_timestamp_and_request_class() {
        let state = complete_acquire_for_test(
            PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
                .acquire_at(
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
            .acquire_at(
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
                .evaluate_immediate_at(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
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
                .acquire_at(PhyModemClient::Wifi, DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
                .unwrap(),
        );
        let state = state.release(PhyModemClient::Wifi).unwrap().into_owner();
        let state = complete_acquire_for_test(
            state
                .acquire_at(
                    PhyModemClient::Ieee802154,
                    DEFAULT_PLL_TRACK_PERIOD_MICROS + 2,
                )
                .unwrap(),
        );
        let state = complete_acquire_for_test(
            state
                .acquire_at(PhyModemClient::Wifi, DEFAULT_PLL_TRACK_PERIOD_MICROS + 2)
                .unwrap(),
        );
        let evaluation = state
            .evaluate_immediate_at(2 * DEFAULT_PLL_TRACK_PERIOD_MICROS + 2)
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
    fn immediate_tracking_preserves_short_circuit_and_refresh_sample_order() {
        let state = state_for_mask(WIFI_BIT | IEEE802154_BIT, 0);
        let mut wifi_due = ScriptedClock::new([
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 2,
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 3,
        ]);
        let evaluation = state.evaluate_immediate_tracking(&mut wifi_due).unwrap();

        assert_eq!(wifi_due.samples_consumed(), 3);
        let request = evaluation.request().unwrap();
        assert!(request.wifi());
        assert!(request.bluetooth_ieee802154());
        let snapshot = evaluation.owner().snapshot();
        assert_eq!(
            snapshot.previous_micros(PhyPllTrackClass::Wifi),
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 2
        );
        assert_eq!(
            snapshot.previous_micros(PhyPllTrackClass::BluetoothIeee802154),
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 3
        );

        let state = state_for_mask(WIFI_BIT | IEEE802154_BIT, 0);
        let mut bluetooth_ieee_due = ScriptedClock::new([
            DEFAULT_PLL_TRACK_PERIOD_MICROS,
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 2,
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 3,
        ]);
        let evaluation = state
            .evaluate_immediate_tracking(&mut bluetooth_ieee_due)
            .unwrap();

        assert_eq!(bluetooth_ieee_due.samples_consumed(), 4);
        let snapshot = evaluation.owner().snapshot();
        assert_eq!(
            snapshot.previous_micros(PhyPllTrackClass::Wifi),
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 2
        );
        assert_eq!(
            snapshot.previous_micros(PhyPllTrackClass::BluetoothIeee802154),
            DEFAULT_PLL_TRACK_PERIOD_MICROS + 3
        );
    }

    #[test]
    fn periodic_callback_requests_active_classes_without_due_check() {
        let state = state_for_mask(IEEE802154_BIT, 0);
        let evaluation = state.evaluate_periodic_at(1).unwrap();
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
    fn periodic_tracking_samples_each_active_class_once_without_due_samples() {
        let state = state_for_mask(WIFI_BIT | IEEE802154_BIT, 0);
        let mut clock = ScriptedClock::new([17, 23]);
        let evaluation = state.evaluate_periodic_tracking(&mut clock).unwrap();

        assert_eq!(clock.samples_consumed(), 2);
        let snapshot = evaluation.owner().snapshot();
        assert_eq!(snapshot.previous_micros(PhyPllTrackClass::Wifi), 17);
        assert_eq!(
            snapshot.previous_micros(PhyPllTrackClass::BluetoothIeee802154),
            23
        );
    }

    #[test]
    fn pending_request_cannot_release_owner_without_explicit_resolution() {
        let outcome = PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
            .acquire_at(
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
            .evaluate_periodic_at(1)
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
    fn pending_request_runs_outer_tracking_before_owner_recovery() {
        let pending = match state_for_mask(IEEE802154_BIT, 0)
            .evaluate_periodic_at(2_000_000)
            .unwrap()
            .into_owner()
        {
            Ok(_) => panic!("periodic IEEE tracking must retain the owner"),
            Err(pending) => pending,
        };
        let mut tracking = pending.begin_tracking(PhyParamTrackingPolicy {
            tracking_inhibited: true,
            rfpll_cap_tracking_enabled: true,
            rfpll_cap_tracking_threshold: None,
            calibration_tracking_threshold: None,
            diagnostics: crate::phy_param_tracking::PhyTrackingDiagnostics::Enabled,
            bluetooth_ieee802154_power_tracking_enabled: true,
            calibration_tracking_enabled: true,
            relaxed_power_tracking_threshold: false,
        });

        assert_eq!(tracking.action(), PhyParamTrackingAction::EnterCritical);
        assert_eq!(
            tracking.advance(PhyParamTrackingCompletion::TemperatureRead),
            Err(PhyParamTrackingTransitionError::WrongCompletion)
        );
        assert_eq!(tracking.action(), PhyParamTrackingAction::EnterCritical);
        tracking
            .advance(PhyParamTrackingCompletion::EnteredCritical)
            .unwrap();
        assert_eq!(tracking.action(), PhyParamTrackingAction::ExitCritical);
        let mut tracking = match tracking.into_owner() {
            Ok(_) => panic!("owner escaped before the terminal action"),
            Err(tracking) => tracking,
        };
        tracking
            .advance(PhyParamTrackingCompletion::ExitedCritical)
            .unwrap();
        let owner = tracking.into_owner().unwrap();
        assert!(owner.snapshot().contains(PhyModemClient::Ieee802154));
    }

    #[test]
    fn periodic_outer_tracking_commits_power_i2c_and_temperature_children() {
        let pending = match state_for_mask(WIFI_BIT | IEEE802154_BIT, 0)
            .evaluate_periodic_at(2_000_000)
            .unwrap()
            .into_owner()
        {
            Ok(_) => panic!("periodic shared tracking must retain the owner"),
            Err(pending) => pending,
        };
        let policy = PhyParamTrackingPolicy {
            tracking_inhibited: false,
            rfpll_cap_tracking_enabled: false,
            rfpll_cap_tracking_threshold: None,
            calibration_tracking_threshold: None,
            diagnostics: crate::phy_param_tracking::PhyTrackingDiagnostics::Enabled,
            bluetooth_ieee802154_power_tracking_enabled: true,
            calibration_tracking_enabled: false,
            relaxed_power_tracking_threshold: false,
        };
        let mut tracking = pending.begin_tracking(policy);
        let mut state = crate::phy_state::PhyState::new(crate::phy_state::PhyConfig::production());
        state.apply_temperature_outcome(crate::phy_temperature::PhyTemperatureOutcome {
            temperature: 95,
            sensor_index: 3,
            next_dac: 4,
        });
        state.apply_channel_outcome(crate::phy_channel::PhyChipChannelOutcome {
            channel: 11,
            frequency_mhz: 2_462,
            cbw: 1,
            init_complete: true,
            temperature: crate::phy_temperature::PhyTemperatureOutcome {
                temperature: 95,
                sensor_index: 3,
                next_dac: 4,
            },
        });

        assert_eq!(
            tracking.begin_tx_power_tracking(&mut state).unwrap_err(),
            PhyParamTrackingChildError::UnsupportedAction
        );
        assert_eq!(
            tracking.begin_rfpll_cap_tracking(&mut state).unwrap_err(),
            PhyParamTrackingChildError::UnsupportedAction
        );
        assert_eq!(
            tracking.begin_calibration_tracking(&mut state).unwrap_err(),
            PhyParamTrackingChildError::UnsupportedAction
        );
        assert_eq!(
            tracking.begin_wifi_i2c_tracking(&mut state).unwrap_err(),
            PhyParamTrackingChildError::UnsupportedAction
        );
        tracking
            .advance(PhyParamTrackingCompletion::EnteredCritical)
            .unwrap();
        let before = state.tx_power_tracking_parameters(false);
        let bluetooth = tracking.begin_tx_power_tracking(&mut state).unwrap();
        let bluetooth = match bluetooth.commit() {
            Ok(_) => panic!("incomplete TX-power child minted a parent completion"),
            Err(bluetooth) => bluetooth,
        };
        assert_eq!(
            bluetooth.state().tx_power_tracking_parameters(false),
            before
        );
        let completion = complete_power_child(bluetooth);
        tracking.advance(completion).unwrap();
        assert_eq!(state.bluetooth_tx_gain_parameters().base, 19);
        assert_eq!(tracking.action(), PhyParamTrackingAction::WifiI2cTrack);

        let i2c = tracking.begin_wifi_i2c_tracking(&mut state).unwrap();
        let i2c = match i2c.commit() {
            Ok(_) => panic!("incomplete Wi-Fi I2C child minted a parent completion"),
            Err(i2c) => i2c,
        };
        assert_eq!(
            i2c.state().wifi_i2c_tracking_parameters().previous_band,
            crate::phy_i2c_tracking::PhyWifiI2cTrackingBand::Nominal
        );
        let completion = complete_wifi_i2c_child(i2c);
        tracking.advance(completion).unwrap();
        assert_eq!(
            state.wifi_i2c_tracking_parameters().previous_band,
            crate::phy_i2c_tracking::PhyWifiI2cTrackingBand::Hot
        );
        let wifi = tracking.begin_tx_power_tracking(&mut state).unwrap();
        assert_eq!(
            wifi.action(),
            crate::phy_power_tracking::PhyTxPowerTrackingAction::SetBbpllCalibration {
                enabled: true,
            }
        );
        let completion = complete_power_child(wifi);
        tracking.advance(completion).unwrap();
        assert_eq!(state.channel_parameters().tx_gain_base, 16);
        assert_eq!(tracking.action(), PhyParamTrackingAction::TemperatureRead);

        let temperature = tracking.begin_temperature_read(&mut state).unwrap();
        let completion = complete_temperature_child(temperature, 15, 50);
        tracking.advance(completion).unwrap();
        assert_eq!(
            state
                .tx_power_tracking_parameters(false)
                .current_temperature,
            1
        );
        assert_eq!(tracking.action(), PhyParamTrackingAction::ExitCritical);
    }

    fn complete_power_child(
        mut child: PhyParamTrackingTxPowerTransition<'_>,
    ) -> PhyParamTrackingCompletion {
        loop {
            if !matches!(
                child.action(),
                crate::phy_power_tracking::PhyTxPowerTrackingAction::Complete(_)
            ) {
                let binding = child.lower_external().unwrap();
                assert_eq!(binding.action(), child.action());
            }
            let completion = match child.action() {
                crate::phy_power_tracking::PhyTxPowerTrackingAction::SetBbpllCalibration {
                    enabled,
                } => crate::phy_power_tracking::PhyTxPowerTrackingCompletion::BbpllCalibrationSet {
                    enabled,
                },
                crate::phy_power_tracking::PhyTxPowerTrackingAction::RegenerateWifiGain {
                    channel,
                    gain_base,
                } => crate::phy_power_tracking::PhyTxPowerTrackingCompletion::WifiGainRegenerated {
                    channel,
                    gain_base,
                },
                crate::phy_power_tracking::PhyTxPowerTrackingAction::RegenerateBluetoothIeee802154Gain {
                    gain_base,
                } => crate::phy_power_tracking::PhyTxPowerTrackingCompletion::BluetoothIeee802154GainRegenerated {
                    gain_base,
                },
                crate::phy_power_tracking::PhyTxPowerTrackingAction::Complete(_) => {
                    return child.commit().unwrap();
                }
            };
            child.advance(completion).unwrap();
        }
    }

    fn complete_wifi_i2c_child(
        mut child: PhyParamTrackingWifiI2cTransition<'_>,
    ) -> PhyParamTrackingCompletion {
        loop {
            match child.action() {
                crate::phy_i2c_tracking::PhyWifiI2cTrackingAction::MaskedWrite(action) => {
                    let binding = child.lower_external().unwrap();
                    let completion = match action {
                        crate::phy_i2c::MaskedI2cWriteAction::ReadByte { address } => {
                            assert!(matches!(
                                binding.action(),
                                crate::phy_cold::PhyColdI2cAction::StartRead {
                                    address: bound_address
                                } if bound_address == address
                            ));
                            crate::phy_i2c_tracking::PhyWifiI2cTrackingCompletion::MaskedWrite(
                                crate::phy_i2c::MaskedI2cWriteCompletion::I2cReadCompleted {
                                    address,
                                    value: 0xa0,
                                },
                            )
                        }
                        crate::phy_i2c::MaskedI2cWriteAction::WriteByte { address, value } => {
                            assert!(matches!(
                                binding.action(),
                                crate::phy_cold::PhyColdI2cAction::StartWrite {
                                    address: bound_address,
                                    value: bound_value,
                                } if bound_address == address && bound_value == value
                            ));
                            crate::phy_i2c_tracking::PhyWifiI2cTrackingCompletion::MaskedWrite(
                                crate::phy_i2c::MaskedI2cWriteCompletion::I2cWriteCompleted {
                                    address,
                                },
                            )
                        }
                        crate::phy_i2c::MaskedI2cWriteAction::Complete => unreachable!(),
                    };
                    child.advance(completion).unwrap();
                }
                crate::phy_i2c_tracking::PhyWifiI2cTrackingAction::Complete(_) => {
                    return child.commit().unwrap();
                }
            }
        }
    }

    fn complete_temperature_child(
        mut child: PhyParamTrackingTemperatureTransition<'_>,
        dac: u8,
        code: u8,
    ) -> PhyParamTrackingCompletion {
        assert!(matches!(
            child.lower_external(),
            Ok(crate::phy_temperature::PhyTemperatureExternalBinding::I2c(
                _
            ))
        ));
        let crate::phy_temperature::PhyTemperatureAction::ReadMasked { field } = child.action()
        else {
            panic!("temperature child did not begin with its DAC read")
        };
        child
            .advance(
                crate::phy_temperature::PhyTemperatureCompletion::MaskedRead { field, value: dac },
            )
            .unwrap();
        assert!(matches!(
            child.lower_external(),
            Ok(crate::phy_temperature::PhyTemperatureExternalBinding::Sample(_))
        ));
        child
            .advance(crate::phy_temperature::PhyTemperatureCompletion::CodeSampled { value: code })
            .unwrap();
        child.commit().unwrap()
    }

    #[test]
    fn time_reversal_rejects_acquire_and_restores_exact_owner() {
        let state = complete_acquire_for_test(
            PhyClientState::new_empty(DEFAULT_PLL_TRACK_PERIOD_MICROS)
                .acquire_at(PhyModemClient::Wifi, DEFAULT_PLL_TRACK_PERIOD_MICROS + 1)
                .unwrap(),
        );
        let before = state.snapshot();
        let failure = state.acquire_at(PhyModemClient::Ieee802154, 0).unwrap_err();
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
                .acquire_at(
                    PhyModemClient::Ieee802154,
                    DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
                )
                .unwrap(),
        );
        let before = state.snapshot();
        let failure = state.evaluate_immediate_at(0).unwrap_err();
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
                .acquire_at(
                    PhyModemClient::Ieee802154,
                    DEFAULT_PLL_TRACK_PERIOD_MICROS + 1,
                )
                .unwrap(),
        );
        let before = state.snapshot();
        let failure = state.evaluate_periodic_at(0).unwrap_err();
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
