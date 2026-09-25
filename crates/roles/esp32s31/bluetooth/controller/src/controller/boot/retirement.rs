//! Idle retirement returns task HAL, PHY and all role graphs from stable slots.

use crate::controller::boot::role_retirement::RoleLeaseRetirement;

use super::{
    ControllerIdleCommandTask, ControllerPublishedTaskService, ControllerTaskRetirementError,
};
use embassy_sync::blocking_mutex::raw::RawMutex;

mod physical;
pub use physical::{
    ControllerColdReleased, ControllerPhysicalShutdownError, ControllerPhysicalShutdownFailure,
    ControllerRestartError, ControllerRestartFailure, ControllerRestarted,
    ControllerRetiredStorage,
};

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Retire this idle task's HCI authority after both transport queues drain.
    ///
    /// Rejection restores the complete idle aggregate, including its exact
    /// next-command token. Success removes the actual task hardware only after
    /// closing the drained HCI epoch, together with its registered PHY client,
    /// calibration cache, BLE PHY/DF allocation graph and all five role-memory
    /// owners. Every role must first return its complete allocation and portable
    /// generation. All three leases originate from this exact Controller split;
    /// none is extracted and HCI stays open on role-readiness rejection.
    /// Modem-timer, IRQ, scheduler residuals and other physical hardware still
    /// require their own consuming shutdown transitions.
    #[allow(
        clippy::result_large_err,
        reason = "a rejected barrier retains the complete idle owner"
    )]
    pub fn try_retire_hci<M: RawMutex, const H2C: usize, const C2H: usize, const PC: usize>(
        self,
        controller: &mut oer_bluetooth_hci::LeControllerCommandEndpoint<'_, M, H2C, C2H, PC>,
    ) -> Result<
        ControllerTaskHciRetired<'runtime, S, SCHEDULER_CAPACITY>,
        (ControllerTaskRetirementError, Self),
    > {
        let (mut task, ready) = self.into_parts();
        if let Err(error) = task.runtime.runtime_retirement_ready() {
            return Err((
                ControllerTaskRetirementError::Runtime(error),
                Self::from_parts(task, ready),
            ));
        }
        if let Err(error) = task.runtime.task_owner().controller_time_retirement_ready() {
            return Err((
                ControllerTaskRetirementError::ControllerTime(error),
                Self::from_parts(task, ready),
            ));
        }
        match task
            .runtime
            .task_owner()
            .try_retire_with(&mut task.ble_phy_owners, || {
                task.roles
                    .try_retire_roles(ready, |ready| controller.try_retire_transport(ready))
            }) {
            Ok((hardware, phy, (roles, hci))) => Ok(ControllerTaskHciRetired {
                _software: task,
                _hardware: hardware,
                _phy: phy,
                _roles: roles,
                hci,
            }),
            Err((error, ready)) => {
                let error = match error {
                    super::role_retirement::RoleBarrierError::Role(error) => {
                        ControllerTaskRetirementError::Role(error)
                    }
                    super::role_retirement::RoleBarrierError::Barrier(error) => {
                        ControllerTaskRetirementError::Hci(error)
                    }
                };
                Err((error, Self::from_parts(task, ready)))
            }
        }
    }
}

/// Retired HCI epoch with task HAL, PHY and role graphs removed from stable storage.
///
/// The controller-time worker moves with its HAL owner. Software borrows remain
/// retained but cannot drive the emptied leases. Registered PHY, calibration
/// cache, BLE environment/resolving-list memory and the hardware-published DF
/// workspace move together with DTM, advertising, connectable advertising,
/// scanning and peripheral graph/RX owners. Raw memory and lower PHY release
/// remain private. The separate platform lease can join this exact retired HCI
/// epoch. This proves neither physical shutdown nor permission to release PHY
/// or hardware-visible memory; software storage remains statically borrowed.
#[must_use = "retain task hardware, software borrows and the retired HCI epoch"]
pub struct ControllerTaskHciRetired<'runtime, S, const SC: usize> {
    _software: ControllerPublishedTaskService<'runtime, S, SC>,
    _hardware: oer_esp32s31_bluetooth::resources::TaskResources,
    _phy: oer_esp32s31_bluetooth::ble_phy::BlePhyRetainedOwners,
    _roles: super::role_retirement::ControllerRoleResources,
    hci: oer_bluetooth_hci::LeControllerHciRetired<'runtime, ()>,
}

impl<S, const SC: usize> ControllerTaskHciRetired<'_, S, SC> {
    /// Execute the finite terminal output transition with the returned IRQ owner.
    /// Hardware-list pointers and faults reject before output release. The task,
    /// role graphs, PHY client and retired HCI epoch remain retained throughout.
    pub fn try_release_controller_output(
        &mut self,
        output: oer_esp32s31_hal::bluetooth::InterruptOutputAfterRoutesOwner,
    ) -> Result<
        oer_esp32s31_hal::bluetooth::InterruptOutputReleasedOwner,
        (
            oer_esp32s31_hal::bluetooth::BluetoothControllerOutputReleaseError,
            oer_esp32s31_hal::bluetooth::InterruptOutputAfterRoutesOwner,
        ),
    > {
        self._hardware.release_controller_output(output)
    }

    /// HCI retirement proof of this Controller epoch, for its platform lease.
    pub fn hci_proof(&self) -> &oer_bluetooth_hci::LeControllerHciRetired<'_, ()> {
        &self.hci
    }

    /// Whether the retired HCI authority belongs to this Controller endpoint.
    pub fn matches_endpoint<M: RawMutex, const H2C: usize, const C2H: usize, const PC: usize>(
        &self,
        endpoint: &oer_bluetooth_hci::LeControllerCommandEndpoint<'_, M, H2C, C2H, PC>,
    ) -> bool {
        self.hci.matches_endpoint(endpoint)
    }
}
