//! Projection of decoded protocol values into the host measurement vocabulary.

use oer_hil_link::Received;
use oer_hil_protocol::{
    base::LinkHealth, network::EvidenceRecord, network::TransportEvidence, system::StackUsage,
};
use oer_hil_run_bundle::run::{Measurement, MeasurementUnit as Unit};
use std::collections::BTreeMap;

pub(super) fn observations(
    prefix: &str,
    messages: &[Received],
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
        messages.len() as u64,
        Unit::Count,
    );
    for message in messages {
        let request = format!("{prefix}.request-{}", message.request_id);
        let session = format!("{prefix}.session-{}", message.session_id);
        if let Some(value) = message.decode::<LinkHealth>() {
            link(&mut records, &request, value);
        }
        if let Some(oer_hil_protocol::network::Evidence(EvidenceRecord::Transport(value))) =
            message.decode()
        {
            transport(&mut records, &session, value)
        } else if let Some(oer_hil_protocol::network::Evidence(EvidenceRecord::FlowTransport(
            value,
        ))) = message.decode()
        {
            transport(
                &mut records,
                &format!("{session}.flow-{}", value.flow_id),
                value.as_session_total(),
            );
        } else if let Some(oer_hil_protocol::network::Evidence(EvidenceRecord::Stack(value))) =
            message.decode()
        {
            stack(&mut records, &session, value);
        } else if let Some(oer_hil_protocol::network::Evidence(EvidenceRecord::Link(value))) =
            message.decode()
        {
            link(&mut records, &session, value);
        } else if let Some(oer_hil_protocol::system::Stacks(value)) = message.decode() {
            stack(&mut records, &request, value);
        } else if let Some(oer_hil_protocol::system::InterruptStacks { cpu0, cpu1 }) =
            message.decode()
        {
            stack_values(
                &mut records,
                &request,
                [("cpu0-irq", cpu0), ("cpu1-irq", cpu1)],
            );
        } else if let Some(oer_hil_protocol::system::MemoryBenchmarkCompleted(value)) =
            message.decode()
        {
            for (name, count) in [
                (
                    "iterations.completed",
                    u64::from(value.completed_iterations),
                ),
                ("iterations.requested", u64::from(value.request.iterations)),
                ("frames-per-iteration", u64::from(value.request.frames)),
                ("elapsed.cycles", value.elapsed_cycles),
                ("elapsed.instructions", value.elapsed_instructions),
                ("foreground.cycles", value.foreground_cycles),
                ("foreground.instructions", value.foreground_instructions),
                ("transfer.polls", u64::from(value.polls)),
            ] {
                add(
                    &mut records,
                    &request,
                    &format!("memory.{name}"),
                    count,
                    Unit::Count,
                );
            }
            add(
                &mut records,
                &request,
                "memory.bytes-per-iteration",
                u64::from(value.request.bytes) * u64::from(value.request.frames),
                Unit::Bytes,
            );
            add(
                &mut records,
                &request,
                "memory.bytes-per-frame",
                u64::from(value.request.bytes),
                Unit::Bytes,
            );
            add(
                &mut records,
                &request,
                "memory.elapsed",
                value.elapsed_micros,
                Unit::Microseconds,
            );
        } else if let Some(oer_hil_protocol::system::TimebaseProbed(value)) = message.decode() {
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
        } else if let Some(oer_hil_protocol::wifi::ScanCompleted(value)) = message.decode() {
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
        } else if let Some(oer_hil_protocol::ieee802154::AirCheckCompleted(value)) =
            message.decode()
        {
            add(
                &mut records,
                &request,
                "ieee802154.air-check.completed-cycles",
                value.completed_cycles.into(),
                Unit::Count,
            );
        } else if let Some(oer_hil_protocol::ieee802154::EventStatusProbed(_)) = message.decode() {
            add(
                &mut records,
                &request,
                "ieee802154.event-status.responses",
                1,
                Unit::Count,
            );
        } else if let Some(oer_hil_protocol::ieee802154::EdEventProbed(value)) = message.decode() {
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
                use oer_hil_protocol::ieee802154::Ieee802154PolledEdOutcome as Ed;
                if let Some(
                    Ed::Complete { polls, .. } | Ed::Aborted { polls, .. } | Ed::Timeout { polls },
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
        } else if let Some(oer_hil_protocol::wifi::AccessPointStopped(value)) = message.decode() {
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
        } else if let Some(oer_hil_protocol::wifi::AccessPointAggregateFill(value)) =
            message.decode()
        {
            let prefix = format!("ap.aggregate-fill.aid-{}", value.association_id);
            for (name, count) in [
                ("aggregates", value.aggregates),
                ("subframes", value.subframes),
                ("maximum-subframes", u32::from(value.maximum_subframes)),
                ("histogram-1", value.histogram[0]),
                ("histogram-2-7", value.histogram[1]),
                ("histogram-8-15", value.histogram[2]),
                ("histogram-16-31", value.histogram[3]),
                ("histogram-32", value.histogram[4]),
            ] {
                add(
                    &mut records,
                    &request,
                    &format!("{prefix}.{name}"),
                    count.into(),
                    Unit::Count,
                );
            }
        } else if let Some(value) = message
            .decode()
            .map(|oer_hil_protocol::wifi::MonitorStopped(value)| value)
            .or_else(|| {
                message
                    .decode()
                    .map(|oer_hil_protocol::wifi::MonitorCaptureCompleted(value)| value)
            })
        {
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
    stack_values(
        records,
        prefix,
        [
            ("cpu0", Some(value.cpu0)),
            ("cpu1", Some(value.cpu1)),
            ("cpu0-irq", value.cpu0_irq),
            ("cpu1-irq", value.cpu1_irq),
        ],
    );
}

fn stack_values<const N: usize>(
    records: &mut BTreeMap<String, Measurement>,
    prefix: &str,
    values: [(&str, Option<oer_hil_protocol::system::StackWatermark>); N],
) {
    for (core, watermark) in values {
        let Some(watermark) = watermark else { continue };
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
