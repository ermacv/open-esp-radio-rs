//! Observation-only RX evidence for the product HIL composition.
//!
//! The production driver owns classification and delivery. This module only
//! samples borrowed semantic events and never consumes, mutates or delays
//! their delivery to the network sink.

#![forbid(unsafe_code)]

#[cfg(feature = "rx-order-telemetry")]
use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(feature = "rx-order-telemetry")]
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
#[cfg(feature = "rx-order-telemetry")]
use open_esp_radio::esp32s31::wifi::mac::rx::PUBLIC_HEADER_SIZE;
use open_esp_radio::{
    esp32s31::wifi::mac::{connected_rx::ConnectedRxEvent, rx::decode_rx_phy_info},
    wifi::ieee80211::data::EthernetFrameParts,
};
use open_esp_radio_esp32s31_embassy_wifi::Esp32s31ConnectedRxObserver;
use open_esp_radio_hil_esp32s31_telemetry::rx_evidence::{
    RxAmpduCounters, RxPhyCounters, RxSmpduCounters,
};
#[cfg(feature = "rx-order-telemetry")]
use open_esp_radio_hil_esp32s31_telemetry::rx_order::{RxOrderCounters, RxOrderTracker};

pub(crate) static RX_PHY: RxPhyCounters = RxPhyCounters::new();
pub(crate) static RX_S_MPDU: RxSmpduCounters = RxSmpduCounters::new();
pub(crate) static BEACON_S_MPDU: RxSmpduCounters = RxSmpduCounters::new();
pub(crate) static RX_AMPDU: RxAmpduCounters = RxAmpduCounters::new();
#[cfg(feature = "rx-order-telemetry")]
pub(crate) static RX_ORDER: RxOrderCounters = RxOrderCounters::new();
pub(crate) static LAST_FORMAT: AtomicU32 = AtomicU32::new(u32::MAX);
pub(crate) static LAST_PHY: AtomicU32 = AtomicU32::new(u32::MAX);

pub(crate) struct HilConnectedRxObserver {
    udp_port: u16,
    phy_sample_cursor: AtomicU32,
    #[cfg(feature = "rx-order-telemetry")]
    order: Mutex<CriticalSectionRawMutex, RefCell<Option<RxOrderTracker>>>,
}

impl HilConnectedRxObserver {
    pub(crate) const fn new(udp_port: u16) -> Self {
        Self {
            udp_port,
            phy_sample_cursor: AtomicU32::new(0),
            #[cfg(feature = "rx-order-telemetry")]
            order: Mutex::new(RefCell::new(None)),
        }
    }
}

impl Esp32s31ConnectedRxObserver for HilConnectedRxObserver {
    fn observe(&self, event: &ConnectedRxEvent<'_>) {
        match *event {
            ConnectedRxEvent::Beacon { metadata, .. } => {
                BEACON_S_MPDU.observe(metadata.s_mpdu);
            }
            ConnectedRxEvent::Ethernet {
                frame,
                raw,
                metadata,
                ..
            } if ipv4_udp_destination_port(frame) == Some(self.udp_port) => {
                #[cfg(feature = "rx-order-telemetry")]
                if let Some(sequence) = ipv4_udp_sequence(frame, self.udp_port) {
                    self.order.lock(|tracker| {
                        tracker.borrow_mut().get_or_insert_default().observe(
                            &RX_ORDER,
                            sequence,
                            public_qos_sequence(raw),
                        );
                    });
                }

                RX_S_MPDU.observe(metadata.s_mpdu);
                RX_AMPDU.observe(metadata.ampdu);
                if self.phy_sample_cursor.fetch_add(1, Ordering::Relaxed) & 63 == 0
                    && let Some(phy) = decode_rx_phy_info(raw)
                {
                    LAST_FORMAT.store(u32::from(phy.baseband_format().raw()), Ordering::Relaxed);
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
                        RX_PHY.observe_he_mcs(signal.mcs);
                    } else {
                        RX_PHY.observe_other();
                    }
                    LAST_PHY.store(packed, Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }
}

fn ipv4_udp_destination_port(frame: EthernetFrameParts<'_>) -> Option<u16> {
    if frame.ether_type != 0x0800 {
        return None;
    }
    let version_and_ihl = *frame.payload.first()?;
    if version_and_ihl >> 4 != 4 || *frame.payload.get(9)? != 17 {
        return None;
    }
    let header_length = usize::from(version_and_ihl & 0x0f).checked_mul(4)?;
    if header_length < 20 {
        return None;
    }
    Some(u16::from_be_bytes([
        *frame.payload.get(header_length + 2)?,
        *frame.payload.get(header_length + 3)?,
    ]))
}

#[cfg(feature = "rx-order-telemetry")]
fn ipv4_udp_sequence(frame: EthernetFrameParts<'_>, destination_port: u16) -> Option<i32> {
    if ipv4_udp_destination_port(frame) != Some(destination_port) {
        return None;
    }
    let header_length = usize::from(*frame.payload.first()? & 0x0f).checked_mul(4)?;
    let sequence_offset = header_length.checked_add(8)?;
    let encoded: [u8; 4] = frame
        .payload
        .get(sequence_offset..sequence_offset + 4)?
        .try_into()
        .ok()?;
    Some(i32::from_be_bytes(encoded))
}

#[cfg(feature = "rx-order-telemetry")]
fn public_qos_sequence(raw: &[u8]) -> Option<(u8, u16)> {
    const DATA_TYPE: u16 = 0x0008;
    const DATA_TYPE_MASK: u16 = 0x000c;
    const QOS_SUBTYPE: u16 = 0x0080;
    const TO_FROM_DS: u16 = 0x0300;

    let frame_offset = PUBLIC_HEADER_SIZE;
    let frame_control = u16::from_le_bytes([*raw.get(frame_offset)?, *raw.get(frame_offset + 1)?]);
    if frame_control & (DATA_TYPE_MASK | QOS_SUBTYPE) != DATA_TYPE | QOS_SUBTYPE {
        return None;
    }
    let sequence_control =
        u16::from_le_bytes([*raw.get(frame_offset + 22)?, *raw.get(frame_offset + 23)?]);
    let qos_offset = frame_offset + 24 + usize::from(frame_control & TO_FROM_DS == TO_FROM_DS) * 6;
    let tid = *raw.get(qos_offset)? & 0x0f;
    Some((tid, sequence_control >> 4))
}
