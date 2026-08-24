//! Owned, bounded ESP-NOW receive handoff for one connected STA epoch.

use core::{
    future::Future,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_sync::channel::{Channel, Receiver, Sender, TrySendError};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::rx::RxPhyInfo;
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{ConnectedRxEvent, ConnectedRxSink};
use open_esp_radio_esp32s31_wifi_sta::standalone_esp_now_rx::{
    StandaloneEspNowRxEvent, StandaloneEspNowRxSink,
};
use open_esp_radio_wifi_softmac::{EspNowOwnedReceivedV1, MacRxMetadata};

use crate::{
    datapath::rx::staging::{
        Esp32s31StagedRxFrame, StagedEthernetPublication, StagedRxDisposition,
    },
    roles::station::rx_protocol::ConnectedRxProtocolSink,
};

/// Owned datagram delivered after strict protocol admission and duplicate
/// suppression. `epoch` lets an application reject a handle retained beyond
/// its connected lifecycle even though old publishers are also fail-closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowOwnedRxEvent {
    pub epoch: u32,
    pub received: EspNowOwnedReceivedV1,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowRxMailboxEpochError {
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowRxPublishOutcome {
    Published,
    Full,
    StaleEpoch,
}

/// Static channel and lifecycle counters. It contains no WPA2 key material and
/// shares neither storage nor capacity with the connected control mailbox.
pub struct EspNowRxMailboxResources<M: RawMutex, const CAPACITY: usize> {
    channel: Channel<M, EspNowOwnedRxEvent, CAPACITY>,
    generation: AtomicU32,
    dropped: AtomicU32,
    stale_publications: AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> EspNowRxMailboxResources<M, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            channel: Channel::new(),
            generation: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
            stale_publications: AtomicU32::new(0),
        }
    }

    /// Start one mailbox epoch after the previous publisher and receiver have
    /// been returned or dropped. The unique resource borrow keeps two epochs
    /// from being created concurrently in safe code.
    ///
    /// Pending datagrams are discarded before the new generation becomes
    /// visible. A publisher retained in violation of the ownership contract
    /// observes a generation mismatch and cannot enqueue into the new epoch.
    pub fn begin_epoch(
        &mut self,
    ) -> Result<
        (
            EspNowRxPublisher<'_, M, CAPACITY>,
            EspNowRxReceiver<'_, M, CAPACITY>,
        ),
        EspNowRxMailboxEpochError,
    > {
        let resources: &Self = self;
        let receiver = resources.channel.receiver();
        while receiver.try_receive().is_ok() {}
        let epoch = self
            .generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .map_err(|_| EspNowRxMailboxEpochError::GenerationExhausted)?
            + 1;
        resources.dropped.store(0, Ordering::Release);
        resources.stale_publications.store(0, Ordering::Release);
        Ok((
            EspNowRxPublisher {
                sender: resources.channel.sender(),
                generation: &resources.generation,
                epoch,
                dropped: &resources.dropped,
                stale_publications: &resources.stale_publications,
            },
            EspNowRxReceiver {
                receiver,
                epoch,
                dropped: &resources.dropped,
                stale_publications: &resources.stale_publications,
            },
        ))
    }
}

impl<M: RawMutex, const CAPACITY: usize> Default for EspNowRxMailboxResources<M, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Callback-side capability. It may publish only into the generation in which
/// it was created and cannot consume application events.
#[derive(Clone, Copy)]
pub struct EspNowRxPublisher<'resources, M: RawMutex, const CAPACITY: usize> {
    sender: Sender<'resources, M, EspNowOwnedRxEvent, CAPACITY>,
    generation: &'resources AtomicU32,
    epoch: u32,
    dropped: &'resources AtomicU32,
    stale_publications: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> EspNowRxPublisher<'_, M, CAPACITY> {
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn try_publish(
        &self,
        received: open_esp_radio_wifi_softmac::EspNowReceivedV1<'_>,
        metadata: MacRxMetadata<RxPhyInfo>,
    ) -> EspNowRxPublishOutcome {
        if self.generation.load(Ordering::Acquire) != self.epoch {
            saturating_increment(self.stale_publications);
            return EspNowRxPublishOutcome::StaleEpoch;
        }
        let event = EspNowOwnedRxEvent {
            epoch: self.epoch,
            received: EspNowOwnedReceivedV1::copy_from(received),
            metadata,
        };
        match self.sender.try_send(event) {
            Ok(()) => EspNowRxPublishOutcome::Published,
            Err(TrySendError::Full(_)) => {
                saturating_increment(self.dropped);
                EspNowRxPublishOutcome::Full
            }
        }
    }
}

impl<M: RawMutex, const CAPACITY: usize> StandaloneEspNowRxSink
    for EspNowRxPublisher<'_, M, CAPACITY>
{
    fn publish(&mut self, event: StandaloneEspNowRxEvent<'_>) {
        let _ = self.try_publish(event.received, event.metadata);
    }
}

/// Application-side capability for one connected epoch.
pub struct EspNowRxReceiver<'resources, M: RawMutex, const CAPACITY: usize> {
    receiver: Receiver<'resources, M, EspNowOwnedRxEvent, CAPACITY>,
    epoch: u32,
    dropped: &'resources AtomicU32,
    stale_publications: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> EspNowRxReceiver<'_, M, CAPACITY> {
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn try_receive(&self) -> Option<EspNowOwnedRxEvent> {
        loop {
            let event = self.receiver.try_receive().ok()?;
            if event.epoch == self.epoch {
                return Some(event);
            }
        }
    }

    pub async fn receive(&self) -> EspNowOwnedRxEvent {
        loop {
            let event = self.receiver.receive().await;
            if event.epoch == self.epoch {
                return event;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    pub fn dropped(&self) -> u32 {
        self.dropped.load(Ordering::Acquire)
    }

    pub fn stale_publications(&self) -> u32 {
        self.stale_publications.load(Ordering::Acquire)
    }

    /// Drain every application event before the lifecycle reuses the static
    /// channel for another association.
    pub fn shutdown(self) -> EspNowRxMailboxShutdown {
        let mut discarded = 0_u32;
        while self.receiver.try_receive().is_ok() {
            discarded = discarded.saturating_add(1);
        }
        EspNowRxMailboxShutdown {
            epoch: self.epoch,
            discarded,
            dropped: self.dropped(),
            stale_publications: self.stale_publications(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowRxMailboxShutdown {
    pub epoch: u32,
    pub discarded: u32,
    pub dropped: u32,
    pub stale_publications: u32,
}

/// Sink decorator which copies only ESP-NOW events into the dedicated
/// mailbox, then forwards the original borrowed event to the existing sink.
/// Connected BlockAck, security and network behavior therefore remains owned
/// by the already composed sink.
pub struct EspNowMailboxConnectedRxSink<'resources, M: RawMutex, S, const CAPACITY: usize> {
    inner: S,
    publisher: EspNowRxPublisher<'resources, M, CAPACITY>,
}

impl<'resources, M: RawMutex, S, const CAPACITY: usize>
    EspNowMailboxConnectedRxSink<'resources, M, S, CAPACITY>
{
    pub const fn new(inner: S, publisher: EspNowRxPublisher<'resources, M, CAPACITY>) -> Self {
        Self { inner, publisher }
    }

    pub const fn inner(&self) -> &S {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    pub fn into_parts(self) -> (S, EspNowRxPublisher<'resources, M, CAPACITY>) {
        (self.inner, self.publisher)
    }
}

impl<M: RawMutex, S: ConnectedRxSink, const CAPACITY: usize> ConnectedRxSink
    for EspNowMailboxConnectedRxSink<'_, M, S, CAPACITY>
{
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::EspNow { received, metadata } = event {
            let _ = self.publisher.try_publish(received, metadata);
        }
        self.inner.publish(event);
    }
}

impl<
    M: RawMutex,
    S: ConnectedRxProtocolSink<FRAME_CAPACITY, FRAME_SLOTS>,
    const MAILBOX_CAPACITY: usize,
    const FRAME_CAPACITY: usize,
    const FRAME_SLOTS: usize,
> ConnectedRxProtocolSink<FRAME_CAPACITY, FRAME_SLOTS>
    for EspNowMailboxConnectedRxSink<'_, M, S, MAILBOX_CAPACITY>
{
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.inner.wait_ready()
    }

    fn wait_staged_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.inner.wait_staged_ready()
    }

    fn publish_staged(
        &mut self,
        frame: Esp32s31StagedRxFrame<'_, FRAME_CAPACITY, FRAME_SLOTS>,
        ethernet: StagedEthernetPublication,
    ) -> StagedRxDisposition {
        self.inner.publish_staged(frame, ethernet)
    }
}

fn saturating_increment(value: &AtomicU32) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(1))
    });
}
