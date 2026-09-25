//! Script only hardware observations; the completion spine and retained owners
//! are production types. This is not an MMIO/DMA or physical IRQ simulation.

use super::*;
use crate::scheduler::{
    completion::*,
    core::{SchedulerFinishedListDrainPending, SchedulerFinishedListDrainState},
};
use oer_bluetooth_ll::connection::LePeripheralConnectionEventInFlight;

pub(super) struct Owners<Event> {
    pub(super) event: Event,
    pub(super) acl: PeripheralConnectionAcl,
    pub(super) control: LePeripheralControl,
}

pub(super) type Completed = Owners<LePeripheralConnectionEventCompleted>;

pub(super) struct Role;
impl SingleItemCompletionRole for Role {
    type Wake = ();
    type Running = Owners<LePeripheralConnectionEventInFlight>;
    type CompletionObserved = Completed;
    type HardwareHeadEmpty = Completed;
    type PostUnlinkAwaiting = Completed;
    type RemovalReady = Completed;
}

#[derive(Default)]
pub(super) struct Hardware {
    pub(super) wake: bool,
    pub(super) finished: bool,
    pub(super) post_unlink_ready: bool,
    pub(super) fail_head: bool,
    pub(super) completions: usize,
    pub(super) unlinks: usize,
}

impl SingleItemCompletionBackend<Role> for Hardware {
    type FaultOwner = Completed;

    fn take_scheduler_wake(&mut self) -> Option<()> {
        core::mem::take(&mut self.wake).then_some(())
    }

    fn observe_completion(
        &mut self,
        running: Owners<LePeripheralConnectionEventInFlight>,
        (): (),
    ) -> Result<SingleItemRunningProgress<Role>, SingleItemCompletionFault<Completed>> {
        if !self.finished {
            return Ok(SingleItemRunningProgress::Running(
                SchedulerFinishedListDrainState::Drained(running),
            ));
        }
        self.completions += 1;
        Ok(SingleItemRunningProgress::CompletionObserved(
            SchedulerFinishedListDrainState::Drained(Owners {
                event: running
                    .event
                    .complete(LePeripheralConnectionEventPeerActivity::Observed),
                acl: running.acl,
                control: running.control,
            }),
        ))
    }

    fn continue_running_drain(
        &mut self,
        _: SchedulerFinishedListDrainPending<Owners<LePeripheralConnectionEventInFlight>>,
    ) -> Result<SingleItemRunningProgress<Role>, SingleItemCompletionFault<Completed>> {
        panic!("the scripted capture has one exhausted list")
    }

    fn continue_completed_drain(
        &mut self,
        _: SchedulerFinishedListDrainPending<Completed>,
    ) -> Result<SingleItemCompletedDrainProgress<Role>, SingleItemCompletionFault<Completed>> {
        panic!("the scripted capture has one exhausted list")
    }

    fn observe_hardware_head_retirement(
        &mut self,
        completed: Completed,
    ) -> Result<Completed, SingleItemCompletionFault<Completed>> {
        if self.fail_head {
            Err(SingleItemCompletionFault {
                cause: SingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished,
                _owner: completed,
            })
        } else {
            Ok(completed)
        }
    }

    fn unlink_and_arm(
        &mut self,
        owner: Completed,
    ) -> Result<Completed, SingleItemCompletionFault<Completed>> {
        self.unlinks += 1;
        Ok(owner)
    }

    fn consume_post_unlink(
        &mut self,
        owner: Completed,
    ) -> Result<SingleItemPostUnlinkProgress<Role>, SingleItemCompletionFault<Completed>> {
        Ok(if self.post_unlink_ready {
            SingleItemPostUnlinkProgress::Ready(owner)
        } else {
            SingleItemPostUnlinkProgress::Pending {
                awaiting: owner,
                disposition: SingleItemPostUnlinkDisposition::Waiting,
            }
        })
    }
}

pub(super) fn advance(
    completion: SingleItemCompletion<Role>,
    hw: &mut Hardware,
) -> SingleItemCompletion<Role> {
    match completion.step(hw) {
        SingleItemCompletionStep::Continue(next) => next,
        _ => panic!("expected one immediate production transition"),
    }
}

pub(super) fn wait(
    completion: SingleItemCompletion<Role>,
    hw: &mut Hardware,
) -> SingleItemCompletion<Role> {
    match completion.step(hw) {
        SingleItemCompletionStep::Waiting(next) => next,
        _ => panic!("pending hardware must retain all owners"),
    }
}
