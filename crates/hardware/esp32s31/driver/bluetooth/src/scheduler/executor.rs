//! Scheduler event executor core for one hardware list.
//!
//! The executor owns the ordered mirror of the hardware list and keeps the
//! scheduler links of every listed item consistent with it. It changes item
//! memory only through [`SchedulerItemAccess`] and never touches a register:
//! each operation returns the hardware action its caller must perform with
//! the HAL. Conflicts are resolved before submission; an overlapping window is
//! rejected, not moved.
//!
//! This core implements insertion while the scheduler is idle, insertion into
//! a running list through the vendor lock and modify transactions, and the
//! completion walk. Cancellation and stop build on the same mirror and item
//! access.

#![forbid(unsafe_code)]

use oer_esp32s31_bluetooth_memory::{ControllerSramLinkAddress, SchedulerItemCompletionStatus};
use oer_esp32s31_hal::bluetooth::BluetoothSchedulerWorkObservation;

use crate::scheduler::{
    list::{SchedulerList, SchedulerListInsertError},
    window::SchedulerRawWindow,
};

/// Scheduler-owned fields of the items named by event identities.
///
/// Implementations write the item memory of the event slot pools. Every
/// method addresses an item that the executor currently owns.
pub trait SchedulerItemAccess<I> {
    /// Controller link of the item.
    fn link(&self, id: I) -> ControllerSramLinkAddress;

    /// Prepare an item for insertion as list insertion does: clear the
    /// scheduler control byte, mark it unexecuted and install both links.
    fn prepare_for_list(
        &mut self,
        id: I,
        previous: Option<ControllerSramLinkAddress>,
        next: Option<ControllerSramLinkAddress>,
    );

    /// Replace the hardware next link of a listed item.
    fn set_next(&mut self, id: I, next: Option<ControllerSramLinkAddress>);

    /// Replace the previous-item link of a listed item.
    fn set_previous(&mut self, id: I, previous: Option<ControllerSramLinkAddress>);

    /// Recorded status, or `None` while hardware has not executed the item.
    fn completion_status(&self, id: I) -> Option<SchedulerItemCompletionStatus>;
}

/// Why an event was not submitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerSubmitError<I> {
    /// The mirror rejected the window.
    List(SchedulerListInsertError<I>),
    /// The scheduler is running; insertion needs a live-list transaction.
    SchedulerBusy,
    /// The scheduler is idle; insertion does not need a live-list transaction.
    SchedulerIdle,
    /// A live-list insertion is in progress; the mirror is frozen until it ends.
    InsertionActive,
}

/// The mirror is frozen by a live-list insertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerInsertionActive;

/// Hardware action after an insertion into an idle scheduler.
///
/// The caller publishes `head` as the hardware-list head and then starts the
/// scheduler with the prepared run interrupts and RUN command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the list head must be published and the scheduler started"]
pub struct SchedulerIdleInsertion {
    pub head: ControllerSramLinkAddress,
}

/// Events that hardware executed, taken from the head of the list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerCompletion<I, const CAPACITY: usize> {
    completed: [Option<(I, SchedulerItemCompletionStatus)>; CAPACITY],
    len: usize,
    out_of_order: bool,
}

impl<I: Copy, const CAPACITY: usize> SchedulerCompletion<I, CAPACITY> {
    /// Completed events in execution order with their recorded status.
    pub fn iter(&self) -> impl Iterator<Item = (I, SchedulerItemCompletionStatus)> + '_ {
        self.completed[..self.len].iter().flatten().copied()
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether an executed event follows an unexecuted one. The executor
    /// leaves such events listed; the caller must treat the list as faulted.
    pub const fn out_of_order(&self) -> bool {
        self.out_of_order
    }
}

/// Executor core for hardware list zero.
pub struct SchedulerExecutor<I, const CAPACITY: usize> {
    list: SchedulerList<I, CAPACITY>,
    live: Option<live::LivePhase<I>>,
}

impl<I: Copy + Eq, const CAPACITY: usize> SchedulerExecutor<I, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            list: SchedulerList::new(),
            live: None,
        }
    }

    /// The ordered mirror of the hardware list.
    pub const fn list(&self) -> &SchedulerList<I, CAPACITY> {
        &self.list
    }

    /// Insert `id` while the scheduler is idle.
    ///
    /// The mirror decides the position before any item memory changes. The
    /// new item and its neighbours are then linked, and the list head is
    /// returned for publication.
    pub fn submit_idle(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        scheduler: BluetoothSchedulerWorkObservation,
        id: I,
        window: SchedulerRawWindow,
    ) -> Result<SchedulerIdleInsertion, SchedulerSubmitError<I>> {
        if self.live.is_some() {
            return Err(SchedulerSubmitError::InsertionActive);
        }
        if scheduler.is_busy() {
            return Err(SchedulerSubmitError::SchedulerBusy);
        }
        self.link_into_list(items, id, window)
            .map_err(SchedulerSubmitError::List)?;
        let (head, _) = self.list.head().expect("the list holds the inserted event");
        Ok(SchedulerIdleInsertion {
            head: items.link(head),
        })
    }

    /// Take the events that hardware executed from the head of the list.
    ///
    /// Hardware has already advanced its head past them, so no register
    /// changes. The new head loses its previous link.
    pub fn take_completed(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
    ) -> Result<SchedulerCompletion<I, CAPACITY>, SchedulerInsertionActive> {
        if self.live.is_some() {
            return Err(SchedulerInsertionActive);
        }
        let scan = self
            .list
            .scan_completion(|id| items.completion_status(id).is_some());
        let mut completion = SchedulerCompletion {
            completed: [None; CAPACITY],
            len: 0,
            out_of_order: scan.out_of_order,
        };
        for _ in 0..scan.completed_prefix {
            let (id, _) = self.list.head().expect("the scan counted a listed head");
            let status = items
                .completion_status(id)
                .expect("the scan found this event executed");
            self.list.remove(id);
            completion.completed[completion.len] = Some((id, status));
            completion.len += 1;
        }
        if completion.len > 0
            && let Some((head, _)) = self.list.head()
        {
            items.set_previous(head, None);
        }
        Ok(completion)
    }

    /// Hardware action that restarts an idle scheduler at the first listed
    /// event that has not executed, as insertion end does.
    pub fn restart_if_idle(
        &self,
        items: &impl SchedulerItemAccess<I>,
        scheduler: BluetoothSchedulerWorkObservation,
    ) -> Option<SchedulerIdleInsertion> {
        if scheduler.is_busy() || self.live.is_some() {
            return None;
        }
        self.list
            .iter()
            .map(|(id, _)| id)
            .find(|id| items.completion_status(*id).is_none())
            .map(|head| SchedulerIdleInsertion {
                head: items.link(head),
            })
    }

    /// Link `id` into the mirror and the item memory.
    fn link_into_list(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        id: I,
        window: SchedulerRawWindow,
    ) -> Result<(), SchedulerListInsertError<I>> {
        let placement = self.list.insert(id, window)?;
        let link = items.link(id);
        let previous = placement.predecessor.map(|previous| items.link(previous));
        let next = placement.successor.map(|next| items.link(next));
        items.prepare_for_list(id, previous, next);
        if let Some(predecessor) = placement.predecessor {
            items.set_next(predecessor, Some(link));
        }
        if let Some(successor) = placement.successor {
            items.set_previous(successor, Some(link));
        }
        Ok(())
    }
}

impl<I: Copy + Eq, const CAPACITY: usize> Default for SchedulerExecutor<I, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

mod live;

pub use live::{
    SchedulerLiveAction, SchedulerLiveFault, SchedulerLiveNext, SchedulerLiveObservation,
    SchedulerLiveStep, SchedulerLiveWait,
};

#[cfg(test)]
mod tests;
