//! Whole-PHY power lifecycle recovered from the current ESP32-S31 library.
//!
//! Physical RF close is independent of protocol-client bookkeeping. A caller
//! may execute it only after the last client has been released and every
//! protocol hardware engine has crossed its stopped boundary.

use crate::analog::i2c::{PhyI2cAddress, analog_registers};

/// One ordered edge in complete current-vendor `phy_close_rf`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PhyRfCloseOperation {
    EnterCritical,
    DisableHardwareFrequencyControl,
    ForceTxRxOff { phase: u8 },
    SettleOneMicrosecond,
    DisableAgc,
    ClearBasebandControl,
    WriteI2c { address: PhyI2cAddress, value: u8 },
    PowerOffRfCircuits,
    ClearImmediateClockPower,
    CloseFrontendBasebandClocks,
    EnableBbpllCalibration,
    ExitCritical,
}

/// Exact default-profile close graph after the vendor's pre-close temperature
/// observation. The S31 critical-section callbacks are empty functions, but
/// their boundaries remain explicit so a future implementation cannot move
/// physical work across them accidentally.
pub(crate) const PHY_RF_CLOSE_OPERATIONS: [PhyRfCloseOperation; 17] = [
    PhyRfCloseOperation::EnterCritical,
    PhyRfCloseOperation::DisableHardwareFrequencyControl,
    PhyRfCloseOperation::ForceTxRxOff { phase: 0 },
    PhyRfCloseOperation::SettleOneMicrosecond,
    PhyRfCloseOperation::ForceTxRxOff { phase: 1 },
    PhyRfCloseOperation::SettleOneMicrosecond,
    PhyRfCloseOperation::DisableAgc,
    PhyRfCloseOperation::ClearBasebandControl,
    PhyRfCloseOperation::SettleOneMicrosecond,
    PhyRfCloseOperation::WriteI2c {
        address: analog_registers::RF_CLOSE_CONTROL,
        value: 0x07,
    },
    PhyRfCloseOperation::PowerOffRfCircuits,
    PhyRfCloseOperation::ClearImmediateClockPower,
    PhyRfCloseOperation::CloseFrontendBasebandClocks,
    PhyRfCloseOperation::EnableBbpllCalibration,
    PhyRfCloseOperation::WriteI2c {
        address: analog_registers::RF_CLOSE_RETENTION_ZERO,
        value: 0x77,
    },
    PhyRfCloseOperation::WriteI2c {
        address: analog_registers::RF_CLOSE_RETENTION_ONE,
        value: 0x77,
    },
    PhyRfCloseOperation::ExitCritical,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_graph_preserves_vendor_parent_and_child_order() {
        assert_eq!(
            PHY_RF_CLOSE_OPERATIONS,
            [
                PhyRfCloseOperation::EnterCritical,
                PhyRfCloseOperation::DisableHardwareFrequencyControl,
                PhyRfCloseOperation::ForceTxRxOff { phase: 0 },
                PhyRfCloseOperation::SettleOneMicrosecond,
                PhyRfCloseOperation::ForceTxRxOff { phase: 1 },
                PhyRfCloseOperation::SettleOneMicrosecond,
                PhyRfCloseOperation::DisableAgc,
                PhyRfCloseOperation::ClearBasebandControl,
                PhyRfCloseOperation::SettleOneMicrosecond,
                PhyRfCloseOperation::WriteI2c {
                    address: analog_registers::RF_CLOSE_CONTROL,
                    value: 0x07,
                },
                PhyRfCloseOperation::PowerOffRfCircuits,
                PhyRfCloseOperation::ClearImmediateClockPower,
                PhyRfCloseOperation::CloseFrontendBasebandClocks,
                PhyRfCloseOperation::EnableBbpllCalibration,
                PhyRfCloseOperation::WriteI2c {
                    address: analog_registers::RF_CLOSE_RETENTION_ZERO,
                    value: 0x77,
                },
                PhyRfCloseOperation::WriteI2c {
                    address: analog_registers::RF_CLOSE_RETENTION_ONE,
                    value: 0x77,
                },
                PhyRfCloseOperation::ExitCritical,
            ]
        );
    }
}
