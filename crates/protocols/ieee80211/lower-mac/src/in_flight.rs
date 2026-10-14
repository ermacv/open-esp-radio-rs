//! Custody of an accepted TX operation, from admission to its terminal.

use crate::TxOutcome;

/// A backend's execution projection. Its concrete view lends only the data
/// needed to execute the payload: no owning MPDU, body or replaceable owner.
/// The backend keeps the projection's fields private and exposes operations
/// on borrowed storage. In particular, the view must not contain a public
/// `&mut Self` or offer an owner extraction/replacement operation.
pub trait ExecutionPayload {
    /// Narrow execution borrow; it cannot outlive the accepted operation.
    type Execution<'a>
    where
        Self: 'a;
    /// Lend the execution view without releasing ownership of this payload.
    fn execution(&mut self) -> Self::Execution<'_>;
}

/// One accepted operation. Context, ticket and payload are indivisible until
/// a terminal consumes it. There is no mutable access to any of these owners.
/// Queueing, publication, withdrawal and quarantine move this whole value.
pub struct InFlight<C, T, M> {
    context: C,
    ticket: T,
    payload: M,
}

impl<C, T, M> InFlight<C, T, M> {
    /// Accept exactly these owners together at the interface's admission
    /// boundary. Callers assemble an attempt before this point; the backend
    /// enqueues this value and never reconstructs it from a side table.
    pub const fn admit(context: C, ticket: T, payload: M) -> Self {
        Self {
            context,
            ticket,
            payload,
        }
    }

    /// Inspect execution metadata without exposing a replaceable owner.
    pub const fn payload(&self) -> &M {
        &self.payload
    }

    /// Inspect the reserved capacity without releasing or replacing it.
    pub const fn ticket(&self) -> &T {
        &self.ticket
    }

    /// Execute through the backend's narrow storage projection.
    pub fn execute(&mut self) -> M::Execution<'_>
    where
        M: ExecutionPayload,
    {
        self.payload.execution()
    }

    /// End this operation once, returning its entire owner with its outcome.
    pub const fn complete(self, outcome: TxOutcome) -> Completed<C, T, M> {
        Completed {
            operation: self,
            outcome,
        }
    }
}

/// The terminal carries the accepted operation itself. There is no public
/// constructor from independently supplied context, ticket, MPDU or body.
pub struct Completed<C, T, M> {
    operation: InFlight<C, T, M>,
    outcome: TxOutcome,
}

impl<C, T, M> Completed<C, T, M> {
    /// The terminal's observation, without releasing any owner.
    pub const fn outcome(&self) -> TxOutcome {
        self.outcome
    }

    /// The higher layer consumes the terminal before recovering its owners.
    pub fn into_parts(self) -> TerminalParts<C, T, M> {
        let InFlight {
            context,
            ticket,
            payload,
        } = self.operation;
        TerminalParts {
            context,
            ticket,
            payload,
            outcome: self.outcome,
        }
    }
}

/// Owners recovered by consuming a terminal. Their higher layer may now
/// classify, renew, retry or release them according to the returned outcome.
pub struct TerminalParts<C, T, M> {
    /// Original opaque operation context.
    pub context: C,
    /// Original admission and terminal capacity.
    pub ticket: T,
    /// One MPDU or the whole submitted aggregate, including external bodies.
    pub payload: M,
    /// Observation which ended this operation.
    pub outcome: TxOutcome,
}

/// One returned MPDU. A fresh MPDU carries its memory and optional body
/// together; a Sent MPDU retains its body until explicit renewal.
pub trait TxMpdu {
    /// The backend's exhaustive buffer/Sent owner enumeration.
    type Buffer;
    /// External body owner.
    type Body;
    /// Consume a returned or cancelled MPDU, recovering its resources.
    fn into_parts(self) -> (Self::Buffer, Option<Self::Body>);
}
