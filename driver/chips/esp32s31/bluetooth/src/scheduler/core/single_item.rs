//! Role-neutral ownership for one scheduler item from `RUN` through software-list
//! removal readiness.
//!
//! Concrete roles provide only memory-completion classification hooks.
//! Finished-list draining, hardware-head retirement, software unlink, removal
//! gating are implemented once here. Packet extraction, recycling, timeline
//! release and list reclamation remain role-specific tails.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerHardwareListHeadRetirementObservation, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerSoftwareListRemovalInterruptStep, BluetoothSchedulerSoftwareListRemovalJoin,
    BluetoothSchedulerSoftwareListRemovalReady,
};

use super::{BluetoothSchedulerFinishedListDrainPending, BluetoothSchedulerFinishedListDrainState};
use crate::{
    BluetoothControllerPoweredTaskRuntime, BluetoothPrimarySchedulerEvent,
    BluetoothSchedulerFinishedHardwareListObserved,
};

pub(crate) trait BluetoothSingleItemSchedulerRole: Sized {
    type RunningItem;
    type CompletionObservedItem;
    type Retained;

    fn running_item_address(item: &Self::RunningItem) -> BluetoothControllerSramAddress;

    fn observe_completion(
        item: Self::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> BluetoothSingleItemRoleCompletionObservation<Self>;

    fn completed_item_address(
        item: &Self::CompletionObservedItem,
    ) -> BluetoothControllerSramAddress;
}

pub(crate) enum BluetoothSingleItemRoleCompletionObservation<Role: BluetoothSingleItemSchedulerRole>
{
    ListMismatch {
        running: Role::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(Role::RunningItem),
    CompletionObserved(Role::CompletionObservedItem),
}

pub(crate) struct BluetoothSingleItemSchedulerRunning<Role: BluetoothSingleItemSchedulerRole> {
    item: Role::RunningItem,
    run: BluetoothSchedulerHardwareRunCommandPublished,
    retained: Role::Retained,
}

impl<Role: BluetoothSingleItemSchedulerRole> BluetoothSingleItemSchedulerRunning<Role> {
    pub(crate) const fn new(
        item: Role::RunningItem,
        run: BluetoothSchedulerHardwareRunCommandPublished,
        retained: Role::Retained,
    ) -> Self {
        Self {
            item,
            run,
            retained,
        }
    }

    pub(crate) fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        Role::running_item_address(&self.item)
    }

    pub(crate) const fn item(&self) -> &Role::RunningItem {
        &self.item
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }
}

pub(crate) struct BluetoothSingleItemSchedulerCompletionObserved<
    Role: BluetoothSingleItemSchedulerRole,
> {
    item: Role::CompletionObservedItem,
    run: BluetoothSchedulerHardwareRunCommandPublished,
    retained: Role::Retained,
}

impl<Role: BluetoothSingleItemSchedulerRole> BluetoothSingleItemSchedulerCompletionObserved<Role> {
    pub(crate) fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        Role::completed_item_address(&self.item)
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }
}

pub(crate) struct BluetoothSingleItemSchedulerRoleItemIdentityMismatch<
    Role: BluetoothSingleItemSchedulerRole,
> {
    _expected: BluetoothControllerSramAddress,
    _item: Role::CompletionObservedItem,
    _run: BluetoothSchedulerHardwareRunCommandPublished,
    _retained: Role::Retained,
}

pub(crate) struct BluetoothSingleItemSchedulerHardwareHeadTransitionMismatch<
    Role: BluetoothSingleItemSchedulerRole,
> {
    _item: Role::CompletionObservedItem,
    _head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    _retained: Role::Retained,
}

pub(crate) struct BluetoothSingleItemSchedulerRemovalTransitionMismatch<
    Role: BluetoothSingleItemSchedulerRole,
> {
    _item: Role::CompletionObservedItem,
    _removal: BluetoothSchedulerSoftwareListRemovalReady,
    _retained: Role::Retained,
}

pub(crate) struct BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<
    Role: BluetoothSingleItemSchedulerRole,
> {
    item: Role::CompletionObservedItem,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    retained: Role::Retained,
}

impl<Role: BluetoothSingleItemSchedulerRole>
    BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>
{
    pub(crate) fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        Role::completed_item_address(&self.item)
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }
}

pub(crate) struct BluetoothSingleItemSchedulerSoftwareListUnlinked<
    Role: BluetoothSingleItemSchedulerRole,
> {
    item: Role::CompletionObservedItem,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    retained: Role::Retained,
}

impl<Role: BluetoothSingleItemSchedulerRole>
    BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>
{
    pub(crate) fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        Role::completed_item_address(&self.item)
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }
}

pub(crate) struct BluetoothSingleItemSchedulerSoftwareListRemovalReady<
    Role: BluetoothSingleItemSchedulerRole,
> {
    item: Role::CompletionObservedItem,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    retained: Role::Retained,
}

impl<Role: BluetoothSingleItemSchedulerRole>
    BluetoothSingleItemSchedulerSoftwareListRemovalReady<Role>
{
    pub(crate) fn into_parts(
        self,
    ) -> (
        Role::CompletionObservedItem,
        BluetoothSchedulerSoftwareListRemovalReady,
        Role::Retained,
    ) {
        (self.item, self.removal, self.retained)
    }
}

pub(crate) enum BluetoothSingleItemSchedulerCompletionStep<Role: BluetoothSingleItemSchedulerRole> {
    DrainAlreadyActive(BluetoothSingleItemSchedulerRunning<Role>),
    SchedulerIdentityMismatch(BluetoothSingleItemSchedulerRunning<Role>),
    RoleItemIdentityMismatch(BluetoothSingleItemSchedulerRoleItemIdentityMismatch<Role>),
    SchedulerStateMismatch(BluetoothSingleItemSchedulerRoleItemIdentityMismatch<Role>),
    NoFinishedList(BluetoothSingleItemSchedulerRunning<Role>),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<BluetoothSingleItemSchedulerRunning<Role>>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(
        BluetoothSchedulerFinishedListDrainState<BluetoothSingleItemSchedulerRunning<Role>>,
    ),
    CompletionObserved(
        BluetoothSchedulerFinishedListDrainState<
            BluetoothSingleItemSchedulerCompletionObserved<Role>,
        >,
    ),
}

pub(crate) enum BluetoothSingleItemSchedulerRunningDrainStep<Role: BluetoothSingleItemSchedulerRole>
{
    SchedulerIdentityMismatch(
        BluetoothSchedulerFinishedListDrainPending<BluetoothSingleItemSchedulerRunning<Role>>,
    ),
    DrainLost(
        BluetoothSchedulerFinishedListDrainPending<BluetoothSingleItemSchedulerRunning<Role>>,
    ),
    RoleItemIdentityMismatch(BluetoothSingleItemSchedulerRoleItemIdentityMismatch<Role>),
    SchedulerStateMismatch(BluetoothSingleItemSchedulerRoleItemIdentityMismatch<Role>),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<BluetoothSingleItemSchedulerRunning<Role>>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(
        BluetoothSchedulerFinishedListDrainState<BluetoothSingleItemSchedulerRunning<Role>>,
    ),
    CompletionObserved(
        BluetoothSchedulerFinishedListDrainState<
            BluetoothSingleItemSchedulerCompletionObserved<Role>,
        >,
    ),
}

pub(crate) enum BluetoothSingleItemSchedulerCompletionObservedDrainStep<
    Role: BluetoothSingleItemSchedulerRole,
> {
    SchedulerIdentityMismatch(
        BluetoothSchedulerFinishedListDrainPending<
            BluetoothSingleItemSchedulerCompletionObserved<Role>,
        >,
    ),
    DrainLost(
        BluetoothSchedulerFinishedListDrainPending<
            BluetoothSingleItemSchedulerCompletionObserved<Role>,
        >,
    ),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<
            BluetoothSingleItemSchedulerCompletionObserved<Role>,
        >,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    RepeatedRoleList {
        _drain: BluetoothSchedulerFinishedListDrainState<
            BluetoothSingleItemSchedulerCompletionObserved<Role>,
        >,
        _observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

pub(crate) enum BluetoothSingleItemSchedulerHardwareHeadRetirementStep<
    Role: BluetoothSingleItemSchedulerRole,
> {
    SchedulerIdentityMismatch(BluetoothSingleItemSchedulerCompletionObserved<Role>),
    FinishedListDrainStillActive(BluetoothSingleItemSchedulerCompletionObserved<Role>),
    ExpectedHeadStillPublished {
        _completed: BluetoothSingleItemSchedulerCompletionObserved<Role>,
        _observed: BluetoothSchedulerHardwareListHead,
    },
    UnexpectedHeadChanged {
        _completed: BluetoothSingleItemSchedulerCompletionObserved<Role>,
        _observed: BluetoothSchedulerHardwareListHead,
    },
    SchedulerStateMismatch(BluetoothSingleItemSchedulerHardwareHeadTransitionMismatch<Role>),
    EmptyObserved(BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>),
}

pub(crate) enum BluetoothSingleItemSchedulerSoftwareListUnlinkStep<
    Role: BluetoothSingleItemSchedulerRole,
> {
    SchedulerIdentityMismatch(BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>),
    Unlinked(BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>),
}

pub(crate) enum BluetoothSingleItemSchedulerSoftwareListRemovalJoin<
    Role: BluetoothSingleItemSchedulerRole,
> {
    SchedulerIdentityMismatch {
        unlinked: BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
        event: BluetoothPrimarySchedulerEvent,
    },
    SchedulerStateMismatch(BluetoothSingleItemSchedulerRemovalTransitionMismatch<Role>),
    Pending(BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>),
    Ready(BluetoothSingleItemSchedulerSoftwareListRemovalReady<Role>),
}

pub(crate) enum BluetoothSingleItemSchedulerSoftwareListRemovalRecheck<
    Role: BluetoothSingleItemSchedulerRole,
> {
    SchedulerIdentityMismatch(BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>),
    StorageUnavailable(BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>),
    SchedulerStateMismatch(BluetoothSingleItemSchedulerRemovalTransitionMismatch<Role>),
    Pending(BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>),
    Ready(BluetoothSingleItemSchedulerSoftwareListRemovalReady<Role>),
}

enum BluetoothSingleItemObservedStep<Role: BluetoothSingleItemSchedulerRole> {
    RoleItemIdentityMismatch(BluetoothSingleItemSchedulerRoleItemIdentityMismatch<Role>),
    SchedulerStateMismatch(BluetoothSingleItemSchedulerRoleItemIdentityMismatch<Role>),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<BluetoothSingleItemSchedulerRunning<Role>>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(
        BluetoothSchedulerFinishedListDrainState<BluetoothSingleItemSchedulerRunning<Role>>,
    ),
    CompletionObserved(
        BluetoothSchedulerFinishedListDrainState<
            BluetoothSingleItemSchedulerCompletionObserved<Role>,
        >,
    ),
}

impl<const CAPACITY: usize> BluetoothControllerPoweredTaskRuntime<'_, CAPACITY> {
    fn classify_single_item_observation<Role: BluetoothSingleItemSchedulerRole>(
        &mut self,
        running: BluetoothSingleItemSchedulerRunning<Role>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
        more: bool,
    ) -> BluetoothSingleItemObservedStep<Role> {
        let expected = running.scheduler_item_address();
        let BluetoothSingleItemSchedulerRunning {
            item,
            run,
            retained,
        } = running;
        match Role::observe_completion(item, observed) {
            BluetoothSingleItemRoleCompletionObservation::ListMismatch {
                running: item,
                observed,
            } => BluetoothSingleItemObservedStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(
                    BluetoothSingleItemSchedulerRunning {
                        item,
                        run,
                        retained,
                    },
                    more,
                ),
                observed,
            },
            BluetoothSingleItemRoleCompletionObservation::StillInFlight(item) => {
                BluetoothSingleItemObservedStep::StillInFlight(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothSingleItemSchedulerRunning {
                            item,
                            run,
                            retained,
                        },
                        more,
                    ),
                )
            }
            BluetoothSingleItemRoleCompletionObservation::CompletionObserved(item) => {
                let completed = Role::completed_item_address(&item);
                let (item, run, retained) = match super::retain_matching_single_item_identity(
                    expected,
                    completed,
                    (item, run, retained),
                ) {
                    Ok(owner) => owner,
                    Err((expected, (item, run, retained))) => {
                        return BluetoothSingleItemObservedStep::RoleItemIdentityMismatch(
                            BluetoothSingleItemSchedulerRoleItemIdentityMismatch {
                                _expected: expected,
                                _item: item,
                                _run: run,
                                _retained: retained,
                            },
                        );
                    }
                };
                if !self
                    ._scheduler_list
                    .retain_completion_observed_first_item(completed)
                {
                    return BluetoothSingleItemObservedStep::SchedulerStateMismatch(
                        BluetoothSingleItemSchedulerRoleItemIdentityMismatch {
                            _expected: expected,
                            _item: item,
                            _run: run,
                            _retained: retained,
                        },
                    );
                }
                BluetoothSingleItemObservedStep::CompletionObserved(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothSingleItemSchedulerCompletionObserved {
                            item,
                            run,
                            retained,
                        },
                        more,
                    ),
                )
            }
        }
    }

    pub(crate) fn observe_single_item_completion<Role: BluetoothSingleItemSchedulerRole>(
        &mut self,
        running: BluetoothSingleItemSchedulerRunning<Role>,
        wake: crate::BluetoothSchedulerWakeBatch,
    ) -> BluetoothSingleItemSchedulerCompletionStep<Role> {
        let address = running.scheduler_item_address();
        if running.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothSingleItemSchedulerCompletionStep::SchedulerIdentityMismatch(running);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active()
            || self
                .task
                .capture_scheduler_finished_lists(self.runtime.scheduler_finished_lists_mut(), wake)
                .is_err()
        {
            return BluetoothSingleItemSchedulerCompletionStep::DrainAlreadyActive(running);
        }
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return BluetoothSingleItemSchedulerCompletionStep::NoFinishedList(running);
        };
        match self.classify_single_item_observation(running, observed, more) {
            BluetoothSingleItemObservedStep::RoleItemIdentityMismatch(owner) => {
                BluetoothSingleItemSchedulerCompletionStep::RoleItemIdentityMismatch(owner)
            }
            BluetoothSingleItemObservedStep::SchedulerStateMismatch(owner) => {
                BluetoothSingleItemSchedulerCompletionStep::SchedulerStateMismatch(owner)
            }
            BluetoothSingleItemObservedStep::UnrelatedList { drain, observed } => {
                BluetoothSingleItemSchedulerCompletionStep::UnrelatedList { drain, observed }
            }
            BluetoothSingleItemObservedStep::StillInFlight(drain) => {
                BluetoothSingleItemSchedulerCompletionStep::StillInFlight(drain)
            }
            BluetoothSingleItemObservedStep::CompletionObserved(drain) => {
                BluetoothSingleItemSchedulerCompletionStep::CompletionObserved(drain)
            }
        }
    }

    pub(crate) fn continue_single_item_running_finished_list_drain<
        Role: BluetoothSingleItemSchedulerRole,
    >(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothSingleItemSchedulerRunning<Role>,
        >,
    ) -> BluetoothSingleItemSchedulerRunningDrainStep<Role> {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothSingleItemSchedulerRunningDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothSingleItemSchedulerRunningDrainStep::DrainLost(pending);
        }
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return BluetoothSingleItemSchedulerRunningDrainStep::DrainLost(pending);
        };
        match self.classify_single_item_observation(pending.into_owner(), observed, more) {
            BluetoothSingleItemObservedStep::RoleItemIdentityMismatch(owner) => {
                BluetoothSingleItemSchedulerRunningDrainStep::RoleItemIdentityMismatch(owner)
            }
            BluetoothSingleItemObservedStep::SchedulerStateMismatch(owner) => {
                BluetoothSingleItemSchedulerRunningDrainStep::SchedulerStateMismatch(owner)
            }
            BluetoothSingleItemObservedStep::UnrelatedList { drain, observed } => {
                BluetoothSingleItemSchedulerRunningDrainStep::UnrelatedList { drain, observed }
            }
            BluetoothSingleItemObservedStep::StillInFlight(drain) => {
                BluetoothSingleItemSchedulerRunningDrainStep::StillInFlight(drain)
            }
            BluetoothSingleItemObservedStep::CompletionObserved(drain) => {
                BluetoothSingleItemSchedulerRunningDrainStep::CompletionObserved(drain)
            }
        }
    }

    pub(crate) fn continue_single_item_completed_finished_list_drain<
        Role: BluetoothSingleItemSchedulerRole,
    >(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothSingleItemSchedulerCompletionObserved<Role>,
        >,
    ) -> BluetoothSingleItemSchedulerCompletionObservedDrainStep<Role> {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothSingleItemSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothSingleItemSchedulerCompletionObservedDrainStep::DrainLost(pending);
        }
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return BluetoothSingleItemSchedulerCompletionObservedDrainStep::DrainLost(pending);
        };
        let drain =
            BluetoothSchedulerFinishedListDrainState::from_worker_step(pending.into_owner(), more);
        if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO {
            BluetoothSingleItemSchedulerCompletionObservedDrainStep::RepeatedRoleList {
                _drain: drain,
                _observed: observed,
            }
        } else {
            BluetoothSingleItemSchedulerCompletionObservedDrainStep::UnrelatedList {
                drain,
                observed,
            }
        }
    }

    pub(crate) fn observe_single_item_hardware_head_retirement<
        Role: BluetoothSingleItemSchedulerRole,
    >(
        &mut self,
        completed: BluetoothSingleItemSchedulerCompletionObserved<Role>,
    ) -> BluetoothSingleItemSchedulerHardwareHeadRetirementStep<Role> {
        let address = completed.scheduler_item_address();
        if completed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothSingleItemSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(
                completed,
            );
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothSingleItemSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(
                completed,
            );
        }
        let BluetoothSingleItemSchedulerCompletionObserved {
            item,
            run,
            retained,
        } = completed;
        match self.task.observe_scheduler_hardware_list_head_retirement(run) {
            BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished {
                run,
                observed,
            } => BluetoothSingleItemSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                _completed: BluetoothSingleItemSchedulerCompletionObserved {
                    item,
                    run,
                    retained,
                },
                _observed: observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged {
                run,
                observed,
            } => BluetoothSingleItemSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                _completed: BluetoothSingleItemSchedulerCompletionObserved {
                    item,
                    run,
                    retained,
                },
                _observed: observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::EmptyObserved(head) => {
                if !self
                    ._scheduler_list
                    .retain_hardware_head_empty_first_item(address)
                {
                    return BluetoothSingleItemSchedulerHardwareHeadRetirementStep::SchedulerStateMismatch(
                        BluetoothSingleItemSchedulerHardwareHeadTransitionMismatch {
                            _item: item,
                            _head: head,
                            _retained: retained,
                        },
                    );
                }
                BluetoothSingleItemSchedulerHardwareHeadRetirementStep::EmptyObserved(
                    BluetoothSingleItemSchedulerHardwareHeadEmptyObserved {
                        item,
                        head,
                        retained,
                    },
                )
            }
        }
    }

    pub(crate) fn unlink_single_item_software_list<Role: BluetoothSingleItemSchedulerRole>(
        &mut self,
        observed: BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> BluetoothSingleItemSchedulerSoftwareListUnlinkStep<Role> {
        let address = observed.scheduler_item_address();
        if observed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self
                ._scheduler_list
                .unlink_software_list_first_item(address)
        {
            return BluetoothSingleItemSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(
                observed,
            );
        }
        let BluetoothSingleItemSchedulerHardwareHeadEmptyObserved {
            item,
            head,
            retained,
        } = observed;
        BluetoothSingleItemSchedulerSoftwareListUnlinkStep::Unlinked(
            BluetoothSingleItemSchedulerSoftwareListUnlinked {
                item,
                head,
                retained,
            },
        )
    }

    pub(crate) fn join_single_item_software_list_removal<Role: BluetoothSingleItemSchedulerRole>(
        &mut self,
        unlinked: BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
        event: BluetoothPrimarySchedulerEvent,
    ) -> BluetoothSingleItemSchedulerSoftwareListRemovalJoin<Role> {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothSingleItemSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                unlinked,
                event,
            };
        }
        let idle = match event.into_software_list_removal_gate() {
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending => {
                return BluetoothSingleItemSchedulerSoftwareListRemovalJoin::Pending(unlinked);
            }
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(idle) => idle,
        };
        let BluetoothSingleItemSchedulerSoftwareListUnlinked {
            item,
            head,
            retained,
        } = unlinked;
        match self.task.finish_scheduler_software_list_removal(idle, head) {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothSingleItemSchedulerSoftwareListRemovalJoin::Pending(
                    BluetoothSingleItemSchedulerSoftwareListUnlinked {
                        item,
                        head,
                        retained,
                    },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                if !self
                    ._scheduler_list
                    .retain_software_list_removal_ready_first_item(address)
                {
                    return BluetoothSingleItemSchedulerSoftwareListRemovalJoin::SchedulerStateMismatch(
                        BluetoothSingleItemSchedulerRemovalTransitionMismatch {
                            _item: item,
                            _removal: removal,
                            _retained: retained,
                        },
                    );
                }
                BluetoothSingleItemSchedulerSoftwareListRemovalJoin::Ready(
                    BluetoothSingleItemSchedulerSoftwareListRemovalReady {
                        item,
                        removal,
                        retained,
                    },
                )
            }
        }
    }

    pub(crate) fn recheck_single_item_software_list_removal<
        Role: BluetoothSingleItemSchedulerRole,
    >(
        &mut self,
        storage: &impl crate::BluetoothSchedulerRunInterruptStorage,
        unlinked: BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
    ) -> BluetoothSingleItemSchedulerSoftwareListRemovalRecheck<Role> {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothSingleItemSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(
                unlinked,
            );
        }
        let BluetoothSingleItemSchedulerSoftwareListUnlinked {
            item,
            head,
            retained,
        } = unlinked;
        let join = match self
            .task
            .recheck_scheduler_software_list_removal(storage, head)
        {
            Ok(join) => join,
            Err(head) => {
                return BluetoothSingleItemSchedulerSoftwareListRemovalRecheck::StorageUnavailable(
                    BluetoothSingleItemSchedulerSoftwareListUnlinked {
                        item,
                        head,
                        retained,
                    },
                );
            }
        };
        match join {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothSingleItemSchedulerSoftwareListRemovalRecheck::Pending(
                    BluetoothSingleItemSchedulerSoftwareListUnlinked {
                        item,
                        head,
                        retained,
                    },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                if !self
                    ._scheduler_list
                    .retain_software_list_removal_ready_first_item(address)
                {
                    return BluetoothSingleItemSchedulerSoftwareListRemovalRecheck::SchedulerStateMismatch(
                        BluetoothSingleItemSchedulerRemovalTransitionMismatch {
                            _item: item,
                            _removal: removal,
                            _retained: retained,
                        },
                    );
                }
                BluetoothSingleItemSchedulerSoftwareListRemovalRecheck::Ready(
                    BluetoothSingleItemSchedulerSoftwareListRemovalReady {
                        item,
                        removal,
                        retained,
                    },
                )
            }
        }
    }
}
