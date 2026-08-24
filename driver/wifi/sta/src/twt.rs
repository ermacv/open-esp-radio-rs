//! Portable individual TWT requester policy and fixed-capacity runtime.
//!
//! This owner performs no I/O and reads no clock. A caller supplies monotonic
//! runtime deadlines for negotiation and the associated station TSF for wake
//! planning. Chip code must separately prove that it can install the accepted
//! agreement before any TWT Setup frame is published.

use open_esp_radio_ieee80211::twt::{
    INDIVIDUAL_TWT_FLOW_CAPACITY, INDIVIDUAL_TWT_SETUP_BODY_LEN, INDIVIDUAL_TWT_TEARDOWN_BODY_LEN,
    IndividualTwtAction, IndividualTwtControl, IndividualTwtFlowId, IndividualTwtParameterSet,
    IndividualTwtSetup, IndividualTwtSetupCommand, IndividualTwtTeardown, TwtWakeDurationUnit,
    TwtWireError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtRequesterConfigError {
    ZeroResponseTimeout,
    ZeroRetryInterval,
    ZeroSetupAttemptLimit,
    ZeroTeardownAttemptLimit,
}

/// Association-scoped retry and deadline policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtRequesterConfig {
    response_timeout_micros: u32,
    retry_interval_micros: u32,
    setup_attempt_limit: u8,
    teardown_attempt_limit: u8,
}

impl IndividualTwtRequesterConfig {
    pub const fn new(
        response_timeout_micros: u32,
        retry_interval_micros: u32,
        setup_attempt_limit: u8,
        teardown_attempt_limit: u8,
    ) -> Result<Self, IndividualTwtRequesterConfigError> {
        if response_timeout_micros == 0 {
            return Err(IndividualTwtRequesterConfigError::ZeroResponseTimeout);
        }
        if retry_interval_micros == 0 {
            return Err(IndividualTwtRequesterConfigError::ZeroRetryInterval);
        }
        if setup_attempt_limit == 0 {
            return Err(IndividualTwtRequesterConfigError::ZeroSetupAttemptLimit);
        }
        if teardown_attempt_limit == 0 {
            return Err(IndividualTwtRequesterConfigError::ZeroTeardownAttemptLimit);
        }
        Ok(Self {
            response_timeout_micros,
            retry_interval_micros,
            setup_attempt_limit,
            teardown_attempt_limit,
        })
    }

    pub const fn response_timeout_micros(self) -> u32 {
        self.response_timeout_micros
    }

    pub const fn retry_interval_micros(self) -> u32 {
        self.retry_interval_micros
    }

    pub const fn setup_attempt_limit(self) -> u8 {
        self.setup_attempt_limit
    }

    pub const fn teardown_attempt_limit(self) -> u8 {
        self.teardown_attempt_limit
    }
}

/// Request parameters supplied before a Dialog Token is allocated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtProposal {
    pub control: IndividualTwtControl,
    pub parameters: IndividualTwtParameterSet,
}

impl IndividualTwtProposal {
    pub fn validate(self) -> Result<Self, IndividualTwtRequesterError> {
        if !self.parameters.requesting_sta {
            return Err(TwtWireError::RequestCommandFromResponder.into());
        }
        if !self.parameters.setup_command.is_requester_command() {
            return Err(TwtWireError::ResponseCommandFromRequester.into());
        }
        self.parameters.validate(self.control)?;
        if !self.parameters.implicit {
            return Err(
                IndividualTwtRequesterError::ExplicitTwtInformationUnsupported(
                    IndividualTwtInformationFrontier::from_fields(self.control, self.parameters),
                ),
            );
        }
        Ok(self)
    }
}

/// Exact protocol state missing before an explicit agreement can be live.
///
/// Explicit agreements need subsequent TWT Information actions to move or
/// suspend their next service edge. The current codec/runtime owns neither
/// that action body nor its schedule-update semantics. Retaining the accepted
/// fields makes the missing frontier observable without treating the initial
/// target as an implicit periodic schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtInformationFrontier {
    pub flow_id: IndividualTwtFlowId,
    pub initial_target_wake_time_tsf: u64,
    pub information_frames_disabled: bool,
}

impl IndividualTwtInformationFrontier {
    const fn from_fields(
        control: IndividualTwtControl,
        parameters: IndividualTwtParameterSet,
    ) -> Self {
        Self {
            flow_id: parameters.flow_id,
            initial_target_wake_time_tsf: parameters.target_wake_time_tsf,
            information_frames_disabled: control.information_frames_disabled,
        }
    }
}

/// Accepted, validated individual TWT agreement in the station TSF domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtAgreement {
    pub flow_id: IndividualTwtFlowId,
    pub control: IndividualTwtControl,
    pub trigger: bool,
    pub implicit: bool,
    pub flow_type: open_esp_radio_ieee80211::twt::IndividualTwtFlowType,
    pub protection: bool,
    pub target_wake_time_tsf: u64,
    pub wake_interval_micros: u64,
    pub wake_duration_micros: u32,
}

impl IndividualTwtAgreement {
    fn from_response(response: IndividualTwtSetup) -> Result<Self, TwtWireError> {
        let parameters = response.parameters.validate(response.control)?;
        Ok(Self {
            flow_id: parameters.flow_id,
            control: response.control,
            trigger: parameters.trigger,
            implicit: parameters.implicit,
            flow_type: parameters.flow_type,
            protection: parameters.protection,
            target_wake_time_tsf: parameters.target_wake_time_tsf,
            wake_interval_micros: parameters.wake_interval_micros()?,
            wake_duration_micros: parameters.wake_duration_micros(response.control)?,
        })
    }

    /// Return the explicit-information frontier for a non-periodic agreement.
    pub const fn information_frontier(self) -> Option<IndividualTwtInformationFrontier> {
        if self.implicit {
            None
        } else {
            Some(IndividualTwtInformationFrontier {
                flow_id: self.flow_id,
                initial_target_wake_time_tsf: self.target_wake_time_tsf,
                information_frames_disabled: self.control.information_frames_disabled,
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtFlowStatus {
    Idle,
    SetupQueued,
    SetupTransmitting,
    AwaitingResponse,
    AwaitingHardwareInstall,
    Active,
    TeardownQueued,
    TeardownTransmitting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtTxKind {
    Setup,
    Teardown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtTxBody {
    Setup([u8; INDIVIDUAL_TWT_SETUP_BODY_LEN]),
    Teardown([u8; INDIVIDUAL_TWT_TEARDOWN_BODY_LEN]),
}

impl IndividualTwtTxBody {
    pub const fn as_slice(&self) -> &[u8] {
        match self {
            Self::Setup(body) => body,
            Self::Teardown(body) => body,
        }
    }
}

/// Affine identity for one action handed to a shared TX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtTransmission {
    pub flow_id: IndividualTwtFlowId,
    pub generation: u32,
    pub kind: IndividualTwtTxKind,
    pub body: IndividualTwtTxBody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtRequesterEvent {
    SetupPublished {
        flow_id: IndividualTwtFlowId,
        response_deadline_micros: u64,
    },
    SetupRetryScheduled {
        flow_id: IndividualTwtFlowId,
        retry_at_micros: u64,
    },
    SetupTimedOut {
        flow_id: IndividualTwtFlowId,
    },
    SetupTxFailed {
        flow_id: IndividualTwtFlowId,
    },
    TeardownComplete {
        flow_id: IndividualTwtFlowId,
    },
    TeardownRetryScheduled {
        flow_id: IndividualTwtFlowId,
        retry_at_micros: u64,
    },
    TeardownTxFailed {
        flow_id: IndividualTwtFlowId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtService {
    Idle,
    Transmit(IndividualTwtTransmission),
    Event(IndividualTwtRequesterEvent),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtSetupDisposition {
    Stale,
    Rejected {
        flow_id: IndividualTwtFlowId,
    },
    Alternative {
        flow_id: IndividualTwtFlowId,
        command: IndividualTwtSetupCommand,
        proposed: IndividualTwtAgreement,
    },
    InstallRequired {
        flow_id: IndividualTwtFlowId,
        generation: u32,
        agreement: IndividualTwtAgreement,
    },
    /// The AP accepted an explicit agreement that this requester cannot keep
    /// synchronized. A teardown is already queued; no hardware install or
    /// wake-plan publication is permitted.
    ExplicitInformationUnsupported {
        flow_id: IndividualTwtFlowId,
        frontier: IndividualTwtInformationFrontier,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtRequesterError {
    Wire(TwtWireError),
    /// Explicit agreements require TWT Information updates before another
    /// wake can be derived; this requester currently supports periodic
    /// implicit agreements only.
    ExplicitTwtInformationUnsupported(IndividualTwtInformationFrontier),
    /// Every nonzero generation has been issued. Reuse would let a stale
    /// completion alias a new TX/install obligation, so this requester stays
    /// fail-closed until it is replaced by a new owner with a wider epoch.
    GenerationExhausted,
    DeadlineOverflow,
    FlowBusy(IndividualTwtFlowId),
    NoAgreement(IndividualTwtFlowId),
    StaleTransmission,
    UnexpectedInstall,
    UnexpectedRemove,
    DemandResponseMismatch,
}

impl From<TwtWireError> for IndividualTwtRequesterError {
    fn from(error: TwtWireError) -> Self {
        Self::Wire(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlowPhase {
    Idle,
    SetupQueued {
        proposal: IndividualTwtProposal,
        attempts_remaining: u8,
        ready_at_micros: u64,
    },
    SetupTransmitting {
        proposal: IndividualTwtProposal,
        dialog_token: u8,
        generation: u32,
        attempts_remaining: u8,
    },
    AwaitingResponse {
        proposal: IndividualTwtProposal,
        dialog_token: u8,
        generation: u32,
        attempts_remaining: u8,
        deadline_micros: u64,
    },
    AwaitingHardwareInstall {
        generation: u32,
        agreement: IndividualTwtAgreement,
    },
    Active(IndividualTwtAgreement),
    TeardownQueued {
        attempts_remaining: u8,
        ready_at_micros: u64,
    },
    TeardownTransmitting {
        generation: u32,
        attempts_remaining: u8,
    },
}

impl FlowPhase {
    const IDLE: Self = Self::Idle;

    const fn status(self) -> IndividualTwtFlowStatus {
        match self {
            Self::Idle => IndividualTwtFlowStatus::Idle,
            Self::SetupQueued { .. } => IndividualTwtFlowStatus::SetupQueued,
            Self::SetupTransmitting { .. } => IndividualTwtFlowStatus::SetupTransmitting,
            Self::AwaitingResponse { .. } => IndividualTwtFlowStatus::AwaitingResponse,
            Self::AwaitingHardwareInstall { .. } => {
                IndividualTwtFlowStatus::AwaitingHardwareInstall
            }
            Self::Active(_) => IndividualTwtFlowStatus::Active,
            Self::TeardownQueued { .. } => IndividualTwtFlowStatus::TeardownQueued,
            Self::TeardownTransmitting { .. } => IndividualTwtFlowStatus::TeardownTransmitting,
        }
    }
}

/// Exactly eight flow slots, matching the three-bit wire identifier.
pub struct IndividualTwtRequester {
    config: IndividualTwtRequesterConfig,
    flows: [FlowPhase; INDIVIDUAL_TWT_FLOW_CAPACITY],
    next_dialog_token: u8,
    generation: u32,
}

impl IndividualTwtRequester {
    pub const fn new(config: IndividualTwtRequesterConfig) -> Self {
        Self {
            config,
            flows: [FlowPhase::IDLE; INDIVIDUAL_TWT_FLOW_CAPACITY],
            next_dialog_token: 1,
            generation: 0,
        }
    }

    pub const fn config(&self) -> IndividualTwtRequesterConfig {
        self.config
    }

    pub const fn status(&self, flow_id: IndividualTwtFlowId) -> IndividualTwtFlowStatus {
        self.flows[flow_id.index()].status()
    }

    pub const fn agreement(&self, flow_id: IndividualTwtFlowId) -> Option<IndividualTwtAgreement> {
        match self.flows[flow_id.index()] {
            FlowPhase::Active(agreement) => Some(agreement),
            _ => None,
        }
    }

    /// Proposal currently crossing the chip-admission/TX boundary.
    pub const fn transmitting_proposal(
        &self,
        flow_id: IndividualTwtFlowId,
    ) -> Option<IndividualTwtProposal> {
        match self.flows[flow_id.index()] {
            FlowPhase::SetupTransmitting { proposal, .. } => Some(proposal),
            _ => None,
        }
    }

    pub fn queue_setup(
        &mut self,
        proposal: IndividualTwtProposal,
        now_micros: u64,
    ) -> Result<(), IndividualTwtRequesterError> {
        let proposal = proposal.validate()?;
        let flow_id = proposal.parameters.flow_id;
        if !matches!(self.flows[flow_id.index()], FlowPhase::Idle) {
            return Err(IndividualTwtRequesterError::FlowBusy(flow_id));
        }
        self.flows[flow_id.index()] = FlowPhase::SetupQueued {
            proposal,
            attempts_remaining: self.config.setup_attempt_limit,
            ready_at_micros: now_micros,
        };
        Ok(())
    }

    /// Queue a teardown after the chip owner has synchronously removed any
    /// installed wake agreement. This method itself never asserts that the
    /// hardware schedule was removed.
    pub fn queue_teardown(
        &mut self,
        flow_id: IndividualTwtFlowId,
        now_micros: u64,
    ) -> Result<(), IndividualTwtRequesterError> {
        if matches!(self.flows[flow_id.index()], FlowPhase::Idle) {
            return Err(IndividualTwtRequesterError::NoAgreement(flow_id));
        }
        if matches!(
            self.flows[flow_id.index()],
            FlowPhase::SetupTransmitting { .. } | FlowPhase::TeardownTransmitting { .. }
        ) {
            return Err(IndividualTwtRequesterError::FlowBusy(flow_id));
        }
        self.flows[flow_id.index()] = FlowPhase::TeardownQueued {
            attempts_remaining: self.config.teardown_attempt_limit,
            ready_at_micros: now_micros,
        };
        Ok(())
    }

    pub fn service(
        &mut self,
        now_micros: u64,
    ) -> Result<IndividualTwtService, IndividualTwtRequesterError> {
        for index in 0..INDIVIDUAL_TWT_FLOW_CAPACITY {
            let FlowPhase::AwaitingResponse {
                proposal,
                attempts_remaining,
                deadline_micros,
                ..
            } = self.flows[index]
            else {
                continue;
            };
            if now_micros < deadline_micros {
                continue;
            }
            let flow_id = IndividualTwtFlowId::new(index as u8)
                .expect("the fixed flow array only contains representable flow IDs");
            if attempts_remaining == 0 {
                self.flows[index] = FlowPhase::Idle;
                return Ok(IndividualTwtService::Event(
                    IndividualTwtRequesterEvent::SetupTimedOut { flow_id },
                ));
            }
            let retry_at_micros = now_micros
                .checked_add(u64::from(self.config.retry_interval_micros))
                .ok_or(IndividualTwtRequesterError::DeadlineOverflow)?;
            self.flows[index] = FlowPhase::SetupQueued {
                proposal,
                attempts_remaining,
                ready_at_micros: retry_at_micros,
            };
            return Ok(IndividualTwtService::Event(
                IndividualTwtRequesterEvent::SetupRetryScheduled {
                    flow_id,
                    retry_at_micros,
                },
            ));
        }

        for index in 0..INDIVIDUAL_TWT_FLOW_CAPACITY {
            let flow_id = IndividualTwtFlowId::new(index as u8)
                .expect("the fixed flow array only contains representable flow IDs");
            match self.flows[index] {
                FlowPhase::SetupQueued {
                    proposal,
                    attempts_remaining,
                    ready_at_micros,
                } if now_micros >= ready_at_micros => {
                    let generation = self.take_generation()?;
                    let dialog_token = self.take_dialog_token();
                    let setup = IndividualTwtSetup {
                        dialog_token,
                        control: proposal.control,
                        parameters: proposal.parameters,
                    };
                    let body = setup.encode_body()?;
                    let attempts_remaining = attempts_remaining - 1;
                    self.flows[index] = FlowPhase::SetupTransmitting {
                        proposal,
                        dialog_token,
                        generation,
                        attempts_remaining,
                    };
                    return Ok(IndividualTwtService::Transmit(IndividualTwtTransmission {
                        flow_id,
                        generation,
                        kind: IndividualTwtTxKind::Setup,
                        body: IndividualTwtTxBody::Setup(body),
                    }));
                }
                FlowPhase::TeardownQueued {
                    attempts_remaining,
                    ready_at_micros,
                } if now_micros >= ready_at_micros => {
                    let body = IndividualTwtTeardown::one(flow_id).encode_body()?;
                    let generation = self.take_generation()?;
                    let attempts_remaining = attempts_remaining - 1;
                    self.flows[index] = FlowPhase::TeardownTransmitting {
                        generation,
                        attempts_remaining,
                    };
                    return Ok(IndividualTwtService::Transmit(IndividualTwtTransmission {
                        flow_id,
                        generation,
                        kind: IndividualTwtTxKind::Teardown,
                        body: IndividualTwtTxBody::Teardown(body),
                    }));
                }
                _ => {}
            }
        }
        Ok(IndividualTwtService::Idle)
    }

    pub fn complete_transmission(
        &mut self,
        transmission: IndividualTwtTransmission,
        acknowledged: bool,
        now_micros: u64,
    ) -> Result<IndividualTwtRequesterEvent, IndividualTwtRequesterError> {
        let index = transmission.flow_id.index();
        match (self.flows[index], transmission.kind) {
            (
                FlowPhase::SetupTransmitting {
                    proposal,
                    dialog_token,
                    generation,
                    attempts_remaining,
                },
                IndividualTwtTxKind::Setup,
            ) if generation == transmission.generation => {
                if acknowledged {
                    let deadline_micros = now_micros
                        .checked_add(u64::from(self.config.response_timeout_micros))
                        .ok_or(IndividualTwtRequesterError::DeadlineOverflow)?;
                    self.flows[index] = FlowPhase::AwaitingResponse {
                        proposal,
                        dialog_token,
                        generation,
                        attempts_remaining,
                        deadline_micros,
                    };
                    Ok(IndividualTwtRequesterEvent::SetupPublished {
                        flow_id: transmission.flow_id,
                        response_deadline_micros: deadline_micros,
                    })
                } else {
                    self.retry_or_finish_setup(
                        transmission.flow_id,
                        proposal,
                        attempts_remaining,
                        now_micros,
                        false,
                    )
                }
            }
            (
                FlowPhase::TeardownTransmitting {
                    generation,
                    attempts_remaining,
                },
                IndividualTwtTxKind::Teardown,
            ) if generation == transmission.generation => {
                if acknowledged {
                    self.flows[index] = FlowPhase::Idle;
                    Ok(IndividualTwtRequesterEvent::TeardownComplete {
                        flow_id: transmission.flow_id,
                    })
                } else if attempts_remaining == 0 {
                    self.flows[index] = FlowPhase::Idle;
                    Ok(IndividualTwtRequesterEvent::TeardownTxFailed {
                        flow_id: transmission.flow_id,
                    })
                } else {
                    let retry_at_micros = now_micros
                        .checked_add(u64::from(self.config.retry_interval_micros))
                        .ok_or(IndividualTwtRequesterError::DeadlineOverflow)?;
                    self.flows[index] = FlowPhase::TeardownQueued {
                        attempts_remaining,
                        ready_at_micros: retry_at_micros,
                    };
                    Ok(IndividualTwtRequesterEvent::TeardownRetryScheduled {
                        flow_id: transmission.flow_id,
                        retry_at_micros,
                    })
                }
            }
            _ => Err(IndividualTwtRequesterError::StaleTransmission),
        }
    }

    /// Cancel an action before physical publication. Used when a chip's
    /// hardware-admission boundary reports `Unsupported`.
    pub fn abort_transmission(
        &mut self,
        transmission: IndividualTwtTransmission,
    ) -> Result<(), IndividualTwtRequesterError> {
        let index = transmission.flow_id.index();
        let matches = match (self.flows[index], transmission.kind) {
            (FlowPhase::SetupTransmitting { generation, .. }, IndividualTwtTxKind::Setup)
            | (FlowPhase::TeardownTransmitting { generation, .. }, IndividualTwtTxKind::Teardown) => {
                generation == transmission.generation
            }
            _ => false,
        };
        if !matches {
            return Err(IndividualTwtRequesterError::StaleTransmission);
        }
        self.flows[index] = FlowPhase::Idle;
        Ok(())
    }

    pub fn on_action(
        &mut self,
        action: IndividualTwtAction,
    ) -> Result<IndividualTwtSetupDisposition, IndividualTwtRequesterError> {
        match action {
            IndividualTwtAction::Setup(setup) => self.on_setup_response(setup),
            IndividualTwtAction::Teardown(_) => Ok(IndividualTwtSetupDisposition::Stale),
        }
    }

    pub fn on_setup_response(
        &mut self,
        response: IndividualTwtSetup,
    ) -> Result<IndividualTwtSetupDisposition, IndividualTwtRequesterError> {
        response.validate()?;
        let flow_id = response.parameters.flow_id;
        let index = flow_id.index();
        let FlowPhase::AwaitingResponse {
            proposal,
            dialog_token,
            generation,
            ..
        } = self.flows[index]
        else {
            return Ok(IndividualTwtSetupDisposition::Stale);
        };
        if response.dialog_token != dialog_token || response.parameters.requesting_sta {
            return Ok(IndividualTwtSetupDisposition::Stale);
        }

        if response.parameters.setup_command == IndividualTwtSetupCommand::Accept
            && !response.parameters.implicit
        {
            let frontier = IndividualTwtInformationFrontier::from_fields(
                response.control,
                response.parameters,
            );
            self.flows[index] = FlowPhase::TeardownQueued {
                attempts_remaining: self.config.teardown_attempt_limit,
                // A response has already crossed the wire. Zero is due in
                // every monotonic runtime domain without inventing a new
                // timestamp parameter at this protocol edge.
                ready_at_micros: 0,
            };
            return Ok(
                IndividualTwtSetupDisposition::ExplicitInformationUnsupported { flow_id, frontier },
            );
        }

        match response.parameters.setup_command {
            IndividualTwtSetupCommand::Reject => {
                self.flows[index] = FlowPhase::Idle;
                Ok(IndividualTwtSetupDisposition::Rejected { flow_id })
            }
            command @ (IndividualTwtSetupCommand::Alternate
            | IndividualTwtSetupCommand::Dictate) => {
                let proposed = IndividualTwtAgreement::from_response(response)?;
                self.flows[index] = FlowPhase::Idle;
                Ok(IndividualTwtSetupDisposition::Alternative {
                    flow_id,
                    command,
                    proposed,
                })
            }
            IndividualTwtSetupCommand::Accept => {
                if proposal.parameters.setup_command == IndividualTwtSetupCommand::Demand
                    && !demand_response_matches(proposal, response)
                {
                    self.flows[index] = FlowPhase::Idle;
                    return Err(IndividualTwtRequesterError::DemandResponseMismatch);
                }
                let agreement = IndividualTwtAgreement::from_response(response)?;
                self.flows[index] = FlowPhase::AwaitingHardwareInstall {
                    generation,
                    agreement,
                };
                Ok(IndividualTwtSetupDisposition::InstallRequired {
                    flow_id,
                    generation,
                    agreement,
                })
            }
            _ => Ok(IndividualTwtSetupDisposition::Stale),
        }
    }

    pub fn commit_hardware_install(
        &mut self,
        flow_id: IndividualTwtFlowId,
        generation: u32,
    ) -> Result<IndividualTwtAgreement, IndividualTwtRequesterError> {
        let index = flow_id.index();
        let FlowPhase::AwaitingHardwareInstall {
            generation: expected,
            agreement,
        } = self.flows[index]
        else {
            return Err(IndividualTwtRequesterError::UnexpectedInstall);
        };
        if expected != generation {
            return Err(IndividualTwtRequesterError::UnexpectedInstall);
        }
        self.flows[index] = FlowPhase::Active(agreement);
        Ok(agreement)
    }

    /// Roll back an accepted peer agreement when chip installation fails.
    /// The AP must subsequently receive the queued teardown; the local wake
    /// planner never observes the failed agreement as active.
    pub fn reject_hardware_install(
        &mut self,
        flow_id: IndividualTwtFlowId,
        generation: u32,
        now_micros: u64,
    ) -> Result<(), IndividualTwtRequesterError> {
        let index = flow_id.index();
        let FlowPhase::AwaitingHardwareInstall {
            generation: expected,
            ..
        } = self.flows[index]
        else {
            return Err(IndividualTwtRequesterError::UnexpectedInstall);
        };
        if expected != generation {
            return Err(IndividualTwtRequesterError::UnexpectedInstall);
        }
        self.flows[index] = FlowPhase::TeardownQueued {
            attempts_remaining: self.config.teardown_attempt_limit,
            ready_at_micros: now_micros,
        };
        Ok(())
    }

    /// Snapshot installed agreements affected by a peer teardown without
    /// changing portable state. The chip owner must commit each successful
    /// hardware removal before the teardown is applied to the remaining
    /// protocol phases.
    pub fn installed_for_teardown(
        &self,
        teardown: IndividualTwtTeardown,
    ) -> [Option<IndividualTwtAgreement>; INDIVIDUAL_TWT_FLOW_CAPACITY] {
        let mut installed = [None; INDIVIDUAL_TWT_FLOW_CAPACITY];
        if teardown.all_flows {
            for (index, phase) in self.flows.iter().enumerate() {
                if let FlowPhase::Active(agreement) = *phase {
                    installed[index] = Some(agreement);
                }
            }
        } else if let FlowPhase::Active(agreement) = self.flows[teardown.flow_id.index()] {
            installed[teardown.flow_id.index()] = Some(agreement);
        }
        installed
    }

    /// Commit one successful chip removal. A stale or duplicated completion
    /// cannot silently erase a newer agreement for the same flow ID.
    pub fn commit_hardware_remove(
        &mut self,
        agreement: IndividualTwtAgreement,
    ) -> Result<(), IndividualTwtRequesterError> {
        let phase = &mut self.flows[agreement.flow_id.index()];
        if !matches!(*phase, FlowPhase::Active(current) if current == agreement) {
            return Err(IndividualTwtRequesterError::UnexpectedRemove);
        }
        *phase = FlowPhase::Idle;
        Ok(())
    }

    /// Apply the peer teardown to every remaining protocol phase. Hardware
    /// owners use `installed_for_teardown` plus `commit_hardware_remove`
    /// first; the returned snapshot supports portable owners with no chip
    /// schedule to roll back.
    pub fn on_peer_teardown(
        &mut self,
        teardown: IndividualTwtTeardown,
    ) -> [Option<IndividualTwtAgreement>; INDIVIDUAL_TWT_FLOW_CAPACITY] {
        let mut removed = [None; INDIVIDUAL_TWT_FLOW_CAPACITY];
        if teardown.all_flows {
            for (index, phase) in self.flows.iter_mut().enumerate() {
                if let FlowPhase::Active(agreement) = *phase {
                    removed[index] = Some(agreement);
                }
                *phase = FlowPhase::Idle;
            }
        } else {
            let index = teardown.flow_id.index();
            if let FlowPhase::Active(agreement) = self.flows[index] {
                removed[index] = Some(agreement);
            }
            self.flows[index] = FlowPhase::Idle;
        }
        removed
    }

    /// Reconnect/stop boundary. No portable agreement survives a new BSSID
    /// epoch; installed agreements are returned for chip rollback.
    pub fn reset_for_reconnect(
        &mut self,
    ) -> [Option<IndividualTwtAgreement>; INDIVIDUAL_TWT_FLOW_CAPACITY] {
        let mut removed = [None; INDIVIDUAL_TWT_FLOW_CAPACITY];
        for (index, phase) in self.flows.iter_mut().enumerate() {
            if let FlowPhase::Active(agreement) = *phase {
                removed[index] = Some(agreement);
            }
            *phase = FlowPhase::Idle;
        }
        self.next_dialog_token = 1;
        // Never wrap an affine identity. Saturation permanently prevents a
        // stale generation from being reissued after enough reconnects.
        self.generation = self.generation.saturating_add(1);
        removed
    }

    pub fn next_deadline_micros(&self) -> Option<u64> {
        self.flows
            .iter()
            .filter_map(|phase| match phase {
                FlowPhase::SetupQueued {
                    ready_at_micros, ..
                }
                | FlowPhase::TeardownQueued {
                    ready_at_micros, ..
                } => Some(*ready_at_micros),
                FlowPhase::AwaitingResponse {
                    deadline_micros, ..
                } => Some(*deadline_micros),
                _ => None,
            })
            .min()
    }

    pub fn plan_next_wake(
        &self,
        station_tsf: u64,
        wake_guard_micros: u32,
    ) -> Result<Option<IndividualTwtWakePlan>, IndividualTwtWakePlanError> {
        let mut best: Option<IndividualTwtWakePlan> = None;
        for phase in self.flows {
            let FlowPhase::Active(agreement) = phase else {
                continue;
            };
            let candidate = plan_agreement_wake(agreement, station_tsf, wake_guard_micros)?;
            best = Some(match best {
                None => candidate,
                Some(current) => current.merge_or_earlier(candidate, station_tsf),
            });
        }
        Ok(best)
    }

    fn retry_or_finish_setup(
        &mut self,
        flow_id: IndividualTwtFlowId,
        proposal: IndividualTwtProposal,
        attempts_remaining: u8,
        now_micros: u64,
        timed_out: bool,
    ) -> Result<IndividualTwtRequesterEvent, IndividualTwtRequesterError> {
        if attempts_remaining == 0 {
            self.flows[flow_id.index()] = FlowPhase::Idle;
            return Ok(if timed_out {
                IndividualTwtRequesterEvent::SetupTimedOut { flow_id }
            } else {
                IndividualTwtRequesterEvent::SetupTxFailed { flow_id }
            });
        }
        let retry_at_micros = now_micros
            .checked_add(u64::from(self.config.retry_interval_micros))
            .ok_or(IndividualTwtRequesterError::DeadlineOverflow)?;
        self.flows[flow_id.index()] = FlowPhase::SetupQueued {
            proposal,
            attempts_remaining,
            ready_at_micros: retry_at_micros,
        };
        Ok(IndividualTwtRequesterEvent::SetupRetryScheduled {
            flow_id,
            retry_at_micros,
        })
    }

    fn take_dialog_token(&mut self) -> u8 {
        let token = self.next_dialog_token;
        self.next_dialog_token = next_dialog_token(token);
        token
    }

    fn take_generation(&mut self) -> Result<u32, IndividualTwtRequesterError> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(IndividualTwtRequesterError::GenerationExhausted)?;
        Ok(self.generation)
    }
}

fn demand_response_matches(proposal: IndividualTwtProposal, response: IndividualTwtSetup) -> bool {
    let requested = proposal.parameters;
    let accepted = response.parameters;
    proposal.control.wake_duration_unit == response.control.wake_duration_unit
        && requested.trigger == accepted.trigger
        && requested.implicit == accepted.implicit
        && requested.flow_type == accepted.flow_type
        && requested.protection == accepted.protection
        && requested.target_wake_time_tsf == accepted.target_wake_time_tsf
        && requested.nominal_minimum_wake_duration == accepted.nominal_minimum_wake_duration
        && requested.wake_interval_mantissa == accepted.wake_interval_mantissa
        && requested.wake_interval_exponent == accepted.wake_interval_exponent
        && requested.twt_channel == accepted.twt_channel
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtWakePlanError {
    InvalidAgreement,
    WakeGuardOutsideInterval {
        flow_id: IndividualTwtFlowId,
        wake_guard_micros: u32,
        interval_micros: u64,
    },
    AmbiguousTsfDistance,
}

/// Earliest active or upcoming service window across all installed flows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtWakePlan {
    pub flow_bitmap: u8,
    pub wake_tsf: u64,
    pub service_start_tsf: u64,
    pub service_end_tsf: u64,
    pub service_open: bool,
}

impl IndividualTwtWakePlan {
    fn merge_or_earlier(self, other: Self, now_tsf: u64) -> Self {
        let self_distance = self.wake_tsf.wrapping_sub(now_tsf);
        let other_distance = other.wake_tsf.wrapping_sub(now_tsf);
        if self.service_open && other.service_open {
            return Self {
                flow_bitmap: self.flow_bitmap | other.flow_bitmap,
                wake_tsf: now_tsf,
                service_start_tsf: if self
                    .service_start_tsf
                    .wrapping_sub(now_tsf)
                    .wrapping_sub(other.service_start_tsf.wrapping_sub(now_tsf))
                    <= i64::MAX as u64
                {
                    other.service_start_tsf
                } else {
                    self.service_start_tsf
                },
                service_end_tsf: later_future_tsf(
                    now_tsf,
                    self.service_end_tsf,
                    other.service_end_tsf,
                ),
                service_open: true,
            };
        }
        if other.service_open || (!self.service_open && other_distance < self_distance) {
            other
        } else if self.service_open || self_distance < other_distance {
            self
        } else {
            Self {
                flow_bitmap: self.flow_bitmap | other.flow_bitmap,
                service_end_tsf: later_future_tsf(
                    now_tsf,
                    self.service_end_tsf,
                    other.service_end_tsf,
                ),
                ..self
            }
        }
    }
}

fn plan_agreement_wake(
    agreement: IndividualTwtAgreement,
    station_tsf: u64,
    wake_guard_micros: u32,
) -> Result<IndividualTwtWakePlan, IndividualTwtWakePlanError> {
    let interval = agreement.wake_interval_micros;
    let duration = u64::from(agreement.wake_duration_micros);
    if interval == 0 || duration == 0 || duration > interval || interval > i64::MAX as u64 {
        return Err(IndividualTwtWakePlanError::InvalidAgreement);
    }
    if u64::from(wake_guard_micros) >= interval {
        return Err(IndividualTwtWakePlanError::WakeGuardOutsideInterval {
            flow_id: agreement.flow_id,
            wake_guard_micros,
            interval_micros: interval,
        });
    }

    let since_target = station_tsf.wrapping_sub(agreement.target_wake_time_tsf);
    let (service_start_tsf, service_open) = if since_target <= i64::MAX as u64 {
        let offset = since_target % interval;
        let current_start = station_tsf.wrapping_sub(offset);
        if offset < duration {
            (current_start, true)
        } else {
            (current_start.wrapping_add(interval), false)
        }
    } else {
        let until_target = agreement.target_wake_time_tsf.wrapping_sub(station_tsf);
        if until_target > i64::MAX as u64 {
            return Err(IndividualTwtWakePlanError::AmbiguousTsfDistance);
        }
        (agreement.target_wake_time_tsf, false)
    };
    let service_end_tsf = service_start_tsf.wrapping_add(duration);
    if !service_open && service_end_tsf.wrapping_sub(station_tsf) > i64::MAX as u64 {
        // The start is in the comparable future half, but the complete
        // service window is not. Publishing a partial window would make
        // merge/end ordering ambiguous across TSF wrap.
        return Err(IndividualTwtWakePlanError::AmbiguousTsfDistance);
    }
    let wake_tsf = if service_open {
        station_tsf
    } else {
        let until_start = service_start_tsf.wrapping_sub(station_tsf);
        station_tsf.wrapping_add(until_start.saturating_sub(u64::from(wake_guard_micros)))
    };
    Ok(IndividualTwtWakePlan {
        flow_bitmap: 1 << agreement.flow_id.get(),
        wake_tsf,
        service_start_tsf,
        service_end_tsf,
        service_open,
    })
}

fn later_future_tsf(now_tsf: u64, left: u64, right: u64) -> u64 {
    if left.wrapping_sub(now_tsf) >= right.wrapping_sub(now_tsf) {
        left
    } else {
        right
    }
}

const fn next_dialog_token(current: u8) -> u8 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

/// Public default stays disabled. It is a named value for compositions that
/// must explicitly prove hardware admission before enabling requester work.
pub const INDIVIDUAL_TWT_REQUESTER_DISABLED: Option<IndividualTwtRequesterConfig> = None;

/// Conservative portable profile; it is not installed by any production
/// ESP32-S31 composition while the hardware boundary remains unsupported.
pub const fn conservative_individual_twt_requester_config() -> IndividualTwtRequesterConfig {
    IndividualTwtRequesterConfig {
        response_timeout_micros: 1_000_000,
        retry_interval_micros: 250_000,
        setup_attempt_limit: 3,
        teardown_attempt_limit: 3,
    }
}

/// Helper for constructing a control field without importing the wire crate.
pub const fn individual_twt_control(
    responder_power_save: bool,
    information_frames_disabled: bool,
    wake_duration_unit: TwtWakeDurationUnit,
) -> IndividualTwtControl {
    IndividualTwtControl {
        responder_power_save,
        information_frames_disabled,
        wake_duration_unit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_ieee80211::twt::IndividualTwtFlowType;

    const CONFIG: IndividualTwtRequesterConfig =
        match IndividualTwtRequesterConfig::new(1_000, 100, 2, 2) {
            Ok(config) => config,
            Err(_) => panic!("valid requester config"),
        };

    fn parameters(implicit: bool) -> IndividualTwtParameterSet {
        IndividualTwtParameterSet {
            requesting_sta: true,
            setup_command: IndividualTwtSetupCommand::Request,
            trigger: false,
            implicit,
            flow_type: IndividualTwtFlowType::Announced,
            flow_id: IndividualTwtFlowId::new(2).unwrap(),
            wake_interval_exponent: 0,
            protection: false,
            target_wake_time_tsf: 10_000,
            nominal_minimum_wake_duration: 1,
            wake_interval_mantissa: 1_024,
            twt_channel: 0,
        }
    }

    fn proposal(implicit: bool) -> IndividualTwtProposal {
        IndividualTwtProposal {
            control: IndividualTwtControl::REQUEST,
            parameters: parameters(implicit),
        }
    }

    #[test]
    fn generation_exhaustion_never_reissues_a_stale_identity() {
        let mut requester = IndividualTwtRequester::new(CONFIG);
        requester.generation = u32::MAX;
        requester.queue_setup(proposal(true), 0).unwrap();

        assert_eq!(
            requester.service(0),
            Err(IndividualTwtRequesterError::GenerationExhausted)
        );
        assert_eq!(
            requester.status(IndividualTwtFlowId::new(2).unwrap()),
            IndividualTwtFlowStatus::SetupQueued
        );
        assert_eq!(requester.next_dialog_token, 1);

        requester.reset_for_reconnect();
        assert_eq!(requester.generation, u32::MAX);
    }

    #[test]
    fn explicit_proposal_reports_the_exact_information_frontier() {
        assert_eq!(
            proposal(false).validate(),
            Err(
                IndividualTwtRequesterError::ExplicitTwtInformationUnsupported(
                    IndividualTwtInformationFrontier {
                        flow_id: IndividualTwtFlowId::new(2).unwrap(),
                        initial_target_wake_time_tsf: 10_000,
                        information_frames_disabled: false,
                    }
                )
            )
        );
    }

    #[test]
    fn peer_accepted_explicit_agreement_is_torn_down_not_installed() {
        let mut requester = IndividualTwtRequester::new(CONFIG);
        requester.queue_setup(proposal(true), 0).unwrap();
        let IndividualTwtService::Transmit(transmission) = requester.service(0).unwrap() else {
            panic!("setup must be ready");
        };
        requester
            .complete_transmission(transmission, true, 10)
            .unwrap();

        let mut response_parameters = parameters(false);
        response_parameters.requesting_sta = false;
        response_parameters.setup_command = IndividualTwtSetupCommand::Accept;
        let disposition = requester
            .on_setup_response(IndividualTwtSetup {
                dialog_token: 1,
                control: IndividualTwtControl::REQUEST,
                parameters: response_parameters,
            })
            .unwrap();
        assert_eq!(
            disposition,
            IndividualTwtSetupDisposition::ExplicitInformationUnsupported {
                flow_id: IndividualTwtFlowId::new(2).unwrap(),
                frontier: IndividualTwtInformationFrontier {
                    flow_id: IndividualTwtFlowId::new(2).unwrap(),
                    initial_target_wake_time_tsf: 10_000,
                    information_frames_disabled: false,
                },
            }
        );
        assert_eq!(requester.next_deadline_micros(), Some(0));
        let IndividualTwtService::Transmit(teardown) = requester.service(10).unwrap() else {
            panic!("rollback teardown must be ready immediately");
        };
        assert_eq!(teardown.kind, IndividualTwtTxKind::Teardown);
    }

    #[test]
    fn wake_plan_rejects_a_window_crossing_the_comparable_tsf_half() {
        let agreement = IndividualTwtAgreement {
            flow_id: IndividualTwtFlowId::new(0).unwrap(),
            control: IndividualTwtControl::REQUEST,
            trigger: false,
            implicit: true,
            flow_type: IndividualTwtFlowType::Announced,
            protection: false,
            target_wake_time_tsf: i64::MAX as u64,
            wake_interval_micros: i64::MAX as u64,
            wake_duration_micros: 256,
        };
        assert_eq!(
            plan_agreement_wake(agreement, 0, 10),
            Err(IndividualTwtWakePlanError::AmbiguousTsfDistance)
        );
    }

    #[test]
    fn wake_plan_preserves_a_future_window_across_tsf_wrap() {
        let agreement = IndividualTwtAgreement {
            flow_id: IndividualTwtFlowId::new(0).unwrap(),
            control: IndividualTwtControl::REQUEST,
            trigger: false,
            implicit: true,
            flow_type: IndividualTwtFlowType::Announced,
            protection: false,
            target_wake_time_tsf: 50,
            wake_interval_micros: 1_000,
            wake_duration_micros: 256,
        };
        assert_eq!(
            plan_agreement_wake(agreement, u64::MAX - 100, 10),
            Ok(IndividualTwtWakePlan {
                flow_bitmap: 1,
                wake_tsf: 40,
                service_start_tsf: 50,
                service_end_tsf: 306,
                service_open: false,
            })
        );
    }
}
