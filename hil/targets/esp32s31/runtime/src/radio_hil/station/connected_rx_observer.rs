#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use open_esp_radio::{
    esp32s31::wifi::lmac::{
        connected_rx::{ConnectedRxEvent, ConnectedRxSink},
        rx::decode_rx_phy_info,
    },
    wifi::lmac::MacRxEvidence,
};
use open_esp_radio_hil_esp32s31_telemetry::{
    rx_evidence::{RxAmpduCounters, RxPhyCounters, RxSmpduCounters},
    rx_order::{RxOrderCounters, RxOrderTracker},
};

use super::super::connected_traffic::{
    ipv4_udp_destination_port, ipv4_udp_sequence, public_qos_sequence,
};

/// Named HIL observation inputs for connected RX.
///
/// The production sink remains generic and receives every event unchanged;
/// this binding only records qualification evidence around that handoff.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilConnectedRxBindings {
    pub(in crate::radio_hil) local_ipv4: &'static AtomicU32,
    pub(in crate::radio_hil) lan_probe_response: &'static AtomicBool,
    pub(in crate::radio_hil) lan_probe_rx_s_mpdu: &'static AtomicU32,
    pub(in crate::radio_hil) lan_probe_ipv4: [u8; 4],
    pub(in crate::radio_hil) udp_port: u16,
    pub(in crate::radio_hil) order_telemetry: bool,
    pub(in crate::radio_hil) beacon_s_mpdu: &'static RxSmpduCounters,
    pub(in crate::radio_hil) order: &'static RxOrderCounters,
    pub(in crate::radio_hil) s_mpdu: &'static RxSmpduCounters,
    pub(in crate::radio_hil) ampdu: &'static RxAmpduCounters,
    pub(in crate::radio_hil) last_format: &'static AtomicU32,
    pub(in crate::radio_hil) last_phy: &'static AtomicU32,
    pub(in crate::radio_hil) phy: &'static RxPhyCounters,
}

/// HIL-only observer layered outside the production RX/backend boundary.
pub(in crate::radio_hil) struct HilConnectedRxObserver<S> {
    control: S,
    station_address: [u8; 6],
    phy_sample_cursor: u8,
    order: RxOrderTracker,
    bindings: RadioHilConnectedRxBindings,
}

impl<S> HilConnectedRxObserver<S> {
    pub(in crate::radio_hil) fn new(
        control: S,
        station_address: [u8; 6],
        bindings: RadioHilConnectedRxBindings,
    ) -> Self {
        Self {
            control,
            station_address,
            phy_sample_cursor: 0,
            order: RxOrderTracker::default(),
            bindings,
        }
    }
}

impl<S: ConnectedRxSink> ConnectedRxSink for HilConnectedRxObserver<S> {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Beacon { metadata, .. } = event {
            self.bindings.beacon_s_mpdu.observe(metadata.s_mpdu);
        }
        if let ConnectedRxEvent::Ethernet {
            frame,
            raw,
            metadata,
            ..
        } = event
        {
            if self.bindings.order_telemetry
                && let Some(sequence) = ipv4_udp_sequence(frame, self.bindings.udp_port)
            {
                self.order
                    .observe(self.bindings.order, sequence, public_qos_sequence(raw));
            }
            let local_ipv4 = self
                .bindings
                .local_ipv4
                .load(Ordering::Acquire)
                .to_be_bytes();
            let is_probe_reply = frame.destination == self.station_address
                && frame.ether_type == 0x0806
                && frame.payload.len() >= 28
                && frame.payload[6..8] == 2_u16.to_be_bytes()
                && frame.payload[14..18] == self.bindings.lan_probe_ipv4
                && frame.payload[18..24] == self.station_address
                && frame.payload[24..28] == local_ipv4;
            if is_probe_reply {
                let s_mpdu = match metadata.s_mpdu {
                    MacRxEvidence::HardwareObserved(s_mpdu) => u32::from(s_mpdu),
                    MacRxEvidence::ProtocolValidated(_) | MacRxEvidence::Unavailable => u32::MAX,
                };
                self.bindings
                    .lan_probe_rx_s_mpdu
                    .store(s_mpdu, Ordering::Relaxed);
                self.bindings
                    .lan_probe_response
                    .store(true, Ordering::Release);
            }
            let benchmark_udp = ipv4_udp_destination_port(frame) == Some(self.bindings.udp_port);
            if benchmark_udp {
                self.bindings.s_mpdu.observe(metadata.s_mpdu);
                self.bindings.ampdu.observe(metadata.ampdu);
                let sample_phy = self.phy_sample_cursor == 0;
                self.phy_sample_cursor = self.phy_sample_cursor.wrapping_add(1) & 63;
                if sample_phy && let Some(phy) = decode_rx_phy_info(raw) {
                    self.bindings
                        .last_format
                        .store(u32::from(phy.baseband_format().raw()), Ordering::Relaxed);
                    let mut packed =
                        u32::from(phy.baseband_format().raw()) | (u32::from(phy.rate) << 4);
                    if let Some(signal) = phy.he_su_signal() {
                        let bandwidth = match signal.bandwidth.mhz() {
                            20 => 0,
                            40 => 1,
                            80 => 2,
                            _ => 3,
                        };
                        packed |= (1 << 31)
                            | (u32::from(signal.mcs) << 9)
                            | (u32::from(signal.guard_interval_and_ltf.encoding()) << 13)
                            | (bandwidth << 15)
                            | (u32::from(signal.dcm) << 17)
                            | (u32::from(signal.ldpc) << 18);
                        self.bindings.phy.observe_he_mcs(signal.mcs);
                    } else {
                        self.bindings.phy.observe_other();
                    }
                    self.bindings.last_phy.store(packed, Ordering::Relaxed);
                }
            }
        }
        self.control.publish(event);
    }
}
