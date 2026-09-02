//! Peripheral-connection scheduler preparation and completion.
//!
//! This module owns the connection-specific descriptor and memory transitions.
//! The parent scheduler retains protocol-neutral timeline, list-epoch and MMIO
//! publication primitives.

use super::*;

#[cfg(any(target_arch = "riscv32", test))]
impl<const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>
{
    /// Admit one causal first-connection window into the common timeline.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc failure returns the exact affine connection candidate"
    )]
    pub(crate) fn admit_peripheral_connection_first_event(
        &mut self,
        candidate: BluetoothPeripheralConnectionFirstEventCandidate,
        admission: BluetoothPeripheralConnectionAdmissionObservation,
    ) -> Result<
        BluetoothPeripheralConnectionFirstPreSequence,
        BluetoothPeripheralConnectionFirstEventPreparationFailure,
    > {
        let requested = candidate.requested_window();
        let timing_policy =
            BluetoothSchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_initial_window(
                requested.start(),
                requested.end(),
                timing_policy,
                admission.sample,
            ) {
            Ok(reservation) => Ok(BluetoothPeripheralConnectionFirstPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(BluetoothPeripheralConnectionFirstEventPreparationFailure {
                candidate,
                error: BluetoothPeripheralConnectionFirstEventPreparationError::Timeline(error),
            }),
        }
    }

    /// Authorize the second deadline and encode only the resolved connection window.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc failure returns the exact affine connection candidate"
    )]
    pub(crate) fn prepare_peripheral_connection_first_event(
        &mut self,
        admitted: BluetoothPeripheralConnectionFirstPreSequence,
        sequence: BluetoothPeripheralConnectionSequenceObservation,
        default_tx_power: open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionDefaultTxPowerDbm,
        direction_finding_workspace: open_esp_radio_esp32s31_bluetooth_memory::BluetoothDirectionFindingWorkspaceLink,
    ) -> Result<
        BluetoothPeripheralConnectionEventPrepared,
        BluetoothPeripheralConnectionFirstEventPreparationFailure,
    > {
        let BluetoothPeripheralConnectionFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return Err(BluetoothPeripheralConnectionFirstEventPreparationFailure {
                    candidate,
                    error: BluetoothPeripheralConnectionFirstEventPreparationError::Sequence(error),
                });
            }
        };
        let resolved_window = reservation.window();
        match candidate.prepare_resolved_event_fields(resolved_window, default_tx_power) {
            Ok(event) => Ok(BluetoothPeripheralConnectionEventPrepared {
                event: event.install_direction_finding_workspace(direction_finding_workspace),
                reservation,
            }),
            Err(candidate) => {
                self.release_scheduler_reservation(reservation);
                Err(BluetoothPeripheralConnectionFirstEventPreparationFailure {
                    candidate,
                    error: BluetoothPeripheralConnectionFirstEventPreparationError::Descriptor,
                })
            }
        }
    }

    /// Release one unpublished connection event and its exact timeline slot.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel_peripheral_connection_first_event(
        &mut self,
        prepared: BluetoothPeripheralConnectionEventPrepared,
    ) -> (
        crate::BluetoothPeripheralConnectionRuntimeAllocation,
        open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
    ) {
        let BluetoothPeripheralConnectionEventPrepared { event, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        event.cancel()
    }

    /// Release an admitted connection candidate before sequence authorization.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel_peripheral_connection_first_pre_sequence(
        &mut self,
        admitted: BluetoothPeripheralConnectionFirstPreSequence,
    ) -> (
        crate::BluetoothPeripheralConnectionRuntimeAllocation,
        open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
    ) {
        let BluetoothPeripheralConnectionFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        self.release_scheduler_reservation(reservation);
        candidate.cancel()
    }

    /// Join the selected connection item to this epoch's empty scheduler list.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc failure retains the complete affine connection event"
    )]
    pub(crate) fn prepare_peripheral_connection_empty_list_merge(
        &mut self,
        prepared: BluetoothPeripheralConnectionEventPrepared,
    ) -> Result<
        BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
        BluetoothPeripheralConnectionEmptySchedulerMergeFailure,
    > {
        let BluetoothPeripheralConnectionEventPrepared { event, reservation } = prepared;
        let event = event.prepare_scheduler_admission();
        let address = event.scheduler_head();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(BluetoothPeripheralConnectionEmptySchedulerMergeFailure {
                error,
                prepared: BluetoothPeripheralConnectionEventPrepared {
                    event: event.cancel(),
                    reservation,
                },
            });
        }
        Ok(BluetoothPeripheralConnectionEmptySchedulerMergePrepared { event, reservation })
    }

    /// Restore an unpublished connection merge through the same scheduler epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc cancellation failure retains the complete affine merge"
    )]
    pub(crate) fn cancel_peripheral_connection_empty_list_merge(
        &mut self,
        merged: BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<
        BluetoothPeripheralConnectionEventPrepared,
        BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    > {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        let BluetoothPeripheralConnectionEmptySchedulerMergePrepared { event, reservation } =
            merged;
        Ok(BluetoothPeripheralConnectionEventPrepared {
            event: event.cancel(),
            reservation,
        })
    }

    /// Publish selector-two RX memory and the exact connection scheduler head.
    ///
    /// Common-list identity is validated before the first irreversible MMIO.
    /// The remaining RX/head suffix is therefore infallible and ordered.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        clippy::result_large_err,
        reason = "the powered task owner and exact connection graph retain every PAC publication prerequisite"
    )]
    pub(crate) fn publish_peripheral_connection_scheduler_head(
        &mut self,
        merged: BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<
        BluetoothPeripheralConnectionSchedulerHeadPublished,
        BluetoothPeripheralConnectionSchedulerHeadPublicationFailure,
    > {
        let address = merged.scheduler_item_address();
        let index = merged.hardware_list_index();
        let head = match self.validate_first_scheduler_item_head(address) {
            Ok(head) => head,
            Err(error) => {
                return Err(
                    BluetoothPeripheralConnectionSchedulerHeadPublicationFailure { error, merged },
                );
            }
        };
        let BluetoothPeripheralConnectionEmptySchedulerMergePrepared { event, reservation } =
            merged;
        let (graph, remainder) = event.prepare_publication().into_parts();
        let graph = unsafe { self.task.publish_peripheral_connection_rx_memory(graph) };
        let event = remainder.join_rx_publication(graph);
        let publication = self.publish_validated_first_scheduler_item_head(address, index, head);
        Ok(BluetoothPeripheralConnectionSchedulerHeadPublished {
            event,
            publication,
            reservation,
        })
    }

    /// Perform one fresh, bounded first-connection completion observation.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_peripheral_connection_completion(
        &mut self,
        running: BluetoothPeripheralConnectionSchedulerRunning,
        wake: BluetoothSchedulerWakeBatch,
    ) -> BluetoothPeripheralConnectionSchedulerCompletionStep {
        let address = running.scheduler_item_address();
        if running.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerCompletionStep::SchedulerIdentityMismatch(
                running,
            );
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPeripheralConnectionSchedulerCompletionStep::DrainAlreadyActive(
                running,
            );
        }

        if self
            .task
            .capture_scheduler_finished_lists(self.runtime.scheduler_finished_lists_mut(), wake)
            .is_err()
        {
            return BluetoothPeripheralConnectionSchedulerCompletionStep::DrainAlreadyActive(
                running,
            );
        }
        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } = step else {
            return BluetoothPeripheralConnectionSchedulerCompletionStep::NoFinishedList(running);
        };

        let BluetoothPeripheralConnectionSchedulerRunning {
            event,
            run,
            reservation,
        } = running;
        match event.observe_completion(observed) {
            BluetoothPeripheralConnectionFirstEventCompletionObservation::ListMismatch {
                running: event,
                observed,
            } => BluetoothPeripheralConnectionSchedulerCompletionStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(
                    BluetoothPeripheralConnectionSchedulerRunning {
                        event,
                        run,
                        reservation,
                    },
                    more,
                ),
                observed,
            },
            BluetoothPeripheralConnectionFirstEventCompletionObservation::StillInFlight(event) => {
                BluetoothPeripheralConnectionSchedulerCompletionStep::StillInFlight(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPeripheralConnectionSchedulerRunning {
                            event,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
            BluetoothPeripheralConnectionFirstEventCompletionObservation::CompletionObserved(
                event,
            ) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothPeripheralConnectionSchedulerCompletionStep::CompletionObserved(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPeripheralConnectionSchedulerCompletionObserved {
                            event,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
        }
    }

    /// Continue the same captured finished-list set while the connection is running.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn continue_peripheral_connection_running_finished_list_drain(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothPeripheralConnectionSchedulerRunning,
        >,
    ) -> BluetoothPeripheralConnectionSchedulerRunningDrainStep {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerRunningDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPeripheralConnectionSchedulerRunningDrainStep::DrainLost(pending);
        }
        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } = step else {
            return BluetoothPeripheralConnectionSchedulerRunningDrainStep::DrainLost(pending);
        };
        let BluetoothPeripheralConnectionSchedulerRunning {
            event,
            run,
            reservation,
        } = pending.into_owner();
        match event.observe_completion(observed) {
            BluetoothPeripheralConnectionFirstEventCompletionObservation::ListMismatch {
                running: event,
                observed,
            } => BluetoothPeripheralConnectionSchedulerRunningDrainStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(
                    BluetoothPeripheralConnectionSchedulerRunning {
                        event,
                        run,
                        reservation,
                    },
                    more,
                ),
                observed,
            },
            BluetoothPeripheralConnectionFirstEventCompletionObservation::StillInFlight(event) => {
                BluetoothPeripheralConnectionSchedulerRunningDrainStep::StillInFlight(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPeripheralConnectionSchedulerRunning {
                            event,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
            BluetoothPeripheralConnectionFirstEventCompletionObservation::CompletionObserved(
                event,
            ) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothPeripheralConnectionSchedulerRunningDrainStep::CompletionObserved(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPeripheralConnectionSchedulerCompletionObserved {
                            event,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
        }
    }

    /// Continue one captured set after list zero completed the connection item.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn continue_peripheral_connection_completed_finished_list_drain(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothPeripheralConnectionSchedulerCompletionObserved,
        >,
    ) -> BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep::DrainLost(
                pending,
            );
        }
        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } = step else {
            return BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep::DrainLost(
                pending,
            );
        };
        let completed = pending.into_owner();
        if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO {
            BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep::RepeatedConnectionList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(completed, more),
                observed,
            }
        } else {
            BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(completed, more),
                observed,
            }
        }
    }

    /// Observe the post-picker hardware-head retirement barrier for a connection.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_peripheral_connection_hardware_head_retirement(
        &mut self,
        completed: BluetoothPeripheralConnectionSchedulerCompletionObserved,
    ) -> BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep {
        let address = completed.scheduler_item_address();
        if completed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(completed);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(completed);
        }

        let BluetoothPeripheralConnectionSchedulerCompletionObserved {
            event,
            run,
            reservation,
        } = completed;
        match self
            .task
            .observe_scheduler_hardware_list_head_retirement(run)
        {
            BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished {
                run,
                observed,
            } => BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                completed: BluetoothPeripheralConnectionSchedulerCompletionObserved {
                    event,
                    run,
                    reservation,
                },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged {
                run,
                observed,
            } => BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                completed: BluetoothPeripheralConnectionSchedulerCompletionObserved {
                    event,
                    run,
                    reservation,
                },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::EmptyObserved(head) => {
                assert_eq!(
                    head.completed_head().address(),
                    Some(address),
                    "the retired hardware head must retain the exact connection item"
                );
                self._scheduler_list
                    .retain_hardware_head_empty_first_item(address);
                BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep::EmptyObserved(
                    BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved {
                        event,
                        head,
                        reservation,
                    },
                )
            }
        }
    }

    /// Remove the exact empty-head connection item from the source-owned list.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn unlink_peripheral_connection_software_list(
        &mut self,
        observed: BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved,
    ) -> BluetoothPeripheralConnectionSchedulerSoftwareListUnlinkStep {
        let address = observed.scheduler_item_address();
        if observed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self
                ._scheduler_list
                .unlink_software_list_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(observed);
        }
        let BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved {
            event,
            head,
            reservation,
        } = observed;
        BluetoothPeripheralConnectionSchedulerSoftwareListUnlinkStep::Unlinked(
            BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
                event,
                head,
                reservation,
            },
        )
    }

    /// Join one serviced primary scheduler event to the already-unlinked connection.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn join_peripheral_connection_software_list_removal(
        &mut self,
        unlinked: BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
        event: crate::BluetoothPrimarySchedulerEvent,
    ) -> BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                unlinked,
                event,
            };
        }
        let idle = match event.into_software_list_removal_gate() {
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending => {
                return BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::Pending(
                    unlinked,
                );
            }
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(idle) => idle,
        };
        let BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
            event,
            head,
            reservation,
        } = unlinked;
        match self.task.finish_scheduler_software_list_removal(idle, head) {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::Pending(
                    BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
                        event,
                        head,
                        reservation,
                    },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::Ready(
                    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady {
                        event,
                        removal,
                        reservation,
                    },
                )
            }
        }
    }

    /// Recheck one unlinked connection without requiring another interrupt edge.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_peripheral_connection_software_list_removal(
        &mut self,
        storage: &impl crate::BluetoothSchedulerRunInterruptStorage,
        unlinked: BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
    ) -> BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(unlinked);
        }
        let BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
            event,
            head,
            reservation,
        } = unlinked;
        let join = match self
            .task
            .recheck_scheduler_software_list_removal(storage, head)
        {
            Ok(join) => join,
            Err(head) => {
                return BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::StorageUnavailable(
                    BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
                        event,
                        head,
                        reservation,
                    },
                );
            }
        };
        match join {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::Pending(
                    BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
                        event,
                        head,
                        reservation,
                    },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::Ready(
                    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady {
                        event,
                        removal,
                        reservation,
                    },
                )
            }
        }
    }
}
