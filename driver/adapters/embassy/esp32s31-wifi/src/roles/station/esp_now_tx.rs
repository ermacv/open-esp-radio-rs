#![expect(
    clippy::result_large_err,
    reason = "bounded TX admission returns the caller-owned 250-byte request"
)]

//! No-allocation application handoff and connected-control scheduling for
//! plaintext ESP-NOW v1/v2 transmit.
//!
//! The application owns copied payloads and observes one terminal completion
//! for every admitted ticket. The scheduler owns peer resolution and the sole
//! ordinary connected TX transaction. It never borrows the WPA2 key slots and
//! never manufactures a PHY rate: the complete typed request is passed to the
//! chip ESP-NOW backend.

use core::{
    cell::RefCell,
    future::Future,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::Mutex as BlockingMutex,
    channel::{Channel, Receiver, Sender, TrySendError},
};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi::esp_now::Esp32s31EspNowTxConfig;
use open_esp_radio_esp32s31_wifi_mac::tx::TxHardware;
use open_esp_radio_esp32s31_wifi_sta::{
    connected_control::ConnectedDisconnectReason,
    single_mpdu_tx::{SingleMpduEspNowTxError, SingleMpduTxError, SingleMpduTxOutcome},
};
use open_esp_radio_ieee80211::{
    channel::WifiChannel,
    esp_now::{
        ESP_NOW_V1_MAX_PAYLOAD_LEN, ESP_NOW_V2_MAX_PAYLOAD_LEN, EspNowRandomValue, EspNowV1Payload,
        EspNowV1WireError, EspNowV2Payload, EspNowV2WireError,
    },
};
use open_esp_radio_wifi_softmac::{EspNowPeerId, EspNowProtocol, interface::BoundVirtualInterface};

use crate::{
    datapath::services::{DatapathControlService, SingleRoleServices},
    datapath::{DatapathControlContext, DatapathControlProgress, WifiTxProgress},
    roles::station::control::{
        ConnectedControlError, ConnectedControlHardware, ConnectedControlShutdown,
        ConnectedControlTimer, ConnectedControlTx, Esp32s31ConnectedControl,
    },
};

/// One fully owned plaintext ESP-NOW v1 application request.
///
/// Construction copies at most 250 bytes. Peer identity and the caller-owned
/// four-byte random value remain attached to that exact payload while it is
/// queued, transmitted, rejected or cancelled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowOwnedV1Tx {
    peer: EspNowPeerId,
    random_value: EspNowRandomValue,
    payload_length: u8,
    payload: [u8; ESP_NOW_V1_MAX_PAYLOAD_LEN],
}

impl EspNowOwnedV1Tx {
    pub fn try_new(
        peer: EspNowPeerId,
        random_value: EspNowRandomValue,
        payload: &[u8],
    ) -> Result<Self, EspNowV1WireError> {
        let payload = EspNowV1Payload::new(payload)?;
        let mut owned = [0; ESP_NOW_V1_MAX_PAYLOAD_LEN];
        owned[..payload.len()].copy_from_slice(payload.bytes());
        Ok(Self {
            peer,
            random_value,
            payload_length: payload.len() as u8,
            payload: owned,
        })
    }

    pub const fn peer(&self) -> EspNowPeerId {
        self.peer
    }

    pub const fn random_value(&self) -> EspNowRandomValue {
        self.random_value
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.payload_length)]
    }
}

/// Generation-fenced identity of one admitted request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowTxTicket {
    epoch: u32,
    request: u32,
}

impl EspNowTxTicket {
    pub const fn epoch(self) -> u32 {
        self.epoch
    }

    pub const fn request(self) -> u32 {
        self.request
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EspNowV2TxLease {
    slot: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EspNowQueuedRequest {
    V1(EspNowOwnedV1Tx),
    V2(EspNowV2TxLease),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EspNowQueuedTx {
    pub(crate) ticket: EspNowTxTicket,
    pub(crate) peer: EspNowPeerId,
    pub(crate) request: EspNowQueuedRequest,
}

#[derive(Clone, Copy)]
struct EspNowV2TxHeader {
    ticket: EspNowTxTicket,
    peer: EspNowPeerId,
    random_value: EspNowRandomValue,
    payload_length: u16,
}

struct EspNowV2TxSlot {
    header: Option<EspNowV2TxHeader>,
    payload: [u8; ESP_NOW_V2_MAX_PAYLOAD_LEN],
}

impl EspNowV2TxSlot {
    const EMPTY: Self = Self {
        header: None,
        payload: [0; ESP_NOW_V2_MAX_PAYLOAD_LEN],
    };
}

/// Borrowed view of an application request while its preallocated slot is
/// synchronously copied into the ordinary TX arena.
pub struct EspNowV2TxRequest<'payload> {
    peer: EspNowPeerId,
    random_value: EspNowRandomValue,
    payload: &'payload [u8],
}

impl EspNowV2TxRequest<'_> {
    pub const fn peer(&self) -> EspNowPeerId {
        self.peer
    }

    pub const fn random_value(&self) -> EspNowRandomValue {
        self.random_value
    }

    pub const fn payload(&self) -> &[u8] {
        self.payload
    }
}

/// Why an admitted request ended without starting a new transmission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxCancelReason {
    StationStopped,
    ConnectionEnded,
    OwnerShutdown,
    StaleEpoch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxRuntimeFailure {
    MissingOrdinaryTxOutcome,
    MissingV2PayloadSlot,
    OffChannel(EspNowOffChannelFailureStage),
    TxLifecycle(SingleMpduTxError),
}

/// Durable terminal stage for an off-channel request whose detailed hardware
/// error remains with the quarantined standalone owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowOffChannelFailureStage {
    QuiesceHomeInterrupts,
    StopHomeReceive,
    SwitchToPeer,
    ActivateTransmitInterrupts,
    QuiesceTransmitInterrupts,
    SwitchHome,
    PrepareHomeReceive,
    StartHomeReceive,
    ActivateHomeInterrupts,
}

/// Exactly one terminal result for an admitted ticket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxTerminal {
    /// The ordinary transaction reached a terminal hardware result. Inspect
    /// `is_success()` or the embedded normalized status to distinguish ACK,
    /// timeout, collision and hardware-failure outcomes.
    Completed(SingleMpduTxOutcome),
    /// Peer/wire admission or the typed chip PHY/publication boundary rejected
    /// the request before a live hardware transaction was created.
    Rejected(SingleMpduEspNowTxError),
    Cancelled(EspNowTxCancelReason),
    RuntimeFailure(EspNowTxRuntimeFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowTxCompletion {
    pub ticket: EspNowTxTicket,
    pub peer: EspNowPeerId,
    pub terminal: EspNowTxTerminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxMailboxEpochError {
    ZeroCapacity,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxBackpressure {
    StaleEpoch,
    QueueFull,
    RequestIdExhausted,
}

/// Failed non-blocking admission, retaining the complete application request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowTxTrySendError {
    pub reason: EspNowTxBackpressure,
    pub request: EspNowOwnedV1Tx,
}

/// Failed v2 admission. The caller's borrowed payload is never consumed;
/// bytes become mailbox-owned only after this call succeeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowV2TxTrySendError {
    Wire(EspNowV2WireError),
    Backpressure(EspNowTxBackpressure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxMailboxInvariantError {
    CompletionQueueFull,
    PublisherInFlight,
    MissingV2PayloadSlot,
}

/// Static request/completion queues, v2 payload slots and reconnect generation.
///
/// Both channels have the same capacity. Admission reserves one completion
/// slot before publishing a request, so stop/reconnect can synchronously emit
/// a terminal cancellation for every accepted ticket without allocation or a
/// completion-overflow policy.
pub struct EspNowTxMailboxResources<M: RawMutex, const CAPACITY: usize> {
    requests: Channel<M, EspNowQueuedTx, CAPACITY>,
    completions: Channel<M, EspNowTxCompletion, CAPACITY>,
    generation: AtomicU32,
    next_request: AtomicU32,
    outstanding: AtomicU32,
    publishers: AtomicU32,
    v2_slots: BlockingMutex<M, RefCell<[EspNowV2TxSlot; CAPACITY]>>,
}

impl<M: RawMutex, const CAPACITY: usize> EspNowTxMailboxResources<M, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            requests: Channel::new(),
            completions: Channel::new(),
            generation: AtomicU32::new(0),
            next_request: AtomicU32::new(0),
            outstanding: AtomicU32::new(0),
            publishers: AtomicU32::new(0),
            v2_slots: BlockingMutex::new(RefCell::new([const { EspNowV2TxSlot::EMPTY }; CAPACITY])),
        }
    }

    /// Start a fresh application/scheduler epoch. The unique mutable borrow
    /// prevents overlapping epochs in safe code; the generation additionally
    /// fences an endpoint retained across an unsafe/static lifecycle bug.
    pub fn begin_epoch(
        &mut self,
    ) -> Result<
        (
            EspNowTxHandle<'_, M, CAPACITY>,
            EspNowTxMailboxOwner<'_, M, CAPACITY>,
        ),
        EspNowTxMailboxEpochError,
    > {
        if CAPACITY == 0 {
            return Err(EspNowTxMailboxEpochError::ZeroCapacity);
        }
        let current = self.generation.load(Ordering::Acquire);
        if current >= u32::MAX - 1 {
            return Err(EspNowTxMailboxEpochError::GenerationExhausted);
        }

        let request_receiver = self.requests.receiver();
        let completion_receiver = self.completions.receiver();
        while request_receiver.try_receive().is_ok() {}
        while completion_receiver.try_receive().is_ok() {}
        for slot in self.v2_slots.get_mut().get_mut() {
            slot.header = None;
        }
        let epoch = current + 1;
        self.next_request.store(0, Ordering::Release);
        self.outstanding.store(0, Ordering::Release);
        self.publishers.store(0, Ordering::Release);
        self.generation.store(epoch, Ordering::Release);

        Ok((
            EspNowTxHandle {
                requests: self.requests.sender(),
                completions: completion_receiver,
                generation: &self.generation,
                next_request: &self.next_request,
                outstanding: &self.outstanding,
                publishers: &self.publishers,
                v2_slots: &self.v2_slots,
                epoch,
            },
            EspNowTxMailboxOwner {
                requests: request_receiver,
                completions: self.completions.sender(),
                generation: &self.generation,
                publishers: &self.publishers,
                v2_slots: &self.v2_slots,
                epoch,
                open: true,
            },
        ))
    }
}

impl<M: RawMutex, const CAPACITY: usize> Default for EspNowTxMailboxResources<M, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Application capability for one connected ESP-NOW TX epoch.
///
/// Both `try_send` and `try_send_v2` are intentionally non-blocking: queue
/// saturation is visible as typed backpressure while an application drains
/// terminal completions.
pub struct EspNowTxHandle<'resources, M: RawMutex, const CAPACITY: usize> {
    requests: Sender<'resources, M, EspNowQueuedTx, CAPACITY>,
    completions: Receiver<'resources, M, EspNowTxCompletion, CAPACITY>,
    generation: &'resources AtomicU32,
    next_request: &'resources AtomicU32,
    outstanding: &'resources AtomicU32,
    publishers: &'resources AtomicU32,
    v2_slots: &'resources BlockingMutex<M, RefCell<[EspNowV2TxSlot; CAPACITY]>>,
    epoch: u32,
}

struct EspNowTxPublicationLease<'resources> {
    publishers: &'resources AtomicU32,
}

impl Drop for EspNowTxPublicationLease<'_> {
    fn drop(&mut self) {
        self.publishers.fetch_sub(1, Ordering::AcqRel);
    }
}

impl<M: RawMutex, const CAPACITY: usize> EspNowTxHandle<'_, M, CAPACITY> {
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn try_send(
        &self,
        request: EspNowOwnedV1Tx,
    ) -> Result<EspNowTxTicket, EspNowTxTrySendError> {
        if self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(EspNowTxTrySendError {
                reason: EspNowTxBackpressure::StaleEpoch,
                request,
            });
        }
        self.publishers.fetch_add(1, Ordering::AcqRel);
        let _publication = EspNowTxPublicationLease {
            publishers: self.publishers,
        };
        // Closing an epoch changes the generation before its final drain. A
        // sender which raced that edge either fails here or remains counted
        // until its synchronous channel publication has finished.
        if self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(EspNowTxTrySendError {
                reason: EspNowTxBackpressure::StaleEpoch,
                request,
            });
        }
        let request_id = self
            .next_request
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map(|previous| previous + 1)
            .map_err(|_| EspNowTxTrySendError {
                reason: EspNowTxBackpressure::RequestIdExhausted,
                request,
            })?;
        if self
            .outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (usize::try_from(current).unwrap_or(usize::MAX) < CAPACITY).then_some(current + 1)
            })
            .is_err()
        {
            return Err(EspNowTxTrySendError {
                reason: EspNowTxBackpressure::QueueFull,
                request,
            });
        }
        let ticket = EspNowTxTicket {
            epoch: self.epoch,
            request: request_id,
        };
        match self.requests.try_send(EspNowQueuedTx {
            ticket,
            peer: request.peer(),
            request: EspNowQueuedRequest::V1(request),
        }) {
            Ok(()) => Ok(ticket),
            Err(TrySendError::Full(queued)) => {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                Err(EspNowTxTrySendError {
                    reason: EspNowTxBackpressure::QueueFull,
                    request: match queued.request {
                        EspNowQueuedRequest::V1(request) => request,
                        EspNowQueuedRequest::V2(_) => {
                            unreachable!("v1 admission cannot publish a v2 lease")
                        }
                    },
                })
            }
        }
    }

    /// Copy and admit one v2 request into a preallocated payload slot.
    ///
    /// The 1470-byte storage never enters an Embassy channel or an async
    /// future; only its generation-fenced slot lease is queued.
    pub fn try_send_v2(
        &self,
        peer: EspNowPeerId,
        random_value: EspNowRandomValue,
        payload: &[u8],
    ) -> Result<EspNowTxTicket, EspNowV2TxTrySendError> {
        let payload = EspNowV2Payload::new(payload).map_err(EspNowV2TxTrySendError::Wire)?;
        if self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(EspNowV2TxTrySendError::Backpressure(
                EspNowTxBackpressure::StaleEpoch,
            ));
        }
        self.publishers.fetch_add(1, Ordering::AcqRel);
        let _publication = EspNowTxPublicationLease {
            publishers: self.publishers,
        };
        if self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(EspNowV2TxTrySendError::Backpressure(
                EspNowTxBackpressure::StaleEpoch,
            ));
        }
        let request_id = self
            .next_request
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map(|previous| previous + 1)
            .map_err(|_| {
                EspNowV2TxTrySendError::Backpressure(EspNowTxBackpressure::RequestIdExhausted)
            })?;
        if self
            .outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (usize::try_from(current).unwrap_or(usize::MAX) < CAPACITY).then_some(current + 1)
            })
            .is_err()
        {
            return Err(EspNowV2TxTrySendError::Backpressure(
                EspNowTxBackpressure::QueueFull,
            ));
        }
        let ticket = EspNowTxTicket {
            epoch: self.epoch,
            request: request_id,
        };
        let slot = self.v2_slots.lock(|slots| {
            let mut slots = slots.borrow_mut();
            let index = slots.iter().position(|slot| slot.header.is_none())?;
            slots[index].payload[..payload.len()].copy_from_slice(payload.bytes());
            slots[index].header = Some(EspNowV2TxHeader {
                ticket,
                peer,
                random_value,
                payload_length: payload.len() as u16,
            });
            Some(index)
        });
        let Some(slot) = slot else {
            self.outstanding.fetch_sub(1, Ordering::AcqRel);
            return Err(EspNowV2TxTrySendError::Backpressure(
                EspNowTxBackpressure::QueueFull,
            ));
        };
        let queued = EspNowQueuedTx {
            ticket,
            peer,
            request: EspNowQueuedRequest::V2(EspNowV2TxLease { slot }),
        };
        match self.requests.try_send(queued) {
            Ok(()) => Ok(ticket),
            Err(TrySendError::Full(queued)) => {
                self.v2_slots.lock(|slots| {
                    slots.borrow_mut()[slot].header = None;
                });
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                let _ = queued;
                Err(EspNowV2TxTrySendError::Backpressure(
                    EspNowTxBackpressure::QueueFull,
                ))
            }
        }
    }

    pub fn try_receive(&self) -> Option<EspNowTxCompletion> {
        loop {
            let completion = self.completions.try_receive().ok()?;
            if completion.ticket.epoch == self.epoch {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                return Some(completion);
            }
        }
    }

    pub async fn receive(&self) -> EspNowTxCompletion {
        loop {
            let completion = self.completions.receive().await;
            if completion.ticket.epoch == self.epoch {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                return completion;
            }
        }
    }

    pub fn outstanding(&self) -> u32 {
        self.outstanding.load(Ordering::Acquire)
    }

    pub fn completion_len(&self) -> usize {
        self.completions.len()
    }
}

/// Scheduler-side endpoint. It can consume accepted requests and publish
/// terminal results, but cannot create application tickets.
pub struct EspNowTxMailboxOwner<'resources, M: RawMutex, const CAPACITY: usize> {
    requests: Receiver<'resources, M, EspNowQueuedTx, CAPACITY>,
    completions: Sender<'resources, M, EspNowTxCompletion, CAPACITY>,
    generation: &'resources AtomicU32,
    publishers: &'resources AtomicU32,
    v2_slots: &'resources BlockingMutex<M, RefCell<[EspNowV2TxSlot; CAPACITY]>>,
    epoch: u32,
    open: bool,
}

impl<M: RawMutex, const CAPACITY: usize> EspNowTxMailboxOwner<'_, M, CAPACITY> {
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    pub const fn is_open(&self) -> bool {
        self.open
    }

    pub fn has_pending(&self) -> bool {
        !self.requests.is_empty()
    }

    pub fn publishers_in_flight(&self) -> u32 {
        self.publishers.load(Ordering::Acquire)
    }

    pub async fn ready(&self) {
        self.requests.ready_to_receive().await;
    }

    pub(crate) fn try_take(&self) -> Option<EspNowQueuedTx> {
        self.requests.try_receive().ok()
    }

    pub(crate) fn with_v2_request<R>(
        &self,
        queued: &EspNowQueuedTx,
        use_request: impl FnOnce(EspNowV2TxRequest<'_>) -> R,
    ) -> Result<R, EspNowTxMailboxInvariantError> {
        let EspNowQueuedRequest::V2(lease) = queued.request else {
            return Err(EspNowTxMailboxInvariantError::MissingV2PayloadSlot);
        };
        self.v2_slots.lock(|slots| {
            let slots = slots.borrow();
            let Some(slot) = slots.get(lease.slot) else {
                return Err(EspNowTxMailboxInvariantError::MissingV2PayloadSlot);
            };
            let Some(header) = slot.header else {
                return Err(EspNowTxMailboxInvariantError::MissingV2PayloadSlot);
            };
            if header.ticket != queued.ticket || header.peer != queued.peer {
                return Err(EspNowTxMailboxInvariantError::MissingV2PayloadSlot);
            }
            Ok(use_request(EspNowV2TxRequest {
                peer: header.peer,
                random_value: header.random_value,
                payload: &slot.payload[..usize::from(header.payload_length)],
            }))
        })
    }

    fn release_v2_slot(&self, queued: &EspNowQueuedTx) {
        let EspNowQueuedRequest::V2(lease) = queued.request else {
            return;
        };
        self.v2_slots.lock(|slots| {
            let mut slots = slots.borrow_mut();
            if slots
                .get(lease.slot)
                .and_then(|slot| slot.header)
                .is_some_and(|header| header.ticket == queued.ticket)
            {
                slots[lease.slot].header = None;
            }
        });
    }

    pub(crate) fn close(&mut self) {
        if !self.open {
            return;
        }
        let _ = self.generation.compare_exchange(
            self.epoch,
            self.epoch + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.open = false;
    }

    pub(crate) fn publish(
        &self,
        queued: EspNowQueuedTx,
        terminal: EspNowTxTerminal,
    ) -> Result<(), EspNowTxMailboxInvariantError> {
        self.release_v2_slot(&queued);
        self.completions
            .try_send(EspNowTxCompletion {
                ticket: queued.ticket,
                peer: queued.peer,
                terminal,
            })
            .map_err(|TrySendError::Full(_)| EspNowTxMailboxInvariantError::CompletionQueueFull)
    }

    pub(crate) fn cancel_pending(
        &self,
        reason: EspNowTxCancelReason,
    ) -> Result<u32, EspNowTxMailboxInvariantError> {
        let mut cancelled = 0_u32;
        while let Some(queued) = self.try_take() {
            let reason = if queued.ticket.epoch == self.epoch {
                reason
            } else {
                EspNowTxCancelReason::StaleEpoch
            };
            self.publish(queued, EspNowTxTerminal::Cancelled(reason))?;
            cancelled = cancelled.saturating_add(1);
        }
        Ok(cancelled)
    }

    pub(crate) fn shutdown(
        mut self,
        reason: EspNowTxCancelReason,
    ) -> Result<EspNowTxMailboxShutdown, EspNowTxMailboxInvariantError> {
        self.close();
        if self.publishers_in_flight() != 0 {
            return Err(EspNowTxMailboxInvariantError::PublisherInFlight);
        }
        Ok(EspNowTxMailboxShutdown {
            epoch: self.epoch,
            cancelled: self.cancel_pending(reason)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowTxMailboxShutdown {
    pub epoch: u32,
    pub cancelled: u32,
}

/// Narrow extension implemented only by the connected TX owner which already
/// arbitrates ordinary network, management and EAPOL transactions.
pub trait EspNowConnectedTx: ConnectedControlTx {
    fn start_esp_now_v1_plaintext<H: TxHardware, const PEERS: usize>(
        &mut self,
        hardware: &mut H,
        protocol: &EspNowProtocol<PEERS>,
        request: &EspNowOwnedV1Tx,
        active_channel: WifiChannel,
        active_station: BoundVirtualInterface,
        config: Esp32s31EspNowTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduEspNowTxError>;

    fn start_esp_now_v2_plaintext<H: TxHardware, const PEERS: usize>(
        &mut self,
        hardware: &mut H,
        protocol: &EspNowProtocol<PEERS>,
        request: EspNowV2TxRequest<'_>,
        active_channel: WifiChannel,
        active_station: BoundVirtualInterface,
        config: Esp32s31EspNowTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduEspNowTxError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31EspNowConnectedControlConfigError {
    StationBinding {
        configured: BoundVirtualInterface,
        active: BoundVirtualInterface,
    },
    ChannelBinding {
        configured: WifiChannel,
        active: WifiChannel,
    },
}

/// Validated station/channel binding captured before an infallible services
/// decoration closure moves the protocol and mailbox owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31EspNowTxBinding {
    active_station: BoundVirtualInterface,
    active_channel: WifiChannel,
    config: Esp32s31EspNowTxConfig,
}

impl Esp32s31EspNowTxBinding {
    pub fn new<const PEERS: usize>(
        protocol: &EspNowProtocol<PEERS>,
        active_station: BoundVirtualInterface,
        active_channel: WifiChannel,
        config: Esp32s31EspNowTxConfig,
    ) -> Result<Self, Esp32s31EspNowConnectedControlConfigError> {
        let configured = protocol.config();
        if configured.station() != active_station {
            return Err(Esp32s31EspNowConnectedControlConfigError::StationBinding {
                configured: configured.station(),
                active: active_station,
            });
        }
        if configured.home_channel() != active_channel {
            return Err(Esp32s31EspNowConnectedControlConfigError::ChannelBinding {
                configured: configured.home_channel(),
                active: active_channel,
            });
        }
        Ok(Self {
            active_station,
            active_channel,
            config,
        })
    }
}

/// Failed opt-in retaining every owner supplied by the application.
pub struct Esp32s31EspNowConnectedControlStartFailure<
    'resources,
    M: RawMutex,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
> {
    pub error: Esp32s31EspNowConnectedControlConfigError,
    pub control: Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY>,
    pub mailbox: EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
    pub protocol: EspNowProtocol<PEERS>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31EspNowConnectedControlError {
    Connected(ConnectedControlError),
    Mailbox(EspNowTxMailboxInvariantError),
    MissingOrdinaryTxOutcome,
    AlreadyShutdown,
}

/// Explicit opt-in decorator for the stock connected control scheduler.
///
/// Existing security, BlockAck, beacon-loss and power transitions retain
/// priority. A queued ESP-NOW request is admitted only when ordinary network
/// TX is not already pending; once admitted, it uses the same descriptor and
/// IRQ/deadline completion loop as every other ordinary station MPDU.
pub struct Esp32s31EspNowConnectedControl<
    'resources,
    M: RawMutex,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
> {
    inner: Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY>,
    mailbox: Option<EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>>,
    protocol: Option<EspNowProtocol<PEERS>>,
    active_station: BoundVirtualInterface,
    active_channel: WifiChannel,
    config: Esp32s31EspNowTxConfig,
    active: Option<EspNowQueuedTx>,
    pending_inner_terminal: Option<Result<ConnectedDisconnectReason, ConnectedControlError>>,
}

impl<
    'resources,
    M: RawMutex,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
> Esp32s31EspNowConnectedControl<'resources, M, CONTROL_CAPACITY, TX_CAPACITY, PEERS>
{
    pub fn new(
        control: Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY>,
        mailbox: EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
        protocol: EspNowProtocol<PEERS>,
        active_station: BoundVirtualInterface,
        active_channel: WifiChannel,
        config: Esp32s31EspNowTxConfig,
    ) -> Result<
        Self,
        Esp32s31EspNowConnectedControlStartFailure<
            'resources,
            M,
            CONTROL_CAPACITY,
            TX_CAPACITY,
            PEERS,
        >,
    > {
        let binding =
            match Esp32s31EspNowTxBinding::new(&protocol, active_station, active_channel, config) {
                Ok(binding) => binding,
                Err(error) => {
                    return Err(Esp32s31EspNowConnectedControlStartFailure {
                        error,
                        control,
                        mailbox,
                        protocol,
                    });
                }
            };
        Ok(Self::from_binding(control, mailbox, protocol, binding))
    }

    /// Infallible half of the opt-in, suitable for the connected assembly's
    /// `map_services` closure after [`Esp32s31EspNowTxBinding::new`] succeeds.
    pub fn from_binding(
        control: Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY>,
        mailbox: EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
        protocol: EspNowProtocol<PEERS>,
        binding: Esp32s31EspNowTxBinding,
    ) -> Self {
        debug_assert_eq!(protocol.config().station(), binding.active_station);
        debug_assert_eq!(protocol.config().home_channel(), binding.active_channel);
        Self {
            inner: control,
            mailbox: Some(mailbox),
            protocol: Some(protocol),
            active_station: binding.active_station,
            active_channel: binding.active_channel,
            config: binding.config,
            active: None,
            pending_inner_terminal: None,
        }
    }

    pub const fn inner(&self) -> &Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY> {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY> {
        &mut self.inner
    }

    pub const fn tx_epoch(&self) -> Option<u32> {
        match &self.mailbox {
            Some(mailbox) => Some(mailbox.epoch()),
            None => None,
        }
    }

    fn mailbox(
        &self,
    ) -> Result<
        &EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
        Esp32s31EspNowConnectedControlError,
    > {
        self.mailbox
            .as_ref()
            .ok_or(Esp32s31EspNowConnectedControlError::AlreadyShutdown)
    }

    fn mailbox_mut(
        &mut self,
    ) -> Result<
        &mut EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
        Esp32s31EspNowConnectedControlError,
    > {
        self.mailbox
            .as_mut()
            .ok_or(Esp32s31EspNowConnectedControlError::AlreadyShutdown)
    }

    fn finish_active<X: ConnectedControlTx>(
        &mut self,
        tx: &mut X,
    ) -> Result<bool, Esp32s31EspNowConnectedControlError> {
        let Some(queued) = self.active.take() else {
            return Ok(false);
        };
        let Some(outcome) = tx.take_last_outcome() else {
            self.mailbox()?
                .publish(
                    queued,
                    EspNowTxTerminal::RuntimeFailure(
                        EspNowTxRuntimeFailure::MissingOrdinaryTxOutcome,
                    ),
                )
                .map_err(Esp32s31EspNowConnectedControlError::Mailbox)?;
            return Err(Esp32s31EspNowConnectedControlError::MissingOrdinaryTxOutcome);
        };
        self.mailbox()?
            .publish(queued, EspNowTxTerminal::Completed(outcome))
            .map_err(Esp32s31EspNowConnectedControlError::Mailbox)?;
        Ok(true)
    }

    fn close_and_cancel(
        &mut self,
        reason: EspNowTxCancelReason,
    ) -> Result<u32, Esp32s31EspNowConnectedControlError> {
        let mailbox = self.mailbox_mut()?;
        mailbox.close();
        mailbox
            .cancel_pending(reason)
            .map_err(Esp32s31EspNowConnectedControlError::Mailbox)
    }

    fn defer_inner_terminal(
        &mut self,
        terminal: Result<ConnectedDisconnectReason, ConnectedControlError>,
    ) -> Result<
        DatapathControlProgress<ConnectedDisconnectReason>,
        Esp32s31EspNowConnectedControlError,
    > {
        self.close_and_cancel(EspNowTxCancelReason::ConnectionEnded)?;
        self.pending_inner_terminal = Some(terminal);
        Ok(DatapathControlProgress::More)
    }

    pub async fn service_with_context<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        context: DatapathControlContext,
    ) -> Result<
        DatapathControlProgress<ConnectedDisconnectReason>,
        Esp32s31EspNowConnectedControlError,
    >
    where
        H: ConnectedControlHardware + TxHardware,
        X: ConnectedControlTx + ConnectedControlTimer + EspNowConnectedTx,
    {
        if self.finish_active(tx)? {
            if context.stop_pending {
                self.close_and_cancel(EspNowTxCancelReason::StationStopped)?;
            }
            return Ok(DatapathControlProgress::More);
        }

        if self.pending_inner_terminal.is_some() {
            self.close_and_cancel(EspNowTxCancelReason::ConnectionEnded)?;
            if self.mailbox()?.publishers_in_flight() != 0 {
                return Ok(DatapathControlProgress::More);
            }
            // Closing prevents a new publication lease. Once the earlier
            // leases reach zero, this second drain is a terminal fence: no
            // admitted request can appear behind it.
            self.close_and_cancel(EspNowTxCancelReason::ConnectionEnded)?;
            return match self
                .pending_inner_terminal
                .take()
                .expect("checked deferred connected-control terminal")
            {
                Ok(exit) => Ok(DatapathControlProgress::Exit(exit)),
                Err(error) => Err(Esp32s31EspNowConnectedControlError::Connected(error)),
            };
        }

        if context.stop_pending {
            let was_open = self.mailbox()?.is_open();
            let cancelled = self.close_and_cancel(EspNowTxCancelReason::StationStopped)?;
            if <
                Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY> as DatapathControlService<
                    H,
                    X,
                >
            >::required_before_stop(&self.inner)
            {
                return match self.inner.service_with_context(hardware, tx, context).await {
                    Ok(DatapathControlProgress::Exit(exit)) => self.defer_inner_terminal(Ok(exit)),
                    Ok(progress) => Ok(progress),
                    Err(error) => self.defer_inner_terminal(Err(error)),
                };
            }
            if self.mailbox()?.publishers_in_flight() != 0 {
                return Ok(DatapathControlProgress::More);
            }
            self.close_and_cancel(EspNowTxCancelReason::StationStopped)?;
            return Ok(if was_open || cancelled != 0 {
                DatapathControlProgress::More
            } else {
                DatapathControlProgress::Idle
            });
        }

        let esp_now_pending = self.mailbox()?.has_pending();
        let inner_ready = self.inner.has_immediate_work()
            || self
                .inner
                .next_alarm_deadline()
                .is_some_and(|deadline| deadline <= tx.now_micros());
        let inner_requires_awake =
            <Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY> as DatapathControlService<
                H,
                X,
            >>::required_before_network_tx(&self.inner);
        if inner_ready || (esp_now_pending && inner_requires_awake) {
            let inner_context = DatapathControlContext {
                network_tx_pending: context.network_tx_pending || esp_now_pending,
                stop_pending: false,
            };
            return match self
                .inner
                .service_with_context(hardware, tx, inner_context)
                .await
            {
                Ok(DatapathControlProgress::Exit(exit)) => self.defer_inner_terminal(Ok(exit)),
                Ok(progress) => Ok(progress),
                Err(error) => self.defer_inner_terminal(Err(error)),
            };
        }

        // Ordinary network TX owns the next arbitration turn. Returning Idle
        // lets DATAPATH claim it in this same scheduler iteration even though
        // the bounded ESP-NOW mailbox remains ready.
        if context.network_tx_pending || !esp_now_pending {
            return Ok(DatapathControlProgress::Idle);
        }

        let Some(queued) = self.mailbox()?.try_take() else {
            return Ok(DatapathControlProgress::Idle);
        };
        if queued.ticket.epoch != self.mailbox()?.epoch() {
            self.mailbox()?
                .publish(
                    queued,
                    EspNowTxTerminal::Cancelled(EspNowTxCancelReason::StaleEpoch),
                )
                .map_err(Esp32s31EspNowConnectedControlError::Mailbox)?;
            return Ok(DatapathControlProgress::More);
        }
        let protocol = self
            .protocol
            .as_ref()
            .ok_or(Esp32s31EspNowConnectedControlError::AlreadyShutdown)?;
        let result = match queued.request {
            EspNowQueuedRequest::V1(request) => tx.start_esp_now_v1_plaintext(
                hardware,
                protocol,
                &request,
                self.active_channel,
                self.active_station,
                self.config,
            ),
            EspNowQueuedRequest::V2(_) => {
                let request = self.mailbox()?.with_v2_request(&queued, |request| {
                    tx.start_esp_now_v2_plaintext(
                        hardware,
                        protocol,
                        request,
                        self.active_channel,
                        self.active_station,
                        self.config,
                    )
                });
                match request {
                    Ok(result) => result,
                    Err(error) => {
                        self.mailbox()?
                            .publish(
                                queued,
                                EspNowTxTerminal::RuntimeFailure(
                                    EspNowTxRuntimeFailure::MissingV2PayloadSlot,
                                ),
                            )
                            .map_err(Esp32s31EspNowConnectedControlError::Mailbox)?;
                        return Err(Esp32s31EspNowConnectedControlError::Mailbox(error));
                    }
                }
            }
        };
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active = Some(queued);
                Ok(DatapathControlProgress::TxPending)
            }
            Ok(WifiTxProgress::Complete) => {
                self.active = Some(queued);
                self.finish_active(tx)?;
                Ok(DatapathControlProgress::More)
            }
            Err(error) => {
                self.mailbox()?
                    .publish(queued, EspNowTxTerminal::Rejected(error))
                    .map_err(Esp32s31EspNowConnectedControlError::Mailbox)?;
                Ok(DatapathControlProgress::More)
            }
        }
    }

    pub fn shutdown<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<Esp32s31EspNowConnectedControlShutdown<PEERS>, Esp32s31EspNowConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        self.finish_active(tx)?;
        self.close_and_cancel(EspNowTxCancelReason::OwnerShutdown)?;
        if self.mailbox()?.publishers_in_flight() != 0 {
            return Err(Esp32s31EspNowConnectedControlError::Mailbox(
                EspNowTxMailboxInvariantError::PublisherInFlight,
            ));
        }
        self.close_and_cancel(EspNowTxCancelReason::OwnerShutdown)?;
        let connected = self
            .inner
            .shutdown(hardware, tx)
            .map_err(Esp32s31EspNowConnectedControlError::Connected)?;
        let mailbox = self
            .mailbox
            .take()
            .ok_or(Esp32s31EspNowConnectedControlError::AlreadyShutdown)?
            .shutdown(EspNowTxCancelReason::OwnerShutdown)
            .map_err(Esp32s31EspNowConnectedControlError::Mailbox)?;
        let protocol = self
            .protocol
            .take()
            .ok_or(Esp32s31EspNowConnectedControlError::AlreadyShutdown)?;
        Ok(Esp32s31EspNowConnectedControlShutdown {
            connected,
            mailbox,
            protocol,
        })
    }
}

/// Replace the stock connected control member with the ESP-NOW scheduler
/// decorator while preserving the exact hardware, RX and ordinary/A-MPDU TX
/// owners. This is the compile-checked production composition frontier used
/// by custom integration roots and `assemble_esp32s31_connected_driver`'s
/// `map_services` closure.
pub fn attach_esp_now_tx<
    'resources,
    M: RawMutex,
    H,
    R,
    X,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
>(
    services: SingleRoleServices<
        H,
        R,
        X,
        Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY>,
    >,
    mailbox: EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
    protocol: EspNowProtocol<PEERS>,
    binding: Esp32s31EspNowTxBinding,
) -> SingleRoleServices<
    H,
    R,
    X,
    Esp32s31EspNowConnectedControl<'resources, M, CONTROL_CAPACITY, TX_CAPACITY, PEERS>,
> {
    let (hardware, rx, tx, control) = services.into_parts();
    SingleRoleServices::with_control(
        hardware,
        rx,
        tx,
        Esp32s31EspNowConnectedControl::from_binding(control, mailbox, protocol, binding),
    )
}

pub struct Esp32s31EspNowConnectedControlShutdown<const PEERS: usize> {
    pub connected: ConnectedControlShutdown,
    pub mailbox: EspNowTxMailboxShutdown,
    pub protocol: EspNowProtocol<PEERS>,
}

impl<
    'resources,
    M,
    H,
    X,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
> DatapathControlService<H, X>
    for Esp32s31EspNowConnectedControl<'resources, M, CONTROL_CAPACITY, TX_CAPACITY, PEERS>
where
    M: RawMutex,
    H: ConnectedControlHardware + TxHardware,
    X: ConnectedControlTx + ConnectedControlTimer + EspNowConnectedTx,
{
    type Error = Esp32s31EspNowConnectedControlError;
    type Exit = ConnectedDisconnectReason;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        Esp32s31EspNowConnectedControl::service_with_context(self, hardware, tx, context)
    }

    fn ready(&self, tx: &X, now_micros: u64) -> bool {
        self.active.is_some()
            || self.pending_inner_terminal.is_some()
            || self.mailbox.as_ref().is_some_and(EspNowTxMailboxOwner::has_pending)
            || <Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY> as DatapathControlService<
                H,
                X,
            >>::ready(&self.inner, tx, now_micros)
    }

    fn required_before_network_tx(&self) -> bool {
        <Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY> as DatapathControlService<
            H,
            X,
        >>::required_before_network_tx(&self.inner)
    }

    fn required_before_stop(&self) -> bool {
        self.active.is_some()
            || self.pending_inner_terminal.is_some()
            || self.mailbox.as_ref().is_some_and(|mailbox| {
                mailbox.is_open()
                    || mailbox.has_pending()
                    || mailbox.publishers_in_flight() != 0
            })
            || <Esp32s31ConnectedControl<'resources, M, CONTROL_CAPACITY> as DatapathControlService<
                H,
                X,
            >>::required_before_stop(&self.inner)
    }

    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a {
        async move {
            if self.active.is_some()
                || self.pending_inner_terminal.is_some()
                || self
                    .mailbox
                    .as_ref()
                    .is_some_and(EspNowTxMailboxOwner::has_pending)
            {
                return;
            }
            let Some(mailbox) = self.mailbox.as_ref() else {
                self.inner.wait_ready(tx).await;
                return;
            };
            match select(self.inner.wait_ready(tx), mailbox.ready()).await {
                Either::First(()) | Either::Second(()) => {}
            }
        }
    }
}
