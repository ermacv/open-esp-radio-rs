//! Executor- and chip-independent station power-save signalling policy.
//!
//! The policy is deliberately split from both the IEEE 802.11 encoder and the
//! ESP32-S31 sleep transaction. A TIM observation can start a PM=1 exchange,
//! but it cannot produce a doze permit until the shared TX owner reports an
//! acknowledged Null Data MPDU. The returned permit is expressed in the
//! station TSF clock domain so executor queue latency cannot move the wake
//! edge past the next mandatory listen/DTIM TBTT.

use open_esp_radio_ieee80211::{
    station_beacon::{StaBeaconObservation, StaTimObservation},
    station_power_save::StaPowerManagement,
};

use crate::request::StationListenInterval;

const TU_MICROS: u64 = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPowerSavePolicyError {
    ZeroBeaconInterval,
    ZeroBeaconMissLimit,
    WakeGuardOutsideBeaconInterval,
    ListenIntervalExceedsBeaconLoss {
        listen_interval: u16,
        beacon_miss_limit: u8,
    },
}

/// Association-owned timing policy. Beacon frames may refresh traffic state,
/// but cannot alter this interval because infrastructure beacons are not
/// cryptographically protected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPowerSavePolicy {
    beacon_interval_tu: u16,
    beacon_interval_micros: u64,
    listen_interval: StationListenInterval,
    beacon_miss_limit: u8,
    wake_guard_micros: u32,
}

impl StaPowerSavePolicy {
    pub const fn new(
        beacon_interval_tu: u16,
        wake_guard_micros: u32,
    ) -> Result<Self, StaPowerSavePolicyError> {
        let listen_interval = match StationListenInterval::new(1) {
            Some(interval) => interval,
            None => unreachable!(),
        };
        Self::for_association(beacon_interval_tu, listen_interval, wake_guard_micros, 1)
    }

    /// Build policy from association timing and the connected link-loss
    /// bound. A station may intentionally skip beacons only while the next
    /// mandatory listen edge remains inside that bound.
    pub const fn for_association(
        beacon_interval_tu: u16,
        listen_interval: StationListenInterval,
        wake_guard_micros: u32,
        beacon_miss_limit: u8,
    ) -> Result<Self, StaPowerSavePolicyError> {
        if beacon_interval_tu == 0 {
            return Err(StaPowerSavePolicyError::ZeroBeaconInterval);
        }
        let beacon_interval_micros = beacon_interval_tu as u64 * TU_MICROS;
        if wake_guard_micros as u64 >= beacon_interval_micros {
            return Err(StaPowerSavePolicyError::WakeGuardOutsideBeaconInterval);
        }
        if beacon_miss_limit == 0 {
            return Err(StaPowerSavePolicyError::ZeroBeaconMissLimit);
        }
        if listen_interval.get() > beacon_miss_limit as u16 {
            return Err(StaPowerSavePolicyError::ListenIntervalExceedsBeaconLoss {
                listen_interval: listen_interval.get(),
                beacon_miss_limit,
            });
        }
        Ok(Self {
            beacon_interval_tu,
            beacon_interval_micros,
            listen_interval,
            beacon_miss_limit,
            wake_guard_micros,
        })
    }

    pub const fn beacon_interval_tu(self) -> u16 {
        self.beacon_interval_tu
    }

    pub const fn beacon_interval_micros(self) -> u64 {
        self.beacon_interval_micros
    }

    pub const fn wake_guard_micros(self) -> u32 {
        self.wake_guard_micros
    }

    pub const fn listen_interval(self) -> StationListenInterval {
        self.listen_interval
    }

    pub const fn beacon_miss_limit(self) -> u8 {
        self.beacon_miss_limit
    }
}

/// One coherent observation supplied by a runtime at an idle scheduling
/// boundary. `station_tsf` must be sampled from the hardware STA TSF domain,
/// not synthesized from Embassy time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPowerSaveOpportunity {
    pub beacon: StaBeaconObservation,
    pub station_tsf: u64,
    pub traffic: StaTrafficState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTrafficState {
    /// No hardware TX owns the descriptor and the network/control queues were
    /// observed empty at the same runner scheduling boundary.
    Quiescent,
    Pending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPowerSaveState {
    Awake,
    AdvertisingPowerSave,
    PowerSave,
    AdvertisingActive,
}

/// Affine service state for one legacy PS-Poll exchange.
///
/// At most one PS-Poll may own TX or await its corresponding unicast MPDU.
/// A new TIM, local-TX request, stop edge or unexpected delivery aborts this
/// state before the planner advertises PM=0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPsPollServiceState {
    Idle,
    Transmitting,
    AwaitingDelivery,
}

/// Association-owned reason selecting the next mandatory receive edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaDozeWakeReason {
    ListenInterval,
    Dtim,
    ListenIntervalAndDtim,
}

/// A single-use authorization for the chip-specific sleep owner.
///
/// This value does not itself touch RF, PHY, clocks or wake registers. Before
/// consuming it, the platform owner must still confirm that `wake_tsf` is in
/// the future in the live station TSF domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaDozePermit {
    pub beacon_timestamp_tsf: u64,
    pub next_listen_tsf: u64,
    pub next_dtim_tsf: u64,
    pub wake_tsf: u64,
    pub wake_after_beacons: u16,
    pub wake_reason: StaDozeWakeReason,
    pub dtim_count: u8,
    pub dtim_period: u8,
}

/// Failure to bind a logical doze permit to the live TSF clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaDozePrepareError {
    WakeDeadlinePassed,
    InvalidTimPhase,
    InvalidWakeGeometry,
}

/// Affine, live authorization presented to a chip-specific doze leaf.
///
/// This token is intentionally neither `Copy` nor `Clone`. Entry consumes it;
/// a failed entry returns the same token so the owner can retry or roll back
/// without manufacturing authority.
pub struct StaPreparedDoze {
    permit: StaDozePermit,
}

impl StaPreparedDoze {
    pub fn prepare(permit: StaDozePermit, station_tsf: u64) -> Result<Self, StaDozePrepareError> {
        if permit.dtim_period == 0 || permit.dtim_count >= permit.dtim_period {
            return Err(StaDozePrepareError::InvalidTimPhase);
        }
        let Some(wake_distance) = future_tsf_distance(station_tsf, permit.wake_tsf) else {
            return Err(StaDozePrepareError::WakeDeadlinePassed);
        };
        let Some(listen_distance) = future_tsf_distance(station_tsf, permit.next_listen_tsf) else {
            return Err(StaDozePrepareError::InvalidWakeGeometry);
        };
        let Some(dtim_distance) = future_tsf_distance(station_tsf, permit.next_dtim_tsf) else {
            return Err(StaDozePrepareError::InvalidWakeGeometry);
        };
        let expected_reason = match listen_distance.cmp(&dtim_distance) {
            core::cmp::Ordering::Less => StaDozeWakeReason::ListenInterval,
            core::cmp::Ordering::Equal => StaDozeWakeReason::ListenIntervalAndDtim,
            core::cmp::Ordering::Greater => StaDozeWakeReason::Dtim,
        };
        if permit.wake_after_beacons == 0
            || wake_distance > listen_distance
            || wake_distance > dtim_distance
            || permit.wake_reason != expected_reason
        {
            return Err(StaDozePrepareError::InvalidWakeGeometry);
        }
        Ok(Self { permit })
    }

    pub const fn permit(&self) -> &StaDozePermit {
        &self.permit
    }

    /// Enter hardware doze using an explicit leaf supplied by the platform
    /// owner. Success creates the only token that authorizes restoration.
    pub fn enter_with<H, E>(
        self,
        hardware: &mut H,
        enter: impl FnOnce(&mut H, &StaDozePermit) -> Result<(), E>,
    ) -> Result<StaDozeRestore, StaDozeEntryFailure<E>> {
        match enter(hardware, &self.permit) {
            Ok(()) => Ok(StaDozeRestore {
                permit: self.permit,
            }),
            Err(error) => Err(StaDozeEntryFailure {
                error,
                prepared: self,
            }),
        }
    }
}

/// Failed hardware entry with the unconsumed logical authority returned.
pub struct StaDozeEntryFailure<E> {
    pub error: E,
    pub prepared: StaPreparedDoze,
}

/// Unique proof that hardware doze entry succeeded and must be restored
/// before control/network TX, reconnect or station shutdown.
pub struct StaDozeRestore {
    permit: StaDozePermit,
}

impl StaDozeRestore {
    pub const fn permit(&self) -> &StaDozePermit {
        &self.permit
    }

    pub fn restore_with<H, E>(
        self,
        hardware: &mut H,
        restore: impl FnOnce(&mut H) -> Result<(), E>,
    ) -> Result<StaDozeRestored, StaDozeRestoreFailure<E>> {
        match restore(hardware) {
            Ok(()) => Ok(StaDozeRestored {
                permit: self.permit,
            }),
            Err(error) => Err(StaDozeRestoreFailure {
                error,
                restore: self,
            }),
        }
    }
}

/// Failed restoration with the obligation token returned intact.
pub struct StaDozeRestoreFailure<E> {
    pub error: E,
    pub restore: StaDozeRestore,
}

/// Receipt proving that one entered doze transaction was restored.
pub struct StaDozeRestored {
    permit: StaDozePermit,
}

impl StaDozeRestored {
    pub const fn permit(&self) -> &StaDozePermit {
        &self.permit
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaStayAwakeReason {
    TrafficPending,
    MissingTim,
    InvalidTimPhase,
    UnicastBuffered,
    GroupBuffered,
    PowerManagementTxPending,
    NoFreshDozeWindow,
    WakeDeadlinePassed,
    AlreadyAwake,
    PsPollServicePending,
    PsPollServiceComplete,
    PsPollServiceRace,
    PsPollDeliveryTimeout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPowerSaveDecision {
    StayAwake(StaStayAwakeReason),
    SendPowerManagement(StaPowerManagement),
    /// Publish one legacy PS-Poll while retaining AP-visible PM=1.
    SendPsPoll,
    /// Keep the receiver awake until one associated unicast MPDU arrives.
    AwaitPsPollDelivery {
        timeout_micros: u64,
    },
    PermitDoze(StaDozePermit),
}

impl StaPowerSaveDecision {
    /// Whether safe progress requires leaving AP-visible power-save rather
    /// than merely keeping the modem awake. Group delivery after a DTIM and a
    /// bounded PS-Poll service exchange are both received while PM=1.
    pub const fn requires_active_advertisement(self) -> bool {
        matches!(
            self,
            Self::StayAwake(
                StaStayAwakeReason::TrafficPending
                    | StaStayAwakeReason::MissingTim
                    | StaStayAwakeReason::InvalidTimPhase
                    | StaStayAwakeReason::UnicastBuffered
                    | StaStayAwakeReason::WakeDeadlinePassed
                    | StaStayAwakeReason::PsPollServiceRace
                    | StaStayAwakeReason::PsPollDeliveryTimeout
            )
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPsPollTxOutcome {
    Acknowledged,
    Failed,
}

/// Terminal outcome of the single in-flight PS-Poll transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPsPollTxCompletion {
    pub outcome: StaPsPollTxOutcome,
}

/// One BSSID- and receiver-validated protected unicast MPDU delivered while
/// the PS-Poll owner is awake. `more_data` is the RX-observed MAC-header hint
/// selecting whether another poll is required; it grants neither a doze
/// permit nor unbounded service authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPsPollDelivery {
    pub more_data: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPsPollServiceEdge {
    TxCompletion,
    Delivery,
    DeliveryTimeout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnexpectedStaPsPollServiceEdge {
    pub power_save_state: StaPowerSaveState,
    pub service_state: StaPsPollServiceState,
    pub edge: StaPsPollServiceEdge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPowerManagementTxOutcome {
    Acknowledged,
    Failed,
}

/// Complete result of the one in-flight PM Null Data transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPowerManagementTxCompletion {
    pub advertised: StaPowerManagement,
    pub outcome: StaPowerManagementTxOutcome,
    /// Live station TSF sampled after TX completion was observed.
    pub station_tsf: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnexpectedStaPowerManagementCompletion {
    pub state: StaPowerSaveState,
    pub advertised: StaPowerManagement,
}

/// Pure finite owner of the AP-visible PM state and the latest safe TIM
/// window. Hardware doze is intentionally outside this type.
pub struct StaPowerSavePlanner {
    policy: StaPowerSavePolicy,
    state: StaPowerSaveState,
    ps_poll: StaPsPollServiceState,
    candidate: Option<StaDozePermit>,
}

impl StaPowerSavePlanner {
    pub const fn new(policy: StaPowerSavePolicy) -> Self {
        Self {
            policy,
            state: StaPowerSaveState::Awake,
            ps_poll: StaPsPollServiceState::Idle,
            candidate: None,
        }
    }

    pub const fn policy(&self) -> StaPowerSavePolicy {
        self.policy
    }

    pub const fn state(&self) -> StaPowerSaveState {
        self.state
    }

    pub const fn candidate(&self) -> Option<StaDozePermit> {
        self.candidate
    }

    pub const fn ps_poll_state(&self) -> StaPsPollServiceState {
        self.ps_poll
    }

    /// Consume a BSSID-authenticated beacon at a runner-owned traffic
    /// boundary. Unsafe or incomplete observations fail closed and erase a
    /// previously cached permit.
    pub fn observe_beacon(&mut self, opportunity: StaPowerSaveOpportunity) -> StaPowerSaveDecision {
        if opportunity.traffic == StaTrafficState::Pending {
            self.candidate = None;
            self.ps_poll = StaPsPollServiceState::Idle;
            return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::TrafficPending);
        }
        let tim = match opportunity.beacon.tim {
            Some(tim) => tim,
            None => {
                self.candidate = None;
                self.ps_poll = StaPsPollServiceState::Idle;
                return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::MissingTim);
            }
        };
        if !valid_tim_phase(tim) {
            self.candidate = None;
            self.ps_poll = StaPsPollServiceState::Idle;
            return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::InvalidTimPhase);
        }
        if self.ps_poll != StaPsPollServiceState::Idle {
            self.candidate = None;
            self.ps_poll = StaPsPollServiceState::Idle;
            return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::PsPollServiceRace);
        }
        if tim.unicast_buffered {
            self.candidate = None;
            if self.state == StaPowerSaveState::PowerSave {
                self.ps_poll = StaPsPollServiceState::Transmitting;
                return StaPowerSaveDecision::SendPsPoll;
            }
            return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::UnicastBuffered);
        }
        if tim.group_buffered {
            self.candidate = None;
            return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::GroupBuffered);
        }

        let listen_beacons = self.policy.listen_interval.get();
        let dtim_beacons = if tim.dtim_count == 0 {
            u16::from(tim.dtim_period)
        } else {
            u16::from(tim.dtim_count)
        };
        let wake_after_beacons = listen_beacons.min(dtim_beacons);
        let wake_reason = match listen_beacons.cmp(&dtim_beacons) {
            core::cmp::Ordering::Less => StaDozeWakeReason::ListenInterval,
            core::cmp::Ordering::Equal => StaDozeWakeReason::ListenIntervalAndDtim,
            core::cmp::Ordering::Greater => StaDozeWakeReason::Dtim,
        };
        let next_listen_tsf = opportunity.beacon.timestamp_tsf.wrapping_add(
            self.policy
                .beacon_interval_micros
                .wrapping_mul(u64::from(listen_beacons)),
        );
        let next_dtim_tsf = opportunity.beacon.timestamp_tsf.wrapping_add(
            self.policy
                .beacon_interval_micros
                .wrapping_mul(u64::from(dtim_beacons)),
        );
        let permit = StaDozePermit {
            beacon_timestamp_tsf: opportunity.beacon.timestamp_tsf,
            next_listen_tsf,
            next_dtim_tsf,
            wake_tsf: opportunity
                .beacon
                .timestamp_tsf
                .wrapping_add(
                    self.policy
                        .beacon_interval_micros
                        .wrapping_mul(u64::from(wake_after_beacons)),
                )
                .wrapping_sub(u64::from(self.policy.wake_guard_micros)),
            wake_after_beacons,
            wake_reason,
            dtim_count: tim.dtim_count,
            dtim_period: tim.dtim_period,
        };
        if !strictly_future_tsf(opportunity.station_tsf, permit.wake_tsf) {
            self.candidate = None;
            return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::WakeDeadlinePassed);
        }
        self.candidate = Some(permit);

        match self.state {
            StaPowerSaveState::Awake => {
                self.state = StaPowerSaveState::AdvertisingPowerSave;
                StaPowerSaveDecision::SendPowerManagement(StaPowerManagement::PowerSave)
            }
            StaPowerSaveState::AdvertisingPowerSave | StaPowerSaveState::AdvertisingActive => {
                StaPowerSaveDecision::StayAwake(StaStayAwakeReason::PowerManagementTxPending)
            }
            StaPowerSaveState::PowerSave => StaPowerSaveDecision::PermitDoze(permit),
        }
    }

    /// Commit or roll back AP-visible state only after the bounded shared TX
    /// transaction has produced its final outcome.
    pub fn complete_power_management(
        &mut self,
        completion: StaPowerManagementTxCompletion,
    ) -> Result<StaPowerSaveDecision, UnexpectedStaPowerManagementCompletion> {
        let expected = match self.state {
            StaPowerSaveState::AdvertisingPowerSave => StaPowerManagement::PowerSave,
            StaPowerSaveState::AdvertisingActive => StaPowerManagement::Active,
            state => {
                return Err(UnexpectedStaPowerManagementCompletion {
                    state,
                    advertised: completion.advertised,
                });
            }
        };
        if completion.advertised != expected {
            return Err(UnexpectedStaPowerManagementCompletion {
                state: self.state,
                advertised: completion.advertised,
            });
        }

        match (completion.advertised, completion.outcome) {
            (StaPowerManagement::PowerSave, StaPowerManagementTxOutcome::Failed) => {
                self.state = StaPowerSaveState::Awake;
                self.ps_poll = StaPsPollServiceState::Idle;
                self.candidate = None;
                Ok(StaPowerSaveDecision::StayAwake(
                    StaStayAwakeReason::NoFreshDozeWindow,
                ))
            }
            (StaPowerManagement::PowerSave, StaPowerManagementTxOutcome::Acknowledged) => {
                self.state = StaPowerSaveState::PowerSave;
                Ok(self.take_live_candidate(completion.station_tsf))
            }
            (StaPowerManagement::Active, StaPowerManagementTxOutcome::Acknowledged) => {
                self.state = StaPowerSaveState::Awake;
                self.ps_poll = StaPsPollServiceState::Idle;
                self.candidate = None;
                Ok(StaPowerSaveDecision::StayAwake(
                    StaStayAwakeReason::AlreadyAwake,
                ))
            }
            (StaPowerManagement::Active, StaPowerManagementTxOutcome::Failed) => {
                // The radio is awake, but the AP must still conservatively be
                // treated as believing that this station is in power-save.
                self.state = StaPowerSaveState::PowerSave;
                self.ps_poll = StaPsPollServiceState::Idle;
                self.candidate = None;
                Ok(StaPowerSaveDecision::StayAwake(
                    StaStayAwakeReason::NoFreshDozeWindow,
                ))
            }
        }
    }

    /// Begin the AP-visible return to continuously active operation. The
    /// radio must already be awake before this decision is acted upon.
    pub fn request_active(&mut self) -> StaPowerSaveDecision {
        self.candidate = None;
        self.ps_poll = StaPsPollServiceState::Idle;
        match self.state {
            StaPowerSaveState::PowerSave => {
                self.state = StaPowerSaveState::AdvertisingActive;
                StaPowerSaveDecision::SendPowerManagement(StaPowerManagement::Active)
            }
            StaPowerSaveState::Awake => {
                StaPowerSaveDecision::StayAwake(StaStayAwakeReason::AlreadyAwake)
            }
            StaPowerSaveState::AdvertisingPowerSave | StaPowerSaveState::AdvertisingActive => {
                StaPowerSaveDecision::StayAwake(StaStayAwakeReason::PowerManagementTxPending)
            }
        }
    }

    /// Commit one bounded PS-Poll TX result. An ACK only opens the receive
    /// window; it is not evidence that the buffered MPDU was delivered.
    pub fn complete_ps_poll(
        &mut self,
        completion: StaPsPollTxCompletion,
    ) -> Result<StaPowerSaveDecision, UnexpectedStaPsPollServiceEdge> {
        if self.state != StaPowerSaveState::PowerSave
            || self.ps_poll != StaPsPollServiceState::Transmitting
        {
            return Err(self.unexpected_ps_poll_edge(StaPsPollServiceEdge::TxCompletion));
        }
        match completion.outcome {
            StaPsPollTxOutcome::Acknowledged => {
                self.ps_poll = StaPsPollServiceState::AwaitingDelivery;
                Ok(StaPowerSaveDecision::AwaitPsPollDelivery {
                    // This is a fail-safe runtime service bound, not a claim
                    // about an AP's SIFS response timing. Missing one complete
                    // association-owned beacon interval restores PM=0.
                    timeout_micros: self.policy.beacon_interval_micros,
                })
            }
            StaPsPollTxOutcome::Failed => {
                self.ps_poll = StaPsPollServiceState::Idle;
                Ok(self.request_active())
            }
        }
    }

    /// Consume exactly one associated unicast delivery for the outstanding
    /// poll. `MoreData` retains PM=1 and starts the next bounded poll.
    pub fn observe_ps_poll_delivery(
        &mut self,
        delivery: StaPsPollDelivery,
    ) -> Result<StaPowerSaveDecision, UnexpectedStaPsPollServiceEdge> {
        if self.state != StaPowerSaveState::PowerSave
            || self.ps_poll != StaPsPollServiceState::AwaitingDelivery
        {
            return Err(self.unexpected_ps_poll_edge(StaPsPollServiceEdge::Delivery));
        }
        self.candidate = None;
        if delivery.more_data {
            self.ps_poll = StaPsPollServiceState::Transmitting;
            Ok(StaPowerSaveDecision::SendPsPoll)
        } else {
            self.ps_poll = StaPsPollServiceState::Idle;
            Ok(StaPowerSaveDecision::StayAwake(
                StaStayAwakeReason::PsPollServiceComplete,
            ))
        }
    }

    /// Expire a receive window that did not produce its corresponding MPDU.
    /// The returned decision always begins acknowledged PM=0 restoration.
    pub fn expire_ps_poll_delivery(
        &mut self,
    ) -> Result<StaPowerSaveDecision, UnexpectedStaPsPollServiceEdge> {
        if self.state != StaPowerSaveState::PowerSave
            || self.ps_poll != StaPsPollServiceState::AwaitingDelivery
        {
            return Err(self.unexpected_ps_poll_edge(StaPsPollServiceEdge::DeliveryTimeout));
        }
        self.ps_poll = StaPsPollServiceState::Idle;
        Ok(self.request_active())
    }

    /// Abort an unsupported or raced service edge before falling back to the
    /// existing acknowledged PM=0 transaction.
    pub fn abort_ps_poll_service(&mut self) -> StaPowerSaveDecision {
        self.ps_poll = StaPsPollServiceState::Idle;
        self.request_active()
    }

    const fn unexpected_ps_poll_edge(
        &self,
        edge: StaPsPollServiceEdge,
    ) -> UnexpectedStaPsPollServiceEdge {
        UnexpectedStaPsPollServiceEdge {
            power_save_state: self.state,
            service_state: self.ps_poll,
            edge,
        }
    }

    fn take_live_candidate(&mut self, station_tsf: u64) -> StaPowerSaveDecision {
        match self.candidate.take() {
            Some(permit) if strictly_future_tsf(station_tsf, permit.wake_tsf) => {
                StaPowerSaveDecision::PermitDoze(permit)
            }
            Some(_) => StaPowerSaveDecision::StayAwake(StaStayAwakeReason::WakeDeadlinePassed),
            None => StaPowerSaveDecision::StayAwake(StaStayAwakeReason::NoFreshDozeWindow),
        }
    }
}

const fn valid_tim_phase(tim: StaTimObservation) -> bool {
    tim.dtim_period != 0 && tim.dtim_count < tim.dtim_period
}

/// Compare two wrapping 64-bit TSF values, accepting only the nearer future
/// half of the counter domain.
const fn strictly_future_tsf(now: u64, deadline: u64) -> bool {
    future_tsf_distance(now, deadline).is_some()
}

const fn future_tsf_distance(now: u64, deadline: u64) -> Option<u64> {
    let distance = deadline.wrapping_sub(now);
    if distance != 0 && distance <= i64::MAX as u64 {
        Some(distance)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
