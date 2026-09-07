use oer_esp32s31_pac::{
    BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
};

use std::rc::Rc;

use super::*;

struct Role;

impl SingleItemCompletionRole for Role {
    type Wake = ();
    type Running = u8;
    type CompletionObserved = u16;
    type HardwareHeadEmpty = u32;
    type PostUnlinkAwaiting = u64;
    type RemovalReady = usize;
}

#[derive(Default)]
struct Backend {
    wake: bool,
    still_running_once: bool,
    post_unlink_pending: Option<SingleItemPostUnlinkDisposition>,
    unrelated_list: Option<BluetoothSchedulerFinishedHardwareListObserved>,
}

impl SingleItemCompletionBackend<Role> for Backend {
    type FaultOwner = ();

    fn take_scheduler_wake(&mut self) -> Option<()> {
        core::mem::replace(&mut self.wake, false).then_some(())
    }

    fn observe_completion(
        &mut self,
        running: u8,
        (): (),
    ) -> Result<SingleItemRunningProgress<Role>, SingleItemCompletionFault<()>> {
        if let Some(observed) = self.unrelated_list.take() {
            return Ok(SingleItemRunningProgress::UnrelatedList {
                drain: SchedulerFinishedListDrainState::Drained(running),
                observed,
            });
        }
        if self.still_running_once {
            self.still_running_once = false;
            return Ok(SingleItemRunningProgress::Running(
                SchedulerFinishedListDrainState::Drained(running),
            ));
        }
        Ok(SingleItemRunningProgress::CompletionObserved(
            SchedulerFinishedListDrainState::Drained(u16::from(running)),
        ))
    }

    fn continue_running_drain(
        &mut self,
        _pending: SchedulerFinishedListDrainPending<u8>,
    ) -> Result<SingleItemRunningProgress<Role>, SingleItemCompletionFault<()>> {
        unreachable!("the model completes in its first captured list")
    }

    fn continue_completed_drain(
        &mut self,
        _pending: SchedulerFinishedListDrainPending<u16>,
    ) -> Result<SingleItemCompletedDrainProgress<Role>, SingleItemCompletionFault<()>> {
        unreachable!("the model completes with an exhausted capture")
    }

    fn observe_hardware_head_retirement(
        &mut self,
        completed: u16,
    ) -> Result<u32, SingleItemCompletionFault<()>> {
        Ok(u32::from(completed))
    }

    fn unlink_and_arm(&mut self, observed: u32) -> Result<u64, SingleItemCompletionFault<()>> {
        Ok(u64::from(observed))
    }

    fn consume_post_unlink(
        &mut self,
        awaiting: u64,
    ) -> Result<SingleItemPostUnlinkProgress<Role>, SingleItemCompletionFault<()>> {
        if let Some(disposition) = self.post_unlink_pending.take() {
            return Ok(SingleItemPostUnlinkProgress::Pending {
                awaiting,
                disposition,
            });
        }
        Ok(SingleItemPostUnlinkProgress::Ready(awaiting as usize))
    }
}

#[test]
fn unrelated_finished_list_returns_the_observation_and_retains_the_running_owner() {
    let lists = BluetoothSchedulerFinishedListObservation::from_lists_for_validation(&[3])
        .expect("one unrelated hardware list is valid");
    let BluetoothSchedulerFinishedListPop::List { observed, .. } = lists.pop_lowest() else {
        panic!("the transferred list must produce one affine observation");
    };
    let mut backend = Backend {
        wake: true,
        unrelated_list: Some(observed),
        ..Backend::default()
    };
    let completion = SingleItemCompletion::<Role>::new(17);
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("the wake must remain paired with the running owner");
    };
    let SingleItemCompletionStep::UnrelatedList {
        completion,
        observed,
    } = completion.step(&mut backend)
    else {
        panic!("an unrelated observation must return to the caller without completing this role");
    };
    assert_eq!(observed.index().get(), 3);
    let SingleItemCompletionStep::Waiting(completion) = completion.step(&mut backend) else {
        panic!("an exhausted capture must await another wake for the same running owner");
    };
    let SingleItemCompletionPhase::RunningAwaitingWake(running) = completion.phase else {
        panic!("the unrelated list must not advance the role to completion");
    };
    assert_eq!(running, 17);
}

#[test]
fn post_unlink_immediate_progress_preserves_the_awaiting_owner_without_waiting() {
    let mut backend = Backend {
        post_unlink_pending: Some(SingleItemPostUnlinkDisposition::Continue),
        ..Backend::default()
    };
    let completion = SingleItemCompletion::<Role> {
        phase: SingleItemCompletionPhase::PostUnlinkAwaiting(29),
    };
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("immediate post-unlink progress must not suspend or claim removal readiness");
    };
    assert_eq!(
        completion.wait_kind(),
        Some(SingleItemCompletionWaitKind::PostUnlink)
    );
    let SingleItemCompletionStep::RemovalReady(ready) = completion.step(&mut backend) else {
        panic!("the following matching publication must return the retained owner");
    };
    assert_eq!(ready, 29);
}

#[test]
fn common_spine_waits_then_advances_each_owner_to_removal_ready() {
    let mut backend = Backend {
        post_unlink_pending: Some(SingleItemPostUnlinkDisposition::Waiting),
        ..Backend::default()
    };
    let completion = SingleItemCompletion::<Role>::new(9);
    assert_eq!(
        completion.wait_kind(),
        Some(SingleItemCompletionWaitKind::Scheduler)
    );
    let SingleItemCompletionStep::Waiting(completion) = completion.step(&mut backend) else {
        panic!("an absent wake must preserve the running owner");
    };

    backend.wake = true;
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("the scheduler wake must remain paired with the running owner");
    };
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("the completion hook must retain the observed owner");
    };
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("head retirement must retain the empty-head owner");
    };
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("unlink must retain the armed post-unlink owner");
    };
    assert_eq!(
        completion.wait_kind(),
        Some(SingleItemCompletionWaitKind::PostUnlink)
    );
    let SingleItemCompletionStep::Waiting(completion) = completion.step(&mut backend) else {
        panic!("post-unlink backpressure must retain the exact awaiting owner");
    };
    assert_eq!(
        completion.wait_kind(),
        Some(SingleItemCompletionWaitKind::PostUnlink)
    );
    let SingleItemCompletionStep::RemovalReady(ready) = completion.step(&mut backend) else {
        panic!("the matching post-unlink publication must expose removal readiness");
    };
    assert_eq!(ready, 9);
}

#[test]
fn role_hook_can_keep_the_item_running_without_losing_its_owner() {
    let mut backend = Backend {
        wake: true,
        still_running_once: true,
        ..Backend::default()
    };
    let completion = SingleItemCompletion::<Role>::new(13);
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("the first wake must remain paired with the running owner");
    };
    let SingleItemCompletionStep::Waiting(completion) = completion.step(&mut backend) else {
        panic!("a role-level still-running observation must await another wake");
    };
    assert_eq!(
        completion.wait_kind(),
        Some(SingleItemCompletionWaitKind::Scheduler)
    );

    backend.wake = true;
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("the next wake must retain the same role owner");
    };
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("the second observation must complete the scripted role item");
    };
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("the completed owner must advance to empty-head observation");
    };
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("the empty-head owner must advance to the post-unlink gate");
    };
    let SingleItemCompletionStep::RemovalReady(ready) = completion.step(&mut backend) else {
        panic!("the post-unlink gate must return the exact role owner");
    };
    assert_eq!(ready, 13);
}

struct IdentityMismatchBackend {
    wake: bool,
    owner: Option<Rc<()>>,
}

impl SingleItemCompletionBackend<Role> for IdentityMismatchBackend {
    type FaultOwner = Rc<()>;

    fn take_scheduler_wake(&mut self) -> Option<()> {
        core::mem::replace(&mut self.wake, false).then_some(())
    }

    fn observe_completion(
        &mut self,
        _running: u8,
        (): (),
    ) -> Result<SingleItemRunningProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>> {
        let Some(owner) = self.owner.take() else {
            panic!("the scripted mismatch owns exactly one affine token");
        };
        Err(SingleItemCompletionFault {
            cause: SingleItemCompletionFaultCause::SchedulerIdentityMismatch,
            _owner: owner,
        })
    }

    fn continue_running_drain(
        &mut self,
        _pending: SchedulerFinishedListDrainPending<u8>,
    ) -> Result<SingleItemRunningProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>> {
        unreachable!("the scripted mismatch occurs during initial classification")
    }

    fn continue_completed_drain(
        &mut self,
        _pending: SchedulerFinishedListDrainPending<u16>,
    ) -> Result<SingleItemCompletedDrainProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>>
    {
        unreachable!("the scripted mismatch occurs before completion drain")
    }

    fn observe_hardware_head_retirement(
        &mut self,
        _completed: u16,
    ) -> Result<u32, SingleItemCompletionFault<Self::FaultOwner>> {
        unreachable!("the scripted mismatch occurs before head retirement")
    }

    fn unlink_and_arm(
        &mut self,
        _observed: u32,
    ) -> Result<u64, SingleItemCompletionFault<Self::FaultOwner>> {
        unreachable!("the scripted mismatch occurs before unlink")
    }

    fn consume_post_unlink(
        &mut self,
        _awaiting: u64,
    ) -> Result<SingleItemPostUnlinkProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>>
    {
        unreachable!("the scripted mismatch occurs before post-unlink")
    }
}

#[test]
fn identity_mismatch_preserves_the_exact_backend_owner() {
    let identity = Rc::new(());
    let mut backend = IdentityMismatchBackend {
        wake: true,
        owner: Some(Rc::clone(&identity)),
    };
    let completion = SingleItemCompletion::<Role>::new(21);
    let SingleItemCompletionStep::Continue(completion) = completion.step(&mut backend) else {
        panic!("the wake must remain paired with the running owner");
    };
    let SingleItemCompletionStep::Fault(fault) = completion.step(&mut backend) else {
        panic!("the scripted scheduler identity mismatch must fail closed");
    };
    assert_eq!(
        fault.cause,
        SingleItemCompletionFaultCause::SchedulerIdentityMismatch
    );
    assert!(Rc::ptr_eq(&fault._owner, &identity));
}
