//! Count actual token consumption, not merely the offer of a token.
use super::progress::{Counters, Device, Event};
use core::task::Context;
use embassy_net::driver::{self, Driver, RxToken, TxToken};
pub(crate) struct Rx<T>(T, &'static Counters);
pub(crate) struct Tx<T>(T, &'static Counters);
impl<T: RxToken> RxToken for Rx<T> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        self.1.record(Event::RxDelivered);
        self.0.consume(f)
    }
}
impl<T: TxToken> TxToken for Tx<T> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let result = self.0.consume(len, f);
        self.1.record(Event::TxAccepted);
        result
    }
}
impl<D: Driver> Driver for Device<D> {
    type RxToken<'a>
        = Rx<D::RxToken<'a>>
    where
        Self: 'a;
    type TxToken<'a>
        = Tx<D::TxToken<'a>>
    where
        Self: 'a;
    fn receive(&mut self, cx: &mut Context) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        match self.inner.receive(cx) {
            Some((rx, tx)) => Some((Rx(rx, self.counters), Tx(tx, self.counters))),
            None => {
                self.counters.record(Event::RxEmpty);
                None
            }
        }
    }
    fn transmit(&mut self, cx: &mut Context) -> Option<Self::TxToken<'_>> {
        let tx = self.inner.transmit(cx);
        self.counters.record(if tx.is_some() {
            Event::TxReady
        } else {
            Event::TxUnavailable
        });
        tx.map(|tx| Tx(tx, self.counters))
    }
    fn link_state(&mut self, cx: &mut Context) -> driver::LinkState {
        self.inner.link_state(cx)
    }
    fn capabilities(&self) -> driver::Capabilities {
        self.inner.capabilities()
    }
    fn hardware_address(&self) -> driver::HardwareAddress {
        self.inner.hardware_address()
    }
}
