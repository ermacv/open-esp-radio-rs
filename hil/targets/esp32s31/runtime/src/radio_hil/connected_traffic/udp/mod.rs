#![forbid(unsafe_code)]

mod rx;
mod tx;

use embassy_net::udp::PacketMetadata;

pub(in crate::radio_hil) struct UdpSocketBuffers<'a> {
    pub(super) rx_metadata: &'a mut [PacketMetadata],
    pub(super) rx: &'a mut [u8],
    pub(super) tx_metadata: &'a mut [PacketMetadata],
    pub(super) tx: &'a mut [u8],
}

impl<'a> UdpSocketBuffers<'a> {
    pub(in crate::radio_hil) fn new(
        rx_metadata: &'a mut [PacketMetadata],
        rx: &'a mut [u8],
        tx_metadata: &'a mut [PacketMetadata],
        tx: &'a mut [u8],
    ) -> Self {
        Self {
            rx_metadata,
            rx,
            tx_metadata,
            tx,
        }
    }
}

pub(in crate::radio_hil) use rx::{
    UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, run_open_radio_udp_rx_benchmark,
};
pub(in crate::radio_hil) use tx::{
    UdpTxBenchmarkConfig, UdpTxSessionSource, run_open_radio_udp_tx_benchmark,
};
