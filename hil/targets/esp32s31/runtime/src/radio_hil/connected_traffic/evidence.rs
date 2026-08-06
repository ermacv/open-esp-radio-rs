//! Pure parsing and interval evidence for connected HIL traffic.
//!
//! Socket ownership, task placement and reporting remain in the composition
//! root. This module owns only the byte-level protocol interpretation shared
//! by the connected RX observer and traffic workloads.

#![forbid(unsafe_code)]

use open_esp_radio::{
    esp32s31::wifi::lmac::rx::PUBLIC_HEADER_SIZE, wifi::ieee80211::data::EthernetFrameParts,
};

pub(in crate::radio_hil) fn ipv4_udp_destination_port(
    frame: EthernetFrameParts<'_>,
) -> Option<u16> {
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

pub(in crate::radio_hil) fn ipv4_udp_sequence(
    frame: EthernetFrameParts<'_>,
    destination_port: u16,
) -> Option<i32> {
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

pub(in crate::radio_hil) fn public_qos_sequence(raw: &[u8]) -> Option<(u8, u16)> {
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

pub(in crate::radio_hil) fn iperf2_udp_sequence(packet: &[u8]) -> Option<i32> {
    let encoded: [u8; 4] = packet.get(..4)?.try_into().ok()?;
    Some(i32::from_be_bytes(encoded))
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(in crate::radio_hil) struct UdpSequenceEvidence {
    pub first: Option<u32>,
    pub highest: u32,
    pub expected: u32,
    pub gap_events: u32,
    pub forward_missing: u32,
    pub maximum_gap: u32,
    pub maximum_gap_at: Option<u32>,
    pub first_gap_at: Option<u32>,
    pub last_gap_at: Option<u32>,
    pub backward: u32,
    pub adjacent_duplicates: u32,
    pub unsequenced: u32,
    pub maximum_interarrival_micros: u32,
    pub maximum_interarrival_at: Option<u32>,
}

impl UdpSequenceEvidence {
    pub fn observe(&mut self, sequence: Option<i32>) {
        let Some(sequence) = sequence
            .filter(|sequence| *sequence >= 0)
            .map(|value| value as u32)
        else {
            self.unsequenced = self.unsequenced.saturating_add(1);
            return;
        };
        let Some(_) = self.first else {
            self.first = Some(sequence);
            self.highest = sequence;
            self.expected = sequence.saturating_add(1);
            return;
        };
        if sequence == self.expected {
            self.highest = sequence;
            self.expected = sequence.saturating_add(1);
        } else if sequence > self.expected {
            let gap = sequence - self.expected;
            self.gap_events = self.gap_events.saturating_add(1);
            self.forward_missing = self.forward_missing.saturating_add(gap);
            if gap > self.maximum_gap {
                self.maximum_gap = gap;
                self.maximum_gap_at = Some(sequence);
            }
            self.first_gap_at.get_or_insert(sequence);
            self.last_gap_at = Some(sequence);
            self.highest = sequence;
            self.expected = sequence.saturating_add(1);
        } else if sequence.saturating_add(1) == self.expected {
            self.adjacent_duplicates = self.adjacent_duplicates.saturating_add(1);
        } else {
            self.backward = self.backward.saturating_add(1);
        }
    }

    pub fn observe_interarrival(&mut self, sequence: Option<i32>, elapsed_micros: u64) {
        let Some(sequence) = sequence
            .filter(|sequence| *sequence >= 0)
            .map(|value| value as u32)
        else {
            return;
        };
        let elapsed_micros = u32::try_from(elapsed_micros).unwrap_or(u32::MAX);
        if elapsed_micros > self.maximum_interarrival_micros {
            self.maximum_interarrival_micros = elapsed_micros;
            self.maximum_interarrival_at = Some(sequence);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UdpSequenceEvidence;

    #[test]
    fn sequence_evidence_separates_gaps_duplicates_and_backward_packets() {
        let mut evidence = UdpSequenceEvidence::default();
        evidence.observe(Some(10));
        evidence.observe(Some(12));
        evidence.observe(Some(12));
        evidence.observe(Some(9));
        evidence.observe(None);
        evidence.observe_interarrival(Some(12), 7_000);
        assert_eq!(evidence.first, Some(10));
        assert_eq!(evidence.gap_events, 1);
        assert_eq!(evidence.forward_missing, 1);
        assert_eq!(evidence.adjacent_duplicates, 1);
        assert_eq!(evidence.backward, 1);
        assert_eq!(evidence.unsequenced, 1);
        assert_eq!(evidence.maximum_interarrival_micros, 7_000);
    }
}
