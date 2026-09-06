//! Observation-only RX evidence for the product HIL composition.
//!
//! The production driver owns classification and delivery. This module only
//! samples borrowed semantic events and never consumes, mutates or delays
//! their delivery to the network sink.

#![forbid(unsafe_code)]

#[cfg(feature = "rx-delivery-telemetry")]
use core::cell::RefCell;
use core::sync::atomic::AtomicU32;
#[cfg(feature = "driver-observation")]
use core::sync::atomic::Ordering;

#[cfg(feature = "rx-delivery-telemetry")]
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
#[cfg(feature = "driver-observation")]
use open_esp_radio_esp32s31_embassy_wifi::{
    Esp32s31ConnectedRxObservation, Esp32s31ConnectedRxObserver, Esp32s31RxEvidence,
    RxObservedEthernetFrame,
};
#[cfg(feature = "rx-delivery-telemetry")]
use open_esp_radio_esp32s31_embassy_wifi::{RxNetworkDeliveryEvent, RxNetworkDeliveryObserver};
#[cfg(feature = "rx-delivery-telemetry")]
use open_esp_radio_hil_esp32s31_telemetry::rx_delivery::{NetworkDropReason, RxDeliveryTracker};
use open_esp_radio_hil_esp32s31_telemetry::rx_evidence::{
    RxAmpduCounters, RxPhyCounters, RxSmpduCounters,
};
#[cfg(feature = "rx-delivery-telemetry")]
use open_esp_radio_hil_protocol::{RxDeliveryEvidence, RxReorderDeliveryEvidence};
#[cfg(feature = "rx-delivery-telemetry")]
use open_esp_radio_network::{FrameLengthError, RxEnqueueError};

pub(crate) static RX_PHY: RxPhyCounters = RxPhyCounters::new();
pub(crate) static RX_S_MPDU: RxSmpduCounters = RxSmpduCounters::new();
pub(crate) static BEACON_S_MPDU: RxSmpduCounters = RxSmpduCounters::new();
pub(crate) static RX_AMPDU: RxAmpduCounters = RxAmpduCounters::new();
#[cfg(feature = "rx-delivery-telemetry")]
static RX_DELIVERY: Mutex<CriticalSectionRawMutex, RefCell<Option<RxDeliveryTracker<128>>>> =
    Mutex::new(RefCell::new(None));
pub(crate) static LAST_FORMAT: AtomicU32 = AtomicU32::new(u32::MAX);
pub(crate) static LAST_PHY: AtomicU32 = AtomicU32::new(u32::MAX);

#[cfg(feature = "driver-observation")]
pub(crate) struct HilConnectedRxObserver {
    udp_port: u16,
}

#[cfg(feature = "driver-observation")]
impl HilConnectedRxObserver {
    pub(crate) const fn new(udp_port: u16) -> Self {
        Self { udp_port }
    }

    #[cfg(feature = "rx-delivery-telemetry")]
    pub(crate) fn begin_delivery_session(session_id: u64) {
        RX_DELIVERY.lock(|tracker| {
            let mut tracker = tracker.borrow_mut();
            tracker
                .get_or_insert_with(RxDeliveryTracker::new)
                .begin(session_id);
        });
    }

    #[cfg(feature = "rx-delivery-telemetry")]
    pub(crate) fn observe_udp_consumer(session_id: u64, sequence: i32) {
        RX_DELIVERY.lock(|tracker| {
            if let Some(tracker) = tracker.borrow_mut().as_mut() {
                tracker.consumed(session_id, sequence);
            }
        });
    }

    #[cfg(feature = "rx-delivery-telemetry")]
    pub(crate) fn finish_delivery_session(
        session_id: u64,
        reorder: RxReorderDeliveryEvidence,
    ) -> Option<RxDeliveryEvidence> {
        RX_DELIVERY.lock(|tracker| {
            tracker
                .borrow_mut()
                .as_mut()
                .and_then(|tracker| tracker.finish(session_id, reorder))
        })
    }
}

#[cfg(feature = "driver-observation")]
impl Esp32s31ConnectedRxObserver for HilConnectedRxObserver {
    fn requests_phy(&self, frame: RxObservedEthernetFrame<'_>) -> bool {
        // A strict interval vector gate cannot be based on one out of every
        // 64 packets: a fallback vector could otherwise remain invisible.
        // The observer already classifies every benchmark UDP publication;
        // requesting its decoded value adds no raw-prefix ownership or wait.
        ipv4_udp_destination_port(frame) == Some(self.udp_port)
    }

    fn observe(&self, event: Esp32s31ConnectedRxObservation<'_>) {
        match event {
            Esp32s31ConnectedRxObservation::Beacon { s_mpdu } => {
                observe_s_mpdu(&BEACON_S_MPDU, s_mpdu);
            }
            Esp32s31ConnectedRxObservation::Ethernet {
                frame,
                s_mpdu,
                ampdu,
                phy,
            } if ipv4_udp_destination_port(frame) == Some(self.udp_port) => {
                observe_s_mpdu(&RX_S_MPDU, s_mpdu);
                observe_ampdu(&RX_AMPDU, ampdu);
                if let Some(phy) = available(phy) {
                    LAST_FORMAT.store(u32::from(phy.baseband_format), Ordering::Relaxed);
                    let mut packed = u32::from(phy.baseband_format) | (u32::from(phy.rate) << 4);
                    if let Some(signal) = phy.ht {
                        packed |= (1 << 30)
                            | (u32::from(signal.mcs) << 9)
                            | (u32::from(signal.short_guard_interval) << 16)
                            | (u32::from(signal.bandwidth_mhz == 40) << 17);
                        RX_PHY.observe_ht(
                            signal.mcs,
                            signal.bandwidth_mhz,
                            signal.short_guard_interval,
                        );
                    } else if let Some(signal) = phy.he_su {
                        let bandwidth = match signal.bandwidth_mhz {
                            20 => 0,
                            40 => 1,
                            80 => 2,
                            _ => 3,
                        };
                        packed |= (1 << 31)
                            | (u32::from(signal.mcs) << 9)
                            | (u32::from(signal.guard_interval_and_ltf) << 13)
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

#[cfg(feature = "rx-delivery-telemetry")]
impl RxNetworkDeliveryObserver for HilConnectedRxObserver {
    fn admitted(&self, event: RxNetworkDeliveryEvent<'_>) {
        let Some(sequence) = ipv4_udp_sequence(event.frame, self.udp_port) else {
            return;
        };
        RX_DELIVERY.lock(|tracker| {
            if let Some(tracker) = tracker.borrow_mut().as_mut() {
                tracker.admitted(
                    sequence,
                    event.qos_sequence.map(|qos| (qos.tid, qos.sequence)),
                );
            }
        });
    }

    fn dropped(&self, event: RxNetworkDeliveryEvent<'_>, error: RxEnqueueError) {
        let Some(sequence) = ipv4_udp_sequence(event.frame, self.udp_port) else {
            return;
        };
        let reason = match error {
            RxEnqueueError::QueueFull => NetworkDropReason::QueueFull,
            RxEnqueueError::PoolExhausted => NetworkDropReason::PoolExhausted,
            RxEnqueueError::LinkDown => NetworkDropReason::LinkDown,
            RxEnqueueError::InvalidLength(
                FrameLengthError::TooShort | FrameLengthError::TooLong,
            ) => NetworkDropReason::InvalidLength,
        };
        RX_DELIVERY.lock(|tracker| {
            if let Some(tracker) = tracker.borrow_mut().as_mut() {
                tracker.dropped(
                    sequence,
                    event.qos_sequence.map(|qos| (qos.tid, qos.sequence)),
                    reason,
                );
            }
        });
    }
}

#[cfg(feature = "driver-observation")]
fn ipv4_udp_destination_port(frame: RxObservedEthernetFrame<'_>) -> Option<u16> {
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

#[cfg(feature = "rx-delivery-telemetry")]
fn ipv4_udp_sequence(frame: RxObservedEthernetFrame<'_>, destination_port: u16) -> Option<i32> {
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

#[cfg(feature = "driver-observation")]
fn observe_s_mpdu(counter: &RxSmpduCounters, evidence: Esp32s31RxEvidence<bool>) {
    match evidence {
        Esp32s31RxEvidence::Hardware(value) => counter.observe_hardware(value),
        Esp32s31RxEvidence::Protocol(_) | Esp32s31RxEvidence::Unavailable => {
            counter.observe_unavailable();
        }
    }
}

#[cfg(feature = "driver-observation")]
fn observe_ampdu(counter: &RxAmpduCounters, evidence: Esp32s31RxEvidence<bool>) {
    match evidence {
        Esp32s31RxEvidence::Hardware(value) => counter.observe_hardware(value),
        Esp32s31RxEvidence::Protocol(value) => counter.observe_protocol(value),
        Esp32s31RxEvidence::Unavailable => counter.observe_unavailable(),
    }
}

#[cfg(feature = "driver-observation")]
fn available<T>(evidence: Esp32s31RxEvidence<T>) -> Option<T> {
    match evidence {
        Esp32s31RxEvidence::Hardware(value) | Esp32s31RxEvidence::Protocol(value) => Some(value),
        Esp32s31RxEvidence::Unavailable => None,
    }
}
