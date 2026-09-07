//! Chip-private composition of restored lifecycle ownership with portable Reset order.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    LeControllerCommandEndpoint, LeControllerResetBarrier, LeControllerResetCompletion,
    LeControllerResponsePending,
};

pub(crate) struct DtmRestoredReset<'epoch, Owner> {
    barrier: LeControllerResetBarrier<'epoch, Owner>,
}

pub(crate) enum DtmRestoredResetCompletion<'epoch, Owner> {
    ResponsePending(LeControllerResponsePending<'epoch, Owner>),
    EndpointMismatch(DtmRestoredReset<'epoch, Owner>),
}

impl<'epoch, Owner> DtmRestoredReset<'epoch, Owner> {
    pub(crate) const fn new(barrier: LeControllerResetBarrier<'epoch, Owner>) -> Self {
        Self { barrier }
    }

    pub(crate) fn matches_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.barrier.accepts_endpoint(controller)
    }

    pub(crate) fn complete<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> DtmRestoredResetCompletion<'epoch, Owner> {
        match controller.complete_reset_after_quiescence(self.barrier) {
            LeControllerResetCompletion::ResponsePending(pending) => {
                DtmRestoredResetCompletion::ResponsePending(pending)
            }
            LeControllerResetCompletion::EndpointMismatch(barrier) => {
                DtmRestoredResetCompletion::EndpointMismatch(Self { barrier })
            }
        }
    }
}

#[cfg(test)]
mod tests;
