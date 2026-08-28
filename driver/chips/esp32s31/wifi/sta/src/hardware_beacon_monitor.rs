//! Honest ESP32-S31 automatic beacon-monitoring admission frontier.
//!
//! The S31 PAC contains exact register projections for station receive
//! identity, raw beacon-miss counters, the station beacon-filter gate and the
//! opaque WDEVPWR bank.  Those projections are not by themselves a complete
//! automatic monitor: reviewed evidence does not convert an association's TU
//! beacon interval into the raw timeout field and does not identify a
//! beacon-miss WDEVPWR cause.  This module binds one connected association to
//! the proven readback, retains the software monitor, and reports the first
//! missing hardware oracle without touching MMIO.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::types::MacStaReceivePolicySnapshot;
use open_esp_radio_ieee80211::station_power_save::StaAssociationId;
use open_esp_radio_wifi_sta::link_monitor::StaBeaconLossConfig;

/// Exact peer identity owned by one connected station association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationBeaconMonitorBinding {
    bssid: [u8; 6],
    association_id: StaAssociationId,
}

impl StationBeaconMonitorBinding {
    pub const fn new(bssid: [u8; 6], association_id: StaAssociationId) -> Self {
        Self {
            bssid,
            association_id,
        }
    }

    pub const fn bssid(self) -> [u8; 6] {
        self.bssid
    }

    pub const fn association_id(self) -> StaAssociationId {
        self.association_id
    }
}

/// Deepest source/runtime boundary reached by one admission attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationHardwareBeaconMonitorStage {
    /// The portable software deadline monitor remains the authoritative link
    /// owner. No hardware-monitoring register has been changed.
    SoftwareMonitorRetained,
    /// The live station RX policy is enabled and its BSSID/AID readback is
    /// exactly bound to this association owner.
    AssociationReadbackBound,
    /// The association's miss limit fits the reviewed four-bit raw field.
    BeaconMissLimitRepresentable,
}

/// First fact preventing an automatic hardware beacon monitor from owning the
/// link-loss decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationHardwareBeaconMonitorBlocker {
    /// A hardware implementation supplied no reviewed station-policy
    /// readback. It therefore cannot prove which association it would arm.
    MissingAssociationReadback,
    /// Interface zero is not an active infrastructure-STA receive context.
    StationReceivePolicyInactive {
        bssid_address_check_enabled: bool,
        interface_is_soft_ap: bool,
        interface_rx_policy_enabled: bool,
    },
    /// The live BSSID/AID image does not belong to this association epoch.
    AssociationBindingMismatch {
        expected_bssid: [u8; 6],
        observed_bssid: [u8; 6],
        expected_association_id: u16,
        observed_association_id: u16,
    },
    /// Some hardware beacon-filter bits are already asserted outside this
    /// owner. Taking over would race an unknown prior lifecycle.
    BeaconFilterAlreadyEnabled { control: u8 },
    /// The software policy cannot be represented by the reviewed four-bit
    /// beacon-miss-limit field without truncation.
    BeaconMissLimitNotRepresentable { requested: u8, maximum: u8 },
    /// Reviewed setters prove a raw sixteen-bit field but not its unit or the
    /// conversion from the association's TU interval.
    MissingBeaconMissTimeoutUnitConversion,
}

/// Exact fail-closed result of one source/runtime admission attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationHardwareBeaconMonitorFrontier {
    reached: StationHardwareBeaconMonitorStage,
    blocker: StationHardwareBeaconMonitorBlocker,
}

impl StationHardwareBeaconMonitorFrontier {
    const fn new(
        reached: StationHardwareBeaconMonitorStage,
        blocker: StationHardwareBeaconMonitorBlocker,
    ) -> Self {
        Self { reached, blocker }
    }

    pub const fn reached(self) -> StationHardwareBeaconMonitorStage {
        self.reached
    }

    pub const fn blocker(self) -> StationHardwareBeaconMonitorBlocker {
        self.blocker
    }

    /// No current frontier result grants hardware ownership of beacon loss.
    pub const fn automatic_monitor_active(self) -> bool {
        false
    }
}

/// Affine association epoch for automatic-monitor admission.
///
/// The value is deliberately neither `Copy` nor `Clone`: the connected
/// control owner creates it once and consumes it at shutdown. This prevents a
/// stale software association from being re-used as a second admission owner.
/// Current admission never mutates hardware, so shutdown has no hidden MMIO
/// restore obligation; the software monitor remains live throughout.
#[derive(Debug)]
pub struct StationHardwareBeaconMonitorEpoch {
    binding: StationBeaconMonitorBinding,
    policy: StaBeaconLossConfig,
    frontier: Option<StationHardwareBeaconMonitorFrontier>,
}

impl StationHardwareBeaconMonitorEpoch {
    pub const fn new(binding: StationBeaconMonitorBinding, policy: StaBeaconLossConfig) -> Self {
        Self {
            binding,
            policy,
            frontier: None,
        }
    }

    pub const fn binding(&self) -> StationBeaconMonitorBinding {
        self.binding
    }

    pub const fn policy(&self) -> StaBeaconLossConfig {
        self.policy
    }

    pub const fn frontier(&self) -> Option<StationHardwareBeaconMonitorFrontier> {
        self.frontier
    }

    /// Evaluate the one-shot hardware admission frontier.
    ///
    /// This is intentionally value-only. Even the furthest successful
    /// preflight stops before raw timeout programming, beacon-filter enable,
    /// WDEVPWR enable/ack, or any RF/PHY/clock transition.
    pub fn evaluate_once(
        &mut self,
        snapshot: Option<MacStaReceivePolicySnapshot>,
    ) -> StationHardwareBeaconMonitorFrontier {
        if let Some(frontier) = self.frontier {
            return frontier;
        }

        let frontier = evaluate_hardware_beacon_monitor(self.binding, self.policy, snapshot);
        self.frontier = Some(frontier);
        frontier
    }

    /// Consume this exact association epoch at connected shutdown.
    ///
    /// The returned evidence confirms that admission remained pre-MMIO. If a
    /// later implementation crosses that boundary, this API must instead
    /// carry and consume an affine PAC restore token.
    pub fn stop(self) -> StationHardwareBeaconMonitorStopped {
        StationHardwareBeaconMonitorStopped {
            binding: self.binding,
            frontier: self.frontier,
        }
    }
}

/// Value-only shutdown evidence for an epoch which never armed hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationHardwareBeaconMonitorStopped {
    pub binding: StationBeaconMonitorBinding,
    pub frontier: Option<StationHardwareBeaconMonitorFrontier>,
}

/// Evaluate every currently proven precondition and stop at the first missing
/// physical semantic.
pub fn evaluate_hardware_beacon_monitor(
    binding: StationBeaconMonitorBinding,
    policy: StaBeaconLossConfig,
    snapshot: Option<MacStaReceivePolicySnapshot>,
) -> StationHardwareBeaconMonitorFrontier {
    let Some(snapshot) = snapshot else {
        return StationHardwareBeaconMonitorFrontier::new(
            StationHardwareBeaconMonitorStage::SoftwareMonitorRetained,
            StationHardwareBeaconMonitorBlocker::MissingAssociationReadback,
        );
    };

    if !snapshot.bssid_address_check_enabled
        || snapshot.interface_is_soft_ap
        || !snapshot.interface_rx_policy_enabled
    {
        return StationHardwareBeaconMonitorFrontier::new(
            StationHardwareBeaconMonitorStage::SoftwareMonitorRetained,
            StationHardwareBeaconMonitorBlocker::StationReceivePolicyInactive {
                bssid_address_check_enabled: snapshot.bssid_address_check_enabled,
                interface_is_soft_ap: snapshot.interface_is_soft_ap,
                interface_rx_policy_enabled: snapshot.interface_rx_policy_enabled,
            },
        );
    }

    if snapshot.bssid != binding.bssid()
        || snapshot.association_id != binding.association_id().get()
    {
        return StationHardwareBeaconMonitorFrontier::new(
            StationHardwareBeaconMonitorStage::SoftwareMonitorRetained,
            StationHardwareBeaconMonitorBlocker::AssociationBindingMismatch {
                expected_bssid: binding.bssid(),
                observed_bssid: snapshot.bssid,
                expected_association_id: binding.association_id().get(),
                observed_association_id: snapshot.association_id,
            },
        );
    }

    if snapshot.beacon_filter_control != 0 {
        return StationHardwareBeaconMonitorFrontier::new(
            StationHardwareBeaconMonitorStage::AssociationReadbackBound,
            StationHardwareBeaconMonitorBlocker::BeaconFilterAlreadyEnabled {
                control: snapshot.beacon_filter_control,
            },
        );
    }

    const MAXIMUM_BEACON_MISS_LIMIT: u8 = 0x0f;
    if policy.miss_limit() > MAXIMUM_BEACON_MISS_LIMIT {
        return StationHardwareBeaconMonitorFrontier::new(
            StationHardwareBeaconMonitorStage::AssociationReadbackBound,
            StationHardwareBeaconMonitorBlocker::BeaconMissLimitNotRepresentable {
                requested: policy.miss_limit(),
                maximum: MAXIMUM_BEACON_MISS_LIMIT,
            },
        );
    }

    StationHardwareBeaconMonitorFrontier::new(
        StationHardwareBeaconMonitorStage::BeaconMissLimitRepresentable,
        StationHardwareBeaconMonitorBlocker::MissingBeaconMissTimeoutUnitConversion,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BSSID: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];

    fn binding() -> StationBeaconMonitorBinding {
        StationBeaconMonitorBinding::new(BSSID, StaAssociationId::new(7).unwrap())
    }

    fn policy(miss_limit: u8) -> StaBeaconLossConfig {
        StaBeaconLossConfig::new(100, miss_limit).unwrap()
    }

    fn snapshot() -> MacStaReceivePolicySnapshot {
        MacStaReceivePolicySnapshot {
            bssid: BSSID,
            association_id: 7,
            minimum_mpdu_start_spacing: 0,
            bssid_address_check_enabled: true,
            interface_is_soft_ap: false,
            interface_rx_policy_enabled: true,
            beacon_filter_control: 0,
        }
    }

    #[test]
    fn exact_binding_reaches_the_first_missing_physical_oracle() {
        let frontier = evaluate_hardware_beacon_monitor(binding(), policy(10), Some(snapshot()));
        assert_eq!(
            frontier.reached(),
            StationHardwareBeaconMonitorStage::BeaconMissLimitRepresentable
        );
        assert_eq!(
            frontier.blocker(),
            StationHardwareBeaconMonitorBlocker::MissingBeaconMissTimeoutUnitConversion
        );
        assert!(!frontier.automatic_monitor_active());
    }

    #[test]
    fn association_identity_and_existing_filter_owner_fail_closed() {
        let mut stale = snapshot();
        stale.association_id = 8;
        assert!(matches!(
            evaluate_hardware_beacon_monitor(binding(), policy(10), Some(stale)).blocker(),
            StationHardwareBeaconMonitorBlocker::AssociationBindingMismatch {
                expected_association_id: 7,
                observed_association_id: 8,
                ..
            }
        ));

        let mut already_enabled = snapshot();
        already_enabled.beacon_filter_control = 0b111;
        assert_eq!(
            evaluate_hardware_beacon_monitor(binding(), policy(10), Some(already_enabled))
                .blocker(),
            StationHardwareBeaconMonitorBlocker::BeaconFilterAlreadyEnabled { control: 0b111 }
        );
    }

    #[test]
    fn four_bit_limit_is_checked_without_truncation() {
        let frontier = evaluate_hardware_beacon_monitor(binding(), policy(16), Some(snapshot()));
        assert_eq!(
            frontier.blocker(),
            StationHardwareBeaconMonitorBlocker::BeaconMissLimitNotRepresentable {
                requested: 16,
                maximum: 15,
            }
        );
    }

    #[test]
    fn affine_epoch_evaluates_once_and_stops_without_a_restore_claim() {
        let mut epoch = StationHardwareBeaconMonitorEpoch::new(binding(), policy(10));
        let first = epoch.evaluate_once(Some(snapshot()));

        let mut contradictory = snapshot();
        contradictory.bssid = [0; 6];
        assert_eq!(epoch.evaluate_once(Some(contradictory)), first);

        let stopped = epoch.stop();
        assert_eq!(stopped.binding, binding());
        assert_eq!(stopped.frontier, Some(first));
    }
}
