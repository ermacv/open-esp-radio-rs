//! Role-neutral ownership for one scheduler item from `RUN` through software-list
//! removal readiness.
//!
//! Concrete roles provide only memory-completion classification hooks.
//! Finished-list draining, hardware-head retirement, software unlink, removal
//! gating are implemented once here. Packet extraction, recycling, timeline
//! release and list reclamation remain role-specific tails.

#![forbid(unsafe_code)]

use crate::{
    interrupt::PrimarySchedulerEvent, runtime_resources::ControllerPoweredTaskRuntime,
    scheduler::BluetoothSchedulerFinishedHardwareListObserved,
};

use {
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHead,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHeadEmptyObserved,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHeadRetirementObservation,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareRunCommandPublished,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerSoftwareListRemovalInterruptStep,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerSoftwareListRemovalJoin,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerSoftwareListRemovalReady,
    oer_esp32s31_hal::types::BluetoothControllerSramAddress,
};

use super::{SchedulerFinishedListDrainPending, SchedulerFinishedListDrainState};

pub(crate) trait SingleItemSchedulerRole: Sized {
    type RunningItem;
    type CompletionObservedItem;
    type Retained;

    fn running_item_address(item: &Self::RunningItem) -> BluetoothControllerSramAddress;

    fn observe_completion(
        item: Self::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> SingleItemRoleCompletionObservation<Self>;

    fn completed_item_address(
        item: &Self::CompletionObservedItem,
    ) -> BluetoothControllerSramAddress;
}

pub(crate) enum SingleItemRoleCompletionObservation<Role: SingleItemSchedulerRole> {
    ListMismatch {
        running: Role::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(Role::RunningItem),
    CompletionObserved(Role::CompletionObservedItem),
}

pub(crate) struct SingleItemSchedulerRunning<Role: SingleItemSchedulerRole> {
    item: Role::RunningItem,
    run: BluetoothSchedulerHardwareRunCommandPublished,
    retained: Role::Retained,
}

impl<Role: SingleItemSchedulerRole> SingleItemSchedulerRunning<Role> {
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

pub(crate) struct SingleItemSchedulerCompletionObserved<Role: SingleItemSchedulerRole> {
    item: Role::CompletionObservedItem,
    run: BluetoothSchedulerHardwareRunCommandPublished,
    retained: Role::Retained,
}

impl<Role: SingleItemSchedulerRole> SingleItemSchedulerCompletionObserved<Role> {
    pub(crate) fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        Role::completed_item_address(&self.item)
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }
}

pub(crate) struct SingleItemSchedulerRoleItemIdentityMismatch<Role: SingleItemSchedulerRole> {
    _expected: BluetoothControllerSramAddress,
    _item: Role::CompletionObservedItem,
    _run: BluetoothSchedulerHardwareRunCommandPublished,
    _retained: Role::Retained,
}

pub(crate) struct SingleItemSchedulerHardwareHeadTransitionMismatch<Role: SingleItemSchedulerRole> {
    _item: Role::CompletionObservedItem,
    _head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    _retained: Role::Retained,
}

pub(crate) struct SingleItemSchedulerRemovalTransitionMismatch<Role: SingleItemSchedulerRole> {
    _item: Role::CompletionObservedItem,
    _removal: BluetoothSchedulerSoftwareListRemovalReady,
    _retained: Role::Retained,
}

pub(crate) struct SingleItemSchedulerHardwareHeadEmptyObserved<Role: SingleItemSchedulerRole> {
    item: Role::CompletionObservedItem,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    retained: Role::Retained,
}

impl<Role: SingleItemSchedulerRole> SingleItemSchedulerHardwareHeadEmptyObserved<Role> {
    pub(crate) fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        Role::completed_item_address(&self.item)
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }
}

pub(crate) struct SingleItemSchedulerSoftwareListUnlinked<Role: SingleItemSchedulerRole> {
    item: Role::CompletionObservedItem,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    retained: Role::Retained,
}

impl<Role: SingleItemSchedulerRole> SingleItemSchedulerSoftwareListUnlinked<Role> {
    pub(crate) fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        Role::completed_item_address(&self.item)
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }
}

pub(crate) struct SingleItemSchedulerSoftwareListRemovalReady<Role: SingleItemSchedulerRole> {
    item: Role::CompletionObservedItem,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    retained: Role::Retained,
}

impl<Role: SingleItemSchedulerRole> SingleItemSchedulerSoftwareListRemovalReady<Role> {
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

pub(crate) enum SingleItemSchedulerCompletionStep<Role: SingleItemSchedulerRole> {
    DrainAlreadyActive(SingleItemSchedulerRunning<Role>),
    SchedulerIdentityMismatch(SingleItemSchedulerRunning<Role>),
    RoleItemIdentityMismatch(SingleItemSchedulerRoleItemIdentityMismatch<Role>),
    SchedulerStateMismatch(SingleItemSchedulerRoleItemIdentityMismatch<Role>),
    NoFinishedList(SingleItemSchedulerRunning<Role>),
    UnrelatedList {
        drain: SchedulerFinishedListDrainState<SingleItemSchedulerRunning<Role>>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(SchedulerFinishedListDrainState<SingleItemSchedulerRunning<Role>>),
    CompletionObserved(
        SchedulerFinishedListDrainState<SingleItemSchedulerCompletionObserved<Role>>,
    ),
}

pub(crate) enum SingleItemSchedulerRunningDrainStep<Role: SingleItemSchedulerRole> {
    SchedulerIdentityMismatch(SchedulerFinishedListDrainPending<SingleItemSchedulerRunning<Role>>),
    DrainLost(SchedulerFinishedListDrainPending<SingleItemSchedulerRunning<Role>>),
    RoleItemIdentityMismatch(SingleItemSchedulerRoleItemIdentityMismatch<Role>),
    SchedulerStateMismatch(SingleItemSchedulerRoleItemIdentityMismatch<Role>),
    UnrelatedList {
        drain: SchedulerFinishedListDrainState<SingleItemSchedulerRunning<Role>>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(SchedulerFinishedListDrainState<SingleItemSchedulerRunning<Role>>),
    CompletionObserved(
        SchedulerFinishedListDrainState<SingleItemSchedulerCompletionObserved<Role>>,
    ),
}

pub(crate) enum SingleItemSchedulerCompletionObservedDrainStep<Role: SingleItemSchedulerRole> {
    SchedulerIdentityMismatch(
        SchedulerFinishedListDrainPending<SingleItemSchedulerCompletionObserved<Role>>,
    ),
    DrainLost(SchedulerFinishedListDrainPending<SingleItemSchedulerCompletionObserved<Role>>),
    UnrelatedList {
        drain: SchedulerFinishedListDrainState<SingleItemSchedulerCompletionObserved<Role>>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    RepeatedRoleList {
        _drain: SchedulerFinishedListDrainState<SingleItemSchedulerCompletionObserved<Role>>,
        _observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

pub(crate) enum SingleItemSchedulerHardwareHeadRetirementStep<Role: SingleItemSchedulerRole> {
    SchedulerIdentityMismatch(SingleItemSchedulerCompletionObserved<Role>),
    FinishedListDrainStillActive(SingleItemSchedulerCompletionObserved<Role>),
    ExpectedHeadStillPublished {
        _completed: SingleItemSchedulerCompletionObserved<Role>,
        _observed: BluetoothSchedulerHardwareListHead,
    },
    UnexpectedHeadChanged {
        _completed: SingleItemSchedulerCompletionObserved<Role>,
        _observed: BluetoothSchedulerHardwareListHead,
    },
    SchedulerStateMismatch(SingleItemSchedulerHardwareHeadTransitionMismatch<Role>),
    EmptyObserved(SingleItemSchedulerHardwareHeadEmptyObserved<Role>),
}

pub(crate) enum SingleItemSchedulerSoftwareListUnlinkStep<Role: SingleItemSchedulerRole> {
    SchedulerIdentityMismatch(SingleItemSchedulerHardwareHeadEmptyObserved<Role>),
    Unlinked(SingleItemSchedulerSoftwareListUnlinked<Role>),
}

pub(crate) enum SingleItemSchedulerSoftwareListRemovalJoin<Role: SingleItemSchedulerRole> {
    SchedulerIdentityMismatch {
        unlinked: SingleItemSchedulerSoftwareListUnlinked<Role>,
        event: PrimarySchedulerEvent,
    },
    SchedulerStateMismatch(SingleItemSchedulerRemovalTransitionMismatch<Role>),
    Pending(SingleItemSchedulerSoftwareListUnlinked<Role>),
    Ready(SingleItemSchedulerSoftwareListRemovalReady<Role>),
}

pub(crate) enum SingleItemSchedulerSoftwareListRemovalRecheck<Role: SingleItemSchedulerRole> {
    SchedulerIdentityMismatch(SingleItemSchedulerSoftwareListUnlinked<Role>),
    StorageUnavailable(SingleItemSchedulerSoftwareListUnlinked<Role>),
    SchedulerStateMismatch(SingleItemSchedulerRemovalTransitionMismatch<Role>),
    Pending(SingleItemSchedulerSoftwareListUnlinked<Role>),
    Ready(SingleItemSchedulerSoftwareListRemovalReady<Role>),
}

enum SingleItemObservedStep<Role: SingleItemSchedulerRole> {
    RoleItemIdentityMismatch(SingleItemSchedulerRoleItemIdentityMismatch<Role>),
    SchedulerStateMismatch(SingleItemSchedulerRoleItemIdentityMismatch<Role>),
    UnrelatedList {
        drain: SchedulerFinishedListDrainState<SingleItemSchedulerRunning<Role>>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(SchedulerFinishedListDrainState<SingleItemSchedulerRunning<Role>>),
    CompletionObserved(
        SchedulerFinishedListDrainState<SingleItemSchedulerCompletionObserved<Role>>,
    ),
}

impl<const CAPACITY: usize> ControllerPoweredTaskRuntime<'_, CAPACITY> {
    fn classify_single_item_observation<Role: SingleItemSchedulerRole>(
        &mut self,
        running: SingleItemSchedulerRunning<Role>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
        more: bool,
    ) -> SingleItemObservedStep<Role> {
        let expected = running.scheduler_item_address();
        let SingleItemSchedulerRunning {
            item,
            run,
            retained,
        } = running;
        match Role::observe_completion(item, observed) {
            SingleItemRoleCompletionObservation::ListMismatch {
                running: item,
                observed,
            } => SingleItemObservedStep::UnrelatedList {
                drain: SchedulerFinishedListDrainState::from_worker_step(
                    SingleItemSchedulerRunning {
                        item,
                        run,
                        retained,
                    },
                    more,
                ),
                observed,
            },
            SingleItemRoleCompletionObservation::StillInFlight(item) => {
                SingleItemObservedStep::StillInFlight(
                    SchedulerFinishedListDrainState::from_worker_step(
                        SingleItemSchedulerRunning {
                            item,
                            run,
                            retained,
                        },
                        more,
                    ),
                )
            }
            SingleItemRoleCompletionObservation::CompletionObserved(item) => {
                let completed = Role::completed_item_address(&item);
                let (item, run, retained) = match super::retain_matching_single_item_identity(
                    expected,
                    completed,
                    (item, run, retained),
                ) {
                    Ok(owner) => owner,
                    Err((expected, (item, run, retained))) => {
                        return SingleItemObservedStep::RoleItemIdentityMismatch(
                            SingleItemSchedulerRoleItemIdentityMismatch {
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
                    return SingleItemObservedStep::SchedulerStateMismatch(
                        SingleItemSchedulerRoleItemIdentityMismatch {
                            _expected: expected,
                            _item: item,
                            _run: run,
                            _retained: retained,
                        },
                    );
                }
                SingleItemObservedStep::CompletionObserved(
                    SchedulerFinishedListDrainState::from_worker_step(
                        SingleItemSchedulerCompletionObserved {
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

    pub(crate) fn observe_single_item_completion<Role: SingleItemSchedulerRole>(
        &mut self,
        running: SingleItemSchedulerRunning<Role>,
        wake: crate::interrupt::SchedulerWakeBatch,
    ) -> SingleItemSchedulerCompletionStep<Role> {
        let address = running.scheduler_item_address();
        if running.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return SingleItemSchedulerCompletionStep::SchedulerIdentityMismatch(running);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active()
            || self
                .task
                .capture_scheduler_finished_lists(self.runtime.scheduler_finished_lists_mut(), wake)
                .is_err()
        {
            return SingleItemSchedulerCompletionStep::DrainAlreadyActive(running);
        }
        let crate::scheduler::SchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return SingleItemSchedulerCompletionStep::NoFinishedList(running);
        };
        match self.classify_single_item_observation(running, observed, more) {
            SingleItemObservedStep::RoleItemIdentityMismatch(owner) => {
                SingleItemSchedulerCompletionStep::RoleItemIdentityMismatch(owner)
            }
            SingleItemObservedStep::SchedulerStateMismatch(owner) => {
                SingleItemSchedulerCompletionStep::SchedulerStateMismatch(owner)
            }
            SingleItemObservedStep::UnrelatedList { drain, observed } => {
                SingleItemSchedulerCompletionStep::UnrelatedList { drain, observed }
            }
            SingleItemObservedStep::StillInFlight(drain) => {
                SingleItemSchedulerCompletionStep::StillInFlight(drain)
            }
            SingleItemObservedStep::CompletionObserved(drain) => {
                SingleItemSchedulerCompletionStep::CompletionObserved(drain)
            }
        }
    }

    pub(crate) fn continue_single_item_running_finished_list_drain<
        Role: SingleItemSchedulerRole,
    >(
        &mut self,
        pending: SchedulerFinishedListDrainPending<SingleItemSchedulerRunning<Role>>,
    ) -> SingleItemSchedulerRunningDrainStep<Role> {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return SingleItemSchedulerRunningDrainStep::SchedulerIdentityMismatch(pending);
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return SingleItemSchedulerRunningDrainStep::DrainLost(pending);
        }
        let crate::scheduler::SchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return SingleItemSchedulerRunningDrainStep::DrainLost(pending);
        };
        match self.classify_single_item_observation(pending.into_owner(), observed, more) {
            SingleItemObservedStep::RoleItemIdentityMismatch(owner) => {
                SingleItemSchedulerRunningDrainStep::RoleItemIdentityMismatch(owner)
            }
            SingleItemObservedStep::SchedulerStateMismatch(owner) => {
                SingleItemSchedulerRunningDrainStep::SchedulerStateMismatch(owner)
            }
            SingleItemObservedStep::UnrelatedList { drain, observed } => {
                SingleItemSchedulerRunningDrainStep::UnrelatedList { drain, observed }
            }
            SingleItemObservedStep::StillInFlight(drain) => {
                SingleItemSchedulerRunningDrainStep::StillInFlight(drain)
            }
            SingleItemObservedStep::CompletionObserved(drain) => {
                SingleItemSchedulerRunningDrainStep::CompletionObserved(drain)
            }
        }
    }

    pub(crate) fn continue_single_item_completed_finished_list_drain<
        Role: SingleItemSchedulerRole,
    >(
        &mut self,
        pending: SchedulerFinishedListDrainPending<SingleItemSchedulerCompletionObserved<Role>>,
    ) -> SingleItemSchedulerCompletionObservedDrainStep<Role> {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return SingleItemSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return SingleItemSchedulerCompletionObservedDrainStep::DrainLost(pending);
        }
        let crate::scheduler::SchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return SingleItemSchedulerCompletionObservedDrainStep::DrainLost(pending);
        };
        let drain = SchedulerFinishedListDrainState::from_worker_step(pending.into_owner(), more);
        if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO {
            SingleItemSchedulerCompletionObservedDrainStep::RepeatedRoleList {
                _drain: drain,
                _observed: observed,
            }
        } else {
            SingleItemSchedulerCompletionObservedDrainStep::UnrelatedList { drain, observed }
        }
    }

    pub(crate) fn observe_single_item_hardware_head_retirement<Role: SingleItemSchedulerRole>(
        &mut self,
        completed: SingleItemSchedulerCompletionObserved<Role>,
    ) -> SingleItemSchedulerHardwareHeadRetirementStep<Role> {
        let address = completed.scheduler_item_address();
        if completed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return SingleItemSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(
                completed,
            );
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return SingleItemSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(
                completed,
            );
        }
        let SingleItemSchedulerCompletionObserved {
            item,
            run,
            retained,
        } = completed;
        match self.task.observe_scheduler_hardware_list_head_retirement(run) {
            BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished {
                run,
                observed,
            } => SingleItemSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                _completed: SingleItemSchedulerCompletionObserved {
                    item,
                    run,
                    retained,
                },
                _observed: observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged {
                run,
                observed,
            } => SingleItemSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                _completed: SingleItemSchedulerCompletionObserved {
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
                    return SingleItemSchedulerHardwareHeadRetirementStep::SchedulerStateMismatch(
                        SingleItemSchedulerHardwareHeadTransitionMismatch {
                            _item: item,
                            _head: head,
                            _retained: retained,
                        },
                    );
                }
                SingleItemSchedulerHardwareHeadRetirementStep::EmptyObserved(
                    SingleItemSchedulerHardwareHeadEmptyObserved {
                        item,
                        head,
                        retained,
                    },
                )
            }
        }
    }

    pub(crate) fn unlink_single_item_software_list<Role: SingleItemSchedulerRole>(
        &mut self,
        observed: SingleItemSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> SingleItemSchedulerSoftwareListUnlinkStep<Role> {
        let address = observed.scheduler_item_address();
        if observed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self
                ._scheduler_list
                .unlink_software_list_first_item(address)
        {
            return SingleItemSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(observed);
        }
        let SingleItemSchedulerHardwareHeadEmptyObserved {
            item,
            head,
            retained,
        } = observed;
        SingleItemSchedulerSoftwareListUnlinkStep::Unlinked(
            SingleItemSchedulerSoftwareListUnlinked {
                item,
                head,
                retained,
            },
        )
    }

    pub(crate) fn join_single_item_software_list_removal<Role: SingleItemSchedulerRole>(
        &mut self,
        unlinked: SingleItemSchedulerSoftwareListUnlinked<Role>,
        event: PrimarySchedulerEvent,
    ) -> SingleItemSchedulerSoftwareListRemovalJoin<Role> {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return SingleItemSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                unlinked,
                event,
            };
        }
        let idle = match event.into_software_list_removal_gate() {
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending => {
                return SingleItemSchedulerSoftwareListRemovalJoin::Pending(unlinked);
            }
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(idle) => idle,
        };
        let SingleItemSchedulerSoftwareListUnlinked {
            item,
            head,
            retained,
        } = unlinked;
        match self.task.finish_scheduler_software_list_removal(idle, head) {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                SingleItemSchedulerSoftwareListRemovalJoin::Pending(
                    SingleItemSchedulerSoftwareListUnlinked {
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
                    return SingleItemSchedulerSoftwareListRemovalJoin::SchedulerStateMismatch(
                        SingleItemSchedulerRemovalTransitionMismatch {
                            _item: item,
                            _removal: removal,
                            _retained: retained,
                        },
                    );
                }
                SingleItemSchedulerSoftwareListRemovalJoin::Ready(
                    SingleItemSchedulerSoftwareListRemovalReady {
                        item,
                        removal,
                        retained,
                    },
                )
            }
        }
    }

    pub(crate) fn recheck_single_item_software_list_removal<Role: SingleItemSchedulerRole>(
        &mut self,
        storage: &impl crate::controller::SchedulerRunInterruptStorage,
        unlinked: SingleItemSchedulerSoftwareListUnlinked<Role>,
    ) -> SingleItemSchedulerSoftwareListRemovalRecheck<Role> {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return SingleItemSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(
                unlinked,
            );
        }
        let SingleItemSchedulerSoftwareListUnlinked {
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
                return SingleItemSchedulerSoftwareListRemovalRecheck::StorageUnavailable(
                    SingleItemSchedulerSoftwareListUnlinked {
                        item,
                        head,
                        retained,
                    },
                );
            }
        };
        match join {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                SingleItemSchedulerSoftwareListRemovalRecheck::Pending(
                    SingleItemSchedulerSoftwareListUnlinked {
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
                    return SingleItemSchedulerSoftwareListRemovalRecheck::SchedulerStateMismatch(
                        SingleItemSchedulerRemovalTransitionMismatch {
                            _item: item,
                            _removal: removal,
                            _retained: retained,
                        },
                    );
                }
                SingleItemSchedulerSoftwareListRemovalRecheck::Ready(
                    SingleItemSchedulerSoftwareListRemovalReady {
                        item,
                        removal,
                        retained,
                    },
                )
            }
        }
    }
}
