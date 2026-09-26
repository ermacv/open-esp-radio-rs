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

    let none = executor.take_completed(&mut items);
    assert!(none.is_empty());

    items.execute(1, 0);
    items.execute(2, 7);
    let completion = executor.take_completed(&mut items);
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

    let completion = executor.take_completed(&mut items);
    assert_eq!(completion.len(), 1);
    assert!(completion.out_of_order());
    let listed: Vec<_> = executor.list().iter().map(|(id, _)| id).collect();
    assert_eq!(listed, [2, 3]);
}
