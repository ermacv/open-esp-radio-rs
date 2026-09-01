use super::*;

pub(crate) fn task_poll_markdown(task_polls: TaskPollSet) -> String {
    if !task_polls.is_complete() {
        return String::from(
            "## Embassy task poll residence\n\n\
             Not collected in the ordinary throughput image. Use the explicit \
             `radio-poll-profile` HIL scenario for diagnostic instrumentation.\n\n",
        );
    }
    let mut output = String::from(
        "## Embassy task poll residence\n\n\
         Wall time includes interrupt preemption. Counters are diagnostic HIL instrumentation, \
         not a production scheduling policy.\n\n",
    );
    for (name, evidence) in [
        ("network", task_polls.network),
        ("radio", task_polls.radio),
        ("udp_rx", task_polls.udp_rx),
        ("udp_tx", task_polls.udp_tx),
        ("tcp", task_polls.tcp),
    ] {
        let average_us = evidence.poll_us as f64 / evidence.polls.max(1) as f64;
        writeln!(
            output,
            "- `{name}`: {} polls / {} us total / {average_us:.2} us average / {} us boot \
             maximum; >100/500/1000/5000 us: {}/{}/{}/{} ({} interval records)",
            evidence.polls,
            evidence.poll_us,
            evidence.poll_boot_max_us,
            evidence.over_100us,
            evidence.over_500us,
            evidence.over_1000us,
            evidence.over_5000us,
            evidence.intervals,
        )
        .expect("writing task-poll evidence to String cannot fail");
    }
    output.push('\n');
    output
}

fn network_scheduler_markdown(
    evidence: Option<open_esp_radio_hil_protocol::NetworkSchedulerEvidence>,
) -> String {
    let Some(evidence) = evidence else {
        return String::from(
            "## Cooperative network scheduler\n\nNot collected in this image.\n\n",
        );
    };
    format!(
        "## Cooperative network scheduler\n\n\
         - Polls: `{}`\n\
         - Ingress calls/packets: `{}` / `{}`; egress passes/TX tokens: `{}` / `{}`\n\
         - Egress credit blocks: `{}`; ingress/egress budget exhaustion: `{}` / `{}`\n\
         - Start ingress/egress: `{}` / `{}`; exit drained/work/credit: `{}` / `{}` / `{}`\n\n",
        evidence.polls,
        evidence.ingress_calls,
        evidence.ingress_packets,
        evidence.egress_passes,
        evidence.egress_tx_tokens,
        evidence.egress_blocked,
        evidence.ingress_budget_exhausted,
        evidence.egress_budget_exhausted,
        evidence.started_with_ingress,
        evidence.started_with_egress,
        evidence.exit_drained,
        evidence.exit_work_budget,
        evidence.exit_egress_credit,
    )
}

pub(crate) fn udp_sequence_markdown(sequence: UdpSequenceEvidence, host_datagrams: u64) -> String {
    let value = |value: Option<u64>| {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| String::from("not-observed"))
    };
    let tail_missing = sequence
        .highest
        .map(|highest| host_datagrams.saturating_sub(highest.saturating_add(1)))
        .unwrap_or(host_datagrams);
    let unrecovered_forward = sequence.forward_missing.saturating_sub(sequence.backward);
    format!(
        "## UDP sequence evidence\n\n\
         - Interval records: `{}`; first/highest/next sequence: `{}` / `{}` / `{}`; host tail after highest: `{tail_missing}`\n\
         - Forward gap events/missing observations/unrecovered after backward arrivals: `{}` / `{}` / `{unrecovered_forward}`\n\
         - Maximum gap/sequence after it: `{}` / `{}`; first/last sequence after a gap: `{}` / `{}`\n\
         - Backward observations/adjacent duplicates/unsequenced datagrams: `{}` / `{}` / `{}`\n\
         - Maximum application interarrival/sequence: `{} us` / `{}`\n\
         - Forward-missing observations are not reduced by later backward arrivals; the two counters remain separate so reordering is not mislabeled as loss.\n\n",
        sequence.intervals,
        value(sequence.first),
        value(sequence.highest),
        value(sequence.next),
        sequence.gap_events,
        sequence.forward_missing,
        sequence.maximum_gap,
        value(sequence.maximum_gap_at),
        value(sequence.first_gap_at),
        value(sequence.last_gap_at),
        sequence.backward,
        sequence.adjacent_duplicates,
        sequence.unsequenced,
        sequence.maximum_interarrival_us,
        value(sequence.maximum_interarrival_at),
    )
}

pub(crate) fn rx_order_markdown(order: RxOrderEvidence) -> String {
    if order.intervals == 0 {
        return String::from(
            "## RX order correlation\n\n\
             Not collected in the ordinary throughput image. Use the explicit \
             RX-order profile for this traffic scenario to correlate UDP and 802.11 ordering.\n\n",
        );
    }
    let classified = order
        .backward_mac_backward
        .saturating_add(order.backward_mac_same)
        .saturating_add(order.backward_mac_forward)
        .saturating_add(order.backward_mac_other_tid)
        .saturating_add(order.backward_mac_unavailable);
    format!(
        "## RX order correlation\n\n\
         - Observer interval records: `{}`; UDP gap events/forward-missing/backward/adjacent duplicates: `{}` / `{}` / `{}` / `{}`\n\
         - Backward UDP classified by 802.11 sequence: MAC-backward `{}`, same MPDU `{}`, MAC-forward `{}`, different TID `{}`, unavailable `{}`; classified `{classified}/{}`\n\
         - MAC-backward is direct evidence that an MPDU crossed the open driver out of its negotiated per-TID BlockAck order. MAC-forward means the application order had already changed before the peer assigned 802.11 sequence numbers. Same-MPDU records can be distinct A-MSDU subframes.\n\n",
        order.intervals,
        order.gap_events,
        order.forward_missing,
        order.backward,
        order.adjacent_duplicates,
        order.backward_mac_backward,
        order.backward_mac_same,
        order.backward_mac_forward,
        order.backward_mac_other_tid,
        order.backward_mac_unavailable,
        order.backward,
    )
}

pub(crate) fn rx_reorder_markdown(reorder: RxReorderEvidence) -> String {
    format!(
        "## RX BlockAck reorder\n\n\
         - Agreement interval records/starts/stops: `{}` / `{}` / `{}`; last TID/start/window: `{}` / `{}` / `{}`\n\
         - First-frame samples/TID/start/sequence/distance: `{}` / `{}` / `{}` / `{}` / `{}`\n\
         - Buffered/released/missing/stale/gap expiries: `{}` / `{}` / `{}` / `{}` / `{}`\n\
         - Occupied at report/maximum: `{}` / `{}` of `{}` negotiated slots\n\n",
        reorder.intervals,
        reorder.starts,
        reorder.stops,
        reorder.last_start_tid,
        reorder.last_start_sequence,
        reorder.window,
        reorder.first_samples,
        reorder.first_tid,
        reorder.first_start_sequence,
        reorder.first_frame_sequence,
        reorder.first_distance,
        reorder.buffered,
        reorder.released,
        reorder.missing,
        reorder.stale,
        reorder.gap_expiries,
        reorder.occupied,
        reorder.maximum_occupied,
        reorder.window,
    )
}

pub(super) struct BidirectionalPerformanceReport<'a> {
    pub(super) options: &'a Options,
    pub(super) host_offer: HostTransmission,
    pub(super) host_sink: Burst,
    pub(super) structured: SessionEvidence,
    pub(super) rx_kbps: u64,
    pub(super) tx_kbps: u64,
    pub(super) host_receive_buffer_bytes: usize,
}

pub(super) fn write_bidirectional_performance_report(
    output: &Path,
    report: BidirectionalPerformanceReport<'_>,
) -> Result<()> {
    let BidirectionalPerformanceReport {
        options,
        host_offer,
        host_sink,
        structured,
        rx_kbps,
        tx_kbps,
        host_receive_buffer_bytes,
    } = report;
    let target_tx_rate = options
        .tx_rate_bps
        .map(|rate| format!("{:.3} Mbit/s", rate as f64 / 1_000_000.0))
        .unwrap_or_else(|| String::from("saturated"));
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio {} bidirectional performance HIL\n\n\
             - Result: `PASS`\n\
             - Evidence boundary: `transport, external host source/sink, stack watermark; driver observation not collected`\n\
             - Device: `{}`\n\
             - Requested/actual downlink offer: `{:.3}` / `{:.3} Mbit/s`\n\
             - Target uplink offered-rate bound: `{target_tx_rate}`\n\
             - Target RX/TX throughput: `{:.3}` / `{:.3} Mbit/s`; combined `{:.3} Mbit/s`\n\
             - Host received target TX: `{:.3} Mbit/s`, `{}` bytes in `{}` datagrams\n\
             - Host target-TX missing/reordered/duplicate datagrams (informational): `{}` / `{}` / `{}`\n\
             - Target transport RX/TX: `{}` / `{}` bytes; `{}` / `{}` datagrams; `{}` us\n\
             - Host UDP `SO_RCVBUF` read-back: `{host_receive_buffer_bytes}` bytes\n\
             - Stack minimum free: CPU0 `{}/{}` bytes (required `{}`); CPU1 `{}/{}` bytes (required `{}`)\n\
             - Evidence CRC32C: `0x{:08x}`\n\n\
             UART evidence is in [`uart.log`](uart.log).\n",
            options.phy.name().to_uppercase(),
            options.address,
            options.rate_bps as f64 / 1_000_000.0,
            host_offer.throughput_bps() as f64 / 1_000_000.0,
            rx_kbps as f64 / 1_000.0,
            tx_kbps as f64 / 1_000.0,
            rx_kbps.saturating_add(tx_kbps) as f64 / 1_000.0,
            host_sink.throughput_kbps() as f64 / 1_000.0,
            host_sink.bytes,
            host_sink.datagrams,
            host_sink.missing,
            host_sink.reordered,
            host_sink.duplicates,
            structured.transport.rx_bytes,
            structured.transport.tx_bytes,
            structured.transport.rx_units,
            structured.transport.tx_units,
            structured.transport.elapsed_micros,
            structured.stack.cpu0.free_bytes,
            structured.stack.cpu0.capacity_bytes,
            structured.stack.cpu0.minimum_free_bytes,
            structured.stack.cpu1.free_bytes,
            structured.stack.cpu1.capacity_bytes,
            structured.stack.cpu1.minimum_free_bytes,
            structured.finished.evidence_crc32c,
        ),
    )?;
    Ok(())
}

pub(super) fn write_report(
    output: &Path,
    options: &Options,
    evidence: BidirectionalEvidence,
    require_exact_delivery: bool,
    failure: Option<&str>,
) -> Result<()> {
    let BidirectionalEvidence {
        host_offer: host,
        target_rx: rx,
        target_tx_floor_kbps: tx_floor,
        host_sink: host_tx,
        session: structured,
        ampdu,
        host_receive_buffer_bytes,
        fixture_rx,
        fixture_tx,
        tx_monitor_rx,
        independent_air_rx,
    } = evidence;
    let rx_median = rx.throughput_median_kbps;
    let pipeline = rx.pipeline;
    let irq = rx.irq;
    let average_service_us = pipeline.service_us as f64 / pipeline.admitted_frames.max(1) as f64;
    let average_reload_us = pipeline.reload_us as f64 / pipeline.reload_transactions.max(1) as f64;
    let average_dispatch_us = pipeline.dispatch_us as f64 / pipeline.protocol_frames.max(1) as f64;
    let average_publish_us =
        pipeline.network_publish_us as f64 / pipeline.network_publications.max(1) as f64;
    let average_wait_us =
        pipeline.network_ready_wait_us as f64 / pipeline.network_ready_waits.max(1) as f64;
    let average_irq_service_us =
        pipeline.rx_irq_to_service_us as f64 / pipeline.rx_irq_service_samples.max(1) as f64;
    let average_subframes = ampdu.subframes as f64 / ampdu.aggregates.max(1) as f64;
    let full32_percent = ampdu.full32 as f64 * 100.0 / ampdu.aggregates.max(1) as f64;
    let average_preparation_us = ampdu.preparation_us as f64 / ampdu.aggregates.max(1) as f64;
    let average_publication_us = ampdu.publication_us as f64 / ampdu.publications.max(1) as f64;
    let terminal_exchanges = ampdu
        .completed
        .saturating_add(ampdu.timeout)
        .saturating_add(ampdu.collision);
    let average_exchange_us = ampdu.exchange_us as f64 / terminal_exchanges.max(1) as f64;
    let average_first_exchange_us =
        ampdu.first_exchange_us as f64 / ampdu.first_exchanges.max(1) as f64;
    let average_retry_exchange_us =
        ampdu.retry_exchange_us as f64 / ampdu.retried_exchanges.max(1) as f64;
    let average_tx_irq_service_us =
        ampdu.tx_irq_service_us as f64 / ampdu.tx_irq_samples.max(1) as f64;
    let average_tx_flight_us = ampdu.tx_flight_us as f64 / ampdu.tx_flight_samples.max(1) as f64;
    let task_poll_report = task_poll_markdown(rx.task_polls);
    let network_scheduler_report = network_scheduler_markdown(structured.network_scheduler);
    let udp_sequence_report = udp_sequence_markdown(rx.sequence, host.datagrams);
    let rx_order_report = rx_order_markdown(rx.order);
    let rx_reorder_report = rx_reorder_markdown(rx.reorder);
    let typed_delivery_report = structured
        .rx_delivery
        .map(|delivery| evidence::rx_delivery::markdown(host.datagrams, delivery))
        .unwrap_or_else(|| {
            String::from(
                "## Typed RX delivery frontier\n\nNot collected in this image. Use the explicit RX-delivery profile.\n\n",
            )
        });
    let target_tx_rate = options
        .tx_rate_bps
        .map(|rate| format!("{:.3} Mbit/s", rate as f64 / 1_000_000.0))
        .unwrap_or_else(|| String::from("saturated"));
    let required_tx_floor = options
        .tx_floor_bps
        .map(|rate| format!("{:.3} Mbit/s", rate as f64 / 1_000_000.0))
        .unwrap_or_else(|| String::from("not configured"));
    let host_tx_mbps = host_tx.throughput_kbps() as f64 / 1_000.0;
    let host_tx_bytes = host_tx.bytes;
    let host_tx_datagrams = host_tx.datagrams;
    let host_tx_missing = host_tx.missing;
    let host_tx_reordered = host_tx.reordered;
    let host_tx_duplicates = host_tx.duplicates;
    let host_tx_maximum_interarrival_us = host_tx.maximum_interarrival_us;
    let host_tx_sequence_after_maximum_interarrival = host_tx.sequence_after_maximum_interarrival;
    let structured_report = format!(
        "- Typed session RX/TX: `{}` / `{}` bytes; `{}` / `{}` datagrams; CRC32C `0x{:08x}`\n\
                 - Stack minimum free: CPU0 `{}/{}` bytes (required `{}`); CPU1 `{}/{}` bytes (required `{}`)\n",
        structured.transport.rx_bytes,
        structured.transport.tx_bytes,
        structured.transport.rx_units,
        structured.transport.tx_units,
        structured.finished.evidence_crc32c,
        structured.stack.cpu0.free_bytes,
        structured.stack.cpu0.capacity_bytes,
        structured.stack.cpu0.minimum_free_bytes,
        structured.stack.cpu1.free_bytes,
        structured.stack.cpu1.capacity_bytes,
        structured.stack.cpu1.minimum_free_bytes,
    );
    let fixture_report = fixture_rx.as_ref().map_or_else(
        || String::from("- AP-side evidence: `external AP; not observed`\n"),
        |fixture| fixture.markdown(),
    );
    let fixture_tx_report = fixture_tx.as_ref().map_or_else(
        || String::from("- AP wireless ingress evidence: `not observed`\n"),
        |fixture| {
            format!(
                "- Local Linux AP filtered target TX ingress: `{}` packets; channel width: `{}` MHz; TX/RX bitrate: `{}` / `{}`\n",
                fixture.udp_packets,
                fixture.channel_width_mhz,
                fixture.tx_bitrate,
                fixture.rx_bitrate,
            )
        },
    );
    let tx_monitor_report = tx_monitor_rx.as_ref().map_or_else(
        || String::from("## OpenWrt AP TX-monitor frontier\n\nNot collected.\n\n"),
        |air| {
            let target = structured.rx_delivery.map(|delivery| delivery.post_reorder);
            let frontier = target.map_or("target delivery evidence unavailable", |target| {
                if air.unique_units == target.data_units
                    && air.unrecovered
                        == u32::try_from(host.datagrams)
                            .unwrap_or(u32::MAX)
                            .saturating_sub(target.data_units)
                {
                    "at or before the OpenWrt AP TX-monitor tap (tap and target have equal sequence cardinality)"
                } else {
                    "between the OpenWrt AP TX-monitor tap and target post-reorder"
                }
            });
            format!(
                "## OpenWrt AP TX-monitor frontier\n\n\
                 - Classification: `{frontier}`\n\
                 - Capture frames/kernel drops: `{}` / `{}`\n\
                 - UDP publications/unique/duplicates: `{}` / `{}` / `{}`\n\
                 - Gap/missing/late/unrecovered: `{}` / `{}` / `{}` / `{}`\n\
                 - Out-of-range/terminal/retry publications: `{}` / `{}` / `{}`\n\
                 - UDP publications missing MAC metadata: `{}`\n\
                 - First anomaly: `{:?}`\n\
                 - Target post-reorder data units: `{}`\n\n",
                air.captured_frames,
                air.kernel_dropped,
                air.data_units,
                air.unique_units,
                air.duplicates,
                air.gap_events,
                air.forward_missing,
                air.late_recovered,
                air.unrecovered,
                air.out_of_range,
                air.terminal_markers,
                air.mac_retry_publications,
                air.missing_mac_metadata,
                air.first_anomaly,
                target.map_or(0, |target| target.data_units),
            )
        },
    );
    let independent_air_report = match (tx_monitor_rx.as_ref(), independent_air_rx.as_ref()) {
        (Some(ap), Some(observer)) => {
            let ap_with_metadata = ap.mac_units.values().copied().sum::<u32>();
            let observer_with_metadata = observer.mac_units.values().copied().sum::<u32>();
            let ap_known_tid = ap
                .mac_units
                .iter()
                .filter(|(key, _)| key.tid != u8::MAX)
                .map(|(_, count)| count)
                .copied()
                .sum::<u32>();
            let observer_known_tid = observer
                .mac_units
                .iter()
                .filter(|(key, _)| key.tid != u8::MAX)
                .map(|(_, count)| count)
                .copied()
                .sum::<u32>();
            let ap_projected = project_mac_units(&ap.mac_units);
            let observer_projected = project_mac_units(&observer.mac_units);
            let matched = ap_projected
                .iter()
                .map(|(key, count)| count.min(observer_projected.get(key).unwrap_or(&0)))
                .copied()
                .sum::<u32>();
            let not_observed = ap_with_metadata.saturating_sub(matched);
            let observer_extra = observer_with_metadata.saturating_sub(matched);
            format!(
                "## Independent laptop air observer\n\n\
                 - Capture frames/kernel drops: `{}` / `{}`\n\
                 - Logical data MPDUs/retry attempts/missing metadata: `{}` / `{}` / `{}`\n\
                 - AP/observer logical MPDUs with MAC metadata: `{}` / `{}`\n\
                 - AP/observer MPDUs with known QoS TID: `{}` / `{}`\n\
                 - Matched by `(sequence, fragment)`: `{}`\n\
                 - AP MPDUs not observed / observer extras: `{}` / `{}`\n\
                 - Correlation limitation: `ath10k's AP monitor tap omits QoS TID; cross-device matching intentionally projects away TID`\n\
                 - Interpretation: `matched MPDUs prove transmission on air; unmatched MPDUs remain ambiguous because a passive observer may miss frames`\n\n",
                observer.captured_frames,
                observer.kernel_dropped,
                observer.logical_data_units,
                observer.retry_attempts,
                observer.missing_mac_metadata,
                ap_with_metadata,
                observer_with_metadata,
                ap_known_tid,
                observer_known_tid,
                matched,
                not_observed,
                observer_extra,
            )
        }
        (None, Some(_)) => String::from(
            "## Independent laptop air observer\n\nInvalid evidence: AP TX-monitor correlation is absent.\n\n",
        ),
        (_, None) => String::from("## Independent laptop air observer\n\nNot collected.\n\n"),
    };
    let result = if failure.is_some() { "FAIL" } else { "PASS" };
    let failure_report = failure
        .map(|failure| format!("- Acceptance failure: `{failure}`\n"))
        .unwrap_or_default();
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio {} bidirectional HIL\n\n\
             - Result: `{result}`\n\
             - Delivery contract: `{}`\n\
             - Device: `{}`\n\
             - Requested downlink: `{:.3} Mbit/s`\n\
             - Actual host offer: `{:.3} Mbit/s`\n\
             - Host payload: `{}` bytes in `{}` datagrams\n\
             - Target TX payload / offered bound: `{}` bytes / `{target_tx_rate}`\n\
             - Required target-to-host floor: `{required_tx_floor}`\n\
             - Host received target TX: `{host_tx_mbps:.3} Mbit/s`, `{host_tx_bytes}` bytes in `{host_tx_datagrams}` datagrams\n\
             - Host target-TX missing/reordered/duplicate datagrams: `{host_tx_missing}` / `{host_tx_reordered}` / `{host_tx_duplicates}`\n\
             - Host target-TX maximum packet interarrival: `{host_tx_maximum_interarrival_us}` us before sequence `{host_tx_sequence_after_maximum_interarrival:?}`\n\
             - Host UDP `SO_RCVBUF` read-back: `{host_receive_buffer_bytes}` bytes\n\
             {failure_report}\
             {structured_report}\
             {fixture_report}\
             {fixture_tx_report}\
             - Host pacing maximum lateness/catch-up/deadline resets: `{} us` / `{}` datagrams / `{}`\n\
            - Direct RX median: `{:.3} Mbit/s`\n\
             - Received host UDP datagrams: `{}` / `{}`\n\
            - Concurrent open-radio TX floor: `{:.3} Mbit/s`\n\
             - Combined conservative floor: `{:.3} Mbit/s`\n\n\
             {udp_sequence_report}\
             {rx_order_report}\
             {rx_reorder_report}\
             {typed_delivery_report}\
             {tx_monitor_report}\
             {independent_air_report}\
             ## RX evidence\n\n\
             - Enqueued/software-dropped frames: `{}` / `{}`\n\
             - Sampled HE-SU MCS0..11 frame histogram: `{:?}`; other sampled PHY frames: `{}`\n\
             - Complete HT benchmark vectors: MCS0..7 LGI40 `{:?}`, SGI40 `{:?}`, HT20 `{:?}`, other `{}`\n\
             - Benchmark UDP datagrams marked S-MPDU / not S-MPDU / unavailable provenance: `{}` / `{}` / `{}`\n\
             - Connected beacons marked S-MPDU / not S-MPDU / unavailable provenance: `{}` / `{}` / `{}`\n\
             - Benchmark UDP datagrams marked A-MPDU / not A-MPDU / unavailable provenance: `{}` / `{}` / `{}`\n\
             - A-MPDU provenance hardware true/false, protocol true/false: `{}` / `{}`, `{}` / `{}`\n\
             - Hardware BUFFER_FULL/FIFO_OVERFLOW: `{}` / `{}`\n\
             - DMA service calls/frontier/admitted: `{}` / `{}` / `{}`; max frontier/admitted: `{}` / `{}`\n\
             - Service-observed BUFFER_FULL increments/samples: `{}` / `{}`; between/during: `{}` / `{}` increments across `{}` / `{}` services; last boot service/phase/counter/frontier/admitted/pool/queue/service time: `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{} us`\n\
             - Frontier service buckets 0 / 1 / 2-3 / 4-7 / 8-15 / 16-31 / 32+: `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - RX IRQ posts/wake epochs/hard entries/coalesced/sampled services/clock-skew rejects: `{}` / `{}` / `{}` / `{}` / `{}` / `{}`; sampled IRQ-to-service: `{:.2} us` average, `{}` us boot maximum\n\
             - MAC entry causes spurious / RX-work-only / RX-mixed / TX-only / TX-mixed / auxiliary-or-unknown-only: `{}` / `{}` / `{}` / `{}` / `{}` / `{}`; classified `{}` entries; extra snapshots `{}`, loop saturations `{}`, auxiliary STATUS OR `0x{:08x}`, unknown STATUS OR `0x{:08x}`\n\
             - Staged bytes: `{}`; invalid empty/oversize units recycled: `{}` / `{}`; service: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Safe reload transactions: `{}`; `{:.2} us` average, `{}` us boot maximum; `{}` us total\n\
             - Backpressured services (bulk-preserve): `{}` (`{}`); pool/queue credit limited: `{}` / `{}`; maximum deferred frames: `{}`; backpressured pool/queue minimum: `{}` / `{}`; all-service pool/queue floor: `{}` / `{}`\n\
             - Protocol frames/data: `{}` / `{}`; dispatch: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - A-MSDU MPDUs/subframes: `{}` / `{}`; raw unit buckets <=1700 / 1701-3400 / >3400 bytes: `{}` / `{}` / `{}`; boot maximum: `{}` bytes\n\
             - Network publications/bytes: `{}` / `{}`; copy+publish: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Network-ready waits: `{}`; `{:.2} us` average, `{}` us boot maximum\n\n\
             {task_poll_report}\
             {network_scheduler_report}\
             ## A-MPDU evidence\n\n\
             - Prepared/completed/publications: `{}` / `{}` / `{}`\n\
             - Subframes: `{}` total, `{:.2}` average, min `{}`, max `{}`\n\
             - Full 32-member aggregates: `{}` (`{:.2}%`)\n\
             - Size buckets 1 / 2-3 / 4-7 / 8-15 / 16-23 / 24-30 / 31 / 32: \
               `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - Build stop at frame limit / capacity limit / empty queue: \
               `{}` / `{}` / `{}`\n\
             - Acknowledged subframes: `{}`; individual fallback retries: `{}`\n\
             - Hardware timeouts/collisions: `{}` / `{}`\n\
             - BlockAck samples/received/full/partial/empty: `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - BlockAck success-without-valid/control-TID/start-outside/max-start-lag: `{}` / `{}` / `{}` / `{}`\n\
             - Preparation: `{:.2} us` average, `{}` us boot maximum\n\
             - Hardware publication programming: `{:.2} us` average, \
               `{}` us boot maximum\n\
             - Aggregate exchange: `{:.2} us` average, `{}` us boot maximum\n\n\
             - First-publication exchange: `{:.2} us` average, `{}` us boot maximum across `{}` exchanges\n\
             - Retried exchange: `{:.2} us` average, `{}` us boot maximum across `{}` exchanges and `{}` publications\n\
             - TX IRQ wake epochs/samples/clock-skew rejects: `{}` / `{}` / `{}`; IRQ-to-service: `{:.2} us` average, `{}` us boot maximum\n\
             - Sampled publication-to-IRQ flight: `{:.2} us` average, `{}` us boot maximum across `{}` samples\n\n\
             - Standby prepared/published/cancelled: `{}` / `{}` / `{}`\n\n\
             UART evidence is in [`uart.log`](uart.log).\n",
            options.phy.name().to_uppercase(),
            if require_exact_delivery {
                "exact"
            } else {
                "performance-health"
            },
            options.address,
            options.rate_bps as f64 / 1_000_000.0,
            host.throughput_bps() as f64 / 1_000_000.0,
            host.bytes,
            host.datagrams,
            options.tx_payload,
            host.maximum_lateness_us(),
            host.maximum_catch_up_datagrams,
            host.deadline_resets,
            rx_median as f64 / 1_000.0,
            rx.received_datagrams,
            host.datagrams,
            tx_floor as f64 / 1_000.0,
            rx_median.saturating_add(tx_floor) as f64 / 1_000.0,
            rx.enqueued,
            rx.dropped,
            rx.he_mcs_histogram,
            rx.other_phy_frames,
            rx.ht40_long_gi_mcs,
            rx.ht40_short_gi_mcs,
            rx.ht20_mcs,
            rx.ht_other_frames,
            rx.s_mpdu.s_mpdu_datagrams,
            rx.s_mpdu.not_s_mpdu_datagrams,
            rx.s_mpdu.unavailable_datagrams,
            rx.s_mpdu.s_mpdu_beacons,
            rx.s_mpdu.not_s_mpdu_beacons,
            rx.s_mpdu.unavailable_beacons,
            rx.ampdu.ampdu_datagrams,
            rx.ampdu.not_ampdu_datagrams,
            rx.ampdu.unavailable_datagrams,
            rx.ampdu.hardware_ampdu_datagrams,
            rx.ampdu.hardware_not_ampdu_datagrams,
            rx.ampdu.protocol_ampdu_datagrams,
            rx.ampdu.protocol_not_ampdu_datagrams,
            rx.buffer_full,
            rx.fifo_overflow,
            pipeline.service_calls,
            pipeline.frontier_frames,
            pipeline.admitted_frames,
            pipeline.maximum_frontier,
            pipeline.maximum_admitted,
            pipeline.dma_buffer_full_increments,
            pipeline.dma_buffer_full_service_samples,
            pipeline.dma_buffer_full_between_services,
            pipeline.dma_buffer_full_during_services,
            pipeline.dma_buffer_full_between_service_samples,
            pipeline.dma_buffer_full_during_service_samples,
            pipeline.dma_buffer_full_last_service,
            pipeline.dma_buffer_full_last_phase,
            pipeline.dma_buffer_full_last_counter,
            pipeline.dma_buffer_full_last_frontier,
            pipeline.dma_buffer_full_last_admitted,
            pipeline.dma_buffer_full_last_pool_credits,
            pipeline.dma_buffer_full_last_queue_credits,
            pipeline.dma_buffer_full_last_service_us,
            pipeline.frontier_zero_services,
            pipeline.frontier_one_services,
            pipeline.frontier_two_three_services,
            pipeline.frontier_four_seven_services,
            pipeline.frontier_eight_fifteen_services,
            pipeline.frontier_sixteen_thirty_one_services,
            pipeline.frontier_thirty_two_plus_services,
            pipeline.rx_irq_posts,
            pipeline.rx_irq_epochs,
            pipeline.mac_irq_entries,
            pipeline.rx_irq_coalesced_posts,
            pipeline.rx_irq_service_samples,
            pipeline.rx_irq_clock_skew_samples,
            average_irq_service_us,
            pipeline.rx_irq_to_service_max_us,
            irq.spurious_entries,
            irq.rx_only_entries,
            irq.rx_mixed_entries,
            irq.tx_only_entries,
            irq.tx_mixed_entries,
            irq.other_only_entries,
            irq.classified_entries(),
            irq.extra_nonzero_snapshots,
            irq.saturated_entries,
            irq.auxiliary_entries,
            irq.unhandled_entries,
            pipeline.staged_bytes,
            pipeline.stage_empty_discards,
            pipeline.stage_too_long_discards,
            average_service_us,
            pipeline.service_max_us,
            pipeline.reload_transactions,
            average_reload_us,
            pipeline.reload_max_us,
            pipeline.reload_us,
            pipeline.backpressured_services,
            pipeline.bulk_capacity_blocked_services,
            pipeline.pool_credit_limited_services,
            pipeline.queue_credit_limited_services,
            pipeline.maximum_deferred_frames,
            pipeline.minimum_backpressured_pool_credits,
            pipeline.minimum_backpressured_queue_credits,
            pipeline.minimum_pool_credits,
            pipeline.minimum_queue_credits,
            pipeline.protocol_frames,
            pipeline.protocol_data_frames,
            average_dispatch_us,
            pipeline.dispatch_max_us,
            pipeline.protocol_amsdu_mpdus,
            pipeline.protocol_amsdu_subframes,
            pipeline.protocol_units_le_1700,
            pipeline.protocol_units_1701_3400,
            pipeline.protocol_units_over_3400,
            pipeline.protocol_unit_max_bytes,
            pipeline.network_publications,
            pipeline.network_published_bytes,
            average_publish_us,
            pipeline.network_publish_max_us,
            pipeline.network_ready_waits,
            average_wait_us,
            pipeline.network_ready_wait_max_us,
            ampdu.aggregates,
            ampdu.completed,
            ampdu.publications,
            ampdu.subframes,
            average_subframes,
            ampdu.minimum,
            ampdu.maximum,
            ampdu.full32,
            full32_percent,
            ampdu.one,
            ampdu.two_three,
            ampdu.four_seven,
            ampdu.eight_fifteen,
            ampdu.sixteen_twentythree,
            ampdu.twentyfour_thirty,
            ampdu.thirtyone,
            ampdu.full32,
            ampdu.stop_frame,
            ampdu.stop_capacity,
            ampdu.stop_empty,
            ampdu.acknowledged,
            ampdu.individual_retry,
            ampdu.timeout,
            ampdu.collision,
            ampdu.block_ack_samples,
            ampdu.block_ack_received,
            ampdu.full_block_ack,
            ampdu.partial_block_ack,
            ampdu.empty_block_ack,
            ampdu.block_ack_success_without,
            ampdu.block_ack_nonzero_control,
            ampdu.block_ack_start_outside,
            ampdu.block_ack_start_lag_max,
            average_preparation_us,
            ampdu.preparation_max_us,
            average_publication_us,
            ampdu.publication_max_us,
            average_exchange_us,
            ampdu.exchange_max_us,
            average_first_exchange_us,
            ampdu.first_exchange_max_us,
            ampdu.first_exchanges,
            average_retry_exchange_us,
            ampdu.retry_exchange_max_us,
            ampdu.retried_exchanges,
            ampdu.retry_publications,
            ampdu.tx_irq_epochs,
            ampdu.tx_irq_samples,
            ampdu.tx_irq_skew,
            average_tx_irq_service_us,
            ampdu.tx_irq_service_max_us,
            average_tx_flight_us,
            ampdu.tx_flight_max_us,
            ampdu.tx_flight_samples,
            ampdu.standby_prepared,
            ampdu.standby_published,
            ampdu.standby_cancelled,
        ),
    )?;
    Ok(())
}
