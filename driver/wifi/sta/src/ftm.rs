//! Bounded, allocation-free FTM requester session policy.
//!
//! This owner implements the single-burst ASAP subset. It owns retry and
//! deadline state, peer/token identity and raw four-timestamp results, but it
//! performs no I/O. In particular, a chip backend must prove antenna-point
//! RX/TX timestamp capture before publishing the request returned by
//! [`FtmRequester::service`].

use open_esp_radio_ieee80211::ftm::{
    FTM_INITIAL_REQUEST_BODY_LEN, FtmBurstDuration, FtmFormatAndBandwidth, FtmMeasurement,
    FtmRequestParameters, FtmResponseParameters, FtmResponseStatus, FtmTimestampPs, FtmToaError,
    FtmTodError, encode_initial_request,
};

const FTM_TIMESTAMP_MASK: u64 = (1_u64 << 48) - 1;
const FTM_TIMESTAMP_HALF_RANGE: u64 = 1_u64 << 47;
const MAX_REQUEST_ATTEMPTS: u8 = 8;
const MAX_TRACKED_INFORMATION_ELEMENTS_LEN: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmRequesterConfigError {
    ZeroResponseTimeout,
    ZeroRetryInterval,
    ZeroSessionTimeout,
    SessionShorterThanResponse,
    ZeroRequestAttempts,
    TooManyRequestAttempts,
    MultipleBurstsUnsupported,
    ScheduledBurstUnsupported,
    PartialTsfPreferenceUnsupported,
    NoMeasurementCountPreferenceUnsupported,
    UnsupportedRequestFormat,
}

/// One association-scoped request and retry policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmRequesterConfig {
    parameters: FtmRequestParameters,
    response_timeout_micros: u32,
    retry_interval_micros: u32,
    session_timeout_micros: u32,
    request_attempt_limit: u8,
}

impl FtmRequesterConfig {
    pub const fn new(
        parameters: FtmRequestParameters,
        response_timeout_micros: u32,
        retry_interval_micros: u32,
        session_timeout_micros: u32,
        request_attempt_limit: u8,
    ) -> Result<Self, FtmRequesterConfigError> {
        if response_timeout_micros == 0 {
            return Err(FtmRequesterConfigError::ZeroResponseTimeout);
        }
        if retry_interval_micros == 0 {
            return Err(FtmRequesterConfigError::ZeroRetryInterval);
        }
        if session_timeout_micros == 0 {
            return Err(FtmRequesterConfigError::ZeroSessionTimeout);
        }
        if session_timeout_micros < response_timeout_micros {
            return Err(FtmRequesterConfigError::SessionShorterThanResponse);
        }
        if request_attempt_limit == 0 {
            return Err(FtmRequesterConfigError::ZeroRequestAttempts);
        }
        if request_attempt_limit > MAX_REQUEST_ATTEMPTS {
            return Err(FtmRequesterConfigError::TooManyRequestAttempts);
        }
        if parameters.number_of_bursts_exponent() != 0 {
            return Err(FtmRequesterConfigError::MultipleBurstsUnsupported);
        }
        if !parameters.asap() {
            return Err(FtmRequesterConfigError::ScheduledBurstUnsupported);
        }
        if parameters.partial_tsf_timer().is_some() {
            return Err(FtmRequesterConfigError::PartialTsfPreferenceUnsupported);
        }
        if parameters.ftms_per_burst() == 0 {
            return Err(FtmRequesterConfigError::NoMeasurementCountPreferenceUnsupported);
        }
        match parameters.format_and_bandwidth() {
            FtmFormatAndBandwidth::NoPreference
            | FtmFormatAndBandwidth::NonHt20Mhz
            | FtmFormatAndBandwidth::HtMixed20Mhz => {}
            _ => return Err(FtmRequesterConfigError::UnsupportedRequestFormat),
        }
        Ok(Self {
            parameters,
            response_timeout_micros,
            retry_interval_micros,
            session_timeout_micros,
            request_attempt_limit,
        })
    }

    pub const fn parameters(self) -> FtmRequestParameters {
        self.parameters
    }

    /// Number of successfully transmitted FTM frames requested on the wire.
    pub const fn requested_ftms_per_burst(self) -> u8 {
        self.parameters.ftms_per_burst()
    }

    /// Maximum complete four-timestamp exchanges deliverable by that burst.
    ///
    /// The first FTM frame has no predecessor and the final frame supplies the
    /// responder timestamps for the preceding frame, so a single ASAP burst
    /// can deliver at most one fewer sample than its FTM frame allocation.
    pub const fn maximum_deliverable_samples(self) -> u8 {
        self.parameters.ftms_per_burst() - 1
    }

    pub const fn response_timeout_micros(self) -> u32 {
        self.response_timeout_micros
    }

    pub const fn retry_interval_micros(self) -> u32 {
        self.retry_interval_micros
    }

    pub const fn session_timeout_micros(self) -> u32 {
        self.session_timeout_micros
    }

    pub const fn request_attempt_limit(self) -> u8 {
        self.request_attempt_limit
    }
}

/// Timestamp capture for the just-received nonterminal FTM frame and its ACK.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmLocalExchangeTiming {
    pub dialog_token: u8,
    pub arrival_t2_ps: FtmTimestampPs,
    pub ack_departure_t3_ps: FtmTimestampPs,
}

impl FtmLocalExchangeTiming {
    pub const fn new(
        dialog_token: u8,
        arrival_t2_ps: FtmTimestampPs,
        ack_departure_t3_ps: FtmTimestampPs,
    ) -> Result<Self, FtmRequesterError> {
        if dialog_token == 0 {
            return Err(FtmRequesterError::ZeroLocalDialogToken);
        }
        Ok(Self {
            dialog_token,
            arrival_t2_ps,
            ack_departure_t3_ps,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmTimestampArithmeticError {
    ResponderClockDiscontinuous,
    AmbiguousResponderWrap,
    AmbiguousInitiatorWrap,
    NegativeRawInterval,
}

/// Raw interval difference from the four protocol timestamps.
///
/// This is not a calibrated RTT and cannot be converted to distance without a
/// chip-specific antenna delay, clock and timestamp-capture contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmRawIntervalDifferencePs {
    pub responder_round_trip_ps: u64,
    pub initiator_turnaround_ps: u64,
    pub difference_ps: u64,
}

/// One owned four-timestamp exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmRawExchange {
    pub dialog_token: u8,
    pub responder_departure_t1_ps: FtmTimestampPs,
    pub initiator_arrival_t2_ps: FtmTimestampPs,
    pub initiator_ack_departure_t3_ps: FtmTimestampPs,
    pub responder_ack_arrival_t4_ps: FtmTimestampPs,
    pub tod_error: FtmTodError,
    pub toa_error: FtmToaError,
}

impl FtmRawExchange {
    pub fn raw_interval_difference(
        self,
    ) -> Result<FtmRawIntervalDifferencePs, FtmTimestampArithmeticError> {
        if self.tod_error.not_continuous {
            return Err(FtmTimestampArithmeticError::ResponderClockDiscontinuous);
        }
        let responder_round_trip_ps = forward_timestamp_delta(
            self.responder_departure_t1_ps,
            self.responder_ack_arrival_t4_ps,
        )
        .ok_or(FtmTimestampArithmeticError::AmbiguousResponderWrap)?;
        let initiator_turnaround_ps = forward_timestamp_delta(
            self.initiator_arrival_t2_ps,
            self.initiator_ack_departure_t3_ps,
        )
        .ok_or(FtmTimestampArithmeticError::AmbiguousInitiatorWrap)?;
        let Some(difference_ps) = responder_round_trip_ps.checked_sub(initiator_turnaround_ps)
        else {
            return Err(FtmTimestampArithmeticError::NegativeRawInterval);
        };
        Ok(FtmRawIntervalDifferencePs {
            responder_round_trip_ps,
            initiator_turnaround_ps,
            difference_ps,
        })
    }
}

const fn forward_timestamp_delta(start: FtmTimestampPs, end: FtmTimestampPs) -> Option<u64> {
    let delta = end.get().wrapping_sub(start.get()) & FTM_TIMESTAMP_MASK;
    if delta < FTM_TIMESTAMP_HALF_RANGE {
        Some(delta)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmSessionTermination {
    PeerTerminalFrame,
}

/// Result returned by value so a later session cannot mutate old samples.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmSessionResult<const MAX_SAMPLES: usize> {
    pub peer: [u8; 6],
    pub session_generation: u32,
    pub negotiated: FtmResponseParameters,
    pub termination: FtmSessionTermination,
    samples: [Option<FtmRawExchange>; MAX_SAMPLES],
    sample_count: usize,
}

impl<const MAX_SAMPLES: usize> FtmSessionResult<MAX_SAMPLES> {
    pub const fn sample_count(&self) -> usize {
        self.sample_count
    }

    pub const fn sample(&self, index: usize) -> Option<FtmRawExchange> {
        if index < self.sample_count {
            self.samples[index]
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmSessionFailure {
    RequestTxFailed,
    InitialResponseTimedOut,
    SessionTimedOut,
    PeerIncapable,
    PeerFailed { retry_after_seconds: u8 },
    HardwareAdmissionRejected,
    ProtocolViolation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmRequesterError {
    Busy,
    NoActiveSession,
    ResultNotReady,
    FailureNotReady,
    CapacityTooSmall {
        required_samples: u8,
        capacity: usize,
    },
    GenerationExhausted,
    DeadlineOverflow,
    StaleTransmission,
    ZeroLocalDialogToken,
    MissingLocalTiming,
    UnexpectedLocalTiming,
    LocalDialogTokenMismatch,
    MissingInitialParameters,
    UnexpectedParameters,
    UnexpectedInformationElements,
    InformationElementsTooLong {
        length: usize,
        capacity: usize,
    },
    InitialRetransmissionMismatch,
    UnsupportedNegotiation,
    TooManyMeasurements,
    DialogTokenReused,
    DialogTokenOutOfSequence {
        expected: u8,
        actual: u8,
    },
    ConflictingDuplicate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmRequestTransmission {
    session_generation: u32,
    transmission_generation: u32,
    attempt: u8,
    body: [u8; FTM_INITIAL_REQUEST_BODY_LEN],
}

impl FtmRequestTransmission {
    pub const fn session_generation(self) -> u32 {
        self.session_generation
    }

    pub const fn transmission_generation(self) -> u32 {
        self.transmission_generation
    }

    pub const fn attempt(self) -> u8 {
        self.attempt
    }

    /// Borrow the exact Action body owned by this transmission identity.
    pub const fn body(&self) -> &[u8; FTM_INITIAL_REQUEST_BODY_LEN] {
        &self.body
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmRequesterEvent {
    RequestPublished {
        response_deadline_micros: u64,
        session_deadline_micros: u64,
    },
    RequestRetryScheduled {
        retry_at_micros: u64,
    },
    Failed(FtmSessionFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmRequesterService {
    Idle,
    Transmit(FtmRequestTransmission),
    Event(FtmRequesterEvent),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmMeasurementDisposition {
    ForeignPeer,
    Stale,
    Duplicate,
    InitialAccepted {
        dialog_token: u8,
        allocated_ftms_per_burst: u8,
        maximum_samples: u8,
    },
    InitialRetransmissionAccepted {
        abandoned_dialog_token: u8,
        dialog_token: u8,
    },
    SampleAccepted {
        dialog_token: u8,
        sample_index: usize,
    },
    DuplicateSample {
        dialog_token: u8,
    },
    Complete {
        samples: usize,
    },
    Failed(FtmSessionFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FtmRequesterPhase {
    Idle,
    RequestQueued {
        ready_at_micros: u64,
        attempts_remaining: u8,
    },
    RequestTransmitting {
        transmission: FtmRequestTransmission,
        attempts_remaining: u8,
    },
    AwaitingInitial {
        attempts_remaining: u8,
        response_deadline_micros: u64,
        session_deadline_micros: u64,
    },
    Measuring {
        session_deadline_micros: u64,
        negotiated: FtmResponseParameters,
    },
    Complete {
        negotiated: FtmResponseParameters,
        termination: FtmSessionTermination,
    },
    Failed(FtmSessionFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LastMeasurement {
    fields: open_esp_radio_ieee80211::ftm::FtmMeasurementFields,
    parameters: Option<FtmResponseParameters>,
    information_elements: OwnedInformationElements,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OwnedInformationElements {
    bytes: [u8; MAX_TRACKED_INFORMATION_ELEMENTS_LEN],
    len: u8,
}

impl OwnedInformationElements {
    fn new(bytes: &[u8]) -> Result<Self, FtmRequesterError> {
        if bytes.len() > MAX_TRACKED_INFORMATION_ELEMENTS_LEN {
            return Err(FtmRequesterError::InformationElementsTooLong {
                length: bytes.len(),
                capacity: MAX_TRACKED_INFORMATION_ELEMENTS_LEN,
            });
        }
        let mut owned = [0_u8; MAX_TRACKED_INFORMATION_ELEMENTS_LEN];
        owned[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            bytes: owned,
            len: bytes.len() as u8,
        })
    }

    fn matches(self, bytes: &[u8]) -> bool {
        usize::from(self.len) == bytes.len() && self.bytes[..bytes.len()] == *bytes
    }
}

/// One fixed-capacity FTM requester.
pub struct FtmRequester<const MAX_SAMPLES: usize> {
    config: FtmRequesterConfig,
    phase: FtmRequesterPhase,
    peer: [u8; 6],
    session_generation: u32,
    transmission_generation: u32,
    dialog_tokens: [u32; 8],
    pending: Option<FtmLocalExchangeTiming>,
    last_measurement: Option<LastMeasurement>,
    samples: [Option<FtmRawExchange>; MAX_SAMPLES],
    sample_count: usize,
}

impl<const MAX_SAMPLES: usize> FtmRequester<MAX_SAMPLES> {
    pub const fn new(config: FtmRequesterConfig) -> Self {
        Self {
            config,
            phase: FtmRequesterPhase::Idle,
            peer: [0; 6],
            session_generation: 0,
            transmission_generation: 0,
            dialog_tokens: [0; 8],
            pending: None,
            last_measurement: None,
            samples: [None; MAX_SAMPLES],
            sample_count: 0,
        }
    }

    pub const fn config(&self) -> FtmRequesterConfig {
        self.config
    }

    pub const fn is_idle(&self) -> bool {
        matches!(self.phase, FtmRequesterPhase::Idle)
    }

    pub const fn failure(&self) -> Option<FtmSessionFailure> {
        match self.phase {
            FtmRequesterPhase::Failed(failure) => Some(failure),
            _ => None,
        }
    }

    pub fn start(&mut self, peer: [u8; 6], now_micros: u64) -> Result<u32, FtmRequesterError> {
        if !self.is_idle() {
            return Err(FtmRequesterError::Busy);
        }
        let required_samples = self.config.maximum_deliverable_samples();
        if usize::from(required_samples) > MAX_SAMPLES {
            return Err(FtmRequesterError::CapacityTooSmall {
                required_samples,
                capacity: MAX_SAMPLES,
            });
        }
        self.session_generation = self
            .session_generation
            .checked_add(1)
            .ok_or(FtmRequesterError::GenerationExhausted)?;
        self.peer = peer;
        self.dialog_tokens.fill(0);
        self.pending = None;
        self.last_measurement = None;
        self.samples.fill(None);
        self.sample_count = 0;
        self.phase = FtmRequesterPhase::RequestQueued {
            ready_at_micros: now_micros,
            attempts_remaining: self.config.request_attempt_limit,
        };
        Ok(self.session_generation)
    }

    pub const fn next_deadline_micros(&self) -> Option<u64> {
        match self.phase {
            FtmRequesterPhase::RequestQueued {
                ready_at_micros, ..
            } => Some(ready_at_micros),
            FtmRequesterPhase::AwaitingInitial {
                response_deadline_micros,
                ..
            } => Some(response_deadline_micros),
            FtmRequesterPhase::Measuring {
                session_deadline_micros,
                ..
            } => Some(session_deadline_micros),
            _ => None,
        }
    }

    pub fn service(&mut self, now_micros: u64) -> Result<FtmRequesterService, FtmRequesterError> {
        match self.phase {
            FtmRequesterPhase::AwaitingInitial {
                attempts_remaining,
                response_deadline_micros,
                ..
            } if now_micros >= response_deadline_micros => {
                if attempts_remaining == 0 {
                    return Ok(self.fail(FtmSessionFailure::InitialResponseTimedOut));
                }
                let retry_at_micros = now_micros
                    .checked_add(u64::from(self.config.retry_interval_micros))
                    .ok_or(FtmRequesterError::DeadlineOverflow)?;
                self.phase = FtmRequesterPhase::RequestQueued {
                    ready_at_micros: retry_at_micros,
                    attempts_remaining,
                };
                return Ok(FtmRequesterService::Event(
                    FtmRequesterEvent::RequestRetryScheduled { retry_at_micros },
                ));
            }
            FtmRequesterPhase::Measuring {
                session_deadline_micros,
                ..
            } if now_micros >= session_deadline_micros => {
                return Ok(self.fail(FtmSessionFailure::SessionTimedOut));
            }
            _ => {}
        }

        let FtmRequesterPhase::RequestQueued {
            ready_at_micros,
            attempts_remaining,
        } = self.phase
        else {
            return Ok(FtmRequesterService::Idle);
        };
        if now_micros < ready_at_micros {
            return Ok(FtmRequesterService::Idle);
        }
        self.transmission_generation = self
            .transmission_generation
            .checked_add(1)
            .ok_or(FtmRequesterError::GenerationExhausted)?;
        let mut body = [0_u8; FTM_INITIAL_REQUEST_BODY_LEN];
        encode_initial_request(self.config.parameters, &mut body)
            .expect("validated FTM parameters always fit the fixed Action body");
        let attempt = self.config.request_attempt_limit - attempts_remaining + 1;
        let attempts_remaining = attempts_remaining - 1;
        let transmission = FtmRequestTransmission {
            session_generation: self.session_generation,
            transmission_generation: self.transmission_generation,
            attempt,
            body,
        };
        self.phase = FtmRequesterPhase::RequestTransmitting {
            transmission,
            attempts_remaining,
        };
        Ok(FtmRequesterService::Transmit(transmission))
    }

    pub fn complete_transmission(
        &mut self,
        transmission: FtmRequestTransmission,
        acknowledged: bool,
        now_micros: u64,
    ) -> Result<FtmRequesterEvent, FtmRequesterError> {
        let attempts_remaining = self.validate_exact_pending_transmission(&transmission)?;
        if acknowledged {
            let response_deadline_micros = now_micros
                .checked_add(u64::from(self.config.response_timeout_micros))
                .ok_or(FtmRequesterError::DeadlineOverflow)?;
            let session_deadline_micros = now_micros
                .checked_add(u64::from(self.config.session_timeout_micros))
                .ok_or(FtmRequesterError::DeadlineOverflow)?;
            self.phase = FtmRequesterPhase::AwaitingInitial {
                attempts_remaining,
                response_deadline_micros,
                session_deadline_micros,
            };
            Ok(FtmRequesterEvent::RequestPublished {
                response_deadline_micros,
                session_deadline_micros,
            })
        } else if attempts_remaining == 0 {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::RequestTxFailed);
            Ok(FtmRequesterEvent::Failed(
                FtmSessionFailure::RequestTxFailed,
            ))
        } else {
            let retry_at_micros = now_micros
                .checked_add(u64::from(self.config.retry_interval_micros))
                .ok_or(FtmRequesterError::DeadlineOverflow)?;
            self.phase = FtmRequesterPhase::RequestQueued {
                ready_at_micros: retry_at_micros,
                attempts_remaining,
            };
            Ok(FtmRequesterEvent::RequestRetryScheduled { retry_at_micros })
        }
    }

    /// Reject a request before DMA/sequence publication at a chip admission
    /// boundary. The transmission identity is still checked to prevent a stale
    /// hardware result from ending a later session.
    pub fn reject_hardware_admission(
        &mut self,
        transmission: FtmRequestTransmission,
    ) -> Result<FtmRequesterEvent, FtmRequesterError> {
        self.validate_exact_pending_transmission(&transmission)?;
        self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::HardwareAdmissionRejected);
        Ok(FtmRequesterEvent::Failed(
            FtmSessionFailure::HardwareAdmissionRejected,
        ))
    }

    /// Validate the complete requester-owned value without consuming phase.
    ///
    /// Both completion paths use this one check so generation fields cannot
    /// authorize a different encoded Action body. A mismatch leaves the exact
    /// pending transmission available for its eventual valid completion.
    fn validate_exact_pending_transmission(
        &self,
        candidate: &FtmRequestTransmission,
    ) -> Result<u8, FtmRequesterError> {
        let FtmRequesterPhase::RequestTransmitting {
            transmission,
            attempts_remaining,
        } = self.phase
        else {
            return Err(FtmRequesterError::StaleTransmission);
        };
        if transmission != *candidate {
            return Err(FtmRequesterError::StaleTransmission);
        }
        Ok(attempts_remaining)
    }

    pub fn on_measurement(
        &mut self,
        peer: [u8; 6],
        measurement: FtmMeasurement<'_>,
        local_timing: Option<FtmLocalExchangeTiming>,
        now_micros: u64,
    ) -> Result<FtmMeasurementDisposition, FtmRequesterError> {
        if peer != self.peer {
            return Ok(FtmMeasurementDisposition::ForeignPeer);
        }
        let session_deadline_micros = match self.phase {
            FtmRequesterPhase::AwaitingInitial {
                session_deadline_micros,
                ..
            }
            | FtmRequesterPhase::Measuring {
                session_deadline_micros,
                ..
            } => session_deadline_micros,
            FtmRequesterPhase::Idle => return Err(FtmRequesterError::NoActiveSession),
            FtmRequesterPhase::Complete { .. } | FtmRequesterPhase::Failed(_) => {
                return Ok(FtmMeasurementDisposition::Stale);
            }
            FtmRequesterPhase::RequestQueued { .. }
            | FtmRequesterPhase::RequestTransmitting { .. } => {
                return Ok(FtmMeasurementDisposition::Stale);
            }
        };
        if now_micros >= session_deadline_micros {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::SessionTimedOut);
            return Ok(FtmMeasurementDisposition::Failed(
                FtmSessionFailure::SessionTimedOut,
            ));
        }
        if let Some(last) = self.last_measurement
            && last.fields.dialog_token == measurement.fields.dialog_token
        {
            if last.fields == measurement.fields
                && last.parameters == measurement.parameters
                && last
                    .information_elements
                    .matches(measurement.information_elements)
                && local_timing == self.pending
            {
                return Ok(FtmMeasurementDisposition::Duplicate);
            }
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::ConflictingDuplicate);
        }

        if matches!(self.phase, FtmRequesterPhase::AwaitingInitial { .. }) {
            return self.accept_initial(measurement, local_timing, session_deadline_micros);
        }
        self.accept_follow_up(measurement, local_timing)
    }

    fn accept_initial(
        &mut self,
        measurement: FtmMeasurement<'_>,
        local_timing: Option<FtmLocalExchangeTiming>,
        session_deadline_micros: u64,
    ) -> Result<FtmMeasurementDisposition, FtmRequesterError> {
        if measurement.fields.follow_up_dialog_token != 0 {
            return Ok(FtmMeasurementDisposition::Stale);
        }
        let Some(negotiated) = measurement.parameters else {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::MissingInitialParameters);
        };
        match negotiated.status {
            FtmResponseStatus::Incapable => {
                self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::PeerIncapable);
                return Ok(FtmMeasurementDisposition::Failed(
                    FtmSessionFailure::PeerIncapable,
                ));
            }
            FtmResponseStatus::Failed {
                retry_after_seconds,
            } => {
                let failure = FtmSessionFailure::PeerFailed {
                    retry_after_seconds,
                };
                self.phase = FtmRequesterPhase::Failed(failure);
                return Ok(FtmMeasurementDisposition::Failed(failure));
            }
            FtmResponseStatus::Success => {}
        }
        self.validate_negotiation(negotiated)?;
        let information_elements =
            self.own_information_elements(measurement.information_elements)?;
        if measurement.fields.dialog_token == 0 {
            if local_timing.is_some() {
                return self.protocol_error(FtmRequesterError::UnexpectedLocalTiming);
            }
            self.pending = None;
            self.last_measurement = Some(LastMeasurement {
                fields: measurement.fields,
                parameters: measurement.parameters,
                information_elements,
            });
            self.phase = FtmRequesterPhase::Complete {
                negotiated,
                termination: FtmSessionTermination::PeerTerminalFrame,
            };
            return Ok(FtmMeasurementDisposition::Complete { samples: 0 });
        }
        if negotiated.ftms_per_burst == 1 {
            return self.protocol_error(FtmRequesterError::TooManyMeasurements);
        }
        let local_timing = self.validate_new_local_timing(measurement, local_timing)?;
        self.remember_dialog_token(measurement.fields.dialog_token);
        self.pending = Some(local_timing);
        self.last_measurement = Some(LastMeasurement {
            fields: measurement.fields,
            parameters: measurement.parameters,
            information_elements,
        });
        self.phase = FtmRequesterPhase::Measuring {
            session_deadline_micros,
            negotiated,
        };
        Ok(FtmMeasurementDisposition::InitialAccepted {
            dialog_token: measurement.fields.dialog_token,
            allocated_ftms_per_burst: negotiated.ftms_per_burst,
            maximum_samples: negotiated.ftms_per_burst - 1,
        })
    }

    fn accept_follow_up(
        &mut self,
        measurement: FtmMeasurement<'_>,
        local_timing: Option<FtmLocalExchangeTiming>,
    ) -> Result<FtmMeasurementDisposition, FtmRequesterError> {
        let FtmRequesterPhase::Measuring { negotiated, .. } = self.phase else {
            return Err(FtmRequesterError::NoActiveSession);
        };
        if measurement.parameters.is_some() {
            return self.accept_initial_retransmission(measurement, local_timing, negotiated);
        }
        if !measurement.information_elements.is_empty() {
            return self.protocol_error(FtmRequesterError::UnexpectedInformationElements);
        }
        let Some(pending) = self.pending else {
            return self.protocol_error(FtmRequesterError::MissingLocalTiming);
        };

        if measurement.fields.dialog_token == 0 && measurement.fields.follow_up_dialog_token == 0 {
            if local_timing.is_some() {
                return self.protocol_error(FtmRequesterError::UnexpectedLocalTiming);
            }
            self.complete_with_terminal(measurement, negotiated);
            return Ok(FtmMeasurementDisposition::Complete {
                samples: self.sample_count,
            });
        }

        let sample = if measurement.fields.follow_up_dialog_token == pending.dialog_token {
            Some(FtmRawExchange {
                dialog_token: pending.dialog_token,
                responder_departure_t1_ps: measurement.fields.tod,
                initiator_arrival_t2_ps: pending.arrival_t2_ps,
                initiator_ack_departure_t3_ps: pending.ack_departure_t3_ps,
                responder_ack_arrival_t4_ps: measurement.fields.toa,
                tod_error: measurement.fields.tod_error,
                toa_error: measurement.fields.toa_error,
            })
        } else if self.is_last_body_retransmission(measurement) {
            None
        } else {
            return Ok(FtmMeasurementDisposition::Stale);
        };

        if measurement.fields.dialog_token == 0 {
            if local_timing.is_some() {
                return self.protocol_error(FtmRequesterError::UnexpectedLocalTiming);
            }
            let Some(sample) = sample else {
                return Ok(FtmMeasurementDisposition::Stale);
            };
            if self.sample_count >= usize::from(negotiated.ftms_per_burst - 1) {
                return self.protocol_error(FtmRequesterError::TooManyMeasurements);
            }
            self.push_sample(sample)?;
            self.complete_with_terminal(measurement, negotiated);
            return Ok(FtmMeasurementDisposition::Complete {
                samples: self.sample_count,
            });
        }

        self.validate_next_dialog_token(pending.dialog_token, measurement.fields.dialog_token)?;
        let local_timing = self.validate_new_local_timing(measurement, local_timing)?;
        let maximum_samples = usize::from(negotiated.ftms_per_burst - 1);
        let disposition = if let Some(sample) = sample {
            if self.sample_count + 1 >= maximum_samples {
                return self.protocol_error(FtmRequesterError::TooManyMeasurements);
            }
            let sample_index = self.push_sample(sample)?;
            FtmMeasurementDisposition::SampleAccepted {
                dialog_token: pending.dialog_token,
                sample_index,
            }
        } else {
            if self.sample_count >= maximum_samples {
                return self.protocol_error(FtmRequesterError::TooManyMeasurements);
            }
            FtmMeasurementDisposition::DuplicateSample {
                dialog_token: measurement.fields.follow_up_dialog_token,
            }
        };
        self.remember_dialog_token(measurement.fields.dialog_token);
        self.pending = Some(local_timing);
        self.last_measurement = Some(LastMeasurement {
            fields: measurement.fields,
            parameters: None,
            information_elements: OwnedInformationElements::new(&[])
                .expect("an empty information-element sequence fits"),
        });
        Ok(disposition)
    }

    fn accept_initial_retransmission(
        &mut self,
        measurement: FtmMeasurement<'_>,
        local_timing: Option<FtmLocalExchangeTiming>,
        negotiated: FtmResponseParameters,
    ) -> Result<FtmMeasurementDisposition, FtmRequesterError> {
        let Some(pending) = self.pending else {
            return self.protocol_error(FtmRequesterError::MissingLocalTiming);
        };
        let Some(last) = self.last_measurement else {
            return self.protocol_error(FtmRequesterError::InitialRetransmissionMismatch);
        };
        if measurement.fields.dialog_token == 0
            || measurement.fields.follow_up_dialog_token != 0
            || measurement.parameters != Some(negotiated)
            || last.parameters != Some(negotiated)
            || last.fields.follow_up_dialog_token != 0
            || !last
                .information_elements
                .matches(measurement.information_elements)
        {
            return self.protocol_error(FtmRequesterError::InitialRetransmissionMismatch);
        }
        self.validate_next_dialog_token(pending.dialog_token, measurement.fields.dialog_token)?;
        let local_timing = self.validate_new_local_timing(measurement, local_timing)?;
        let information_elements =
            self.own_information_elements(measurement.information_elements)?;
        self.remember_dialog_token(measurement.fields.dialog_token);
        self.pending = Some(local_timing);
        self.last_measurement = Some(LastMeasurement {
            fields: measurement.fields,
            parameters: measurement.parameters,
            information_elements,
        });
        Ok(FtmMeasurementDisposition::InitialRetransmissionAccepted {
            abandoned_dialog_token: pending.dialog_token,
            dialog_token: measurement.fields.dialog_token,
        })
    }

    fn complete_with_terminal(
        &mut self,
        measurement: FtmMeasurement<'_>,
        negotiated: FtmResponseParameters,
    ) {
        self.pending = None;
        self.last_measurement = Some(LastMeasurement {
            fields: measurement.fields,
            parameters: measurement.parameters,
            information_elements: OwnedInformationElements::new(measurement.information_elements)
                .expect("validated terminal information elements fit"),
        });
        self.phase = FtmRequesterPhase::Complete {
            negotiated,
            termination: FtmSessionTermination::PeerTerminalFrame,
        };
    }

    fn validate_negotiation(
        &mut self,
        negotiated: FtmResponseParameters,
    ) -> Result<(), FtmRequesterError> {
        let requested = self.config.parameters;
        let request_format = requested.format_and_bandwidth();
        let format_allowed = matches!(
            negotiated.format_and_bandwidth,
            FtmFormatAndBandwidth::NonHt20Mhz | FtmFormatAndBandwidth::HtMixed20Mhz
        ) && (request_format == FtmFormatAndBandwidth::NoPreference
            || request_format == negotiated.format_and_bandwidth);
        if negotiated.number_of_bursts_exponent != 0
            || !negotiated.asap
            || !negotiated.asap_capable
            || negotiated.ftms_per_burst == 0
            || negotiated.ftms_per_burst > requested.ftms_per_burst()
            || usize::from(negotiated.ftms_per_burst - 1) > MAX_SAMPLES
            || negotiated.min_delta_ftm_100us < requested.min_delta_ftm_100us()
            || negotiated.burst_duration == FtmBurstDuration::NoPreference
            || negotiated.burst_duration.wire() > requested.burst_duration().wire()
            || negotiated.burst_period_100ms != 0
            || !format_allowed
        {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::UnsupportedNegotiation);
        }
        Ok(())
    }

    fn validate_next_dialog_token(
        &mut self,
        previous: u8,
        actual: u8,
    ) -> Result<(), FtmRequesterError> {
        if self.token_seen(actual) {
            return self.protocol_error(FtmRequesterError::DialogTokenReused);
        }
        let expected = if previous == u8::MAX { 1 } else { previous + 1 };
        if actual != expected {
            return self
                .protocol_error(FtmRequesterError::DialogTokenOutOfSequence { expected, actual });
        }
        Ok(())
    }

    fn own_information_elements(
        &mut self,
        bytes: &[u8],
    ) -> Result<OwnedInformationElements, FtmRequesterError> {
        match OwnedInformationElements::new(bytes) {
            Ok(elements) => Ok(elements),
            Err(error) => self.protocol_error(error),
        }
    }

    fn protocol_error<T>(&mut self, error: FtmRequesterError) -> Result<T, FtmRequesterError> {
        self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
        Err(error)
    }

    fn validate_new_local_timing(
        &mut self,
        measurement: FtmMeasurement<'_>,
        local_timing: Option<FtmLocalExchangeTiming>,
    ) -> Result<FtmLocalExchangeTiming, FtmRequesterError> {
        if measurement.fields.dialog_token == 0 {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::ZeroLocalDialogToken);
        }
        let Some(local_timing) = local_timing else {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::MissingLocalTiming);
        };
        if local_timing.dialog_token != measurement.fields.dialog_token {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::LocalDialogTokenMismatch);
        }
        Ok(local_timing)
    }

    fn push_sample(&mut self, sample: FtmRawExchange) -> Result<usize, FtmRequesterError> {
        if self.sample_count >= MAX_SAMPLES {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::TooManyMeasurements);
        }
        let index = self.sample_count;
        self.samples[index] = Some(sample);
        self.sample_count += 1;
        Ok(index)
    }

    fn is_last_body_retransmission(&self, measurement: FtmMeasurement<'_>) -> bool {
        self.last_measurement.is_some_and(|last| {
            last.parameters.is_none()
                && last.information_elements.matches(&[])
                && last.fields.follow_up_dialog_token == measurement.fields.follow_up_dialog_token
                && last.fields.tod == measurement.fields.tod
                && last.fields.toa == measurement.fields.toa
                && last.fields.tod_error == measurement.fields.tod_error
                && last.fields.toa_error == measurement.fields.toa_error
        })
    }

    fn token_seen(&self, token: u8) -> bool {
        if token == 0 {
            return false;
        }
        let index = usize::from(token) / 32;
        let bit = u32::from(token) % 32;
        self.dialog_tokens[index] & (1_u32 << bit) != 0
    }

    fn remember_dialog_token(&mut self, token: u8) {
        debug_assert_ne!(token, 0);
        let index = usize::from(token) / 32;
        let bit = u32::from(token) % 32;
        self.dialog_tokens[index] |= 1_u32 << bit;
    }

    fn fail(&mut self, failure: FtmSessionFailure) -> FtmRequesterService {
        self.phase = FtmRequesterPhase::Failed(failure);
        FtmRequesterService::Event(FtmRequesterEvent::Failed(failure))
    }

    pub fn take_result(&mut self) -> Result<FtmSessionResult<MAX_SAMPLES>, FtmRequesterError> {
        let FtmRequesterPhase::Complete {
            negotiated,
            termination,
        } = self.phase
        else {
            return Err(FtmRequesterError::ResultNotReady);
        };
        let result = FtmSessionResult {
            peer: self.peer,
            session_generation: self.session_generation,
            negotiated,
            termination,
            samples: self.samples,
            sample_count: self.sample_count,
        };
        self.reset_to_idle();
        Ok(result)
    }

    pub fn take_failure(&mut self) -> Result<FtmSessionFailure, FtmRequesterError> {
        let FtmRequesterPhase::Failed(failure) = self.phase else {
            return Err(FtmRequesterError::FailureNotReady);
        };
        self.reset_to_idle();
        Ok(failure)
    }

    fn reset_to_idle(&mut self) {
        self.phase = FtmRequesterPhase::Idle;
        self.pending = None;
        self.last_measurement = None;
        self.samples.fill(None);
        self.sample_count = 0;
        self.dialog_tokens.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_ieee80211::ftm::{
        FTM_MEASUREMENT_PREFIX_LEN, FTM_PARAMETERS_ELEMENT_LEN, FtmMeasurementFields,
        encode_measurement,
    };

    fn ps(value: u64) -> FtmTimestampPs {
        FtmTimestampPs::new(value).unwrap()
    }

    fn request_parameters(count: u8) -> FtmRequestParameters {
        FtmRequestParameters::new(
            0,
            FtmBurstDuration::Millis8,
            2,
            None,
            true,
            count,
            FtmFormatAndBandwidth::HtMixed20Mhz,
            0,
        )
        .unwrap()
    }

    fn config(count: u8, attempts: u8) -> FtmRequesterConfig {
        FtmRequesterConfig::new(request_parameters(count), 1_000, 100, 10_000, attempts).unwrap()
    }

    fn response(count: u8) -> FtmResponseParameters {
        FtmResponseParameters {
            status: FtmResponseStatus::Success,
            number_of_bursts_exponent: 0,
            burst_duration: FtmBurstDuration::Millis8,
            min_delta_ftm_100us: 2,
            partial_tsf_timer: 10,
            asap_capable: true,
            asap: true,
            ftms_per_burst: count,
            format_and_bandwidth: FtmFormatAndBandwidth::HtMixed20Mhz,
            burst_period_100ms: 0,
        }
    }

    fn measurement_body<const N: usize>(
        fields: FtmMeasurementFields,
        parameters: Option<FtmResponseParameters>,
    ) -> [u8; N] {
        let mut body = [0_u8; N];
        let element = parameters.map(|parameters| parameters.encode_element().unwrap());
        let information_elements = element.as_ref().map_or(&[][..], |element| &element[..]);
        assert_eq!(
            encode_measurement(fields, information_elements, &mut body),
            Ok(N)
        );
        body
    }

    fn initial_body(
        token: u8,
        count: u8,
    ) -> [u8; FTM_MEASUREMENT_PREFIX_LEN + FTM_PARAMETERS_ELEMENT_LEN] {
        measurement_body(
            FtmMeasurementFields {
                dialog_token: token,
                follow_up_dialog_token: 0,
                tod: FtmTimestampPs::ZERO,
                toa: FtmTimestampPs::ZERO,
                tod_error: FtmTodError {
                    max_error_exponent: 0,
                    not_continuous: false,
                },
                toa_error: FtmToaError {
                    max_error_exponent: 0,
                },
            },
            Some(response(count)),
        )
    }

    fn follow_up_body(
        dialog_token: u8,
        follow_up_dialog_token: u8,
        t1: u64,
        t4: u64,
    ) -> [u8; FTM_MEASUREMENT_PREFIX_LEN] {
        measurement_body(
            FtmMeasurementFields {
                dialog_token,
                follow_up_dialog_token,
                tod: ps(t1),
                toa: ps(t4),
                tod_error: FtmTodError {
                    max_error_exponent: 2,
                    not_continuous: false,
                },
                toa_error: FtmToaError {
                    max_error_exponent: 3,
                },
            },
            None,
        )
    }

    fn start_published<const N: usize>(requester: &mut FtmRequester<N>, peer: [u8; 6]) {
        requester.start(peer, 10).unwrap();
        let FtmRequesterService::Transmit(transmission) = requester.service(10).unwrap() else {
            panic!("request must be ready")
        };
        assert_eq!(transmission.body()[..3], [4, 32, 1]);
        requester
            .complete_transmission(transmission, true, 20)
            .unwrap();
    }

    #[test]
    fn config_rejects_unbounded_or_scheduled_profiles() {
        let multiple = FtmRequestParameters::new(
            1,
            FtmBurstDuration::Millis8,
            2,
            None,
            true,
            4,
            FtmFormatAndBandwidth::HtMixed20Mhz,
            1,
        )
        .unwrap();
        assert_eq!(
            FtmRequesterConfig::new(multiple, 1, 1, 1, 1),
            Err(FtmRequesterConfigError::MultipleBurstsUnsupported)
        );
        let unbounded = FtmRequestParameters::new(
            0,
            FtmBurstDuration::Millis8,
            2,
            None,
            true,
            0,
            FtmFormatAndBandwidth::HtMixed20Mhz,
            0,
        )
        .unwrap();
        assert_eq!(
            FtmRequesterConfig::new(unbounded, 1, 1, 1, 1),
            Err(FtmRequesterConfigError::NoMeasurementCountPreferenceUnsupported)
        );
    }

    #[test]
    fn request_retries_have_affine_transmission_identity() {
        let mut requester = FtmRequester::<2>::new(config(2, 2));
        requester.start([1; 6], 0).unwrap();
        let FtmRequesterService::Transmit(first) = requester.service(0).unwrap() else {
            panic!()
        };
        assert_eq!(
            requester.complete_transmission(first, false, 10),
            Ok(FtmRequesterEvent::RequestRetryScheduled {
                retry_at_micros: 110
            })
        );
        assert_eq!(requester.service(109), Ok(FtmRequesterService::Idle));
        let FtmRequesterService::Transmit(second) = requester.service(110).unwrap() else {
            panic!()
        };
        assert_ne!(
            first.transmission_generation(),
            second.transmission_generation()
        );
        assert_eq!(
            requester.complete_transmission(first, true, 111),
            Err(FtmRequesterError::StaleTransmission)
        );
        requester.complete_transmission(second, true, 111).unwrap();
        assert_eq!(requester.next_deadline_micros(), Some(1_111));
    }

    #[test]
    fn mutated_body_cannot_complete_or_reject_the_exact_pending_transmission() {
        let mut completion = FtmRequester::<1>::new(config(2, 1));
        completion.start([1; 6], 0).unwrap();
        let FtmRequesterService::Transmit(valid_completion) = completion.service(0).unwrap() else {
            panic!()
        };
        let mut mutated_completion = valid_completion;
        mutated_completion.body[2] = 0;
        assert_eq!(
            completion.complete_transmission(mutated_completion, true, 10),
            Err(FtmRequesterError::StaleTransmission)
        );
        assert_eq!(
            completion.complete_transmission(valid_completion, true, 10),
            Ok(FtmRequesterEvent::RequestPublished {
                response_deadline_micros: 1_010,
                session_deadline_micros: 10_010,
            })
        );

        let mut admission = FtmRequester::<1>::new(config(2, 1));
        admission.start([2; 6], 20).unwrap();
        let FtmRequesterService::Transmit(valid_admission) = admission.service(20).unwrap() else {
            panic!()
        };
        let mut mutated_admission = valid_admission;
        mutated_admission.body[0] = 0;
        assert_eq!(
            admission.reject_hardware_admission(mutated_admission),
            Err(FtmRequesterError::StaleTransmission)
        );
        assert_eq!(
            admission.reject_hardware_admission(valid_admission),
            Ok(FtmRequesterEvent::Failed(
                FtmSessionFailure::HardwareAdmissionRejected
            ))
        );
    }

    #[test]
    fn three_ftm_frames_deliver_two_owned_samples_without_claiming_distance() {
        let peer = [0x22; 6];
        let mut requester = FtmRequester::<2>::new(config(3, 1));
        start_published(&mut requester, peer);

        let initial = initial_body(1, 3);
        assert_eq!(
            requester.on_measurement(
                peer,
                FtmMeasurement::decode_body(&initial).unwrap(),
                Some(FtmLocalExchangeTiming::new(1, ps(1_100), ps(1_200)).unwrap()),
                30,
            ),
            Ok(FtmMeasurementDisposition::InitialAccepted {
                dialog_token: 1,
                allocated_ftms_per_burst: 3,
                maximum_samples: 2,
            })
        );

        let follow_up = follow_up_body(2, 1, 1_000, 1_400);
        assert_eq!(
            requester.on_measurement(
                peer,
                FtmMeasurement::decode_body(&follow_up).unwrap(),
                Some(FtmLocalExchangeTiming::new(2, ps(1_500), ps(1_600)).unwrap()),
                40,
            ),
            Ok(FtmMeasurementDisposition::SampleAccepted {
                dialog_token: 1,
                sample_index: 0,
            })
        );
        let terminal = follow_up_body(0, 2, 1_400, 1_800);
        assert_eq!(
            requester.on_measurement(
                peer,
                FtmMeasurement::decode_body(&terminal).unwrap(),
                None,
                50,
            ),
            Ok(FtmMeasurementDisposition::Complete { samples: 2 })
        );

        let result = requester.take_result().unwrap();
        assert_eq!(result.peer, peer);
        assert_eq!(result.sample_count(), 2);
        assert_eq!(
            result.sample(0).unwrap().raw_interval_difference(),
            Ok(FtmRawIntervalDifferencePs {
                responder_round_trip_ps: 400,
                initiator_turnaround_ps: 100,
                difference_ps: 300,
            })
        );
        assert_eq!(
            result.sample(1).unwrap().raw_interval_difference(),
            Ok(FtmRawIntervalDifferencePs {
                responder_round_trip_ps: 400,
                initiator_turnaround_ps: 100,
                difference_ps: 300,
            })
        );
        assert!(requester.is_idle());
    }

    #[test]
    fn ftm_retransmission_deduplicates_follow_up_but_owns_new_token() {
        let peer = [3; 6];
        let mut requester = FtmRequester::<2>::new(config(3, 1));
        start_published(&mut requester, peer);
        let initial = initial_body(1, 3);
        requester
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&initial).unwrap(),
                Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
                30,
            )
            .unwrap();
        let measured = follow_up_body(2, 1, 80, 160);
        requester
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&measured).unwrap(),
                Some(FtmLocalExchangeTiming::new(2, ps(200), ps(220)).unwrap()),
                40,
            )
            .unwrap();
        let retry = follow_up_body(3, 1, 80, 160);
        assert_eq!(
            requester.on_measurement(
                peer,
                FtmMeasurement::decode_body(&retry).unwrap(),
                Some(FtmLocalExchangeTiming::new(3, ps(300), ps(320)).unwrap()),
                50,
            ),
            Ok(FtmMeasurementDisposition::DuplicateSample { dialog_token: 1 })
        );
        let terminal = follow_up_body(0, 3, 280, 360);
        requester
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&terminal).unwrap(),
                None,
                60,
            )
            .unwrap();
        let result = requester.take_result().unwrap();
        assert_eq!(result.sample_count(), 2);
        assert!(result.sample(0).is_some());
        assert_eq!(result.sample(1).unwrap().dialog_token, 3);
    }

    #[test]
    fn abandoned_retransmission_token_cannot_be_reused() {
        let peer = [4; 6];
        let mut requester = FtmRequester::<3>::new(config(4, 1));
        start_published(&mut requester, peer);
        let initial = initial_body(1, 4);
        requester
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&initial).unwrap(),
                Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
                30,
            )
            .unwrap();
        let measured = follow_up_body(2, 1, 80, 160);
        requester
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&measured).unwrap(),
                Some(FtmLocalExchangeTiming::new(2, ps(200), ps(220)).unwrap()),
                40,
            )
            .unwrap();
        let retry = follow_up_body(3, 1, 80, 160);
        requester
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&retry).unwrap(),
                Some(FtmLocalExchangeTiming::new(3, ps(300), ps(320)).unwrap()),
                50,
            )
            .unwrap();
        let reused = follow_up_body(2, 3, 280, 360);
        assert_eq!(
            requester.on_measurement(
                peer,
                FtmMeasurement::decode_body(&reused).unwrap(),
                Some(FtmLocalExchangeTiming::new(2, ps(400), ps(420)).unwrap()),
                60,
            ),
            Err(FtmRequesterError::DialogTokenReused)
        );
        assert_eq!(
            requester.failure(),
            Some(FtmSessionFailure::ProtocolViolation)
        );
    }

    #[test]
    fn capacity_and_hardware_admission_fail_before_publication() {
        let mut too_small = FtmRequester::<1>::new(config(2, 1));
        assert_eq!(too_small.start([1; 6], 0), Ok(1));
        assert!(matches!(
            too_small.service(0),
            Ok(FtmRequesterService::Transmit(_))
        ));

        let mut too_small = FtmRequester::<1>::new(config(3, 1));
        assert_eq!(
            too_small.start([1; 6], 0),
            Err(FtmRequesterError::CapacityTooSmall {
                required_samples: 2,
                capacity: 1,
            })
        );

        let mut requester = FtmRequester::<2>::new(config(3, 1));
        requester.start([1; 6], 0).unwrap();
        let FtmRequesterService::Transmit(transmission) = requester.service(0).unwrap() else {
            panic!()
        };
        assert_eq!(
            requester.reject_hardware_admission(transmission),
            Ok(FtmRequesterEvent::Failed(
                FtmSessionFailure::HardwareAdmissionRejected
            ))
        );
        assert_eq!(
            requester.take_failure(),
            Ok(FtmSessionFailure::HardwareAdmissionRejected)
        );
    }

    #[test]
    fn one_ftm_frame_is_a_terminal_initial_with_zero_samples() {
        let peer = [5; 6];
        let mut requester = FtmRequester::<0>::new(config(1, 1));
        start_published(&mut requester, peer);
        let terminal_initial = initial_body(0, 1);
        assert_eq!(
            requester.on_measurement(
                peer,
                FtmMeasurement::decode_body(&terminal_initial).unwrap(),
                None,
                30,
            ),
            Ok(FtmMeasurementDisposition::Complete { samples: 0 })
        );
        let result = requester.take_result().unwrap();
        assert_eq!(result.negotiated.ftms_per_burst, 1);
        assert_eq!(result.sample_count(), 0);

        let mut nonterminal = FtmRequester::<0>::new(config(1, 1));
        start_published(&mut nonterminal, peer);
        let invalid = initial_body(1, 1);
        assert_eq!(
            nonterminal.on_measurement(
                peer,
                FtmMeasurement::decode_body(&invalid).unwrap(),
                Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
                30,
            ),
            Err(FtmRequesterError::TooManyMeasurements)
        );
        assert_eq!(nonterminal.sample_count, 0);
    }

    #[test]
    fn initial_retransmission_replaces_timing_and_tokens_wrap_consecutively() {
        let peer = [6; 6];
        let mut requester = FtmRequester::<2>::new(config(3, 1));
        start_published(&mut requester, peer);
        let initial = initial_body(254, 3);
        requester
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&initial).unwrap(),
                Some(FtmLocalExchangeTiming::new(254, ps(100), ps(120)).unwrap()),
                30,
            )
            .unwrap();

        let retransmission = initial_body(255, 3);
        assert_eq!(
            requester.on_measurement(
                peer,
                FtmMeasurement::decode_body(&retransmission).unwrap(),
                Some(FtmLocalExchangeTiming::new(255, ps(200), ps(220)).unwrap()),
                40,
            ),
            Ok(FtmMeasurementDisposition::InitialRetransmissionAccepted {
                abandoned_dialog_token: 254,
                dialog_token: 255,
            })
        );

        let follow_up = follow_up_body(1, 255, 180, 260);
        assert_eq!(
            requester.on_measurement(
                peer,
                FtmMeasurement::decode_body(&follow_up).unwrap(),
                Some(FtmLocalExchangeTiming::new(1, ps(300), ps(320)).unwrap()),
                50,
            ),
            Ok(FtmMeasurementDisposition::SampleAccepted {
                dialog_token: 255,
                sample_index: 0,
            })
        );
        let terminal = follow_up_body(0, 1, 280, 360);
        requester
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&terminal).unwrap(),
                None,
                60,
            )
            .unwrap();
        let result = requester.take_result().unwrap();
        assert_eq!(result.sample_count(), 2);
        assert_eq!(result.sample(0).unwrap().dialog_token, 255);
        assert_eq!(result.sample(0).unwrap().initiator_arrival_t2_ps, ps(200));
    }

    #[test]
    fn nonconsecutive_dialog_token_fails_before_sample_mutation() {
        let peer = [7; 6];
        let mut requester = FtmRequester::<2>::new(config(3, 1));
        start_published(&mut requester, peer);
        let initial = initial_body(1, 3);
        requester
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&initial).unwrap(),
                Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
                30,
            )
            .unwrap();
        let skipped = follow_up_body(3, 1, 80, 160);
        assert_eq!(
            requester.on_measurement(
                peer,
                FtmMeasurement::decode_body(&skipped).unwrap(),
                Some(FtmLocalExchangeTiming::new(3, ps(200), ps(220)).unwrap()),
                40,
            ),
            Err(FtmRequesterError::DialogTokenOutOfSequence {
                expected: 2,
                actual: 3,
            })
        );
        assert_eq!(requester.sample_count, 0);
        assert_eq!(
            requester.failure(),
            Some(FtmSessionFailure::ProtocolViolation)
        );
    }

    #[test]
    fn initial_retransmission_must_preserve_the_negotiated_body() {
        let peer = [8; 6];
        let mut requester = FtmRequester::<2>::new(config(3, 1));
        start_published(&mut requester, peer);
        let initial = initial_body(1, 3);
        requester
            .on_measurement(
                peer,
                FtmMeasurement::decode_body(&initial).unwrap(),
                Some(FtmLocalExchangeTiming::new(1, ps(100), ps(120)).unwrap()),
                30,
            )
            .unwrap();

        let changed = initial_body(2, 2);
        assert_eq!(
            requester.on_measurement(
                peer,
                FtmMeasurement::decode_body(&changed).unwrap(),
                Some(FtmLocalExchangeTiming::new(2, ps(200), ps(220)).unwrap()),
                40,
            ),
            Err(FtmRequesterError::InitialRetransmissionMismatch)
        );
        assert_eq!(requester.sample_count, 0);
        assert_eq!(
            requester.failure(),
            Some(FtmSessionFailure::ProtocolViolation)
        );
    }

    #[test]
    fn timestamp_wrap_is_bounded_to_half_range() {
        let exchange = FtmRawExchange {
            dialog_token: 1,
            responder_departure_t1_ps: ps(FTM_TIMESTAMP_MASK - 9),
            responder_ack_arrival_t4_ps: ps(20),
            initiator_arrival_t2_ps: ps(100),
            initiator_ack_departure_t3_ps: ps(110),
            tod_error: FtmTodError {
                max_error_exponent: 1,
                not_continuous: false,
            },
            toa_error: FtmToaError {
                max_error_exponent: 1,
            },
        };
        assert_eq!(
            exchange.raw_interval_difference(),
            Ok(FtmRawIntervalDifferencePs {
                responder_round_trip_ps: 30,
                initiator_turnaround_ps: 10,
                difference_ps: 20,
            })
        );
        let ambiguous = FtmRawExchange {
            responder_ack_arrival_t4_ps: ps(FTM_TIMESTAMP_HALF_RANGE),
            responder_departure_t1_ps: ps(0),
            ..exchange
        };
        assert_eq!(
            ambiguous.raw_interval_difference(),
            Err(FtmTimestampArithmeticError::AmbiguousResponderWrap)
        );
    }
}
