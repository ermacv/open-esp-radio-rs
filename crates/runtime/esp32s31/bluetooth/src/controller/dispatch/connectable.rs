//! Connectable advertising dispatch and its transfer into a peripheral owner.
//!
//! This handler borrows the sole command actor. It retains first-event,
//! recurring, response and stopping owners in the same slot across each wait.
//! Connection admission transfers that slot to peripheral dispatch. An empty
//! output lets the outer dispatcher recheck time and select the next phase.

use super::*;

impl<'runtime, S, const CAPACITY: usize> ControllerCommandTask<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    #[allow(
        clippy::manual_async_fn,
        reason = "inline(always) applies to the async poll body to bound affine transition stack frames"
    )]
    pub(super) fn step_connectable<
        'epoch,
        'packet,
        WakeMutex: RawMutex,
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
        Recheck: DtmControllerTimeRecheck,
        DelaySource: LegacyAdvertisingDelaySource
            + oer_esp32s31_bluetooth::le::peripheral::PeripheralEncryptionRandomSource,
    >(
        &mut self,
        wakers: &RuntimeNotifications<WakeMutex>,
        controller: &mut LeControllerCommandEndpoint<
            'epoch,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        packet: &mut Option<&'packet mut [u8]>,
        recheck: &mut Recheck,
        advertising_delay: &mut DelaySource,
        output: &mut Option<ControllerCommandBoundary<'runtime, 'epoch, 'packet, S, CAPACITY>>,
    ) -> impl core::future::Future<Output = ()> {
        // Inline the poll body, not just future construction: outlining it
        // adds another stack frame carrying the large affine transition enums.
        #[inline(always)]
        async move {
            match self.phase() {
                ControllerCommandPhase::LegacyConnectableAdvertisingFirst => {
                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingRetry(_,)
                    ) {
                        let ControllerCommandState::LegacyConnectableAdvertisingRetry(retry) =
                            self.owner.take()
                        else {
                            unreachable!("the selected connectable retry did not change")
                        };
                        if let Some(observation) = self.store_legacy_connectable_advertising_drive(
                            ControllerCommandPhase::LegacyConnectableAdvertisingFirst,
                            drive_legacy_connectable_advertising_first_ready(retry.retry()),
                        ) {
                            return *output = Some(observation);
                        }
                        return;
                    }
                    let ControllerCommandState::LegacyConnectableAdvertisingFirst(wait) =
                        self.owner.current_mut()
                    else {
                        unreachable!("the selected connectable wait did not change")
                    };
                    wait.wait_for_recheck(recheck.wait_until_absolute_recheck())
                        .await;
                    let ControllerCommandState::LegacyConnectableAdvertisingFirst(wait) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited connectable wait did not change")
                    };
                    match wait.resume() {
                        LegacyConnectableAdvertisingFirstResume::Ready(drive) => {
                            if let Some(observation) = self
                                .store_legacy_connectable_advertising_drive(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingFirst,
                                    drive,
                                )
                            {
                                *output = Some(observation)
                            }
                        }
                        LegacyConnectableAdvertisingFirstResume::NotReady(wait) => {
                            self.store_retained_state(
                                ControllerCommandPhase::LegacyConnectableAdvertisingFirst,
                                ControllerCommandState::LegacyConnectableAdvertisingFirst(wait),
                            );
                        }
                    }
                }
                ControllerCommandPhase::LegacyConnectableAdvertisingResponse => {
                    let ControllerCommandState::LegacyConnectableAdvertisingResponse(pending) =
                        self.owner.current()
                    else {
                        unreachable!("the selected connectable response did not change")
                    };
                    let radio_ready = match pending.radio_wait() {
                        Some(LegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => {
                            match select(
                                wakers.wait_scheduler_ready(wake),
                                pending.wait_response_capacity(controller),
                            )
                            .await
                            {
                                Either::First(()) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ));
                                }
                            }
                        }
                        Some(LegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
                            match select(
                                wakers.wait_post_unlink_or_recheck(
                                    wake,
                                    recheck.wait_until_absolute_recheck(),
                                ),
                                pending.wait_response_capacity(controller),
                            )
                            .await
                            {
                                Either::First(_) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ));
                                }
                            }
                        }
                        None => true,
                    };
                    let ControllerCommandState::LegacyConnectableAdvertisingResponse(pending) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited connectable response did not change")
                    };
                    if radio_ready {
                        if let Some(observation) =
                            drive_legacy_connectable_advertising_initial_pending_ready_with(
                                pending,
                                (&mut *self, &mut *advertising_delay),
                                LegacyConnectableAdvertisingReadyContinuations::new(
                                    |(actor, _): (&mut Self, &mut DelaySource), pending| {
                                        actor.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandState::LegacyConnectableAdvertisingResponse(pending),
                                );
                                        None
                                    },
                                    |(actor, _): (&mut Self, &mut DelaySource),
                                     pending,
                                     observed| {
                                        Some(actor.store_unowned_finished_list(
                                ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                UnownedFinishedListOwner::LegacyConnectableAdvertisingInitialPending {
                                    _pending: pending,
                                    observed,
                                },
                            ))
                                    },
                                    |(actor, delay_source): (&mut Self, &mut DelaySource),
                                     completed| {
                                        let delay = delay_source.next_advertising_delay();
                                        recurring::begin_response(actor, ControllerCommandPhase::LegacyConnectableAdvertisingResponse, completed, delay)
                                    },
                                    |(actor, _): (&mut Self, &mut DelaySource), accepted| {
                                        actor.store_peripheral_connection_first_drive(
                                ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                begin_legacy_connectable_peripheral_first_response_pending(accepted),
                            )
                                    },
                                    |(actor, _): (&mut Self, &mut DelaySource), fault| {
                                        Some(actor.terminal_boundary(
                                ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                ControllerCommandBoundary::LegacyConnectableAdvertisingPendingFailStop(fault),
                                    ))
                                    },
                                ),
                            )
                        {
                            *output = Some(observation)
                        }
                    } else {
                        match pending.try_publish(controller) {
                            LegacyConnectableAdvertisingResponsePublication::Published(active) => {
                                self.store_transition(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandStimulus::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(
                                        active,
                                    ),
                                );
                                // RUN was reported when the running owner was installed.
                            }
                            LegacyConnectableAdvertisingResponsePublication::Pending(pending) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandState::LegacyConnectableAdvertisingResponse(
                                        pending,
                                    ),
                                )
                            }
                            LegacyConnectableAdvertisingResponsePublication::EndpointMismatch(
                                pending,
                            ) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandState::LegacyConnectableAdvertisingResponse(
                                        pending,
                                    ),
                                );
                                *output =
                                    Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ))
                            }
                            LegacyConnectableAdvertisingResponsePublication::Fault {
                                pending,
                                error,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingResponse,
                                    ControllerCommandState::LegacyConnectableAdvertisingResponse(
                                        pending,
                                    ),
                                );
                                *output =
                                    Some(self.retain_boundary(ControllerCommandBoundary::HciFault(
                                        error,
                                    )))
                            }
                        }
                    }
                }
                ControllerCommandPhase::LegacyConnectableAdvertisingActive => {
                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(
                            _
                        )
                    ) {
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(
                        wait,
                    ) = self.owner.current()
                    else {
                        unreachable!("the selected recurring cancellation did not change")
                    };
                        let ready = wait
                            .wait_for_recheck(recheck.wait_until_absolute_recheck())
                            .await;
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringCancellation(
                        wait,
                    ) = self.owner.take()
                    else {
                        unreachable!("the awaited recurring cancellation did not change")
                    };
                        if let Some(observation) = self.store_connectable_recurring_stop_drive(
                            ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                            wait.resume_with(ready, &ConnectableRecurringStopDriveHandler),
                        ) {
                            return *output = Some(observation);
                        }
                        return;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(
                            _
                        )
                    ) {
                        let phase = ControllerCommandPhase::LegacyConnectableAdvertisingActive;
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(
                        wait,
                    ) = self.owner.current()
                    else {
                        unreachable!("selected recurrence wait is retained")
                    };
                        let outcome = select(
                            wait.wait_for_recheck(recheck.wait_until_absolute_recheck()),
                            wait.wait_response_capacity(controller),
                        )
                        .await;
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(
                        wait,
                    ) = self.owner.take()
                    else {
                        unreachable!("awaited recurrence wait is retained")
                    };
                        match outcome {
                            Either::First(ready) => {
                                if let Some(observation) =
                                    recurring::resume_response_wait(self, phase, wait, ready)
                                {
                                    return *output = Some(observation);
                                }
                            }
                            Either::Second(Ok(())) => {
                                if let Some(observation) = recurring::publish_response::<
                                    S,
                                    CAPACITY,
                                    ConnectableRecurringSequencePendingPhase,
                                    _,
                                    HOST_TO_CONTROLLER_DEPTH,
                                    CONTROLLER_TO_HOST_DEPTH,
                                    PACKET_CAPACITY,
                                >(
                                    self, wait.into_state(), controller
                                ) {
                                    return *output = Some(observation);
                                }
                            }
                            Either::Second(Err(_)) => {
                                self.store_retained_state(phase, ControllerCommandState::LegacyConnectableAdvertisingRecurringResponseWait(wait));
                                return *output =
                                    Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ));
                            }
                        }
                        return;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(_)
                    ) {
                        let phase = ControllerCommandPhase::LegacyConnectableAdvertisingActive;
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(
                        wait,
                    ) = self.owner.current()
                    else {
                        unreachable!("selected recurrence wait is retained")
                    };
                        let outcome = select(
                            wait.wait_for_recheck(recheck.wait_until_absolute_recheck()),
                            wait.wait_command_available(controller),
                        )
                        .await;
                        let ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(
                        wait,
                    ) = self.owner.take()
                    else {
                        unreachable!("awaited recurrence wait is retained")
                    };
                        match outcome {
                            Either::First(ready) => {
                                if let Some(observation) =
                                    recurring::resume_command_wait(self, phase, wait, ready)
                                {
                                    return *output = Some(observation);
                                }
                            }
                            Either::Second(Ok(())) => {
                                let buffer = packet
                                    .take()
                                    .expect("recurring command intake retains its scratch buffer");
                                let (buffer, boundary) =
                                    recurring::route_command::<
                                        S,
                                        CAPACITY,
                                        ConnectableRecurringSequencePendingPhase,
                                        _,
                                        HOST_TO_CONTROLLER_DEPTH,
                                        CONTROLLER_TO_HOST_DEPTH,
                                        PACKET_CAPACITY,
                                    >(
                                        self, phase, wait.into_state(), controller, buffer
                                    )
                                    .into_parts();
                                *packet = buffer;
                                if let Some(observation) = boundary {
                                    return *output = Some(observation);
                                }
                            }
                            Either::Second(Err(_)) => {
                                self.store_retained_state(phase, ControllerCommandState::LegacyConnectableAdvertisingRecurringCommandWait(wait));
                                return *output =
                                    Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ));
                            }
                        }
                        return;
                    }

                    if recurring::is_retry(self.owner.current()) {
                        let buffer = packet
                            .take()
                            .expect("recurrence retains its scratch buffer");
                        let (buffer, boundary) =
                            recurring::retry_ready(self, controller, buffer).into_parts();
                        *packet = buffer;
                        if let Some(observation) = boundary {
                            return *output = Some(observation);
                        }
                        return;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(_)
                    ) {
                        let ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(
                            pending,
                        ) = self.owner.current()
                        else {
                            unreachable!("the selected connectable response did not change")
                        };
                        let radio_ready = match pending.radio_wait() {
                            Some(LegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => {
                                match select(
                                    wakers.wait_scheduler_ready(wake),
                                    pending.wait_response_capacity(controller),
                                )
                                .await
                                {
                                    Either::First(()) => true,
                                    Either::Second(Ok(())) => false,
                                    Either::Second(Err(_)) => {
                                        return *output = Some(self.retain_boundary(
                                            ControllerCommandBoundary::EndpointMismatch,
                                        ));
                                    }
                                }
                            }
                            Some(LegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
                                match select(
                                    wakers.wait_post_unlink_or_recheck(
                                        wake,
                                        recheck.wait_until_absolute_recheck(),
                                    ),
                                    pending.wait_response_capacity(controller),
                                )
                                .await
                                {
                                    Either::First(_) => true,
                                    Either::Second(Ok(())) => false,
                                    Either::Second(Err(_)) => {
                                        return *output = Some(self.retain_boundary(
                                            ControllerCommandBoundary::EndpointMismatch,
                                        ));
                                    }
                                }
                            }
                            None => true,
                        };
                        let ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(
                            pending,
                        ) = self.owner.take()
                        else {
                            unreachable!("the awaited connectable response did not change")
                        };
                        if radio_ready {
                            if let Some(observation) =
                                drive_legacy_connectable_advertising_pending_ready_with(
                                    pending,
                                    (&mut *self, &mut *advertising_delay),
                                    LegacyConnectableAdvertisingReadyContinuations::new(
                                        |(actor, _): (&mut Self, &mut DelaySource), pending| {
                                            actor.store_retained_state(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    );
                                            None
                                        },
                                        |(actor, _): (&mut Self, &mut DelaySource),
                                         pending,
                                         observed| {
                                            Some(actor.store_unowned_finished_list(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    UnownedFinishedListOwner::LegacyConnectableAdvertisingPending { _pending: pending, observed },
                                ))
                                        },
                                        |(actor, delay_source): (&mut Self, &mut DelaySource),
                                         completed| {
                                            let delay = delay_source.next_advertising_delay();
                                            recurring::begin_response(actor, ControllerCommandPhase::LegacyConnectableAdvertisingActive, completed, delay)
                                        },
                                        |(actor, _): (&mut Self, &mut DelaySource), accepted| {
                                            actor.store_peripheral_connection_first_drive(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    begin_legacy_connectable_peripheral_first_response_pending(accepted),
                                )
                                        },
                                        |(actor, _): (&mut Self, &mut DelaySource), fault| {
                                            Some(actor.terminal_boundary(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandBoundary::LegacyConnectableAdvertisingPendingFailStop(fault),
                                ))
                                        },
                                    ),
                                )
                            {
                                return *output = Some(observation);
                            }
                        } else {
                            match pending.try_publish(controller) {
                                LegacyConnectableAdvertisingActiveResponsePublication::Published(active) => self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(active),
                                ),
                                LegacyConnectableAdvertisingActiveResponsePublication::Pending(pending) => self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                ),
                                LegacyConnectableAdvertisingActiveResponsePublication::EndpointMismatch(pending) => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    );
                                    return *output = Some(self.retain_boundary(ControllerCommandBoundary::EndpointMismatch));
                                }
                                LegacyConnectableAdvertisingActiveResponsePublication::Fault { pending, error } => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    );
                                    return *output = Some(self.retain_boundary(ControllerCommandBoundary::HciFault(error)));
                                }
                            }
                        }
                        return;
                    }

                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::LegacyConnectableAdvertisingStopping(_)
                    ) {
                        let ControllerCommandState::LegacyConnectableAdvertisingStopping(stopping) =
                            self.owner.current()
                        else {
                            unreachable!("the selected connectable stopping owner did not change")
                        };
                        match stopping.radio_wait() {
                            Some(LegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => {
                                wakers.wait_scheduler_ready(wake).await
                            }
                            Some(LegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
                                let _ = wakers
                                    .wait_post_unlink_or_recheck(
                                        wake,
                                        recheck.wait_until_absolute_recheck(),
                                    )
                                    .await;
                            }
                            None => {}
                        }
                        let ControllerCommandState::LegacyConnectableAdvertisingStopping(stopping) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited connectable stopping owner did not change")
                        };
                        match drive_legacy_connectable_advertising_stopping_ready(stopping) {
                            LegacyConnectableAdvertisingStoppingStep::Continue(stopping)
                            | LegacyConnectableAdvertisingStoppingStep::Waiting(stopping) => self.store_retained_state(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                ControllerCommandState::LegacyConnectableAdvertisingStopping(stopping),
                            ),
                            LegacyConnectableAdvertisingStoppingStep::UnrelatedList { stopping, observed } => return *output = Some(self.store_unowned_finished_list(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                UnownedFinishedListOwner::LegacyConnectableAdvertisingStopping { _stopping: stopping, observed },
                            )),
                            LegacyConnectableAdvertisingStoppingStep::NoConnection(completed) => {
                                if let Some(observation) = self.store_connectable_recurring_stop_drive(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    finish_legacy_connectable_advertising_no_connection_stopping_with(
                                        completed,
                                        &ConnectableRecurringStopDriveHandler,
                                    ),
                                ) {
                                    return *output = Some(observation);
                                }
                            }
                            LegacyConnectableAdvertisingStoppingStep::ConnectionAccepted(accepted) => {
                                if let Some(observation) = self.store_peripheral_connection_stopping_step(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    begin_legacy_connectable_peripheral_first_stopping(accepted),
                                ) {
                                    return *output = Some(observation);
                                }
                            }
                            LegacyConnectableAdvertisingStoppingStep::FailStop(fault) => return *output = Some(self.terminal_boundary(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                ControllerCommandBoundary::LegacyConnectableAdvertisingStoppingFailStop(fault),
                            )),
                        }
                        return;
                    }

                    let ControllerCommandState::LegacyConnectableAdvertisingActive(active) =
                        self.owner.current()
                    else {
                        unreachable!("the selected connectable active owner did not change")
                    };
                    let radio_ready = match active.radio_wait() {
                        Some(LegacyConnectableAdvertisingActiveWait::Scheduler(wake)) => {
                            match select(
                                wakers.wait_scheduler_ready(wake),
                                active.wait_command_available(controller),
                            )
                            .await
                            {
                                Either::First(()) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ));
                                }
                            }
                        }
                        Some(LegacyConnectableAdvertisingActiveWait::PostUnlink(wake)) => {
                            match select(
                                wakers.wait_post_unlink_or_recheck(
                                    wake,
                                    recheck.wait_until_absolute_recheck(),
                                ),
                                active.wait_command_available(controller),
                            )
                            .await
                            {
                                Either::First(_) => true,
                                Either::Second(Ok(())) => false,
                                Either::Second(Err(_)) => {
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ));
                                }
                            }
                        }
                        None => true,
                    };
                    let ControllerCommandState::LegacyConnectableAdvertisingActive(active) =
                        self.owner.take()
                    else {
                        unreachable!("the driven connectable active owner did not change")
                    };
                    if radio_ready {
                        match drive_legacy_connectable_advertising_active_ready(active) {
                            LegacyConnectableAdvertisingHciActiveStep::Continue(active)
                            | LegacyConnectableAdvertisingHciActiveStep::Waiting(active) => self.store_retained_state(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                ControllerCommandState::LegacyConnectableAdvertisingActive(active),
                            ),
                            LegacyConnectableAdvertisingHciActiveStep::UnrelatedList { session: active, observed } => *output = Some(self.store_unowned_finished_list(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                UnownedFinishedListOwner::LegacyConnectableAdvertisingActive { _active: active, observed },
                            )),
                            LegacyConnectableAdvertisingHciActiveStep::NoConnection(completed) => {
                                let delay = advertising_delay.next_advertising_delay();
                                if let Some(observation) = recurring::begin_command(self, ControllerCommandPhase::LegacyConnectableAdvertisingActive, completed, delay) {
                                    *output = Some(observation)
                                }
                            }
                            LegacyConnectableAdvertisingHciActiveStep::ConnectionAccepted(accepted) => {
                                if let Some(observation) = self.store_peripheral_connection_first_drive(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    begin_legacy_connectable_peripheral_first_command_ready(accepted),
                                ) {
                                    *output = Some(observation)
                                }
                            }
                            LegacyConnectableAdvertisingHciActiveStep::FailStop(fault) => *output = Some(self.terminal_boundary(
                                ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                ControllerCommandBoundary::LegacyConnectableAdvertisingActiveFailStop(fault),
                            )),
                        }
                    } else {
                        let buffer = packet.take().expect(
                            "connectable advertising intake retains its sole scratch buffer",
                        );
                        match active.try_route_controller_command_with_buffer(controller, buffer) {
                            LegacyConnectableAdvertisingCommandIntake::Routed { route, buffer } => {
                                *packet = Some(buffer);
                                match route {
                                    LegacyConnectableAdvertisingCommandRoute::ResponsePending(pending) => self.store_retained_state(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandState::LegacyConnectableAdvertisingActiveResponse(pending),
                                    ),
                                    LegacyConnectableAdvertisingCommandRoute::Stopping(stopping) => self.store_retained_state(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandState::LegacyConnectableAdvertisingStopping(stopping),
                                    ),
                                    LegacyConnectableAdvertisingCommandRoute::EndpointMismatch(mismatch) => *output = Some(self.terminal_boundary(
                                        ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                        ControllerCommandBoundary::LegacyConnectableAdvertisingCommandEndpointMismatch(mismatch),
                                    )),
                                }
                            }
                            LegacyConnectableAdvertisingCommandIntake::Empty { active, buffer } => {
                                *packet = Some(buffer);
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(
                                        active,
                                    ),
                                );
                            }
                            LegacyConnectableAdvertisingCommandIntake::EndpointMismatch {
                                active,
                                buffer: _,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(
                                        active,
                                    ),
                                );
                                *output =
                                    Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ))
                            }
                            LegacyConnectableAdvertisingCommandIntake::Channel {
                                active,
                                buffer: _,
                                error,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(
                                        active,
                                    ),
                                );
                                *output =
                                    Some(self.retain_boundary(ControllerCommandBoundary::HciFault(
                                        error,
                                    )))
                            }
                            LegacyConnectableAdvertisingCommandIntake::NonCommand {
                                active,
                                frame,
                            } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::LegacyConnectableAdvertisingActive,
                                    ControllerCommandState::LegacyConnectableAdvertisingActive(
                                        active,
                                    ),
                                );
                                *output =
                                    Some(self.retain_boundary(
                                        ControllerCommandBoundary::NonCommand(frame),
                                    ))
                            }
                        }
                    }
                }
                _ => unreachable!("connectable dispatch requires its selected role phase"),
            }
        }
    }
}
