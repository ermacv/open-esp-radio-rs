//! Joint retention of role memory and admission to terminal task retirement.

/// The role that has not returned its complete allocation/generation owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerRoleRetirementError {
    /// The DTM session graph is checked out.
    Dtm,
    /// Advertising graph or portable generation remains checked out.
    Advertising,
    /// Connectable graph or portable generation remains checked out.
    ConnectableAdvertising,
    /// The passive scanner graph is checked out.
    Scanning,
    /// Connection graph/RX pool is checked out or has unreclaimed contents.
    PeripheralConnection,
}

/// Exact rejection before returning task HAL, PHY and role-memory owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerTaskRetirementError {
    /// Scheduler software still retains pending work or a reservation.
    Runtime(oer_esp32s31_bluetooth::runtime_resources::ControllerRuntimeRetirementError),
    /// A controller-time request, orphan drain or ownership fault remains.
    ControllerTime(oer_esp32s31_bluetooth::controller_time::ControllerTimeRetirementError),
    /// An allocation or portable generation remains checked out by a role.
    Role(ControllerRoleRetirementError),
    /// The original HCI epoch did not reach its lossless drain barrier.
    Hci(oer_bluetooth_hci::LeControllerHciRetirementError),
}

/// All five role allocations, with their original identities and policies.
/// The group stays private after extraction; hardware pointers are not revoked.
#[cfg(any(target_arch = "riscv32", test))]
pub(super) struct ControllerRoleResources {
    pub(super) dtm_resources: crate::le::dtm::DtmRuntimeResources,
    pub(super) legacy_advertising_resources:
        crate::le::advertising::LegacyAdvertisingRuntimeResources,
    pub(super) passive_scan_resources: crate::le::scanning::PassiveScanRuntimeResources,
    pub(super) peripheral_connection_resources:
        crate::le::peripheral::PeripheralConnectionRuntimeResources,
    pub(super) legacy_connectable_advertising_resources:
        crate::le::advertising::LegacyConnectableAdvertisingRuntimeResources,
}

#[cfg(any(target_arch = "riscv32", test))]
impl ControllerRoleResources {
    pub(super) fn retirement_ready(&self) -> Result<(), ControllerRoleRetirementError> {
        self.other_roles_ready()?;
        use ControllerRoleRetirementError as Error;
        if !self.peripheral_connection_resources.allocation_is_idle() {
            return Err(Error::PeripheralConnection);
        }
        Ok(())
    }

    pub(super) fn other_roles_ready(&self) -> Result<(), ControllerRoleRetirementError> {
        use ControllerRoleRetirementError as Error;
        if !self.dtm_resources.session_is_idle() {
            return Err(Error::Dtm);
        }
        if !self.legacy_advertising_resources.event_is_idle() {
            return Err(Error::Advertising);
        }
        if !self
            .legacy_connectable_advertising_resources
            .retirement_ready()
        {
            return Err(Error::ConnectableAdvertising);
        }
        if !self.passive_scan_resources.event_is_idle() {
            return Err(Error::Scanning);
        }
        Ok(())
    }
}

/// Terminal retirement of the leased role group.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) trait RoleLeaseRetirement {
    /// Check real role owners before invoking the transport barrier. A rejected
    /// role or HCI barrier leaves the complete group in its original slot.
    fn try_retire_roles<Authority, Proof, Error>(
        &mut self,
        authority: Authority,
        barrier: impl FnOnce(Authority) -> Result<Proof, (Error, Authority)>,
    ) -> Result<(ControllerRoleResources, Proof), (RoleBarrierError<Error>, Authority)>;
}

#[cfg(any(target_arch = "riscv32", test))]
impl RoleLeaseRetirement
    for oer_esp32s31_bluetooth::resources::runtime_owner::RuntimeOwnerLease<
        '_,
        ControllerRoleResources,
    >
{
    fn try_retire_roles<Authority, Proof, Error>(
        &mut self,
        authority: Authority,
        barrier: impl FnOnce(Authority) -> Result<Proof, (Error, Authority)>,
    ) -> Result<(ControllerRoleResources, Proof), (RoleBarrierError<Error>, Authority)> {
        if let Err(error) = self.retirement_ready() {
            return Err((RoleBarrierError::Role(error), authority));
        }
        self.try_retire(|| barrier(authority))
            .map_err(|(error, authority)| (RoleBarrierError::Barrier(error), authority))
    }
}

#[cfg(any(target_arch = "riscv32", test))]
pub(super) enum RoleBarrierError<E> {
    Role(ControllerRoleRetirementError),
    Barrier(E),
}

#[cfg(test)]
mod tests;
