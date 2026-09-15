//! Endpoint-facing command, event, ACL and response coordination.

use super::*;

impl<'a, S: SchedulerRunInterruptStorage, const N: usize>
    PeripheralConnectionActiveSession<'a, S, N>
{
    /// Wait until the matching Host queue may contain another command.
    pub async fn wait_command_available<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        match &self.order {
            Order::CommandReady(ready) => controller.wait_command_available(ready).await,
            Order::ResponsePending(_) => {
                unreachable!("a response-pending connection cannot accept a command")
            }
        }
    }

    /// Consume and route at most one command while preserving both active axes.
    pub fn try_route_controller_command_with_buffer<
        'command,
        'buffer,
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<'command, M, H2C, C2H, PACKET>,
        buffer: &'buffer mut [u8],
    ) -> PeripheralConnectionCommandIntake<'a, 'command, 'buffer, S, N> {
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
        let Order::CommandReady(ready) = order else {
            unreachable!("a response-pending connection cannot route a command")
        };
        let live_handle = host_events.live_handle();
        let local_procedure_busy =
            control.local_procedure_pending() || encryption.blocks_unrelated_transmission();
        let local_version_available = control.remote_version_request_available();
        let long_term_key_request_pending = encryption.long_term_key_request().is_some();
        let ready = ready.map_owner(|radio| PeripheralConnectionState {
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
        });
        match controller.try_receive_active_peripheral_with_buffer(
            ready,
            live_handle,
            buffer,
            |mut state, packet| {
                state.acl.accept_host_packet(packet);
                let completed = state.acl.take_completed_host_packets();
                state.host_events.observe_acl_completed(completed);
                state
            },
        ) {
            HciIntake::Command { command, buffer } => {
                let route = match controller.route_active_peripheral_classified_command(
                    command,
                    live_handle,
                    local_procedure_busy,
                    local_version_available,
                    long_term_key_request_pending,
                ) {
                    HciCommandRoute::ResponsePending(pending) => {
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::Disconnect(disconnect) => {
                        let pending = disconnect.into_accepted_status().map_owner(|accepted| {
                            let (mut state, command) = accepted.into_parts();
                            state.disconnect = Some(command);
                            state
                        });
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::ReadRemoteFeatures(request) => {
                        let pending = request.into_accepted_status().map_owner(|mut state| {
                            use oer_bluetooth_ll::control::LeRemoteFeaturesAdmission;
                            assert!(matches!(
                                state.control.admit_remote_feature_request(),
                                LeRemoteFeaturesAdmission::Admitted
                                    | LeRemoteFeaturesAdmission::Cached(_)
                            ));
                            state.read_remote_features_after_status = true;
                            state
                        });
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::ReadRemoteVersionInformation(request) => {
                        let pending = request.into_accepted_status().map_owner(|mut state| {
                            use oer_bluetooth_ll::control::LeRemoteVersionAdmission;
                            assert!(matches!(
                                state.control.admit_remote_version_request(),
                                LeRemoteVersionAdmission::Admitted
                                    | LeRemoteVersionAdmission::Cached(_)
                            ));
                            state.read_remote_version_after_status = true;
                            state
                        });
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::LongTermKeyReply(reply) => {
                        let pending = reply.into_accepted_complete().map_owner(|accepted| {
                            let (mut state, long_term_key) = accepted.into_parts();
                            state
                                .encryption
                                .provide_long_term_key(
                                    oer_bluetooth_ll::security::LeLongTermKey::new(long_term_key),
                                )
                                .unwrap_or_else(|_| {
                                    unreachable!(
                                        "the HCI router admitted only a pending LTK request"
                                    )
                                });
                            state
                        });
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::LongTermKeyNegativeReply(reply) => {
                        let pending = reply.into_accepted_complete().map_owner(|mut state| {
                            state.encryption.reject_long_term_key().unwrap_or_else(|_| {
                                unreachable!("the HCI router admitted only a pending LTK request")
                            });
                            state
                        });
                        PeripheralConnectionCommandRoute::ResponsePending(Self::from_pending(
                            pending,
                        ))
                    }
                    HciCommandRoute::ResetBarrier(barrier) => {
                        PeripheralConnectionCommandRoute::ResetBarrier(
                            PeripheralConnectionResetBarrier { barrier },
                        )
                    }
                    HciCommandRoute::EndpointMismatch(command) => {
                        PeripheralConnectionCommandRoute::EndpointMismatch(
                            PeripheralConnectionCommandMismatch { _command: command },
                        )
                    }
                };
                PeripheralConnectionCommandIntake::Routed { route, buffer }
            }
            HciIntake::Acl { ready, buffer } => PeripheralConnectionCommandIntake::Acl {
                session: Self::from_ready(ready),
                buffer,
            },
            HciIntake::HostCompletedPackets {
                ready,
                command,
                buffer,
            } => {
                let (mut state, ready) = ready.into_parts();
                let result = command.and_then(|command| {
                    let credit_handle = state.acl.credit_handle(live_handle);
                    state
                        .acl
                        .accept_host_completed_packets(command, credit_handle)
                });
                match result {
                    Ok(()) => PeripheralConnectionCommandIntake::HostCompletedPackets {
                        session: state.into_session(Order::CommandReady(ready)),
                        buffer,
                    },
                    Err(response) => {
                        let pending = ready
                            .map_owner(|()| state)
                            .begin_host_completed_packets_error(response);
                        PeripheralConnectionCommandIntake::Routed {
                            route: PeripheralConnectionCommandRoute::ResponsePending(
                                Self::from_pending(pending),
                            ),
                            buffer,
                        }
                    }
                }
            }
            HciIntake::Empty { ready, buffer } => PeripheralConnectionCommandIntake::Empty {
                session: Self::from_ready(ready),
                buffer,
            },
            HciIntake::EndpointMismatch { ready, buffer } => {
                PeripheralConnectionCommandIntake::EndpointMismatch {
                    session: Self::from_ready(ready),
                    buffer,
                }
            }
            HciIntake::Channel {
                ready,
                buffer,
                error,
            } => PeripheralConnectionCommandIntake::Channel {
                session: Self::from_ready(ready),
                buffer,
                error,
            },
            HciIntake::NonCommand { ready, frame } => {
                PeripheralConnectionCommandIntake::NonCommand {
                    session: Self::from_ready(ready),
                    frame,
                }
            }
        }
    }

    fn from_ready(
        ready: oer_bluetooth_hci::LeControllerCommandReady<'a, PeripheralConnectionState<'a, S, N>>,
    ) -> Self {
        let (state, ready) = ready.into_parts();
        state.into_session(Order::CommandReady(ready))
    }

    fn from_pending(
        pending: oer_bluetooth_hci::LeControllerResponsePending<
            'a,
            PeripheralConnectionState<'a, S, N>,
        >,
    ) -> Self {
        let (state, pending) = pending.into_parts();
        state.into_session(Order::ResponsePending(pending))
    }

    /// Whether an established/failed connection or disconnection event awaits HCI publication.
    pub const fn has_pending_host_event(&self) -> bool {
        self.host_events.has_pending() || self.acl.has_controller_packet()
    }

    /// Whether command/data intake has room for another owned Host ACL packet.
    pub const fn can_accept_host_packet(&self) -> bool {
        self.acl.can_accept_host_packet()
    }

    /// Whether the next retained Controller ACL packet awaits a Host credit.
    pub fn host_event_is_flow_controlled<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> bool {
        if !self.host_events.connection_result_published() || !self.acl.has_controller_packet() {
            return false;
        }
        let profile = controller.controller_to_host_acl_profile();
        self.acl.controller_packet_is_flow_controlled(
            profile.is_flow_controlled(),
            profile.total_packets(),
        )
    }

    /// Wait for possible capacity without moving the retained connection event.
    pub async fn wait_host_event_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        if !self.order.accepts_endpoint(controller) {
            return Err(LeControllerEndpointMismatch);
        }
        controller.wait_peripheral_connection_event_capacity().await;
        Ok(())
    }

    /// Try the next ordered unsolicited event after older command completion.
    pub fn try_publish_host_event<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        mut self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> PeripheralConnectionHostEventPublication<Self> {
        use host_events::PeripheralConnectionHostEventPublication as Publication;
        if !self.order.accepts_endpoint(controller) {
            return PeripheralConnectionHostEventPublication::EndpointMismatch(self);
        }
        if !matches!(&self.order, Order::CommandReady(_)) {
            return PeripheralConnectionHostEventPublication::OrderedResponsePending(self);
        }
        observe_encryption_host_events(&mut self.encryption, &mut self.host_events);
        match self.host_events.try_publish(controller) {
            Publication::Published => {
                return PeripheralConnectionHostEventPublication::Published(self);
            }
            Publication::Masked => {
                if self.host_events.take_long_term_key_request_masked() {
                    self.encryption.reject_long_term_key().unwrap_or_else(|_| {
                        unreachable!("only the retained LTK request can be masked")
                    });
                }
                return PeripheralConnectionHostEventPublication::Masked(self);
            }
            Publication::Pending => {
                return PeripheralConnectionHostEventPublication::Pending(self);
            }
            Publication::Fault(error) => {
                return PeripheralConnectionHostEventPublication::Fault {
                    session: self,
                    error,
                };
            }
            Publication::None => {}
        }
        if self.host_events.connection_result_published() {
            let profile = controller.controller_to_host_acl_profile();
            match self.acl.try_publish_controller_packet(
                controller,
                profile.maximum_payload(),
                profile.is_flow_controlled(),
                profile.total_packets(),
            ) {
                acl::ControllerAclPublication::Published => {
                    return PeripheralConnectionHostEventPublication::Published(self);
                }
                acl::ControllerAclPublication::Pending => {
                    return PeripheralConnectionHostEventPublication::Pending(self);
                }
                acl::ControllerAclPublication::FlowControlled => {
                    return PeripheralConnectionHostEventPublication::FlowControlled(self);
                }
                acl::ControllerAclPublication::Fault(error) => {
                    return PeripheralConnectionHostEventPublication::Fault {
                        session: self,
                        error,
                    };
                }
                acl::ControllerAclPublication::None => {}
            }
        }
        PeripheralConnectionHostEventPublication::None(self)
    }

    /// Cancellation leaves the exact response and radio phase in this session.
    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<ResponseWait, LeControllerEndpointMismatch> {
        self.order.wait_response_capacity(controller).await
    }

    pub fn try_publish_response<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Publication<Self> {
        let Self {
            order,
            mut control,
            encryption,
            supervision,
            termination,
            mut procedure,
            progress_deadline,
            mut host_events,
            acl,
            disconnect,
            read_remote_features_after_status,
            read_remote_version_after_status,
        } = self;
        match order.try_publish_response(controller) {
            OrderPublication::CommandReady(order) => Publication::CommandReady(Self {
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
            }),
            OrderPublication::Published(order) => {
                if let Some(command) = disconnect {
                    control.request_host_termination(command.reason());
                }
                if read_remote_features_after_status {
                    if matches!(order.owner(), radio::Radio::Stopped { .. }) {
                        control.close_remote_feature_request();
                    } else if let Some(features) = control.activate_remote_feature_request() {
                        host_events.observe_remote_features(
                            oer_bluetooth_ll::control::LeRemoteFeaturesResult::Success(features),
                        );
                    }
                    observe_remote_feature_result(&mut control, &mut procedure, &mut host_events);
                }
                if read_remote_version_after_status {
                    if matches!(order.owner(), radio::Radio::Stopped { .. }) {
                        control.close_remote_version_request();
                    } else if let Some(version) = control.activate_remote_version_request() {
                        host_events.observe_remote_version(
                            oer_bluetooth_ll::control::LeRemoteVersionResult::Success(version),
                        );
                    }
                    observe_remote_version_result(&mut control, &mut procedure, &mut host_events);
                }
                Publication::Published(Self {
                    order,
                    control,
                    encryption,
                    supervision,
                    termination,
                    procedure,
                    progress_deadline,
                    host_events,
                    acl,
                    disconnect: None,
                    read_remote_features_after_status: false,
                    read_remote_version_after_status: false,
                })
            }
            OrderPublication::Pending(order) => Publication::Pending(Self {
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
            }),
            OrderPublication::EndpointMismatch(order) => Publication::EndpointMismatch(Self {
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
            }),
            OrderPublication::Fault { order, error } => Publication::Fault {
                state: Self {
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
                },
                error,
            },
        }
    }
}
