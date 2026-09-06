//! Stack/driver boundary counters. No wakeups or admission policy are changed.
#![forbid(unsafe_code)]

use core::{
    sync::atomic::{AtomicU32, Ordering},
    task::Waker,
};
use embassy_net::driver::{self, Driver, PacketBuf};

#[derive(Clone, Copy)]
pub(crate) enum Event {
    NetworkPoll,
    PollWithoutTransfer,
    TxReady,
    TxUnavailable,
    TxAccepted,
    TxRejected,
    RxEmpty,
    RxDelivered,
}

const COUNT: usize = Event::RxDelivered as usize + 1;
pub(crate) struct Counters([AtomicU32; COUNT]);

#[derive(Clone, Copy)]
pub(crate) struct Snapshot([u32; COUNT]);

impl Counters {
    pub const fn new() -> Self {
        Self([const { AtomicU32::new(0) }; COUNT])
    }
    pub fn record(&self, event: Event) {
        self.0[event as usize].fetch_add(1, Ordering::Relaxed);
    }
    pub fn transfers(&self) -> u32 {
        self.0[Event::TxAccepted as usize]
            .load(Ordering::Relaxed)
            .wrapping_add(self.0[Event::RxDelivered as usize].load(Ordering::Relaxed))
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot(core::array::from_fn(|i| self.0[i].load(Ordering::Relaxed)))
    }
}

impl Snapshot {
    pub fn get(self, event: Event) -> u32 {
        self.0[event as usize]
    }
    pub fn delta(self, earlier: Self) -> Self {
        Self(core::array::from_fn(|i| {
            self.0[i].wrapping_sub(earlier.0[i])
        }))
    }
}

pub(crate) struct Device<D> {
    inner: D,
    counters: &'static Counters,
}

impl<D> Device<D> {
    pub fn new(inner: D, counters: &'static Counters) -> Self {
        Self { inner, counters }
    }
}

impl<D: Driver> Driver for Device<D> {
    fn capabilities(&self) -> driver::Capabilities {
        self.inner.capabilities()
    }
    fn hardware_address(&self) -> driver::HardwareAddress {
        self.inner.hardware_address()
    }
    fn link_state(&mut self) -> driver::LinkState {
        self.inner.link_state()
    }
    fn register_waker(&mut self, waker: &Waker) -> Result<(), driver::NotSupported> {
        self.inner.register_waker(waker)
    }
    fn receive(&mut self) -> Option<PacketBuf> {
        let packet = self.inner.receive();
        self.counters.record(if packet.is_some() {
            Event::RxDelivered
        } else {
            Event::RxEmpty
        });
        packet
    }
    fn can_transmit(&mut self) -> bool {
        let ready = self.inner.can_transmit();
        self.counters.record(if ready {
            Event::TxReady
        } else {
            Event::TxUnavailable
        });
        ready
    }
    fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        let result = self.inner.transmit(packet);
        self.counters.record(if result.is_ok() {
            Event::TxAccepted
        } else {
            Event::TxRejected
        });
        result
    }
    fn set_multicast_filter(&mut self, addrs: &[[u8; 6]]) {
        self.inner.set_multicast_filter(addrs);
    }
}

/// A poll without a packet transfer may still perform internal protocol work.
/// This counter measures only the boundary, not whether the entire poll is useful.
pub(crate) async fn observe<F: core::future::Future>(future: F, counters: &Counters) -> F::Output {
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|cx| {
        let before = counters.transfers();
        let result = future.as_mut().poll(cx);
        counters.record(Event::NetworkPoll);
        if counters.transfers() == before {
            counters.record(Event::PollWithoutTransfer);
        }
        result
    })
    .await
}
