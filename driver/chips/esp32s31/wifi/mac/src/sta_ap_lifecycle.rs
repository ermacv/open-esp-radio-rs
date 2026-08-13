//! Finite same-channel STA plus SoftAP ownership policy.
//!
//! Register qualification alone cannot prove that the two roles preserve one
//! another while sharing the physical MAC. This state machine is the small,
//! allocation-free policy boundary between independently reviewed register
//! leaves and a future runtime orchestrator. It deliberately performs no MMIO
//! and does not claim scheduler or DMA ownership.

use open_esp_radio_ieee80211::channel::WifiChannel;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApRole {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApLifecycleState {
    Idle,
    Station { channel: WifiChannel },
    AccessPoint { channel: WifiChannel },
    Concurrent { channel: WifiChannel },
}

impl StaApLifecycleState {
    pub const fn channel(self) -> Option<WifiChannel> {
        match self {
            Self::Idle => None,
            Self::Station { channel }
            | Self::AccessPoint { channel }
            | Self::Concurrent { channel } => Some(channel),
        }
    }

    pub const fn station_active(self) -> bool {
        matches!(self, Self::Station { .. } | Self::Concurrent { .. })
    }

    pub const fn access_point_active(self) -> bool {
        matches!(self, Self::AccessPoint { .. } | Self::Concurrent { .. })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApTransition {
    StartStationCold,
    StartStationPreserveAccessPoint,
    StartAccessPointCold,
    StartAccessPointPreserveStation,
    StopStationLastRole,
    StopStationPreserveAccessPoint,
    StopAccessPointLastRole,
    StopAccessPointPreserveStation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApLifecycleError {
    RoleAlreadyActive(StaApRole),
    RoleInactive(StaApRole),
    ChannelConflict {
        active: WifiChannel,
        requested: WifiChannel,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaApLifecycle {
    state: StaApLifecycleState,
}

impl StaApLifecycle {
    pub const fn new() -> Self {
        Self {
            state: StaApLifecycleState::Idle,
        }
    }

    pub const fn state(&self) -> StaApLifecycleState {
        self.state
    }

    pub fn start_station(
        &mut self,
        channel: WifiChannel,
    ) -> Result<StaApTransition, StaApLifecycleError> {
        let (state, transition) = match self.state {
            StaApLifecycleState::Idle => (
                StaApLifecycleState::Station { channel },
                StaApTransition::StartStationCold,
            ),
            StaApLifecycleState::AccessPoint { channel: active } if active == channel => (
                StaApLifecycleState::Concurrent { channel },
                StaApTransition::StartStationPreserveAccessPoint,
            ),
            StaApLifecycleState::AccessPoint { channel: active } => {
                return Err(StaApLifecycleError::ChannelConflict {
                    active,
                    requested: channel,
                });
            }
            StaApLifecycleState::Station { .. } | StaApLifecycleState::Concurrent { .. } => {
                return Err(StaApLifecycleError::RoleAlreadyActive(StaApRole::Station));
            }
        };
        self.state = state;
        Ok(transition)
    }

    pub fn start_access_point(
        &mut self,
        channel: WifiChannel,
    ) -> Result<StaApTransition, StaApLifecycleError> {
        let (state, transition) = match self.state {
            StaApLifecycleState::Idle => (
                StaApLifecycleState::AccessPoint { channel },
                StaApTransition::StartAccessPointCold,
            ),
            StaApLifecycleState::Station { channel: active } if active == channel => (
                StaApLifecycleState::Concurrent { channel },
                StaApTransition::StartAccessPointPreserveStation,
            ),
            StaApLifecycleState::Station { channel: active } => {
                return Err(StaApLifecycleError::ChannelConflict {
                    active,
                    requested: channel,
                });
            }
            StaApLifecycleState::AccessPoint { .. } | StaApLifecycleState::Concurrent { .. } => {
                return Err(StaApLifecycleError::RoleAlreadyActive(
                    StaApRole::AccessPoint,
                ));
            }
        };
        self.state = state;
        Ok(transition)
    }

    pub fn stop_station(&mut self) -> Result<StaApTransition, StaApLifecycleError> {
        let (state, transition) = match self.state {
            StaApLifecycleState::Station { .. } => (
                StaApLifecycleState::Idle,
                StaApTransition::StopStationLastRole,
            ),
            StaApLifecycleState::Concurrent { channel } => (
                StaApLifecycleState::AccessPoint { channel },
                StaApTransition::StopStationPreserveAccessPoint,
            ),
            StaApLifecycleState::Idle | StaApLifecycleState::AccessPoint { .. } => {
                return Err(StaApLifecycleError::RoleInactive(StaApRole::Station));
            }
        };
        self.state = state;
        Ok(transition)
    }

    pub fn stop_access_point(&mut self) -> Result<StaApTransition, StaApLifecycleError> {
        let (state, transition) = match self.state {
            StaApLifecycleState::AccessPoint { .. } => (
                StaApLifecycleState::Idle,
                StaApTransition::StopAccessPointLastRole,
            ),
            StaApLifecycleState::Concurrent { channel } => (
                StaApLifecycleState::Station { channel },
                StaApTransition::StopAccessPointPreserveStation,
            ),
            StaApLifecycleState::Idle | StaApLifecycleState::Station { .. } => {
                return Err(StaApLifecycleError::RoleInactive(StaApRole::AccessPoint));
            }
        };
        self.state = state;
        Ok(transition)
    }
}

impl Default for StaApLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(primary: u8) -> WifiChannel {
        WifiChannel::mhz20(primary).unwrap()
    }

    #[test]
    fn station_then_access_point_preserves_station_until_its_own_stop() {
        let mut lifecycle = StaApLifecycle::new();
        let shared = channel(6);

        assert_eq!(
            lifecycle.start_station(shared),
            Ok(StaApTransition::StartStationCold)
        );
        assert_eq!(
            lifecycle.start_access_point(shared),
            Ok(StaApTransition::StartAccessPointPreserveStation)
        );
        assert_eq!(
            lifecycle.stop_access_point(),
            Ok(StaApTransition::StopAccessPointPreserveStation)
        );
        assert_eq!(
            lifecycle.state(),
            StaApLifecycleState::Station { channel: shared }
        );
        assert_eq!(
            lifecycle.stop_station(),
            Ok(StaApTransition::StopStationLastRole)
        );
        assert_eq!(lifecycle.state(), StaApLifecycleState::Idle);
    }

    #[test]
    fn access_point_then_station_preserves_access_point_until_its_own_stop() {
        let mut lifecycle = StaApLifecycle::new();
        let shared = channel(11);

        assert_eq!(
            lifecycle.start_access_point(shared),
            Ok(StaApTransition::StartAccessPointCold)
        );
        assert_eq!(
            lifecycle.start_station(shared),
            Ok(StaApTransition::StartStationPreserveAccessPoint)
        );
        assert_eq!(
            lifecycle.stop_station(),
            Ok(StaApTransition::StopStationPreserveAccessPoint)
        );
        assert_eq!(
            lifecycle.state(),
            StaApLifecycleState::AccessPoint { channel: shared }
        );
        assert_eq!(
            lifecycle.stop_access_point(),
            Ok(StaApTransition::StopAccessPointLastRole)
        );
        assert_eq!(lifecycle.state(), StaApLifecycleState::Idle);
    }

    #[test]
    fn a_second_role_cannot_silently_move_the_shared_radio() {
        let mut lifecycle = StaApLifecycle::new();
        lifecycle.start_station(channel(1)).unwrap();

        assert_eq!(
            lifecycle.start_access_point(channel(6)),
            Err(StaApLifecycleError::ChannelConflict {
                active: channel(1),
                requested: channel(6),
            })
        );
        assert_eq!(
            lifecycle.state(),
            StaApLifecycleState::Station {
                channel: channel(1)
            }
        );
    }

    #[test]
    fn duplicate_and_inactive_operations_fail_without_state_change() {
        let mut lifecycle = StaApLifecycle::new();
        let shared = channel(3);
        assert_eq!(
            lifecycle.stop_station(),
            Err(StaApLifecycleError::RoleInactive(StaApRole::Station))
        );
        lifecycle.start_access_point(shared).unwrap();
        assert_eq!(
            lifecycle.start_access_point(shared),
            Err(StaApLifecycleError::RoleAlreadyActive(
                StaApRole::AccessPoint
            ))
        );
        assert_eq!(
            lifecycle.state(),
            StaApLifecycleState::AccessPoint { channel: shared }
        );
    }
}
