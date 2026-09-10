//! Bounded observations of ARP transfers across the original stack driver API.
//! Delegation preserves packet ownership, return values and wake policy.
#![forbid(unsafe_code)]

use super::super::rx_qualification::observe_stack_arp;
use core::task::Waker;
use embassy_net::driver::{self, Driver, PacketBuf};
use open_esp_radio_hil_esp32s31_telemetry::arp_frontier::Stage;

pub(crate) struct Device<D>(D);
impl<D> Device<D> {
    pub(crate) fn new(inner: D) -> Self {
        Self(inner)
    }
}
impl<D: Driver> Driver for Device<D> {
    fn capabilities(&self) -> driver::Capabilities {
        self.0.capabilities()
    }
    fn hardware_address(&self) -> driver::HardwareAddress {
        self.0.hardware_address()
    }
    fn link_state(&mut self) -> driver::LinkState {
        self.0.link_state()
    }
    fn register_waker(&mut self, waker: &Waker) -> Result<(), driver::NotSupported> {
        self.0.register_waker(waker)
    }
    fn receive(&mut self) -> Option<PacketBuf> {
        let packet = self.0.receive()?;
        observe_stack_arp(&packet, Stage::StackReceived);
        Some(packet)
    }
    fn can_transmit(&mut self) -> bool {
        self.0.can_transmit()
    }
    fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        // Retain only the small ARP header, never an additional packet owner.
        let arp: Option<[u8; 42]> = if packet.get(12..14) == Some(&[8, 6]) {
            packet.get(..42).and_then(|bytes| bytes.try_into().ok())
        } else {
            None
        };
        let result = self.0.transmit(packet);
        if let Some(arp) = arp {
            observe_stack_arp(
                &arp,
                if result.is_ok() {
                    Stage::TxAccepted
                } else {
                    Stage::TxRejected
                },
            );
        }
        result
    }
    fn set_multicast_filter(&mut self, addrs: &[[u8; 6]]) {
        self.0.set_multicast_filter(addrs);
    }
}
