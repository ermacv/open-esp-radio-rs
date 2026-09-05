//! Projection of decoded protocol values into the host measurement vocabulary.

use crate::evidence::run::{Measurement, MeasurementUnit as Unit};
use open_esp_radio_hil_protocol::{
    Envelope, Event, EvidenceRecord, LinkHealth, StackUsage, TransportEvidence,
};
use std::collections::BTreeMap;

pub(super) fn observations(
    prefix: &str,
    events: &[Envelope<Event>],
    received_bytes: u64,
) -> Vec<Measurement> {
    let mut records = BTreeMap::new();
    add(
        &mut records,
        prefix,
        "capture.received-bytes",
        received_bytes,
        Unit::Bytes,
    );
    add(
        &mut records,
        prefix,
        "capture.events",
        events.len() as u64,
        Unit::Count,
    );
    for event in events {
        let request = format!("{prefix}.request-{}", event.request_id);
        let session = format!("{prefix}.session-{}", event.session_id);
        match event.body {
            Event::Evidence(EvidenceRecord::Transport(value)) => {
                transport(&mut records, &session, value)
            }
            Event::Evidence(EvidenceRecord::FlowTransport(value)) => {
                transport(
                    &mut records,
                    &format!("{session}.flow-{}", value.flow_id),
                    value.as_session_total(),
                );
            }
            Event::Evidence(EvidenceRecord::Stack(value)) => stack(&mut records, &session, value),
            Event::Evidence(EvidenceRecord::Link(value)) => link(&mut records, &session, value),
            Event::StackUsage(value) => stack(&mut records, &request, value),
            Event::LinkHealth(value) => link(&mut records, &request, value),
            Event::TimebaseProbeCompleted(value) => {
                for (name, count) in [
                    ("intervals", u64::from(value.intervals)),
                    ("early-intervals", u64::from(value.early_intervals)),
                ] {
                    add(
                        &mut records,
                        &request,
                        &format!("timebase.{name}"),
                        count,
                        Unit::Count,
                    );
                }
                for (name, micros) in [
                    ("elapsed", value.elapsed_micros),
                    ("interval.min", value.minimum_interval_micros.into()),
                    ("interval.max", value.maximum_interval_micros.into()),
                ] {
                    add(
                        &mut records,
                        &request,
                        &format!("timebase.{name}"),
                        micros,
                        Unit::Microseconds,
                    );
                }
            }
            Event::WifiScanCompleted(value) => {
                add(
                    &mut records,
                    &request,
                    "scan.elapsed",
                    value.elapsed_micros,
                    Unit::Microseconds,
                );
                for (name, count) in [
                    ("frames", value.observed_frames.into()),
                    ("bss", value.unique_bss.into()),
                    ("dropped-bss", value.dropped_unique_bss.into()),
                ] {
                    add(
                        &mut records,
                        &request,
                        &format!("scan.{name}"),
                        count,
                        Unit::Count,
                    );
                }
            }
            Event::Ieee802154EventStatusProbeCompleted(_) => {
                add(
                    &mut records,
                    &request,
                    "ieee802154.event-status.responses",
                    1,
                    Unit::Count,
                );
            }
            Event::Ieee802154EdEventProbeCompleted(value) => {
                add(
                    &mut records,
                    &request,
                    "ieee802154.ed-event.responses",
                    1,
                    Unit::Count,
                );
                for (attempt, outcome) in [
                    ("first", Some(value.production_ed_first)),
                    ("second", value.production_ed_second),
                ] {
                    use open_esp_radio_hil_protocol::Ieee802154PolledEdOutcome as Ed;
                    if let Some(
                        Ed::Complete { polls, .. }
                        | Ed::Aborted { polls, .. }
                        | Ed::Timeout { polls },
                    ) = outcome
                    {
                        add(
                            &mut records,
                            &request,
                            &format!("ieee802154.ed.{attempt}.polls"),
                            polls.into(),
                            Unit::Count,
                        );
                    }
                }
            }
            Event::WifiAccessPointStopped(value) => {
                for (name, count) in [
                    (
                        "maximum-associated-peers",
                        value.maximum_associated_peers.into(),
                    ),
                    (
                        "maximum-authorized-peers",
                        value.maximum_authorized_peers.into(),
                    ),
                    ("handshake-failures", value.wpa2_handshake_failures),
                    ("handshake-timeouts", value.wpa2_handshake_timeouts),
                ] {
                    add(
                        &mut records,
                        &request,
                        &format!("ap.{name}"),
                        count.into(),
                        Unit::Count,
                    );
                }
            }
            Event::WifiMonitorStopped(value) | Event::WifiMonitorCaptureCompleted(value) => {
                add(
                    &mut records,
                    &request,
                    "monitor.elapsed",
                    value.elapsed_micros,
                    Unit::Microseconds,
                );
                add(
                    &mut records,
                    &request,
                    "monitor.bytes",
                    value.captured_bytes,
                    Unit::Bytes,
                );
                for (name, count) in [
                    ("frames", value.captured_frames),
                    ("published-frames", value.published_frames),
                    ("full-drops", value.full_drops),
                    ("oversized-drops", value.oversized_drops),
                    ("channel-mismatches", value.channel_mismatches),
                    ("generation-mismatches", value.generation_mismatches),
                    ("exported-frames", value.exported_frames),
                ] {
                    add(
                        &mut records,
                        &request,
                        &format!("monitor.{name}"),
                        count.into(),
                        Unit::Count,
                    );
                }
            }
            // Qualifying raw register images, metadata or a radio feature is
            // outside this numeric projection. Those typed events remain in
            // protocol.jsonl and keep their workload-specific validator.
            _ => {}
        }
    }
    records.into_values().collect()
}

fn add(
    records: &mut BTreeMap<String, Measurement>,
    prefix: &str,
    name: &str,
    value: u64,
    unit: Unit,
) {
    let name = format!("{prefix}.{name}");
    // Retained traffic evidence is replayed under a new request ID. The first
    // publication is the observation; replay must not duplicate or replace it.
    records
        .entry(name.clone())
        .or_insert_with(|| Measurement::observed(name, value, unit));
}

fn transport(records: &mut BTreeMap<String, Measurement>, prefix: &str, value: TransportEvidence) {
    for (name, count) in [
        ("rx.units", value.rx_units),
        ("tx.units", value.tx_units),
        ("errors", value.transport_errors.into()),
    ] {
        add(
            records,
            prefix,
            &format!("transport.{name}"),
            count,
            Unit::Count,
        );
    }
    add(
        records,
        prefix,
        "transport.elapsed",
        value.elapsed_micros,
        Unit::Microseconds,
    );
    for (direction, bytes) in [("rx", value.rx_bytes), ("tx", value.tx_bytes)] {
        add(
            records,
            prefix,
            &format!("transport.{direction}.bytes"),
            bytes,
            Unit::Bytes,
        );
        if value.elapsed_micros != 0 {
            let bps = (u128::from(bytes) * 8_000_000 / u128::from(value.elapsed_micros))
                .min(u128::from(u64::MAX)) as u64;
            add(
                records,
                prefix,
                &format!("transport.{direction}.rate"),
                bps,
                Unit::BitsPerSecond,
            );
        }
    }
}

fn stack(records: &mut BTreeMap<String, Measurement>, prefix: &str, value: StackUsage) {
    for (core, watermark) in [("cpu0", value.cpu0), ("cpu1", value.cpu1)] {
        for (name, bytes) in [
            ("capacity", watermark.capacity_bytes),
            ("free", watermark.free_bytes),
            ("minimum-free", watermark.minimum_free_bytes),
        ] {
            add(
                records,
                prefix,
                &format!("stack.{core}.{name}"),
                bytes.into(),
                Unit::Bytes,
            );
        }
    }
}

fn link(records: &mut BTreeMap<String, Measurement>, prefix: &str, value: LinkHealth) {
    for (name, count) in [
        ("rx.frames", value.rx_frames),
        ("rx.cobs-errors", value.rx_cobs_errors),
        ("rx.checksum-errors", value.rx_checksum_errors),
        ("rx.decode-errors", value.rx_decode_errors),
        ("rx.overflows", value.rx_overflows),
        ("tx.frames", value.tx_frames),
        ("tx.dropped", value.tx_dropped),
        ("text.dropped", value.text_dropped),
        ("text.truncated", value.text_truncated),
    ] {
        add(
            records,
            prefix,
            &format!("link.lifetime.{name}"),
            count.into(),
            Unit::Count,
        );
    }
}
