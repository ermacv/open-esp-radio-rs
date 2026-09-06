//! Explicit checksum-cost experiments owned by HIL, never product capabilities.
//!
//! Only diagnostic requests skip validation or omit the legal IPv4 UDP checksum.
//! The ordinary path preserves the production driver's software checksums.
#![forbid(unsafe_code)]

use core::task::Waker;
use embassy_net::driver::{self, Driver, PacketBuf};
use open_esp_radio_hil_protocol::{WifiRxChecksumPolicy, WifiTxUdpChecksumPolicy};

pub(crate) struct Device<D> {
    inner: D,
    rx: WifiRxChecksumPolicy,
    tx: WifiTxUdpChecksumPolicy,
}

impl<D> Device<D> {
    pub(crate) fn new(inner: D, rx: WifiRxChecksumPolicy, tx: WifiTxUdpChecksumPolicy) -> Self {
        Self { inner, rx, tx }
    }
}

impl<D: Driver> Driver for Device<D> {
    fn capabilities(&self) -> driver::Capabilities {
        let mut caps = self.inner.capabilities();
        if self.rx == WifiRxChecksumPolicy::AssumeValidDiagnostic {
            caps.checksum.ipv4.rx = true;
            caps.checksum.udp.rx = true;
        }
        if self.tx == WifiTxUdpChecksumPolicy::OmitIpv4Diagnostic {
            caps.checksum.udp.tx = true;
        }
        caps
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
        self.inner.receive()
    }
    fn can_transmit(&mut self) -> bool {
        self.inner.can_transmit()
    }
    fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        self.inner.transmit(packet)
    }
    fn set_multicast_filter(&mut self, addrs: &[[u8; 6]]) {
        self.inner.set_multicast_filter(addrs);
    }
}
