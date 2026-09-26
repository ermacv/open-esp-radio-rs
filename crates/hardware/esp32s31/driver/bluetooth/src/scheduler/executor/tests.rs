use core::num::NonZeroU32;
use std::vec::Vec;

use oer_esp32s31_bluetooth_memory::{ControllerSramLinkAddress, SchedulerItemCompletionStatus};
use oer_esp32s31_hal::bluetooth::BluetoothSchedulerWorkObservation;

use super::{SchedulerExecutor, SchedulerItemAccess, SchedulerSubmitError};
use crate::scheduler::{list::SchedulerListInsertError, window::SchedulerRawWindow};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Item {
    next: Option<ControllerSramLinkAddress>,
    previous: Option<ControllerSramLinkAddress>,
    status: Option<SchedulerItemCompletionStatus>,
    prepared: bool,
}

/// Item memory for event identities 0..8.
struct Items([Item; 8]);

impl Items {
    fn new() -> Self {
        Self([Item::default(); 8])
    }

    fn execute(&mut self, id: u8, status: u32) {
        self.0[usize::from(id)].status = Some(NonZeroU32::new(status).map_or(
            SchedulerItemCompletionStatus::Zero,
            SchedulerItemCompletionStatus::NonZero,
        ));
    }

    fn chain(&self, head: ControllerSramLinkAddress) -> Vec<u8> {
        let mut order = Vec::new();
        let mut cursor = Some(head);
        while let Some(link) = cursor {
            let id = id_of(link);
            order.push(id);
            cursor = self.0[usize::from(id)].next;
        }
        order
    }
}

fn link(id: u8) -> ControllerSramLinkAddress {
    ControllerSramLinkAddress::new(0x2f00_1000 + 0x100 * u32::from(id))
        .expect("test link is representable")
}

fn id_of(link: ControllerSramLinkAddress) -> u8 {
    (0..8)
        .find(|id| self::link(*id) == link)
        .expect("test link names an item")
}

impl SchedulerItemAccess<u8> for Items {
    fn link(&self, id: u8) -> ControllerSramLinkAddress {
        link(id)
    }

    fn prepare_for_list(
        &mut self,
        id: u8,
        previous: Option<ControllerSramLinkAddress>,
        next: Option<ControllerSramLinkAddress>,
    ) {
        self.0[usize::from(id)] = Item {
            next,
            previous,
            status: None,
            prepared: true,
        };
    }

    fn set_next(&mut self, id: u8, next: Option<ControllerSramLinkAddress>) {
        self.0[usize::from(id)].next = next;
    }

    fn set_previous(&mut self, id: u8, previous: Option<ControllerSramLinkAddress>) {
        self.0[usize::from(id)].previous = previous;
    }

    fn completion_status(&self, id: u8) -> Option<SchedulerItemCompletionStatus> {
        self.0[usize::from(id)].status
    }
}

fn idle() -> BluetoothSchedulerWorkObservation {
    BluetoothSchedulerWorkObservation::from_fields_for_validation(false, false, 0)
}

fn busy() -> BluetoothSchedulerWorkObservation {
    BluetoothSchedulerWorkObservation::from_fields_for_validation(true, false, 0)
}

fn window(start: u32) -> SchedulerRawWindow {
    SchedulerRawWindow::new(start, start + 50).expect("test window is valid")
}

#[test]
fn idle_insertion_links_the_chain_in_start_order() {
    let mut executor = SchedulerExecutor::<u8, 4>::new();
    let mut items = Items::new();

    let first = executor
        .submit_idle(&mut items, idle(), 1, window(100))
        .unwrap();
    assert_eq!(first.head, link(1));
    let _ = executor
        .submit_idle(&mut items, idle(), 2, window(300))
        .unwrap();
    let _ = executor
        .submit_idle(&mut items, idle(), 3, window(200))
        .unwrap();
    let head = executor
        .submit_idle(&mut items, idle(), 4, window(0))
        .unwrap();

    assert_eq!(head.head, link(4));
    assert_eq!(items.chain(head.head), [4, 1, 3, 2]);
    assert!(items.0[4].prepared);
    assert_eq!(items.0[4].previous, None);
    assert_eq!(items.0[1].previous, Some(link(4)));
    assert_eq!(items.0[3].previous, Some(link(1)));
    assert_eq!(items.0[2].previous, Some(link(3)));
    assert_eq!(items.0[2].next, None);
}

#[test]
fn rejected_submissions_change_no_item() {
    let mut executor = SchedulerExecutor::<u8, 4>::new();
    let mut items = Items::new();
    let _ = executor
        .submit_idle(&mut items, idle(), 1, window(100))
        .unwrap();
    let before = items.0;

    assert_eq!(
        executor.submit_idle(&mut items, busy(), 2, window(300)),
        Err(SchedulerSubmitError::SchedulerBusy)
    );
    assert_eq!(
        executor.submit_idle(&mut items, idle(), 2, window(120)),
        Err(SchedulerSubmitError::List(
            SchedulerListInsertError::Overlap { with: 1 }
        ))
    );
    assert_eq!(items.0, before);
    assert_eq!(executor.list().len(), 1);
}

#[test]
fn completion_takes_executed_events_from_the_head() {
    let mut executor = SchedulerExecutor::<u8, 4>::new();
    let mut items = Items::new();
    for (id, start) in [(1, 0), (2, 100), (3, 200)] {
        let _ = executor
            .submit_idle(&mut items, idle(), id, window(start))
            .unwrap();
    }

    let none = executor.take_completed(&mut items).unwrap();
    assert!(none.is_empty());

    items.execute(1, 0);
    items.execute(2, 7);
    let completion = executor.take_completed(&mut items).unwrap();
    let completed: Vec<_> = completion.iter().collect();
    assert_eq!(
        completed,
        [
            (1, SchedulerItemCompletionStatus::Zero),
            (
                2,
                SchedulerItemCompletionStatus::NonZero(NonZeroU32::new(7).unwrap())
            ),
        ]
    );
    assert!(!completion.out_of_order());
    assert_eq!(executor.list().head().map(|(id, _)| id), Some(3));
    assert_eq!(items.0[3].previous, None);
}

#[test]
fn out_of_order_completion_leaves_the_later_event_listed() {
    let mut executor = SchedulerExecutor::<u8, 4>::new();
    let mut items = Items::new();
    for (id, start) in [(1, 0), (2, 100), (3, 200)] {
        let _ = executor
            .submit_idle(&mut items, idle(), id, window(start))
            .unwrap();
    }
    items.execute(1, 0);
    items.execute(3, 0);

    let completion = executor.take_completed(&mut items).unwrap();
    assert_eq!(completion.len(), 1);
    assert!(completion.out_of_order());
    let listed: Vec<_> = executor.list().iter().map(|(id, _)| id).collect();
    assert_eq!(listed, [2, 3]);
}

mod live {
    use std::vec::Vec;

    use oer_esp32s31_hal::bluetooth::{
        BluetoothSchedulerExecutionLockDisposition as Lock,
        BluetoothSchedulerExecutionModifyDisposition as Modify,
        BluetoothSchedulerLockModifyObservation,
    };

    use super::{Items, busy, idle, link, window};
    use crate::scheduler::executor::{
        SchedulerExecutor, SchedulerLiveAction as Action, SchedulerLiveFault,
        SchedulerLiveNext as Next, SchedulerLiveObservation as Observation, SchedulerLiveStep,
        SchedulerLiveWait as Wait, SchedulerSubmitError,
    };

    fn actions(step: &SchedulerLiveStep) -> Vec<Action> {
        step.actions().collect()
    }

    fn lock_modify(busy: bool, start: bool) -> Observation {
        Observation::LockModify(
            BluetoothSchedulerLockModifyObservation::from_fields_for_validation(busy, start, 0),
        )
    }

    fn running_list() -> (SchedulerExecutor<u8, 4>, Items) {
        let mut executor = SchedulerExecutor::new();
        let mut items = Items::new();
        let _ = executor
            .submit_idle(&mut items, idle(), 1, window(0))
            .unwrap();
        let _ = executor
            .submit_idle(&mut items, idle(), 2, window(200))
            .unwrap();
        (executor, items)
    }

    #[test]
    fn a_retained_lock_links_behind_the_predecessor_and_releases_after_lock_modify() {
        let (mut executor, mut items) = running_list();

        let step = executor
            .begin_live_insertion(&items, busy(), 3, window(100))
            .unwrap();
        assert_eq!(actions(&step), [Action::PublishExecutionLock(link(1))]);
        assert_eq!(step.next(), Next::Await(Wait::ExecutionLock));
        // Nothing is linked before the lock is granted.
        assert_eq!(items.chain(link(1)), [1, 2]);

        let step = executor
            .advance_live_insertion(&mut items, Observation::ExecutionLock(Lock::Pending))
            .unwrap();
        assert_eq!(step.next(), Next::Await(Wait::ExecutionLock));

        let step = executor
            .advance_live_insertion(
                &mut items,
                Observation::ExecutionLock(Lock::ExecutionLockRetained),
            )
            .unwrap();
        assert_eq!(actions(&step), [Action::PublishLockModify(link(3))]);
        assert_eq!(items.chain(link(1)), [1, 3, 2]);

        // The mirror is frozen until the insertion ends.
        assert!(executor.take_completed(&mut items).is_err());
        assert_eq!(
            executor.submit_idle(&mut items, idle(), 4, window(400)),
            Err(SchedulerSubmitError::InsertionActive)
        );

        let step = executor
            .advance_live_insertion(&mut items, lock_modify(true, true))
            .unwrap();
        assert_eq!(step.next(), Next::Await(Wait::LockModify));
        let step = executor
            .advance_live_insertion(&mut items, lock_modify(true, false))
            .unwrap();
        assert_eq!(actions(&step), [Action::ReleaseExecutionLock]);
        assert_eq!(step.next(), Next::Finished);
        assert!(executor.take_completed(&mut items).is_ok());
    }

    #[test]
    fn a_lock_that_is_not_retained_falls_back_to_modify_and_publishes_the_new_head() {
        let (mut executor, mut items) = running_list();
        let _ = executor
            .begin_live_insertion(&items, busy(), 3, window(100))
            .unwrap();

        let step = executor
            .advance_live_insertion(
                &mut items,
                Observation::ExecutionLock(Lock::ReconcileCurrentHead),
            )
            .unwrap();
        assert_eq!(
            actions(&step),
            [Action::ReleaseExecutionLock, Action::PublishExecutionModify]
        );
        assert_eq!(step.next(), Next::Await(Wait::ExecutionModify));

        let step = executor
            .advance_live_insertion(&mut items, Observation::ExecutionModify(Modify::Ready))
            .unwrap();
        assert_eq!(
            actions(&step),
            [Action::PublishHead(link(3)), Action::ReleaseExecutionModify]
        );
        assert_eq!(step.next(), Next::Finished);
        assert_eq!(items.chain(link(1)), [1, 3, 2]);
    }

    #[test]
    fn a_new_first_event_uses_modify() {
        let (mut executor, mut items) = running_list();
        let step = executor
            .begin_live_insertion(&items, busy(), 3, window(0xffff_ff00))
            .unwrap();
        assert_eq!(actions(&step), [Action::PublishExecutionModify]);
        let step = executor
            .advance_live_insertion(&mut items, Observation::ExecutionModify(Modify::Ready))
            .unwrap();
        assert_eq!(
            actions(&step),
            [Action::PublishHead(link(3)), Action::ReleaseExecutionModify]
        );
        assert_eq!(items.chain(link(3)), [3, 1, 2]);
    }

    #[test]
    fn faults_end_the_insertion_without_linking() {
        let (mut executor, mut items) = running_list();
        let _ = executor
            .begin_live_insertion(&items, busy(), 3, window(100))
            .unwrap();
        assert_eq!(
            executor
                .advance_live_insertion(&mut items, Observation::ExecutionModify(Modify::Ready)),
            Err(SchedulerLiveFault::UnexpectedObservation)
        );
        assert_eq!(
            executor.advance_live_insertion(
                &mut items,
                Observation::ExecutionLock(Lock::UnsupportedHardwareResult)
            ),
            Err(SchedulerLiveFault::UnsupportedExecutionLockResult)
        );
        assert_eq!(items.chain(link(1)), [1, 2]);
        assert!(!executor.list().contains(3));

        let _ = executor
            .begin_live_insertion(&items, busy(), 3, window(0xffff_ff00))
            .unwrap();
        assert_eq!(
            executor.advance_live_insertion(
                &mut items,
                Observation::ExecutionModify(Modify::HardwareRejected)
            ),
            Err(SchedulerLiveFault::ExecutionModifyRejected)
        );
        assert!(!executor.list().contains(3));
        assert_eq!(
            executor
                .advance_live_insertion(&mut items, Observation::ExecutionModify(Modify::Ready)),
            Err(SchedulerLiveFault::NoInsertion)
        );
    }

    #[test]
    fn live_insertion_needs_a_running_scheduler_and_restart_follows_an_idle_one() {
        let (mut executor, mut items) = running_list();
        assert_eq!(
            executor.begin_live_insertion(&items, idle(), 3, window(100)),
            Err(SchedulerSubmitError::SchedulerIdle)
        );
        items.execute(1, 0);
        let restart = executor.restart_if_idle(&items, idle()).unwrap();
        assert_eq!(restart.head, link(2));
        assert!(executor.restart_if_idle(&items, busy()).is_none());
    }
}
