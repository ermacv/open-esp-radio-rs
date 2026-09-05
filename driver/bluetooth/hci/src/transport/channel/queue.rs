//! Async packet FIFO and cancellation-safe readiness registration.

use super::*;

pub(super) struct AsyncPacketQueue<M, const DEPTH: usize, const PACKET_CAPACITY: usize>
where
    M: RawMutex,
{
    state: Mutex<M, RefCell<AsyncPacketQueueState<DEPTH, PACKET_CAPACITY>>>,
}

impl<M, const DEPTH: usize, const PACKET_CAPACITY: usize>
    AsyncPacketQueue<M, DEPTH, PACKET_CAPACITY>
where
    M: RawMutex,
{
    pub(super) const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(AsyncPacketQueueState::new())),
        }
    }

    pub(super) async fn send(&self, packet: PacketSlot<PACKET_CAPACITY>) {
        poll_fn(|context| {
            self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.try_send(packet) {
                    Poll::Ready(())
                } else {
                    state.sender_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    pub(super) async fn wait_send_ready(&self) {
        poll_fn(|context| {
            self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.length < DEPTH {
                    Poll::Ready(())
                } else {
                    state.sender_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    pub(super) fn try_send(&self, packet: PacketSlot<PACKET_CAPACITY>) -> Result<(), ()> {
        self.state.lock(|state| {
            if state.borrow_mut().try_send(packet) {
                Ok(())
            } else {
                Err(())
            }
        })
    }

    pub(super) async fn receive(&self) -> PacketSlot<PACKET_CAPACITY> {
        poll_fn(|context| {
            self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if let Some(packet) = state.try_receive() {
                    Poll::Ready(packet)
                } else {
                    state.receiver_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    pub(super) async fn wait_receive_ready(&self) {
        poll_fn(|context| {
            self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.length > 0 {
                    Poll::Ready(())
                } else {
                    state.receiver_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    pub(super) fn try_receive(&self) -> Result<PacketSlot<PACKET_CAPACITY>, ()> {
        self.state
            .lock(|state| state.borrow_mut().try_receive().ok_or(()))
    }

    pub(super) fn is_pristine(&self) -> bool {
        self.state.lock(|state| {
            let state = state.borrow();
            state.length == 0 && !state.has_published_packet
        })
    }

    #[cfg(test)]
    pub(super) fn vacant_storage_is_zeroed(&self) -> bool {
        self.state.lock(|state| {
            let state = state.borrow();
            state
                .slots
                .iter()
                .all(|slot| slot.length != 0 || slot.bytes.iter().all(|byte| *byte == 0))
        })
    }
}

struct AsyncPacketQueueState<const DEPTH: usize, const PACKET_CAPACITY: usize> {
    slots: [PacketSlot<PACKET_CAPACITY>; DEPTH],
    head: usize,
    length: usize,
    has_published_packet: bool,
    receiver_waker: WakerRegistration,
    sender_waker: WakerRegistration,
}

impl<const DEPTH: usize, const PACKET_CAPACITY: usize>
    AsyncPacketQueueState<DEPTH, PACKET_CAPACITY>
{
    const fn new() -> Self {
        Self {
            slots: [PacketSlot::EMPTY; DEPTH],
            head: 0,
            length: 0,
            has_published_packet: false,
            receiver_waker: WakerRegistration::new(),
            sender_waker: WakerRegistration::new(),
        }
    }

    fn try_send(&mut self, packet: PacketSlot<PACKET_CAPACITY>) -> bool {
        if self.length == DEPTH {
            return false;
        }
        let tail = (self.head + self.length) % DEPTH;
        self.slots[tail] = packet;
        self.length += 1;
        self.has_published_packet = true;
        self.receiver_waker.wake();
        true
    }

    fn try_receive(&mut self) -> Option<PacketSlot<PACKET_CAPACITY>> {
        if self.length == 0 {
            return None;
        }
        let packet = self.slots[self.head];
        self.slots[self.head].bytes.fill(0);
        self.slots[self.head].length = 0;
        self.head = (self.head + 1) % DEPTH;
        self.length -= 1;
        self.sender_waker.wake();
        Some(packet)
    }
}
