//! Peripheral first-event, ACL and Reset dispatch for the sole command actor.
//!
//! A step borrows the actor, its HCI endpoint and its reusable packet buffer.
//! Every await leaves the lower transaction in `ControllerOwnerSlot`; taking
//! that owner and storing its successor are synchronous. An empty output returns to
//! the outer dispatcher, which rechecks the controller-time budget before
//! selecting another step. This module owns no independent task or session.

use super::*;

impl<'runtime, S, const CAPACITY: usize> ControllerCommandTask<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    #[allow(
        clippy::manual_async_fn,
        reason = "inline(always) applies to the async poll body to bound affine transition stack frames"
    )]
    pub(super) fn step_peripheral<
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
                ControllerCommandPhase::PeripheralConnectionFirst => {
                    if matches!(
                        self.owner.current(),
                        ControllerCommandState::PeripheralConnectionFirstRetry(_)
                    ) {
                        let ControllerCommandState::PeripheralConnectionFirstRetry(retry) =
                            self.owner.take()
                        else {
                            unreachable!("the selected peripheral retry did not change")
                        };
                        let retry = match retry.try_publish_response(controller) {
                        LegacyConnectablePeripheralFirstResponsePublication::CommandReady(
                            retry,
                        )
                        | LegacyConnectablePeripheralFirstResponsePublication::Published(retry)
                        | LegacyConnectablePeripheralFirstResponsePublication::Pending(retry) => {
                            retry
                        }
                        LegacyConnectablePeripheralFirstResponsePublication::EndpointMismatch(
                            retry,
                        ) => {
                            self.store_retained_state(
                                ControllerCommandPhase::PeripheralConnectionFirst,
                                ControllerCommandState::PeripheralConnectionFirstRetry(retry),
                            );
                            return *output = Some(
                                self.retain_boundary(ControllerCommandBoundary::EndpointMismatch),
                            );
                        }
                        LegacyConnectablePeripheralFirstResponsePublication::Fault {
                            state: retry,
                            error,
                        } => {
                            self.store_retained_state(
                                ControllerCommandPhase::PeripheralConnectionFirst,
                                ControllerCommandState::PeripheralConnectionFirstRetry(retry),
                            );
                            return *output = Some(
                                self.retain_boundary(ControllerCommandBoundary::HciFault(error)),
                            );
                        }
                    };
                        if let Some(observation) = self.store_peripheral_connection_first_drive(
                            ControllerCommandPhase::PeripheralConnectionFirst,
                            retry.retry(),
                        ) {
                            return *output = Some(observation);
                        }
                        return;
                    }

                    let ControllerCommandState::PeripheralConnectionFirst(wait) =
                        self.owner.current()
                    else {
                        unreachable!("the selected peripheral first wait did not change")
                    };
                    let controller_time_ready = match wait.hci_axis() {
                        LegacyConnectablePeripheralFirstHciAxis::CommandReady => Some(
                            wait.wait_controller_time(recheck.wait_until_absolute_recheck())
                                .await,
                        ),
                        LegacyConnectablePeripheralFirstHciAxis::ResponsePending => {
                            match select(
                                wait.wait_controller_time(recheck.wait_until_absolute_recheck()),
                                wait.wait_response_capacity(controller),
                            )
                            .await
                            {
                                Either::First(ready) => Some(ready),
                                Either::Second(Ok(_)) => None,
                                Either::Second(Err(_)) => {
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ));
                                }
                            }
                        }
                    };
                    let ControllerCommandState::PeripheralConnectionFirst(wait) = self.owner.take()
                    else {
                        unreachable!("the awaited peripheral first wait did not change")
                    };
                    if let Some(ready) = controller_time_ready {
                        if let Some(observation) = self.store_peripheral_connection_first_drive(
                            ControllerCommandPhase::PeripheralConnectionFirst,
                            wait.resume_controller_time(ready),
                        ) {
                            *output = Some(observation)
                        }
                    } else {
                        match wait.try_publish_response(controller) {
                        LegacyConnectablePeripheralFirstResponsePublication::CommandReady(wait)
                        | LegacyConnectablePeripheralFirstResponsePublication::Published(wait)
                        | LegacyConnectablePeripheralFirstResponsePublication::Pending(wait) => {
                            self.store_retained_state(
                                ControllerCommandPhase::PeripheralConnectionFirst,
                                ControllerCommandState::PeripheralConnectionFirst(wait),
                            );
                        }
                        LegacyConnectablePeripheralFirstResponsePublication::EndpointMismatch(
                            wait,
                        ) => {
                            self.store_retained_state(
                                ControllerCommandPhase::PeripheralConnectionFirst,
                                ControllerCommandState::PeripheralConnectionFirst(wait),
                            );
                            *output = Some(
                                self.retain_boundary(ControllerCommandBoundary::EndpointMismatch),
                            )
                        }
                        LegacyConnectablePeripheralFirstResponsePublication::Fault {
                            state: wait,
                            error,
                        } => {
                            self.store_retained_state(
                                ControllerCommandPhase::PeripheralConnectionFirst,
                                ControllerCommandState::PeripheralConnectionFirst(wait),
                            );
                            *output = Some(
                                self.retain_boundary(ControllerCommandBoundary::HciFault(error)),
                            )
                        }
                    }
                    }
                }
                ControllerCommandPhase::PeripheralConnectionActive => {
                    if let ControllerCommandState::PeripheralConnectionResetStopping(stopping) =
                        self.owner.current()
                    {
                        match stopping.radio_wait() {
                            Some(PeripheralConnectionActiveWait::Scheduler(wake)) => {
                                let _ = select(
                                    wakers.wait_scheduler_ready(wake),
                                    recheck.wait_until_absolute_recheck(),
                                )
                                .await;
                            }
                            Some(PeripheralConnectionActiveWait::PostUnlink(wake)) => {
                                wakers
                                    .wait_post_unlink_or_recheck(
                                        wake,
                                        recheck.wait_until_absolute_recheck(),
                                    )
                                    .await;
                            }
                            Some(PeripheralConnectionActiveWait::ControllerTime) => {
                                recheck.wait_until_absolute_recheck().await
                            }
                            Some(
                                PeripheralConnectionActiveWait::HostEventCapacityOrControllerTime,
                            ) => recheck.wait_until_absolute_recheck().await,
                            None => {}
                        }
                        let ControllerCommandState::PeripheralConnectionResetStopping(stopping) =
                            self.owner.take()
                        else {
                            unreachable!("the awaited peripheral Reset owner did not change")
                        };
                        match stopping.step(advertising_delay) {
                            PeripheralConnectionResetStep::Continue(stopping) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::PeripheralConnectionActive,
                                    ControllerCommandState::PeripheralConnectionResetStopping(
                                        stopping,
                                    ),
                                );
                            }
                            PeripheralConnectionResetStep::Ready(reset) => {
                                self.store_transition(
                                    ControllerCommandPhase::PeripheralConnectionActive,
                                    ControllerCommandStimulus::IdleReset,
                                    ControllerCommandState::IdleReset(reset),
                                );
                            }
                            PeripheralConnectionResetStep::Fault(fault) => {
                                return *output = Some(self.terminal_boundary(
                                ControllerCommandPhase::PeripheralConnectionActive,
                                ControllerCommandBoundary::PeripheralConnectionActiveResetFailStop(
                                    fault,
                                ),
                            ));
                            }
                        }
                        return;
                    }
                    let should_try_host_event = matches!(
                        self.owner.current(),
                        ControllerCommandState::PeripheralConnectionActive(running)
                            if running.hci_axis()
                                == LegacyConnectablePeripheralFirstHciAxis::CommandReady
                                && running.has_pending_host_event()
                                && !running.host_event_is_flow_controlled(controller)
                    );
                    if should_try_host_event {
                        let ControllerCommandState::PeripheralConnectionActive(running) =
                            self.owner.take()
                        else {
                            unreachable!("the selected peripheral active owner did not change")
                        };
                        match running.try_publish_host_event(controller) {
                            PeripheralConnectionHostEventPublication::None(running)
                            | PeripheralConnectionHostEventPublication::Published(running)
                            | PeripheralConnectionHostEventPublication::Masked(running) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::PeripheralConnectionActive,
                                    ControllerCommandState::PeripheralConnectionActive(running),
                                );
                                return;
                            }
                            PeripheralConnectionHostEventPublication::OrderedResponsePending(
                                running,
                            )
                            | PeripheralConnectionHostEventPublication::Pending(running)
                            | PeripheralConnectionHostEventPublication::FlowControlled(running) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::PeripheralConnectionActive,
                                    ControllerCommandState::PeripheralConnectionActive(running),
                                );
                            }
                            PeripheralConnectionHostEventPublication::EndpointMismatch(running) => {
                                self.store_retained_state(
                                    ControllerCommandPhase::PeripheralConnectionActive,
                                    ControllerCommandState::PeripheralConnectionActive(running),
                                );
                                return *output =
                                    Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ));
                            }
                            PeripheralConnectionHostEventPublication::Fault { session, error } => {
                                self.store_retained_state(
                                    ControllerCommandPhase::PeripheralConnectionActive,
                                    ControllerCommandState::PeripheralConnectionActive(session),
                                );
                                return *output =
                                    Some(self.retain_boundary(
                                        ControllerCommandBoundary::HciFault(error),
                                    ));
                            }
                        }
                    }
                    let ControllerCommandState::PeripheralConnectionActive(running) =
                        self.owner.current()
                    else {
                        unreachable!("the selected peripheral active owner did not change")
                    };
                    let radio_wait = running.radio_wait();
                    let response_pending = running.hci_axis()
                        == LegacyConnectablePeripheralFirstHciAxis::ResponsePending;
                    let host_event_pending = running.has_pending_host_event();
                    let host_event_flow_controlled =
                        running.host_event_is_flow_controlled(controller);
                    let work_plan =
                        crate::session::peripheral::active::plan_peripheral_connection_work(
                            response_pending,
                            host_event_pending,
                            host_event_flow_controlled,
                        );
                    let radio = work_plan.poll_radio().then_some(async {
                        match radio_wait {
                            None => {}
                            Some(PeripheralConnectionActiveWait::Scheduler(wake)) => {
                                let _ = select(
                                    wakers.wait_scheduler_ready(wake),
                                    recheck.wait_until_absolute_recheck(),
                                )
                                .await;
                            }
                            Some(PeripheralConnectionActiveWait::PostUnlink(wake)) => {
                                wakers
                                    .wait_post_unlink_or_recheck(
                                        wake,
                                        recheck.wait_until_absolute_recheck(),
                                    )
                                    .await;
                            }
                            Some(PeripheralConnectionActiveWait::ControllerTime) => {
                                recheck.wait_until_absolute_recheck().await
                            }
                            Some(
                                PeripheralConnectionActiveWait::HostEventCapacityOrControllerTime,
                            ) => {
                                let _ = crate::session::peripheral::active::wait_peripheral_connection_backpressure(
                                    running.wait_host_event_capacity(controller),
                                    recheck.wait_until_absolute_recheck(),
                                )
                                .await;
                            }
                        }
                    });
                    let hci_work = work_plan.hci();
                    let response = async {
                        match hci_work {
                            crate::session::peripheral::active::PeripheralConnectionHciWork::OrderedResponse => {
                                Either::First(running.wait_response_capacity(controller).await)
                            }
                            crate::session::peripheral::active::PeripheralConnectionHciWork::HostEvent => Either::Second(
                                running.wait_host_event_capacity(controller).await,
                            ),
                            crate::session::peripheral::active::PeripheralConnectionHciWork::Command => {
                                Either::Second(running.wait_command_available(controller).await)
                            }
                        }
                    };
                    match crate::session::peripheral::active::wait_peripheral_connection_work(
                        radio, response,
                    )
                    .await
                    {
                        crate::session::peripheral::active::PeripheralConnectionWork::Radio => {
                            let ControllerCommandState::PeripheralConnectionActive(running) =
                                self.owner.take()
                            else {
                                unreachable!("the awaited peripheral radio owner did not change")
                            };
                            match running.step_radio(advertising_delay) {
                                PeripheralConnectionActiveStep::Stopped { task, reason } => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::Idle,
                                        ControllerCommandState::Idle(task),
                                    );
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::IdleRestored(
                                            ControllerIdleCompletion::PeripheralDisconnected {
                                                reason,
                                            },
                                        ),
                                    ));
                                }
                                PeripheralConnectionActiveStep::Continue(running) => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandState::PeripheralConnectionActive(running),
                                    );
                                }
                                PeripheralConnectionActiveStep::Published(running) => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandState::PeripheralConnectionActive(running),
                                    );
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::PeripheralConnectionActive,
                                    ));
                                }
                                PeripheralConnectionActiveStep::Fault(fault) => {
                                    return *output = Some(self.terminal_boundary(ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandBoundary::PeripheralConnectionActiveFailStop(fault)));
                                }
                            }
                            return;
                        }
                        crate::session::peripheral::active::PeripheralConnectionWork::Hci(Either::First(Err(_))) => {
                            return *output = Some(self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch));
                        }
                        crate::session::peripheral::active::PeripheralConnectionWork::Hci(Either::Second(Err(_))) => {
                            return *output = Some(self
                                .retain_boundary(ControllerCommandBoundary::EndpointMismatch));
                        }
                        crate::session::peripheral::active::PeripheralConnectionWork::Hci(Either::Second(Ok(())))
                            if hci_work == crate::session::peripheral::active::PeripheralConnectionHciWork::HostEvent =>
                        {
                            let ControllerCommandState::PeripheralConnectionActive(running) =
                                self.owner.take()
                            else {
                                unreachable!("the awaited peripheral active owner did not change")
                            };
                            match running.try_publish_host_event(controller) {
                                PeripheralConnectionHostEventPublication::None(running)
                                | PeripheralConnectionHostEventPublication::Published(running)
                                | PeripheralConnectionHostEventPublication::Masked(running)
                                | PeripheralConnectionHostEventPublication::OrderedResponsePending(
                                    running,
                                )
                                | PeripheralConnectionHostEventPublication::Pending(running)
                                | PeripheralConnectionHostEventPublication::FlowControlled(
                                    running,
                                ) => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandState::PeripheralConnectionActive(running),
                                    );
                                }
                                PeripheralConnectionHostEventPublication::EndpointMismatch(
                                    running,
                                ) => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandState::PeripheralConnectionActive(running),
                                    );
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ));
                                }
                                PeripheralConnectionHostEventPublication::Fault {
                                    session,
                                    error,
                                } => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandState::PeripheralConnectionActive(session),
                                    );
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::HciFault(error),
                                    ));
                                }
                            }
                            return;
                        }
                        crate::session::peripheral::active::PeripheralConnectionWork::Hci(Either::Second(Ok(()))) => {
                            let ControllerCommandState::PeripheralConnectionActive(running) =
                                self.owner.take()
                            else {
                                unreachable!("the awaited peripheral command owner did not change")
                            };
                            let buffer = packet.take().expect(
                                "peripheral command intake retains its sole scratch buffer",
                            );
                            match running
                                .try_route_controller_command_with_buffer(controller, buffer)
                            {
                                PeripheralConnectionCommandIntake::Routed { route, buffer } => {
                                    *packet = Some(buffer);
                                    match route {
                                        PeripheralConnectionCommandRoute::ResponsePending(
                                            running,
                                        ) => self.store_retained_state(
                                            ControllerCommandPhase::PeripheralConnectionActive,
                                            ControllerCommandState::PeripheralConnectionActive(
                                                running,
                                            ),
                                        ),
                                        PeripheralConnectionCommandRoute::ResetBarrier(
                                            stopping,
                                        ) => self.store_retained_state(
                                            ControllerCommandPhase::PeripheralConnectionActive,
                                            ControllerCommandState::PeripheralConnectionResetStopping(
                                                stopping,
                                            ),
                                        ),
                                        PeripheralConnectionCommandRoute::EndpointMismatch(
                                            mismatch,
                                        ) => {
                                            return *output = Some(self.terminal_boundary(
                                                ControllerCommandPhase::PeripheralConnectionActive,
                                                ControllerCommandBoundary::PeripheralConnectionCommandEndpointMismatch(
                                                    mismatch,
                                                ),
                                            ));
                                        }
                                    }
                                }
                                PeripheralConnectionCommandIntake::Empty { session, buffer } => {
                                    *packet = Some(buffer);
                                    self.store_retained_state(
                                        ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandState::PeripheralConnectionActive(session),
                                    );
                                }
                                PeripheralConnectionCommandIntake::EndpointMismatch {
                                    session,
                                    buffer: _,
                                } => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandState::PeripheralConnectionActive(session),
                                    );
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::EndpointMismatch,
                                    ));
                                }
                                PeripheralConnectionCommandIntake::Channel {
                                    session,
                                    buffer: _,
                                    error,
                                } => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandState::PeripheralConnectionActive(session),
                                    );
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::HciFault(error),
                                    ));
                                }
                                PeripheralConnectionCommandIntake::Acl { session, buffer }
                                | PeripheralConnectionCommandIntake::HostCompletedPackets {
                                    session,
                                    buffer,
                                } => {
                                    *packet = Some(buffer);
                                    self.store_retained_state(
                                        ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandState::PeripheralConnectionActive(session),
                                    );
                                }
                                PeripheralConnectionCommandIntake::NonCommand {
                                    session,
                                    frame,
                                } => {
                                    self.store_retained_state(
                                        ControllerCommandPhase::PeripheralConnectionActive,
                                        ControllerCommandState::PeripheralConnectionActive(session),
                                    );
                                    return *output = Some(self.retain_boundary(
                                        ControllerCommandBoundary::NonCommand(frame),
                                    ));
                                }
                            }
                            return;
                        }
                        crate::session::peripheral::active::PeripheralConnectionWork::Hci(Either::First(Ok(_))) => {}
                    }
                    let ControllerCommandState::PeripheralConnectionActive(running) =
                        self.owner.take()
                    else {
                        unreachable!("the awaited peripheral active owner did not change")
                    };
                    match running.try_publish_response(controller) {
                    LegacyConnectablePeripheralFirstHciResponsePublication::CommandReady(
                        running,
                    )
                    | LegacyConnectablePeripheralFirstHciResponsePublication::Published(running)
                    | LegacyConnectablePeripheralFirstHciResponsePublication::Pending(running) => {
                        self.store_retained_state(
                            ControllerCommandPhase::PeripheralConnectionActive,
                            ControllerCommandState::PeripheralConnectionActive(running),
                        );
                    }
                    LegacyConnectablePeripheralFirstHciResponsePublication::EndpointMismatch(
                        running,
                    ) => {
                        self.store_retained_state(
                            ControllerCommandPhase::PeripheralConnectionActive,
                            ControllerCommandState::PeripheralConnectionActive(running),
                        );
                        *output = Some(
                            self.retain_boundary(ControllerCommandBoundary::EndpointMismatch),
                        )
                    }
                    LegacyConnectablePeripheralFirstHciResponsePublication::Fault {
                        state: running,
                        error,
                    } => {
                        self.store_retained_state(
                            ControllerCommandPhase::PeripheralConnectionActive,
                            ControllerCommandState::PeripheralConnectionActive(running),
                        );
                        *output =
                            Some(self.retain_boundary(ControllerCommandBoundary::HciFault(error)))
                    }
                }
                }
                _ => unreachable!("peripheral dispatch requires its selected role phase"),
            }
        }
    }
}
