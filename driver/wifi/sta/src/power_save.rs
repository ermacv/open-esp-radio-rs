//! Executor- and chip-independent station power-save signalling policy.
//!
//! The policy is deliberately split from both the IEEE 802.11 encoder and the
//! ESP32-S31 sleep transaction. A TIM observation can start a PM=1 exchange,
//! but it cannot produce a doze permit until the shared TX owner reports an
//! acknowledged Null Data MPDU. The returned permit is expressed in the
//! station TSF clock domain so executor queue latency cannot move the wake
//! edge past the next TBTT.

use open_esp_radio_ieee80211::{
    station_beacon::{StaBeaconObservation, StaTimObservation},
    station_power_save::StaPowerManagement,
};

const TU_MICROS: u64 = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPowerSavePolicyError {
    ZeroBeaconInterval,
    WakeGuardOutsideBeaconInterval,
}

/// Association-owned timing policy. Beacon frames may refresh traffic state,
/// but cannot alter this interval because infrastructure beacons are not
/// cryptographically protected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPowerSavePolicy {
    beacon_interval_tu: u16,
    beacon_interval_micros: u64,
    wake_guard_micros: u32,
}

impl StaPowerSavePolicy {
    pub const fn new(
        beacon_interval_tu: u16,
        wake_guard_micros: u32,
    ) -> Result<Self, StaPowerSavePolicyError> {
        if beacon_interval_tu == 0 {
            return Err(StaPowerSavePolicyError::ZeroBeaconInterval);
        }
        let beacon_interval_micros = beacon_interval_tu as u64 * TU_MICROS;
        if wake_guard_micros as u64 >= beacon_interval_micros {
            return Err(StaPowerSavePolicyError::WakeGuardOutsideBeaconInterval);
        }
        Ok(Self {
            beacon_interval_tu,
            beacon_interval_micros,
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

/// A single-use authorization for the chip-specific sleep owner.
///
/// This value does not itself touch RF, PHY, clocks or wake registers. Before
/// consuming it, the platform owner must still confirm that `wake_tsf` is in
/// the future in the live station TSF domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaDozePermit {
    pub beacon_timestamp_tsf: u64,
    pub wake_tsf: u64,
    pub dtim_count: u8,
    pub dtim_period: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaStayAwakeReason {
    TrafficPending,
    MissingTim,
    InvalidTimPhase,
    BufferedTraffic,
    PowerManagementTxPending,
    NoFreshDozeWindow,
    WakeDeadlinePassed,
    AlreadyAwake,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPowerSaveDecision {
    StayAwake(StaStayAwakeReason),
    SendPowerManagement(StaPowerManagement),
    PermitDoze(StaDozePermit),
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
    candidate: Option<StaDozePermit>,
}

impl StaPowerSavePlanner {
    pub const fn new(policy: StaPowerSavePolicy) -> Self {
        Self {
            policy,
            state: StaPowerSaveState::Awake,
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

    /// Consume a BSSID-authenticated beacon at a runner-owned traffic
    /// boundary. Unsafe or incomplete observations fail closed and erase a
    /// previously cached permit.
    pub fn observe_beacon(&mut self, opportunity: StaPowerSaveOpportunity) -> StaPowerSaveDecision {
        if opportunity.traffic == StaTrafficState::Pending {
            self.candidate = None;
            return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::TrafficPending);
        }
        let tim = match opportunity.beacon.tim {
            Some(tim) => tim,
            None => {
                self.candidate = None;
                return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::MissingTim);
            }
        };
        if !valid_tim_phase(tim) {
            self.candidate = None;
            return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::InvalidTimPhase);
        }
        if tim.unicast_buffered || tim.group_buffered {
            self.candidate = None;
            return StaPowerSaveDecision::StayAwake(StaStayAwakeReason::BufferedTraffic);
        }

        let permit = StaDozePermit {
            beacon_timestamp_tsf: opportunity.beacon.timestamp_tsf,
            wake_tsf: opportunity
                .beacon
                .timestamp_tsf
                .wrapping_add(self.policy.beacon_interval_micros)
                .wrapping_sub(u64::from(self.policy.wake_guard_micros)),
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
                self.candidate = None;
                Ok(StaPowerSaveDecision::StayAwake(
                    StaStayAwakeReason::AlreadyAwake,
                ))
            }
            (StaPowerManagement::Active, StaPowerManagementTxOutcome::Failed) => {
                // The radio is awake, but the AP must still conservatively be
                // treated as believing that this station is in power-save.
                self.state = StaPowerSaveState::PowerSave;
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
    let distance = deadline.wrapping_sub(now);
    distance != 0 && distance <= i64::MAX as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: StaPowerSavePolicy = match StaPowerSavePolicy::new(100, 2_000) {
        Ok(policy) => policy,
        Err(_) => panic!("valid test policy"),
    };

    fn beacon(tim: Option<StaTimObservation>) -> StaBeaconObservation {
        StaBeaconObservation {
            timestamp_tsf: 1_000_000,
            // Deliberately different: policy must retain the association's
            // protected interval rather than trust this beacon field.
            interval_tu: 500,
            capability_information: 0,
            tim,
        }
    }

    fn tim() -> StaTimObservation {
        StaTimObservation {
            dtim_count: 1,
            dtim_period: 3,
            unicast_buffered: false,
            group_buffered: false,
        }
    }

    fn opportunity(
        tim: Option<StaTimObservation>,
        station_tsf: u64,
        traffic: StaTrafficState,
    ) -> StaPowerSaveOpportunity {
        StaPowerSaveOpportunity {
            beacon: beacon(tim),
            station_tsf,
            traffic,
        }
    }

    #[test]
    fn policy_rejects_vacuous_or_unwakeable_intervals() {
        assert_eq!(
            StaPowerSavePolicy::new(0, 1),
            Err(StaPowerSavePolicyError::ZeroBeaconInterval)
        );
        assert_eq!(
            StaPowerSavePolicy::new(1, 1_024),
            Err(StaPowerSavePolicyError::WakeGuardOutsideBeaconInterval)
        );
        assert_eq!(POLICY.beacon_interval_micros(), 102_400);
    }

    #[test]
    fn doze_requires_acknowledged_pm_one_and_uses_association_interval() {
        let mut planner = StaPowerSavePlanner::new(POLICY);
        assert_eq!(
            planner.observe_beacon(opportunity(
                Some(tim()),
                1_000_100,
                StaTrafficState::Quiescent,
            )),
            StaPowerSaveDecision::SendPowerManagement(StaPowerManagement::PowerSave)
        );
        assert_eq!(planner.state(), StaPowerSaveState::AdvertisingPowerSave);

        let permit = StaDozePermit {
            beacon_timestamp_tsf: 1_000_000,
            wake_tsf: 1_100_400,
            dtim_count: 1,
            dtim_period: 3,
        };
        assert_eq!(planner.candidate(), Some(permit));
        assert_eq!(
            planner
                .complete_power_management(StaPowerManagementTxCompletion {
                    advertised: StaPowerManagement::PowerSave,
                    outcome: StaPowerManagementTxOutcome::Acknowledged,
                    station_tsf: 1_001_000,
                })
                .unwrap(),
            StaPowerSaveDecision::PermitDoze(permit)
        );
        assert_eq!(planner.state(), StaPowerSaveState::PowerSave);
    }

    #[test]
    fn failed_or_late_pm_one_never_grants_doze() {
        let mut failed = StaPowerSavePlanner::new(POLICY);
        failed.observe_beacon(opportunity(
            Some(tim()),
            1_000_100,
            StaTrafficState::Quiescent,
        ));
        assert_eq!(
            failed
                .complete_power_management(StaPowerManagementTxCompletion {
                    advertised: StaPowerManagement::PowerSave,
                    outcome: StaPowerManagementTxOutcome::Failed,
                    station_tsf: 1_001_000,
                })
                .unwrap(),
            StaPowerSaveDecision::StayAwake(StaStayAwakeReason::NoFreshDozeWindow)
        );
        assert_eq!(failed.state(), StaPowerSaveState::Awake);

        let mut late = StaPowerSavePlanner::new(POLICY);
        late.observe_beacon(opportunity(
            Some(tim()),
            1_000_100,
            StaTrafficState::Quiescent,
        ));
        assert_eq!(
            late.complete_power_management(StaPowerManagementTxCompletion {
                advertised: StaPowerManagement::PowerSave,
                outcome: StaPowerManagementTxOutcome::Acknowledged,
                station_tsf: 1_100_400,
            })
            .unwrap(),
            StaPowerSaveDecision::StayAwake(StaStayAwakeReason::WakeDeadlinePassed)
        );
        assert_eq!(late.state(), StaPowerSaveState::PowerSave);
    }

    #[test]
    fn incomplete_unsafe_or_busy_observation_fails_closed() {
        let cases = [
            (
                opportunity(None, 1_000_100, StaTrafficState::Quiescent),
                StaStayAwakeReason::MissingTim,
            ),
            (
                opportunity(
                    Some(StaTimObservation {
                        dtim_count: 3,
                        ..tim()
                    }),
                    1_000_100,
                    StaTrafficState::Quiescent,
                ),
                StaStayAwakeReason::InvalidTimPhase,
            ),
            (
                opportunity(
                    Some(StaTimObservation {
                        unicast_buffered: true,
                        ..tim()
                    }),
                    1_000_100,
                    StaTrafficState::Quiescent,
                ),
                StaStayAwakeReason::BufferedTraffic,
            ),
            (
                opportunity(Some(tim()), 1_000_100, StaTrafficState::Pending),
                StaStayAwakeReason::TrafficPending,
            ),
        ];
        for (input, reason) in cases {
            let mut planner = StaPowerSavePlanner::new(POLICY);
            assert_eq!(
                planner.observe_beacon(input),
                StaPowerSaveDecision::StayAwake(reason)
            );
            assert_eq!(planner.state(), StaPowerSaveState::Awake);
            assert_eq!(planner.candidate(), None);
        }
    }

    #[test]
    fn power_save_beacon_can_grant_one_fresh_permit() {
        let mut planner = StaPowerSavePlanner::new(POLICY);
        planner.observe_beacon(opportunity(
            Some(tim()),
            1_000_100,
            StaTrafficState::Quiescent,
        ));
        planner
            .complete_power_management(StaPowerManagementTxCompletion {
                advertised: StaPowerManagement::PowerSave,
                outcome: StaPowerManagementTxOutcome::Acknowledged,
                station_tsf: 1_100_400,
            })
            .unwrap();

        let decision = planner.observe_beacon(opportunity(
            Some(tim()),
            1_000_200,
            StaTrafficState::Quiescent,
        ));
        assert!(matches!(decision, StaPowerSaveDecision::PermitDoze(_)));
    }

    #[test]
    fn active_transition_is_also_ack_gated() {
        let mut planner = StaPowerSavePlanner::new(POLICY);
        planner.observe_beacon(opportunity(
            Some(tim()),
            1_000_100,
            StaTrafficState::Quiescent,
        ));
        planner
            .complete_power_management(StaPowerManagementTxCompletion {
                advertised: StaPowerManagement::PowerSave,
                outcome: StaPowerManagementTxOutcome::Acknowledged,
                station_tsf: 1_001_000,
            })
            .unwrap();
        assert_eq!(
            planner.request_active(),
            StaPowerSaveDecision::SendPowerManagement(StaPowerManagement::Active)
        );
        assert_eq!(planner.state(), StaPowerSaveState::AdvertisingActive);
        assert_eq!(
            planner
                .complete_power_management(StaPowerManagementTxCompletion {
                    advertised: StaPowerManagement::Active,
                    outcome: StaPowerManagementTxOutcome::Acknowledged,
                    station_tsf: 1_002_000,
                })
                .unwrap(),
            StaPowerSaveDecision::StayAwake(StaStayAwakeReason::AlreadyAwake)
        );
        assert_eq!(planner.state(), StaPowerSaveState::Awake);
    }

    #[test]
    fn unexpected_completion_does_not_mutate_state() {
        let mut planner = StaPowerSavePlanner::new(POLICY);
        assert_eq!(
            planner.complete_power_management(StaPowerManagementTxCompletion {
                advertised: StaPowerManagement::PowerSave,
                outcome: StaPowerManagementTxOutcome::Acknowledged,
                station_tsf: 0,
            }),
            Err(UnexpectedStaPowerManagementCompletion {
                state: StaPowerSaveState::Awake,
                advertised: StaPowerManagement::PowerSave,
            })
        );
        assert_eq!(planner.state(), StaPowerSaveState::Awake);
    }
}
