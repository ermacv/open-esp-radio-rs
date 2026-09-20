//! Finite active and Reset stepping of the sole retained session owner.

use super::*;

impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    PeripheralConnectionActiveSession<'a, S, N>
{
    /// Expiry consumes the same complete session as ordinary progress. Keep
    /// stop/fault construction separate so neither branch duplicates its large
    /// affine temporaries in the ordinary radio-step frame.
    #[inline(never)]
    fn expire_progress(
        self,
        deadline_phase: radio::DeadlinePhase,
    ) -> PeripheralConnectionActiveStep<'a, S, N> {
        let Self {
            order,
            control,
            encryption,
            supervision,
            termination,
            procedure,
            progress_deadline,
            host_events,
            acl,
            disconnect,
            read_remote_features_after_status,
            read_remote_version_after_status,
        } = self;
        let (radio, order) = order.into_parts();
        if matches!(deadline_phase, radio::DeadlinePhase::SchedulerCompletion) {
            match radio.begin_completion_abort() {
                Ok(radio) => {
                    return PeripheralConnectionActiveStep::Continue(Self {
                        order: order.map_owner(|()| radio),
                        control,
                        encryption,
                        supervision,
                        termination,
                        procedure,
                        progress_deadline:
                            super::super::progress::PeripheralConnectionProgressDeadline::for_stop(
                                S::monotonic_micros(),
                            ),
                        host_events,
                        acl,
                        disconnect,
                        read_remote_features_after_status,
                        read_remote_version_after_status,
                    });
                }
                Err(radio) => {
                    return PeripheralConnectionActiveStep::Fault(
                        PeripheralConnectionActiveFault {
                            radio,
                            _order: order,
                            _control: control,
                            _encryption: encryption,
                            _supervision: supervision,
                            _termination: termination,
                            _procedure: procedure,
                            _progress_deadline: progress_deadline,
                            _host_events: host_events,
                            _acl: acl,
                            _disconnect: disconnect,
                            _read_remote_features_after_status: read_remote_features_after_status,
                            _read_remote_version_after_status: read_remote_version_after_status,
                        },
                    );
                }
            }
        }
        let cause = match deadline_phase {
            radio::DeadlinePhase::MaintenanceRestoration => {
                PeripheralConnectionActiveFaultCause::MaintenanceRestorationExpired
            }
            radio::DeadlinePhase::SchedulerStop => {
                PeripheralConnectionActiveFaultCause::CompletionAbortDeadlineExpired
            }
            radio::DeadlinePhase::PostUnlink => {
                PeripheralConnectionActiveFaultCause::UnlinkDeadlineExpired
            }
            radio::DeadlinePhase::ControllerTime => {
                PeripheralConnectionActiveFaultCause::ControllerTimeDeadlineExpired
            }
            radio::DeadlinePhase::SchedulerCompletion => unreachable!(),
        };
        PeripheralConnectionActiveStep::Fault(PeripheralConnectionActiveFault {
            radio: radio.expire_deadline(cause),
            _order: order,
            _control: control,
            _encryption: encryption,
            _supervision: supervision,
            _termination: termination,
            _procedure: procedure,
            _progress_deadline: progress_deadline,
            _host_events: host_events,
            _acl: acl,
            _disconnect: disconnect,
            _read_remote_features_after_status: read_remote_features_after_status,
            _read_remote_version_after_status: read_remote_version_after_status,
        })
    }

    // Do not merge the affine radio transition's temporaries into the much
    // larger controller dispatch future's stack frame.
    #[inline(never)]
    pub fn step_radio(
        self,
        random: &mut impl super::super::PeripheralEncryptionRandomSource,
    ) -> PeripheralConnectionActiveStep<'a, S, N> {
        let Self {
            order,
            mut control,
            mut encryption,
            mut supervision,
            mut termination,
            mut procedure,
            mut progress_deadline,
            mut host_events,
            mut acl,
            disconnect,
            read_remote_features_after_status,
            read_remote_version_after_status,
        } = self;
        let (mut radio, order) = order.into_parts();
        let previous_phase = radio.deadline_phase();
        if let Some(deadline_phase) = radio.expired_phase(&mut progress_deadline) {
            return Self {
                order: order.map_owner(|()| radio),
                control,
                encryption,
                supervision,
                termination,
                procedure,
                progress_deadline,
                host_events,
                acl,
                disconnect,
                read_remote_features_after_status,
                read_remote_version_after_status,
            }
            .expire_progress(deadline_phase);
        }
        if matches!(&radio, radio::Radio::Stopped { .. }) {
            control.close_remote_feature_request();
            control.close_remote_version_request();
        }
        observe_remote_feature_result(&mut control, &mut procedure, &mut host_events);
        observe_remote_version_result(&mut control, &mut procedure, &mut host_events);
        observe_encryption_host_events(&mut encryption, &mut host_events);
        if encryption.termination_reason() == Some(0x06) {
            control.request_local_termination(0x06);
        }
        let (radio, order) = match (radio, order) {
            (radio::Radio::Stopped { task, reason }, Order::CommandReady(ready))
                if host_events.idle_retirement_ready(Axis::CommandReady, &acl) =>
            {
                return PeripheralConnectionActiveStep::Stopped {
                    task: crate::controller::ControllerIdleCommandTask::from_parts(task, ready),
                    reason,
                };
            }
            pair => pair,
        };
        let step = radio.step(
            &mut control,
            &mut encryption,
            &mut acl,
            radio::Deadlines {
                supervision: &mut supervision,
                termination: &mut termination,
                procedure: &mut procedure,
            },
            &mut host_events,
            random,
        );
        observe_remote_feature_result(&mut control, &mut procedure, &mut host_events);
        observe_remote_version_result(&mut control, &mut procedure, &mut host_events);
        observe_encryption_host_events(&mut encryption, &mut host_events);
        let (radio, published, maintenance) = match step {
            radio::Step::Continue(radio) => {
                radio.begin_unlink_budget(previous_phase, &mut progress_deadline);
                (radio, false, None)
            }
            radio::Step::Published(radio, maintenance) => {
                progress_deadline = radio.progress_deadline();
                (radio, true, maintenance)
            }
            radio::Step::Fault(radio) => {
                return PeripheralConnectionActiveStep::Fault(PeripheralConnectionActiveFault {
                    radio,
                    _order: order,
                    _control: control,
                    _encryption: encryption,
                    _supervision: supervision,
                    _termination: termination,
                    _procedure: procedure,
                    _progress_deadline: progress_deadline,
                    _host_events: host_events,
                    _acl: acl,
                    _disconnect: disconnect,
                    _read_remote_features_after_status: read_remote_features_after_status,
                    _read_remote_version_after_status: read_remote_version_after_status,
                });
            }
        };
        let session = Self {
            order: order.map_owner(|()| radio),
            control,
            encryption,
            supervision,
            termination,
            procedure,
            progress_deadline,
            host_events,
            acl,
            disconnect,
            read_remote_features_after_status,
            read_remote_version_after_status,
        };
        if published {
            PeripheralConnectionActiveStep::Published(session, maintenance)
        } else {
            PeripheralConnectionActiveStep::Continue(session)
        }
    }
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    PeripheralConnectionResetBarrier<'a, S, N>
{
    /// Borrow the current radio wait while retaining Reset ownership.
    pub fn radio_wait(&self) -> Option<PeripheralConnectionActiveWait<'_>> {
        self.barrier.owner().radio.reset_wait()
    }

    /// Retire the connection graph before exposing the idle Reset barrier.
    pub fn step(
        self,
        random: &mut impl super::super::PeripheralEncryptionRandomSource,
    ) -> PeripheralConnectionResetStep<'a, S, N> {
        let (mut state, barrier) = self.barrier.into_parts();
        let previous_phase = state.radio.deadline_phase();
        if let Some(deadline_phase) = state.radio.expired_phase(&mut state.progress_deadline) {
            let PeripheralConnectionState {
                radio,
                control,
                encryption,
                supervision,
                termination,
                procedure,
                progress_deadline,
                host_events,
                acl,
                disconnect,
                read_remote_features_after_status,
                read_remote_version_after_status,
            } = state;
            if matches!(deadline_phase, radio::DeadlinePhase::SchedulerCompletion) {
                match radio.begin_completion_abort() {
                    Ok(radio) => {
                        return PeripheralConnectionResetStep::Continue(Self {
                            barrier: barrier.map_owner(|()| PeripheralConnectionState {
                                radio,
                                control,
                                encryption,
                                supervision,
                                termination,
                                procedure,
                                progress_deadline:
                                    super::super::progress::PeripheralConnectionProgressDeadline::for_stop(
                                        S::monotonic_micros(),
                                    ),
                                host_events,
                                acl,
                                disconnect,
                                read_remote_features_after_status,
                                read_remote_version_after_status,
                            }),
                        });
                    }
                    Err(radio) => {
                        return PeripheralConnectionResetStep::Fault(
                            PeripheralConnectionResetFault {
                                radio,
                                _barrier: barrier,
                                _control: control,
                                _encryption: encryption,
                                _supervision: supervision,
                                _termination: termination,
                                _procedure: procedure,
                                _progress_deadline: progress_deadline,
                                _host_events: host_events,
                                _acl: acl,
                                _disconnect: disconnect,
                                _read_remote_features_after_status:
                                    read_remote_features_after_status,
                                _read_remote_version_after_status: read_remote_version_after_status,
                            },
                        );
                    }
                }
            }
            let cause = match deadline_phase {
                radio::DeadlinePhase::MaintenanceRestoration => {
                    PeripheralConnectionActiveFaultCause::MaintenanceRestorationExpired
                }
                radio::DeadlinePhase::SchedulerStop => {
                    PeripheralConnectionActiveFaultCause::CompletionAbortDeadlineExpired
                }
                radio::DeadlinePhase::PostUnlink => {
                    PeripheralConnectionActiveFaultCause::UnlinkDeadlineExpired
                }
                radio::DeadlinePhase::ControllerTime => {
                    PeripheralConnectionActiveFaultCause::ControllerTimeDeadlineExpired
                }
                radio::DeadlinePhase::SchedulerCompletion => unreachable!(),
            };
            return PeripheralConnectionResetStep::Fault(PeripheralConnectionResetFault {
                radio: radio.expire_deadline(cause),
                _barrier: barrier,
                _control: control,
                _encryption: encryption,
                _supervision: supervision,
                _termination: termination,
                _procedure: procedure,
                _progress_deadline: progress_deadline,
                _host_events: host_events,
                _acl: acl,
                _disconnect: disconnect,
                _read_remote_features_after_status: read_remote_features_after_status,
                _read_remote_version_after_status: read_remote_version_after_status,
            });
        }
        match state.radio.step_reset(
            &mut state.control,
            &mut state.encryption,
            &mut state.acl,
            radio::Deadlines {
                supervision: &mut state.supervision,
                termination: &mut state.termination,
                procedure: &mut state.procedure,
            },
            &mut state.host_events,
            random,
        ) {
            radio::ResetStep::Published(radio) => {
                state.progress_deadline = radio.progress_deadline();
                state.radio = radio;
                PeripheralConnectionResetStep::Continue(Self {
                    barrier: barrier.map_owner(|()| state),
                })
            }
            radio::ResetStep::Continue(radio) => {
                radio.begin_unlink_budget(previous_phase, &mut state.progress_deadline);
                state.radio = radio;
                PeripheralConnectionResetStep::Continue(Self {
                    barrier: barrier.map_owner(|()| state),
                })
            }
            radio::ResetStep::Quiesced(task) => PeripheralConnectionResetStep::Ready(
                crate::controller::ControllerIdleResetBarrier::new(barrier.map_owner(|()| task)),
            ),
            radio::ResetStep::Fault(radio) => {
                PeripheralConnectionResetStep::Fault(PeripheralConnectionResetFault {
                    radio,
                    _barrier: barrier,
                    _control: state.control,
                    _encryption: state.encryption,
                    _supervision: state.supervision,
                    _termination: state.termination,
                    _procedure: state.procedure,
                    _progress_deadline: state.progress_deadline,
                    _host_events: state.host_events,
                    _acl: state.acl,
                    _disconnect: state.disconnect,
                    _read_remote_features_after_status: state.read_remote_features_after_status,
                    _read_remote_version_after_status: state.read_remote_version_after_status,
                })
            }
        }
    }
}
