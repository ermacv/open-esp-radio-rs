//! Bluetooth membership in the shared PHY domain and BTBB baseband.
//!
//! The Controller enable runs `esp_phy_enable(PHY_MODEM_BT)` and
//! `esp_btbb_enable` after the controller HAL, scheduler and low-power
//! hardware are initialized and before the BLE base stack is enabled. The
//! radio system registers or wakes the shared domain first; this module only
//! enters and leaves it. The BTBB gain byte is projected by the PHY client
//! from the registration that describes the lease.

use oer_esp32s31_hal::shared_radio::{ClientQuiescence, EmptyQuiescentWindow, SharedRadioLease};
use oer_esp32s31_phy::{
    bluetooth_client::{
        BluetoothPhyClientError, BluetoothPhyLeaveFailure, BluetoothPhyMembership, join_bluetooth,
        leave_bluetooth,
    },
    concurrent::{ConcurrentAcquire, ConcurrentPhy},
};

use crate::{low_power::ControllerLowPowerHardwareInitialized, resources::TaskResources};

/// Powered Controller that holds a client bit in the shared PHY domain and a
/// reference on the shared BTBB baseband.
///
/// The BLE PHY engine is initialized from this state. Its membership stays
/// with the Controller until `leave_phy` at shutdown.
#[must_use = "the joined Controller retains its PHY membership"]
pub struct ControllerPhyJoined<'cells, const MODEM_TIMER_CAPACITY: usize> {
    pub(crate) controller: ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY>,
    pub(crate) membership: BluetoothPhyMembership,
}

/// Rejected join retaining the powered Controller.
///
/// The Controller has run its HAL, scheduler and low-power initialization,
/// which no verified transition reverses before the shutdown of a running
/// epoch; the retained owner is fail-stop.
#[must_use = "a failed PHY join retains the powered Controller"]
pub struct ControllerPhyJoinFailure<'cells, const MODEM_TIMER_CAPACITY: usize> {
    _controller: ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY>,
    error: BluetoothPhyClientError,
}

impl<const MODEM_TIMER_CAPACITY: usize> ControllerPhyJoinFailure<'_, MODEM_TIMER_CAPACITY> {
    /// The rejection; the client set and BTBB are unchanged.
    pub const fn error(&self) -> BluetoothPhyClientError {
        self.error
    }
}

impl<'cells, const MODEM_TIMER_CAPACITY: usize>
    ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY>
{
    /// Enter the shared PHY domain and BTBB as the Bluetooth client.
    ///
    /// [`ConcurrentAcquire::TrackingDue`] means the domain must run its
    /// tracking before the Controller uses RF; [`ControllerPhyJoined::quiescence`]
    /// issues the proof the maintenance admission needs.
    ///
    /// # Errors
    ///
    /// The domain is not registered and settled for the lease, Bluetooth
    /// already holds BTBB, or the client set rejected the client.
    #[allow(
        clippy::result_large_err,
        reason = "the failure retains the complete allocation-free Controller epoch"
    )]
    pub fn join_phy(
        self,
        lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
        clock: &impl oer_time::Clock,
    ) -> Result<
        (
            ControllerPhyJoined<'cells, MODEM_TIMER_CAPACITY>,
            ConcurrentAcquire,
        ),
        ControllerPhyJoinFailure<'cells, MODEM_TIMER_CAPACITY>,
    > {
        match join_bluetooth(lease, self.task().hal(), clock) {
            Ok((membership, acquired)) => Ok((
                ControllerPhyJoined {
                    controller: self,
                    membership,
                },
                acquired,
            )),
            Err(error) => Err(ControllerPhyJoinFailure {
                _controller: self,
                error,
            }),
        }
    }
}

impl<const MODEM_TIMER_CAPACITY: usize> ControllerPhyJoined<'_, MODEM_TIMER_CAPACITY> {
    /// Promise that the Controller performs no RF and does not touch the
    /// shared PHY until `release_by_micros` (PHY monotonic clock).
    ///
    /// No scheduler work can start before the BLE PHY engine is initialized,
    /// and the proof borrows this owner for its whole lifetime.
    ///
    /// # Errors
    ///
    /// The window is empty.
    pub fn quiescence(
        &mut self,
        issued_at_micros: u64,
        release_by_micros: u64,
    ) -> Result<ClientQuiescence<'_>, EmptyQuiescentWindow> {
        self.controller
            .task_mut()
            .hal_mut()
            .quiescence(issued_at_micros, release_by_micros)
    }
}

/// Leave the shared PHY domain and drop the BTBB reference of a retired
/// Controller. Returns whether Bluetooth was the last PHY client, in which
/// case the radio system closes RF.
///
/// # Errors
///
/// The domain rejected the release; nothing changed and the membership is
/// returned.
pub(crate) fn leave_phy(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    task: &TaskResources,
    membership: BluetoothPhyMembership,
) -> Result<bool, BluetoothPhyLeaveFailure> {
    leave_bluetooth(lease, task.hal(), membership)
}
