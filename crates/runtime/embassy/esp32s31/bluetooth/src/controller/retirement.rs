//! Finite idle-only return of command ownership through the HCI drain barrier.

use super::ControllerCommandPhase;

/// A rejected retirement leaves the complete command actor with its caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerCommandRetirementError {
    /// The actor already transferred its owner to a terminal boundary.
    OwnerUnavailable,
    /// A radio, command or response lifecycle still owns the actor.
    NotIdle(ControllerCommandPhase),
    /// A role allocation/generation or the exact HCI epoch could not retire.
    Task(oer_esp32s31_bluetooth::controller::ControllerTaskRetirementError),
}

#[cfg(target_arch = "riscv32")]
use {
    super::{ControllerCommandState, ControllerCommandTask, SchedulerRunInterruptStorage},
    embassy_sync::blocking_mutex::raw::RawMutex,
    oer_bluetooth_hci::LeControllerCommandEndpoint,
    oer_esp32s31_bluetooth::controller::ControllerTaskHciRetired,
};

#[cfg(target_arch = "riscv32")]
impl<'runtime, S: SchedulerRunInterruptStorage, const CAPACITY: usize>
    ControllerCommandTask<'runtime, S, CAPACITY>
{
    /// Transfer the exact idle command authority for a quiescent maintenance
    /// window. HCI remains open and no hardware or PHY readiness is implied.
    #[inline(never)]
    #[allow(
        clippy::result_large_err,
        reason = "rejection returns the unchanged actor"
    )]
    pub fn try_into_idle(
        mut self,
    ) -> Result<
        oer_esp32s31_bluetooth::controller::ControllerIdleCommandTask<'runtime, S, CAPACITY>,
        (ControllerCommandRetirementError, Self),
    > {
        let result = self.owner.try_transfer(|state| match state {
            ControllerCommandState::Idle(idle) => Ok(idle),
            state => Err((
                ControllerCommandRetirementError::NotIdle(state.phase()),
                state,
            )),
        });
        match result {
            Some(Ok(idle)) => Ok(idle),
            Some(Err(error)) => Err((error, self)),
            None => Err((ControllerCommandRetirementError::OwnerUnavailable, self)),
        }
    }

    /// Return the idle task only after retiring its sole HCI command authority.
    ///
    /// Active, stopping and response-pending states are returned unchanged.
    /// Missing role allocations or portable generations reject before HCI closure.
    /// An idle actor with unread or unprocessed packets also remains runnable;
    /// Host ACL credit return stays open after this rejection. A successful
    /// return consumes the actor and pairs its lower task with the retired HCI
    /// epoch. It does not stop the separate modem timer or interrupt service,
    /// release PHY ownership, or reclaim the final static Controller storage.
    #[inline(never)]
    #[allow(
        clippy::result_large_err,
        reason = "failed retirement returns the complete no-alloc actor"
    )]
    pub fn try_retire_hci<M: RawMutex, const H2C: usize, const C2H: usize, const PC: usize>(
        mut self,
        controller: &mut LeControllerCommandEndpoint<'_, M, H2C, C2H, PC>,
    ) -> Result<
        ControllerTaskHciRetired<'runtime, S, CAPACITY>,
        (ControllerCommandRetirementError, Self),
    > {
        let result = self.owner.try_transfer(|state| match state {
            ControllerCommandState::Idle(idle) => {
                idle.try_retire_hci(controller).map_err(|(error, idle)| {
                    (
                        ControllerCommandRetirementError::Task(error),
                        ControllerCommandState::Idle(idle),
                    )
                })
            }
            state => Err((
                ControllerCommandRetirementError::NotIdle(state.phase()),
                state,
            )),
        });
        match result {
            Some(Ok(retired)) => Ok(retired),
            Some(Err(error)) => Err((error, self)),
            None => Err((ControllerCommandRetirementError::OwnerUnavailable, self)),
        }
    }
}
